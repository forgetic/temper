//! The workers in contact (engine-model.md, sections 2 and 8): each known by
//! its channel, from the hello it says first on it until the channel is lost.
//! A worker that comes back dials a new channel, and is known again by what
//! its hello lists: the fleet needs no other name for it.
//!
//! A hello is the worker's entrance. It is turned away, its channel to be
//! closed, if the fleet has no room for another worker, if it lists more
//! runs than a worker may host, or if the fleet could not track every
//! attempt it lists that it does not know; nothing of it is kept then. Its
//! slots are used up to the limit, and the workstreams it lists up to the
//! limit, each within the bytes of a key. Each run it lists is kept,
//! cancelled again or found (see the attempts' table). A second hello on a
//! channel breaks the worker's contract, and is counted and dropped.
//!
//! A lost channel's attempts are kept for the grace, adrift, and the worker
//! is forgotten. A worker that refuses an assignment as busy is shutting
//! down, or fuller than the fleet knew: nothing more is placed on it.
//!
//! Placement takes a worker in contact with a free slot that holds the
//! workstream's checkout, the first in the order of the protocol's names for
//! the channels; or, with none, the one with the most free slots. A worker
//! holds the workstreams its hello listed and those of the runs placed on it
//! since, while there is room.

use alloc::boxed::Box;
use core::mem;

use temper_lib::{Env, Id, Queue, Set, Slab, Token};

use crate::attempt::{self, Attempt};
use crate::boundary::{Hello, Hosted, Phase, Request};
use crate::facts::Fact;
use crate::limits::Limits;
use crate::model::Model;

/// A worker in contact.
#[derive(Debug)]
pub(crate) struct Channel {
    /// The protocol's name for its channel.
    pub(crate) token: Token,
    /// How many runs it hosts at once, as its hello said, within the limits.
    pub(crate) slots: u32,
    /// It refused an assignment as busy: nothing more is placed on it.
    pub(crate) draining: bool,
    /// The attempts it hosts or holds the answers of, each taking a slot.
    pub(crate) hosts: Set<Id<Attempt>>,
    /// The workstreams it holds checkouts for.
    pub(crate) workstreams: Set<Box<[u8]>>,
}

/// The most bytes of a key, as a slice's length.
pub(crate) fn bytes(limit: u32) -> usize {
    usize::try_from(limit).unwrap_or(usize::MAX)
}

/// The protocol's name for the channel `channel`, which is in contact.
pub(crate) fn token(channels: &Slab<Channel>, channel: Id<Channel>) -> Token {
    channels.get(channel).expect("an attempt is on a worker in contact").token
}

/// Keeps `workstream` among those `channel` holds, if it is not and there is
/// room; drops it otherwise.
pub(crate) fn cache(channel: &mut Channel, limits: &Limits, workstream: Box<[u8]>) {
    if channel.workstreams.len() < limits.workstreams && workstream.len() <= bytes(limits.workstream_bytes) {
        let room = channel.workstreams.insert(workstream);
        assert!(room.is_ok(), "checked for room above");
    }
}

/// Hello, on a new channel: the worker is in contact, or turned away.
pub(crate) fn hello(model: &mut Model, env: &Env<Limits>, channel: Token, hello: Hello, out: &mut Queue<Request>) {
    let limits = &env.limits;
    if model.tokens.contains_key(&channel) {
        model.facts.push(Fact::Dropped);
        return;
    }
    let Hello { slots, workstreams, hosting } = hello;
    let listed = u32::try_from(hosting.len()).unwrap_or(u32::MAX);
    let mut unknown: u32 = 0;
    for hosted in &hosting {
        if !model.names.contains_key(&(hosted.run, hosted.attempt)) {
            unknown = unknown.saturating_add(1);
        }
    }
    let room = model.attempts.capacity().saturating_sub(model.attempts.len());
    if model.channels.is_full() || listed > limits.slots || unknown > room {
        model.facts.push(Fact::TurnedAway);
        out.push(Request::Refuse { channel });
        return;
    }
    let mut worker = Channel {
        token: channel,
        slots: slots.min(limits.slots),
        draining: false,
        hosts: Set::with_capacity(limits.slots),
        workstreams: Set::with_capacity(limits.workstreams),
    };
    for workstream in workstreams {
        if !workstream.is_empty() {
            cache(&mut worker, limits, workstream);
        }
    }
    let Ok(id) = model.channels.insert(worker) else {
        unreachable!("checked for room above");
    };
    let named = model.tokens.insert(channel, id);
    assert!(named == Ok(None), "a channel is named once, with room for every worker");
    for Hosted { run, attempt, phase } in hosting {
        let answered = match phase {
            Phase::Answered => true,
            Phase::Preparing | Phase::Starting | Phase::Active | Phase::Waiting | Phase::Ending => false,
        };
        attempt::listed(model, env, id, run, attempt, answered, out);
    }
    model.facts.push(Fact::Hello { listed });
    model.placing = true;
}

/// Lost: the worker's attempts are kept for the grace, adrift. A channel
/// the fleet does not know never said hello, or was turned away.
pub(crate) fn lost(model: &mut Model, env: &Env<Limits>, channel: Token) {
    let Some(id) = model.tokens.remove(&channel) else {
        return;
    };
    let entry = model.channels.get_mut(id).expect("a named channel is in contact");
    let hosts = mem::replace(&mut entry.hosts, Set::with_capacity(0));
    let until = env.now.saturating_add(env.limits.grace);
    for &attempt in &hosts {
        attempt::adrift(model, attempt, until);
    }
    model.channels.retire(id);
    model.facts.push(Fact::Lost { adrift: hosts.len() });
}

/// The worker of `channel` refused an assignment as busy: nothing more is
/// placed on it.
pub(crate) fn drain(model: &mut Model, channel: Token) {
    let Some(&id) = model.tokens.get(&channel) else {
        return;
    };
    model.channels.get_mut(id).expect("a named channel is in contact").draining = true;
}

/// The worker to place an attempt of `workstream` on: one with a free slot
/// that holds its checkout, or else the one with the most free slots.
pub(crate) fn choose(model: &Model, workstream: &[u8]) -> Option<Id<Channel>> {
    let mut best: Option<(Id<Channel>, u32)> = None;
    for (_, &id) in &model.tokens {
        let channel = model.channels.get(id).expect("a named channel is in contact");
        let free = channel.slots.saturating_sub(channel.hosts.len());
        if channel.draining || free == 0 {
            continue;
        }
        if channel.workstreams.contains(workstream) {
            return Some(id);
        }
        best = match best {
            Some((other, most)) if most >= free => Some((other, most)),
            Some(_) | None => Some((id, free)),
        };
    }
    let (id, _) = best?;
    Some(id)
}
