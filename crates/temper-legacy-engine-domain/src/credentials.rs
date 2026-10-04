//! Routes account metadata and fenced notices. Values never enter the tree.
use alloc::boxed::Box;

use skein_lib::{Env, Id, List, Queue, Time, Token};
use temper_engine_domain_fleet as fleet;

use crate::domain::Domain;
use crate::items::Entry;
use crate::limits::Limits;
use crate::{Item, Request, accounts};

pub(crate) fn step(domain: &mut Domain, env: &Env<Limits>, event: accounts::Event) {
    let child = Env { now: env.now, wall: env.wall, limits: env.limits.accounts };
    accounts::step(&mut domain.accounts, &child, event, &mut domain.account_out);
}

pub(crate) fn fire(domain: &mut Domain, env: &Env<Limits>) {
    let child = Env { now: env.now, wall: env.wall, limits: env.limits.accounts };
    accounts::fire(&mut domain.accounts, &child, &mut domain.account_out);
}

pub(crate) fn ready(domain: &Domain) -> bool {
    domain.account_start < domain.config.accounts.len() || domain.account_wake || !domain.grant_pending.is_empty()
}

pub(crate) fn resume(domain: &mut Domain, env: &Env<Limits>) {
    if let Some(account) = domain.config.accounts.get(domain.account_start).copied() {
        domain.account_start = domain.account_start.saturating_add(1);
        let mut valid = None;
        if let Some(span) = account.valid {
            let expires = domain.account_epoch.saturating_add(span);
            if expires > env.now {
                valid = Some(expires.saturating_since(env.now));
            }
        }
        return step(
            domain,
            env,
            accounts::Event::Add { account: account.account, generation: account.generation, valid },
        );
    }
    if let Some(&(id, account)) = domain.grant_pending.first() {
        domain.grant_pending.remove(&(id, account));
        let Some(entry) = domain.items.get(id) else {
            return;
        };
        let Some(live) = entry.live else {
            return;
        };
        let item = entry.item;
        let attempt = live.attempt;
        if !live.started || !uses(domain, item, attempt, account) {
            return;
        }
        let Some(grant) = current(domain, account, env.now) else {
            return;
        };
        return crate::route::fleet_step(
            domain,
            env,
            fleet::Event::Grant {
                run: crate::translate::run_of(item),
                attempt: Token::new(attempt),
                grant: fleet::Grant { account, generation: grant.generation, valid: grant.valid },
            },
        );
    }
    domain.account_wake = false;
    // One ready attempt per resume, leaving unavailable attempts asleep.
    let mut found = None;
    for &id in &domain.account_waiting {
        if can_start(domain, id, env.now) {
            found = Some(id);
            break;
        }
    }
    if let Some(id) = found {
        domain.account_wake = true;
        domain.account_waiting.remove(&id);
        crate::runs::place(domain, env, id);
    }
}

pub(crate) fn can_start(domain: &Domain, id: Id<Entry>, now: Time) -> bool {
    let Some(entry) = domain.items.get(id) else {
        return false;
    };
    let Some(assignment) = entry.assignment.as_ref() else {
        return false;
    };
    if entry.live.is_none() {
        return false;
    }
    for model in &assignment.charter.models {
        let index = usize::try_from(model.endpoint).expect("endpoint index fits");
        let account = *domain.config.endpoints.get(index).expect("configured models name endpoints");
        if domain.accounts.grant(account, now).is_none() {
            return false;
        }
    }
    true
}

pub(crate) fn uses(domain: &Domain, item: Item, attempt: u64, account: u32) -> bool {
    let Some(id) = crate::items::find(domain, item) else {
        return false;
    };
    let entry = crate::jobs::get(domain, id);
    let Some(live) = entry.live else {
        return false;
    };
    if live.attempt != attempt {
        return false;
    }
    let models = match entry.assignment.as_ref() {
        Some(assignment) => &assignment.charter.models,
        None => &domain.config.models,
    };
    for model in models {
        let index = usize::try_from(model.endpoint).expect("endpoint index fits");
        if domain.config.endpoints.get(index) == Some(&account) {
            return true;
        }
    }
    match entry.assignment.as_ref() {
        Some(assignment) => {
            for repository in &assignment.workspace.repositories {
                let index = usize::try_from(repository.repository).expect("repository index fits");
                if domain.config.identities.get(index) == Some(&account) {
                    return true;
                }
            }
        }
        // Adopted runs have no in-memory assignment. The root's assignment
        // builder always gives them their item's single repository.
        None => {
            let index = usize::try_from(entry.item.repository).expect("repository index fits");
            if domain.config.identities.get(index) == Some(&account) {
                return true;
            }
        }
    }
    false
}

