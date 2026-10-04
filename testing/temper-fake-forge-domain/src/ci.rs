//! CI: statuses per repository, commit and context. When a commit becomes a
//! branch's or a pull request's head, each context of the repository's checks
//! is pending at once, then passes or fails after a drawn latency, or never
//! reports, by the repository's chances; or, where a cue is configured, as the
//! commit's content says (testing.md, 4.2: the stand-in until CI
//! follows content for real). CI runs once per commit and repository; a
//! context that reported may be run again once, by the repository's chance:
//! pending again at once, then a verdict drawn again, so a head's CI can
//! move after it settled. Users with write permission report statuses of
//! their own.
//!
//! As on Forgejo, a status is the commit's, and moves no pull request's
//! updated time, unless the configuration says otherwise.

use alloc::boxed::Box;

use skein_lib::bytes::{copy_of, find};
use skein_lib::{Env, Id, Map};

use crate::api::{Answer, Check, Error, Permission, What};
use crate::domain::{self, Alarm, Config, Domain};
use crate::faults;
use crate::hooks::Hook;
use crate::observe::Observation;
use crate::store::{Repository, Status, fits};

/// CI starts on `commit`, which became a head in the repository `id`, unless
/// it ran on it already.
pub(crate) fn start(domain: &mut Domain, env: &Env<Config>, id: Id<Repository>, commit: u64) {
    let limits = &env.limits.limits;
    let repository = domain.repositories.get_mut(id).expect("a repository of the forge");
    let Some(first) = repository.checks.contexts.first() else {
        return;
    };
    if let Some(statuses) = repository.statuses.get(&commit)
        && statuses.contains_key(&**first)
    {
        return;
    }
    if !repository.statuses.contains_key(&commit) {
        if !room(domain, id) {
            domain.tally.unreported = domain.tally.unreported.saturating_add(1);
            return;
        }
        let repository = domain.repositories.get_mut(id).expect("a repository of the forge");
        repository.statuses.insert(commit, Map::with_capacity(limits.contexts)).expect("room was made");
    }
    let repository = domain.repositories.get(id).expect("a repository of the forge");
    let contexts = u32::try_from(repository.checks.contexts.len()).expect("contexts within the limits");
    for context in 0..contexts {
        let Some(name) = name(domain, id, context) else {
            break;
        };
        let pending = Status { state: Check::Pending, author: env.limits.ci, at: domain::clock(env), rerun: false };
        if !set(domain, id, commit, &name, pending) {
            domain.tally.unreported = domain.tally.unreported.saturating_add(1);
            return;
        }
        reported(domain, env, id, commit, name, Check::Pending, env.limits.ci);
        let checks = &domain.repositories.get(id).expect("a repository of the forge").checks;
        let silent = checks.silent;
        let min = checks.latency_min;
        let max = checks.latency_max;
        if domain.rng.chance(silent) {
            domain.tally.silent = domain.tally.silent.saturating_add(1);
            continue;
        }
        let span = faults::draw(domain, min, max);
        let alarm = Alarm::Check { repository: id, commit, context };
        domain.timers.arm(alarm, env.now.saturating_add(span)).expect("a timer per context of a commit with statuses");
    }
}

/// A check's timer: CI reports the context `context` on `commit`, as cued
/// by the commit's content, or by chance.
pub(crate) fn report(domain: &mut Domain, env: &Env<Config>, id: Id<Repository>, commit: u64, context: u32) {
    let Some(name) = name(domain, id, context) else {
        unreachable!("a check's context is one of its repository's");
    };
    let checks = &domain.repositories.get(id).expect("a repository of the forge").checks;
    let state = match &checks.cue {
        Some(cue) => {
            let tree = &domain.commits.get(&commit).expect("a commit of the store").tree;
            let green = match tree.get(&*cue.path) {
                Some(file) => find(file, &cue.green).is_some(),
                None => false,
            };
            if green { Check::Passed } else { Check::Failed }
        }
        None => {
            let passes = checks.passes;
            if domain.rng.chance(passes) { Check::Passed } else { Check::Failed }
        }
    };
    let repository = domain.repositories.get(id).expect("a repository of the forge");
    let reruns = repository.checks.reruns;
    let (min, max) = (repository.checks.latency_min, repository.checks.latency_max);
    let rerun = match repository.statuses.get(&commit) {
        Some(statuses) => match statuses.get(&*name) {
            Some(status) => status.rerun,
            None => false,
        },
        None => false,
    };
    let status = Status { state, author: env.limits.ci, at: domain::clock(env), rerun };
    let kept = set(domain, id, commit, &name, status);
    assert!(kept, "a pending status is replaced");
    domain.tally.verdicts = domain.tally.verdicts.saturating_add(1);
    reported(domain, env, id, commit, name, state, env.limits.ci);
    if !rerun && domain.rng.chance(reruns) {
        let span = faults::draw(domain, min, max);
        let alarm = Alarm::Rerun { repository: id, commit, context };
        domain.timers.arm(alarm, env.now.saturating_add(span)).expect("a timer per context of a commit with statuses");
    }
}

