//! Webhooks: a repository's subscriber hears of each change to it (which
//! kind; the item it is about; for a push, the branch and where it went; for
//! a status, the commit), after a drawn latency, late, or not at all, as a
//! real forge's deliveries go. Each is its own request, so they may arrive in
//! another order than the changes.
//!
//! As on Forgejo, a status reported on a pull request's head is heard as a
//! status of its commit, not as a change to the pull request.

use alloc::boxed::Box;

use temper_lib::bytes::copy_of;
use temper_lib::{Env, Id, Queue};

use crate::api::Change;
use crate::boundary::Request;
use crate::faults;
use crate::model::{Alarm, Config, Model};
use crate::store::Repository;

/// What a webhook tells of a change.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Hook {
    change: Change,
    number: Option<u64>,
    branch: Option<Box<[u8]>>,
    commit: Option<u64>,
}

impl Hook {
    /// A change of `change` to the item `number`.
    pub(crate) fn item(change: Change, number: u64) -> Hook {
        Hook { change, number: Some(number), branch: None, commit: None }
    }

    /// `branch` moved to `commit`, or was deleted.
    pub(crate) fn push(branch: &[u8], commit: Option<u64>) -> Hook {
        Hook { change: Change::Push, number: None, branch: Some(copy_of(branch)), commit }
    }

    /// A status reported on `commit`.
    pub(crate) fn status(commit: u64) -> Hook {
        Hook { change: Change::Status, number: None, branch: None, commit: Some(commit) }
    }

    /// A wiki page written or deleted.
    pub(crate) fn wiki() -> Hook {
        Hook { change: Change::Wiki, number: None, branch: None, commit: None }
    }
}

/// A webhook on its way.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Delivery {
    repository: Id<Repository>,
    hook: Hook,
}

/// Something changed in `repository`: its subscriber, if it has one, hears of
/// it later, or never.
pub(crate) fn notify(model: &mut Model, env: &Env<Config>, repository: Id<Repository>, hook: Hook) {
    if !model.repositories.get(repository).expect("a repository of the forge").hooked {
        return;
    }
    let config = &env.limits;
    if model.rng.chance(config.hooks_lost) {
        model.tally.hooks_lost = model.tally.hooks_lost.saturating_add(1);
        return;
    }
    let late = model.rng.chance(config.hooks_late);
    let span = if late {
        faults::draw(model, config.late_min, config.late_max)
    } else {
        faults::draw(model, config.hook_min, config.hook_max)
    };
    if model.deliveries.is_full() {
        model.tally.hooks_dropped = model.tally.hooks_dropped.saturating_add(1);
        return;
    }
    let id = model.deliveries.insert(Delivery { repository, hook }).expect("checked for room above");
    if late {
        model.tally.hooks_late = model.tally.hooks_late.saturating_add(1);
    }
    model.timers.arm(Alarm::Hook(id), env.now.saturating_add(span)).expect("a timer per delivery fits");
}

/// A delivery's timer: the webhook goes out.
pub(crate) fn deliver(model: &mut Model, id: Id<Delivery>, out: &mut Queue<Request>) {
    let delivery = model.deliveries.get(id).expect("a delivery lives until its timer fires");
    let repository = model.repositories.get(delivery.repository).expect("a repository of the forge");
    let Hook { change, number, branch, commit } = &delivery.hook;
    let hook = Request::Hook {
        repository: copy_of(&repository.name),
        change: *change,
        number: *number,
        branch: branch.clone(),
        commit: *commit,
    };
    out.push(hook);
    model.deliveries.retire(id);
    model.tally.hooks = model.tally.hooks.saturating_add(1);
}
