//! Capability refusals invalidate the adopted facts before another decision
//! can use them (jig's domain/connectors.md, section 12).
use crate::{Domain, Kinds, Limits, Protection, Request, Role, Stored};
use skein_lib::{Env, Queue, Token};
use temper_engine_domain_forge_client::{self as client, api};

#[derive(Debug)]
pub(crate) enum Stage {
    Permission,
    Settings,
    Protection,
}
#[derive(Debug)]
pub(crate) struct Pending {
    pub(crate) repository: api::Repository,
    pub(crate) stage: Stage,
    pub(crate) permission: Option<api::Permission>,
    pub(crate) settings: Option<api::Settings>,
}

pub(crate) fn refresh(d: &mut Domain, env: &Env<Limits>, repository: api::Repository, out: &mut Queue<Request>) {
    let Some(row) = d.repositories.get_mut(&repository) else { return };
    row.kinds = kinds(api::Permission::None, row.role, &row.settings);
    out.push(Request::Save { read_afresh: None, release: None, record: Stored::Repository(row.clone()) });
    for (_, pending) in &d.capabilities {
        if pending.repository == repository {
            return;
        }
    }
    let Some(author) = d.writers.get(&repository.forge).copied() else { return };
    if d.capabilities.len() >= env.limits.repositories {
        return;
    }
    let Some(sequence) = d.sequence.checked_add(1) else { return };
    if sequence >= 1_u64 << 60_u32 {
        return;
    }
    d.sequence = sequence;
    let owner = Token::new(sequence | (1_u64 << 60_u32));
    d.capabilities
        .insert(owner, Pending { repository, stage: Stage::Permission, permission: None, settings: None })
        .expect("one refresh per adopted repository");
    crate::domain::child(
        d,
        env,
        client::Event::Read { owner, repository, read: api::Read::Permission { user: author } },
        out,
    );
}

pub(crate) fn answered(
    d: &mut Domain,
    env: &Env<Limits>,
    owner: Token,
    result: Result<api::Answer, api::Error>,
    out: &mut Queue<Request>,
) {
    let Some(mut pending) = d.capabilities.remove(&owner) else { return };
    let repository = pending.repository;
    let read = match pending.stage {
        Stage::Permission => {
            let Ok(api::Answer::Permission(permission)) = result else { return };
            pending.permission = Some(permission);
            pending.stage = Stage::Settings;
            api::Read::Settings
        }
        Stage::Settings => {
            let Ok(api::Answer::Settings(settings)) = result else { return };
            let branch = settings.default_branch.clone();
            pending.settings = Some(settings);
            pending.stage = Stage::Protection;
            api::Read::Protection { branch }
        }
        Stage::Protection => {
            let protection = match result {
                Ok(api::Answer::Protection(Some(rule))) => Protection::Rule(rule),
                Ok(api::Answer::Protection(None)) => Protection::Absent,
                Err(api::Error::Forbidden) => Protection::Unknown,
                Ok(_) | Err(_) => return,
            };
            let Some(row) = d.repositories.get_mut(&repository) else { return };
            let permission = pending.permission.expect("permission precedes settings");
            let settings = pending.settings.expect("settings precede protection");
            row.kinds = kinds(permission, row.role, &settings);
            row.settings = settings;
            row.protection = protection;
            out.push(Request::Save { read_afresh: None, release: None, record: Stored::Repository(row.clone()) });
            return;
        }
    };
    d.capabilities.insert(owner, pending).expect("refresh advances in its reserved slot");
    crate::domain::child(d, env, client::Event::Read { owner, repository, read }, out);
}

fn kinds(permission: api::Permission, role: Role, settings: &api::Settings) -> Kinds {
    let read = permission != api::Permission::None;
    let write = match permission {
        api::Permission::Write | api::Permission::Admin => role != Role::Context,
        api::Permission::None | api::Permission::Read => false,
    };
    Kinds {
        read,
        push: write,
        open: write,
        land: write && (settings.merge || settings.squash || settings.rebase),
        review: write,
        status: write,
        comment: write,
        issue: write,
        branch: write,
    }
}
