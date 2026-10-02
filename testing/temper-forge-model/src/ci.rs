//! CI: statuses per repository, commit and context. When a commit becomes a
//! branch's or a pull request's head, each context of the repository's checks
//! is pending at once, then passes or fails after a drawn latency, or never
//! reports, by the repository's chances; or, where a cue is configured, as the
//! commit's content says (testing-pyramid.md, 4.2: the stand-in until CI
//! follows content for real). CI runs once per commit and repository. Users
//! with write permission report statuses of their own.
//!
//! As on Forgejo, a status is the commit's, and moves no pull request's
//! updated time, unless the configuration says otherwise.

use alloc::boxed::Box;

use temper_lib::bytes::{copy_of, find};
use temper_lib::{Env, Id, Map};

use crate::api::{Answer, Change, Check, Error, Permission, What};
use crate::faults;
use crate::model::{self, Alarm, Config, Model};
use crate::observe::Observation;
use crate::store::{Repository, Status, fits};

/// CI starts on `commit`, which became a head in the repository `id`, unless
/// it ran on it already.
pub(crate) fn start(model: &mut Model, env: &Env<Config>, id: Id<Repository>, commit: u64) {
    let limits = &env.limits.limits;
    let repository = model.repositories.get_mut(id).expect("a repository of the forge");
    let Some(first) = repository.checks.contexts.first() else {
        return;
    };
    if let Some(statuses) = repository.statuses.get(&commit)
        && statuses.contains_key(&**first)
    {
        return;
    }
    if !repository.statuses.contains_key(&commit) {
        if repository.statuses.len() >= repository.statuses.capacity() {
            model.tally.unreported = model.tally.unreported.saturating_add(1);
            return;
        }
        repository.statuses.insert(commit, Map::with_capacity(limits.contexts)).expect("checked for room above");
    }
    let contexts = u32::try_from(repository.checks.contexts.len()).expect("contexts within the limits");
    for context in 0..contexts {
        let Some(name) = name(model, id, context) else {
            break;
        };
        let pending = Status { state: Check::Pending, author: env.limits.ci, at: model::clock(env) };
        if !set(model, id, commit, &name, pending) {
            model.tally.unreported = model.tally.unreported.saturating_add(1);
            return;
        }
        reported(model, env, id, commit, name, Check::Pending, env.limits.ci);
        let checks = &model.repositories.get(id).expect("a repository of the forge").checks;
        let silent = checks.silent;
        let min = checks.latency_min;
        let max = checks.latency_max;
        if model.rng.chance(silent) {
            model.tally.silent = model.tally.silent.saturating_add(1);
            continue;
        }
        let span = faults::draw(model, min, max);
        let alarm = Alarm::Check { repository: id, commit, context };
        model.timers.arm(alarm, env.now.saturating_add(span)).expect("a timer per context of a commit with statuses");
    }
}

/// A check's timer: CI reports the context `context` on `commit`, as cued
/// by the commit's content, or by chance.
pub(crate) fn report(model: &mut Model, env: &Env<Config>, id: Id<Repository>, commit: u64, context: u32) {
    let Some(name) = name(model, id, context) else {
        unreachable!("a check's context is one of its repository's");
    };
    let checks = &model.repositories.get(id).expect("a repository of the forge").checks;
    let state = match &checks.cue {
        Some(cue) => {
            let tree = &model.commits.get(&commit).expect("a commit of the store").tree;
            let green = match tree.get(&*cue.path) {
                Some(file) => find(file, &cue.green).is_some(),
                None => false,
            };
            if green { Check::Passed } else { Check::Failed }
        }
        None => {
            let passes = checks.passes;
            if model.rng.chance(passes) { Check::Passed } else { Check::Failed }
        }
    };
    let status = Status { state, author: env.limits.ci, at: model::clock(env) };
    let kept = set(model, id, commit, &name, status);
    assert!(kept, "a pending status is replaced");
    model.tally.verdicts = model.tally.verdicts.saturating_add(1);
    reported(model, env, id, commit, name, state, env.limits.ci);
}

/// Reports `state` of `context` on `commit`, as `user`.
pub(crate) fn status(
    model: &mut Model,
    env: &Env<Config>,
    id: Id<Repository>,
    user: u64,
    commit: u64,
    context: Box<[u8]>,
    state: Check,
) -> Result<Answer, Error> {
    let limits = &env.limits.limits;
    let repository = model.repositories.get_mut(id).expect("a repository of the forge");
    repository.require(user, Permission::Write)?;
    fits(&context, limits.name_bytes)?;
    if !repository.has.contains(&commit) {
        return Err(Error::Missing(What::Commit));
    }
    if !repository.statuses.contains_key(&commit) {
        if repository.statuses.len() >= repository.statuses.capacity() {
            return Err(Error::Full);
        }
        repository.statuses.insert(commit, Map::with_capacity(limits.contexts)).expect("checked for room above");
    }
    if !set(model, id, commit, &context, Status { state, author: user, at: model::clock(env) }) {
        return Err(Error::Full);
    }
    reported(model, env, id, commit, context, state, user);
    Ok(Answer::Done)
}

/// The name of the context `context` of the repository's checks.
fn name(model: &Model, id: Id<Repository>, context: u32) -> Option<Box<[u8]>> {
    let repository = model.repositories.get(id).expect("a repository of the forge");
    let index = usize::try_from(context).ok()?;
    Some(copy_of(repository.checks.contexts.get(index)?))
}

/// Sets the status of `context` on `commit`, which has statuses, or says
/// there is no room for it.
fn set(model: &mut Model, id: Id<Repository>, commit: u64, context: &[u8], status: Status) -> bool {
    let repository = model.repositories.get_mut(id).expect("a repository of the forge");
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
    model: &mut Model,
    env: &Env<Config>,
    id: Id<Repository>,
    commit: u64,
    context: Box<[u8]>,
    state: Check,
    by: u64,
) {
    let repository = model.repositories.get_mut(id).expect("a repository of the forge");
    if env.limits.status_updates {
        let heads = repository.heads(commit);
        for &number in &heads {
            repository.touch(number, model::clock(env));
        }
    }
    let observation = Observation::Reported { repository: copy_of(&repository.name), commit, context, state, by };
    model::changed(model, env, id, observation, Change::Status, None);
}