pub(crate) fn grants(domain: &Domain, id: Id<Entry>, now: Time) -> Box<[accounts::Grant]> {
    let count = u32::try_from(domain.config.accounts.len().saturating_add(domain.config.identities.len()))
        .expect("configured account counts fit");
    let mut grants = List::with_capacity(count);
    let entry = crate::jobs::get(domain, id);
    for account in &domain.config.accounts {
        let Some(live) = entry.live else {
            continue;
        };
        if !uses(domain, entry.item, live.attempt, account.account) {
            continue;
        }
        if let Some(grant) = domain.accounts.grant(account.account, now) {
            grants.push(grant).expect("room for every configured grant");
        }
    }
    let assignment = entry.assignment.as_ref().expect("prepared attempt has assignment");
    for repository in &assignment.workspace.repositories {
        let index = usize::try_from(repository.repository).expect("repository index fits");
        let account = *domain.config.identities.get(index).expect("configured repository has identity");
        let mut seen = false;
        for grant in &grants {
            if grant.account == account {
                seen = true;
            }
        }
        if !seen {
            grants
                .push(accounts::Grant { account, generation: 0, valid: skein_lib::Duration::from_nanos(u64::MAX) })
                .expect("room for every repository identity");
        }
    }
    grants.into_boxed()
}

pub(crate) fn route(domain: &mut Domain, _env: &Env<Limits>, request: accounts::Request, out: &mut Queue<Request>) {
    match request {
        accounts::Request::Granted { grant } => {
            enqueue(domain, grant.account);
        }
        accounts::Request::Availability { account, usable } => {
            if usable {
                domain.account_wake = true;
                enqueue(domain, account);
            }
        }
        request @ (accounts::Request::Refresh { .. }
        | accounts::Request::Keep { .. }
        | accounts::Request::Cancel { .. }) => out.push(Request::Account { request }),
        accounts::Request::Closed { account: _ } => {}
        accounts::Request::Refused { account: _ } => unreachable!("configured accounts are validated at startup"),
    }
}

fn enqueue(domain: &mut Domain, account: u32) {
    for (_, &id) in &domain.names {
        let entry = domain.items.get(id).expect("a named item is held");
        if let Some(live) = entry.live
            && live.started
            && uses(domain, entry.item, live.attempt, account)
        {
            domain.grant_pending.insert((id, account)).expect("one pending grant per live item and account");
        }
    }
}

fn current(domain: &Domain, account: u32, now: Time) -> Option<accounts::Grant> {
    if let Some(grant) = domain.accounts.grant(account, now) {
        return Some(grant);
    }
    for entry in &domain.config.accounts {
        if entry.account == account {
            return None;
        }
    }
    if domain.config.identities.contains(&account) {
        Some(accounts::Grant { account, generation: 0, valid: skein_lib::Duration::from_nanos(u64::MAX) })
    } else {
        None
    }
}

pub(crate) fn redial(domain: &mut Domain, id: Id<Entry>) {
    let entry = crate::jobs::get(domain, id);
    let Some(live) = entry.live else {
        return;
    };
    let item = entry.item;
    for account in &domain.config.accounts {
        if uses(domain, item, live.attempt, account.account) {
            domain.grant_pending.insert((id, account.account)).expect("bounded account fanout");
        }
    }
    for &account in &domain.config.identities {
        if uses(domain, item, live.attempt, account) {
            domain.grant_pending.insert((id, account)).expect("bounded account fanout");
        }
    }
}

pub(crate) fn forget(domain: &mut Domain, id: Id<Entry>) {
    domain.account_waiting.remove(&id);
    for account in &domain.config.accounts {
        domain.grant_pending.remove(&(id, account.account));
    }
    for &account in &domain.config.identities {
        domain.grant_pending.remove(&(id, account));
    }
}
