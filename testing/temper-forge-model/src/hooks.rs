//! Webhooks: a repository's subscriber hears of each change to it (which
//! kind, and the item it is about), after a drawn latency, late, or not at
//! all, as a real forge's deliveries go. Each is its own request, so they may
//! arrive in another order than the changes.

use temper_lib::bytes::copy_of;
use temper_lib::{Env, Id, Queue};

use crate::api::Change;
use crate::faults;
use crate::model::{Alarm, Config, Model, Request};
use crate::store::Repository;

/// A webhook on its way.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Delivery {
    repository: Id<Repository>,
    change: Change,
    number: Option<u64>,
}

/// Something of `change` changed in `repository`: its subscriber, if it has
/// one, hears of it later, or never.
pub(crate) fn notify(
    model: &mut Model,
    env: &Env<Config>,
    repository: Id<Repository>,
    change: Change,
    number: Option<u64>,
) {
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
    let Ok(id) = model.deliveries.insert(Delivery { repository, change, number }) else {
        model.tally.hooks_dropped = model.tally.hooks_dropped.saturating_add(1);
        return;
    };
    if late {
        model.tally.hooks_late = model.tally.hooks_late.saturating_add(1);
    }
    model.timers.arm(Alarm::Hook(id), env.now.saturating_add(span)).expect("a timer per delivery fits");
}

/// A delivery's timer: the webhook goes out.
pub(crate) fn deliver(model: &mut Model, id: Id<Delivery>, out: &mut Queue<Request>) {
    let delivery = *model.deliveries.get(id).expect("a delivery lives until its timer fires");
    let repository = model.repositories.get(delivery.repository).expect("a repository of the forge");
    out.push(Request::Hook { repository: copy_of(&repository.name), change: delivery.change, number: delivery.number });
    model.deliveries.retire(id);
    model.tally.hooks = model.tally.hooks.saturating_add(1);
}