/// A re-run's timer: CI runs the context `context` on `commit` again, once:
/// pending at once, and a verdict after a drawn latency.
pub(crate) fn rerun(domain: &mut Domain, env: &Env<Config>, id: Id<Repository>, commit: u64, context: u32) {
    let Some(name) = name(domain, id, context) else {
        unreachable!("a re-run's context is one of its repository's");
    };
    let pending = Status { state: Check::Pending, author: env.limits.ci, at: domain::clock(env), rerun: true };
    let kept = set(domain, id, commit, &name, pending);
    assert!(kept, "a context reported is replaced");
    reported(domain, env, id, commit, name, Check::Pending, env.limits.ci);
    let checks = &domain.repositories.get(id).expect("a repository of the forge").checks;
    let (min, max) = (checks.latency_min, checks.latency_max);
    let span = faults::draw(domain, min, max);
    let alarm = Alarm::Check { repository: id, commit, context };
    domain.timers.arm(alarm, env.now.saturating_add(span)).expect("a timer per context of a commit with statuses");
}

/// Reports `state` of `context` on `commit`, as `user`.
pub(crate) fn status(
    domain: &mut Domain,
    env: &Env<Config>,
    id: Id<Repository>,
    user: u64,
    commit: u64,
    context: Box<[u8]>,
    state: Check,
) -> Result<Answer, Error> {
    let limits = &env.limits.limits;
    let repository = domain.repositories.get_mut(id).expect("a repository of the forge");
    repository.require(user, Permission::Write)?;
    fits(&context, limits.name_bytes)?;
    if !repository.has.contains(&commit) {
        return Err(Error::Missing(What::Commit));
    }
    if !repository.statuses.contains_key(&commit) {
        if !room(domain, id) {
            return Err(Error::Full);
        }
        let repository = domain.repositories.get_mut(id).expect("a repository of the forge");
        repository.statuses.insert(commit, Map::with_capacity(limits.contexts)).expect("room was made");
    }
    if !set(domain, id, commit, &context, Status { state, author: user, at: domain::clock(env), rerun: true }) {
        return Err(Error::Full);
    }
    reported(domain, env, id, commit, context, state, user);
    Ok(Answer::Done)
}

/// Whether the repository `id` has room for the statuses of one more
/// commit, making it if it must by forgetting those of the oldest commit
/// that is no longer any branch's head or any open pull request's, and its
/// pending checks with them. Forgejo keeps every status; a fake that keeps
/// a bounded number forgets only what no head shows.
fn room(domain: &mut Domain, id: Id<Repository>) -> bool {
    let repository = domain.repositories.get(id).expect("a repository of the forge");
    if repository.statuses.len() < repository.statuses.capacity() {
        return true;
    }
    let mut stale = None;
    for (&commit, _) in &repository.statuses {
        if !repository.is_head(commit) {
            stale = Some(commit);
            break;
        }
    }
    let Some(commit) = stale else {
        return false;
    };
    let contexts = u32::try_from(repository.checks.contexts.len()).expect("contexts within the limits");
    let repository = domain.repositories.get_mut(id).expect("a repository of the forge");
    repository.statuses.remove(&commit);
    for context in 0..contexts {
        domain.timers.cancel(Alarm::Check { repository: id, commit, context });
        domain.timers.cancel(Alarm::Rerun { repository: id, commit, context });
    }
    domain.tally.forgotten = domain.tally.forgotten.saturating_add(1);
    true
}

/// The name of the context `context` of the repository's checks.
fn name(domain: &Domain, id: Id<Repository>, context: u32) -> Option<Box<[u8]>> {
    let repository = domain.repositories.get(id).expect("a repository of the forge");
    let index = usize::try_from(context).ok()?;
    Some(copy_of(repository.checks.contexts.get(index)?))
}

/// Sets the status of `context` on `commit`, which has statuses, or says
/// there is no room for it.
fn set(domain: &mut Domain, id: Id<Repository>, commit: u64, context: &[u8], status: Status) -> bool {
    let repository = domain.repositories.get_mut(id).expect("a repository of the forge");
    let statuses = repository.statuses.get_mut(&commit).expect("a commit with statuses");
    if let Some(kept) = statuses.get_mut(context) {
        *kept = status;
        return true;
    }
    statuses.insert(copy_of(context), status).is_ok()
}

/// `context` on `commit` became `state`: observed and heard. Where the
/// configuration says so (Forgejo does not), the open pull requests whose
/// head it is are updated.
fn reported(
    domain: &mut Domain,
    env: &Env<Config>,
    id: Id<Repository>,
    commit: u64,
    context: Box<[u8]>,
    state: Check,
    by: u64,
) {
    let repository = domain.repositories.get_mut(id).expect("a repository of the forge");
    if env.limits.status_updates {
        let heads = repository.heads(commit);
        for &number in &heads {
            repository.touch(number, domain::clock(env));
        }
    }
    let observation = Observation::Reported { repository: copy_of(&repository.name), commit, context, state, by };
    domain::changed(domain, env, id, observation, Hook::status(commit, by));
}
