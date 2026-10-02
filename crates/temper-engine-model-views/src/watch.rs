//! Watchers: a person's live stream of a run, an item or a board, from its
//! watch to its end (engine-model.md, sections 2 and 11).
//!
//! A watcher has at most one delivery in flight. A chunk for a watcher that
//! has none goes out at once; one for a watcher with a delivery in flight
//! waits in its backlog, and the whole backlog goes out in one delivery once
//! that one has ended. The backlog is bounded: a chunk that finds it full
//! drops what waits, counted as missed, and waits in its place, so the
//! watcher is told how much it missed right before the chunks it catches up
//! from. Nothing a watcher does holds anything else up, and nothing grows.
//!
//! A watch ends once the person stops watching, or once its run has finished
//! and the watcher has had what waited for it; it ends only once its
//! delivery in flight has, so the parent's token for it is free when it is
//! told so.
//!
//! The transition table. Every other cell is unreachable by the boundary's
//! contract: a delivery ends only while it is in flight, and a closed watcher
//! is no longer named.
//!
//! ```text
//! state     event                                next      requests
//! (none)    watch, admitted                      Idle      watching
//!           watch, busy or of a run not followed (none)    refused
//! Idle      chunk                                Sending   deliver it
//!           unwatch                              Closed    ended, unwatched
//!           its run finished                     Closed    ended, finished
//! Sending   chunk, room in the backlog           Sending   (it waits)
//!           chunk, the backlog full              Sending   (what waits is missed; it waits)
//!           delivered, nothing waiting           Idle
//!           delivered, chunks waiting            Sending   deliver them, and what was missed
//!           unwatch                              Closing   (what waits is dropped)
//!           its run finished                     Draining
//! Draining  chunk                                Draining  (dropped: the watch is ending)
//!           delivered, chunks waiting            Draining  deliver them, and what was missed
//!           delivered, nothing waiting           Closed    ended, finished
//!           unwatch                              Closing   (what waits is dropped)
//!           its run finished                     Draining
//! Closing   chunk, unwatch, its run finished     Closing   (dropped)
//!           delivered                            Closed    ended, unwatched
//! ```

use alloc::boxed::Box;
use core::mem;

use temper_lib::{Env, Id, List, Queue, Time, Token};

use crate::boundary::{Chunk, End, Kind, Phase, Refusal, Request, Subject};
use crate::facts::{Fact, Facts};
use crate::limits::Limits;
use crate::model::Model;

/// A person watching, under the parent's token for the watch.
#[derive(Debug)]
pub(crate) struct Watcher {
    pub(crate) token: Token,
    pub(crate) subject: Subject,
    /// What waits for the delivery in flight to end, oldest first: empty
    /// unless one is in flight.
    pub(crate) backlog: Queue<Chunk>,
    pub(crate) state: Stream,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Stream {
    /// Caught up: no delivery in flight, and nothing waits.
    Idle,
    /// A delivery is in flight, and `missed` chunks were dropped from the
    /// backlog since it went.
    Sending { missed: u64 },
    /// As `Sending`, and the watched run has finished: the watch ends once
    /// the backlog is out.
    Draining { missed: u64 },
    /// The person stopped watching while a delivery was in flight: the watch
    /// ends once it has.
    Closing,
    /// Terminal: holds nothing.
    Closed,
}

/// A chunk less its content: what each watcher's copy is made from.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Head {
    Report { run: Token, kind: Kind, at: Time },
    Phase { item: Token, phase: Phase, at: Time },
}

/// A watch: refused at the entrance, or taken, caught up from now on.
pub(crate) fn watch(model: &mut Model, env: &Env<Limits>, token: Token, subject: Subject, out: &mut Queue<Request>) {
    assert!(!model.names.contains_key(&token), "the parent names each watch it has open once");
    if let Some(refusal) = refusal(model, &env.limits, subject) {
        model.facts.push(Fact::Refused { refusal });
        out.push(Request::Refused { watcher: token, refusal });
        return;
    }
    let backlog = Queue::with_capacity(env.limits.backlog);
    let watcher = Watcher { token, subject, backlog, state: Stream::Idle };
    let id = model.watchers.insert(watcher).expect("room for a watcher, checked at the entrance");
    let named = model.names.insert(token, id);
    assert!(named == Ok(None), "a name for each watcher, and a watcher for each name");
    model.facts.push(Fact::Watching);
    out.push(Request::Watching { watcher: token });
}

/// Why a watch is refused at the entrance, if it is: of a run not followed,
/// or with no room for one more watcher.
fn refusal(model: &Model, limits: &Limits, subject: Subject) -> Option<Refusal> {
    match subject {
        Subject::Run(run) => {
            if !model.runs.contains_key(&run) {
                return Some(Refusal::Unknown);
            }
        }
        Subject::Item(_) | Subject::Board(_) => {}
    }
    if model.names.len() >= limits.watchers { Some(Refusal::Busy) } else { None }
}

/// The person stopped watching. A watch that has ended already is no longer
/// named, and the event is dropped.
pub(crate) fn unwatch(model: &mut Model, token: Token, out: &mut Queue<Request>) {
    let Some(&id) = model.names.get(&token) else {
        return;
    };
    let watcher = model.watchers.get_mut(id).expect("a named watcher is live");
    let state = mem::replace(&mut watcher.state, Stream::Closed);
    watcher.state = match state {
        Stream::Idle => close(token, End::Unwatched, &mut model.facts, out),
        Stream::Sending { .. } | Stream::Draining { .. } => abandon(&mut watcher.backlog),
        Stream::Closing => Stream::Closing,
        Stream::Closed => unreachable!("a closed watcher is no longer named"),
    };
    conclude(model, id);
}

/// The delivery in flight to `token` has ended. One to a watch that is no
/// longer named is dropped.
pub(crate) fn delivered(model: &mut Model, token: Token, out: &mut Queue<Request>) {
    let Some(&id) = model.names.get(&token) else {
        return;
    };
    let watcher = model.watchers.get_mut(id).expect("a named watcher is live");
    let state = mem::replace(&mut watcher.state, Stream::Closed);
    watcher.state = match state {
        Stream::Idle => unreachable!("a delivery ends only while it is in flight"),
        Stream::Sending { missed } => caught_up(token, &mut watcher.backlog, missed, &mut model.facts, out),
        Stream::Draining { missed } => drained(token, &mut watcher.backlog, missed, &mut model.facts, out),
        Stream::Closing => close(token, End::Unwatched, &mut model.facts, out),
        Stream::Closed => unreachable!("a closed watcher is no longer named"),
    };
    conclude(model, id);
}

/// Offers a chunk made from `head` and `content` to each watcher of `first`
/// or of `second`, and says how many took it.
pub(crate) fn offer(
    model: &mut Model,
    first: Subject,
    second: Subject,
    head: Head,
    content: &[u8],
    out: &mut Queue<Request>,
) -> u32 {
    let mut took: u32 = 0;
    for (_, &id) in &model.names {
        let watcher = model.watchers.get_mut(id).expect("a named watcher is live");
        if watcher.subject != first && watcher.subject != second {
            continue;
        }
        let chunk = make(head, content);
        let state = mem::replace(&mut watcher.state, Stream::Closed);
        watcher.state = match state {
            Stream::Idle => {
                took = took.saturating_add(1);
                send(watcher.token, chunk, &mut model.facts, out)
            }
            Stream::Sending { missed } => {
                took = took.saturating_add(1);
                Stream::Sending { missed: wait(&mut watcher.backlog, chunk, missed, &mut model.facts) }
            }
            Stream::Draining { missed } => Stream::Draining { missed },
            Stream::Closing => Stream::Closing,
            Stream::Closed => unreachable!("a closed watcher is no longer named"),
        };
    }
    took
}

/// The run `run` has finished: its watchers end, once they have had what
/// waits for them.
pub(crate) fn finish(model: &mut Model, run: Token, out: &mut Queue<Request>) {
    // Ending a watcher unnames it, so the watchers are found first.
    let mut found = List::with_capacity(model.names.len());
    for (_, &id) in &model.names {
        let watcher = model.watchers.get(id).expect("a named watcher is live");
        if watcher.subject == Subject::Run(run) {
            found.push(id).expect("room for every watcher");
        }
    }
    for &id in &found {
        let watcher = model.watchers.get_mut(id).expect("a watcher found is live");
        let token = watcher.token;
        let state = mem::replace(&mut watcher.state, Stream::Closed);
        watcher.state = match state {
            Stream::Idle => close(token, End::Finished, &mut model.facts, out),
            Stream::Sending { missed } | Stream::Draining { missed } => Stream::Draining { missed },
            Stream::Closing => Stream::Closing,
            Stream::Closed => unreachable!("a closed watcher is no longer named"),
        };
        conclude(model, id);
    }
}

/// A copy of the chunk `head` and `content` make, for one watcher.
fn make(head: Head, content: &[u8]) -> Chunk {
    match head {
        Head::Report { run, kind, at } => Chunk::Report { run, kind, at, content: temper_lib::bytes::copy_of(content) },
        Head::Phase { item, phase, at } => Chunk::Phase { item, phase, at },
    }
}

/// Idle, a chunk: it goes out at once.
fn send(token: Token, chunk: Chunk, facts: &mut Facts, out: &mut Queue<Request>) -> Stream {
    let chunks: Box<[Chunk]> = Box::new([chunk]);
    facts.push(Fact::Delivered { chunks: 1 });
    out.push(Request::Deliver { watcher: token, missed: 0, chunks });
    Stream::Sending { missed: 0 }
}

/// A delivery in flight, a chunk: it waits, and if the backlog is full, what
/// waits is missed. Returns what has been missed since the delivery went.
fn wait(backlog: &mut Queue<Chunk>, chunk: Chunk, missed: u64, facts: &mut Facts) -> u64 {
    let Err(chunk) = backlog.try_push(chunk) else {
        return missed;
    };
    let dropped = backlog.len();
    clear(backlog);
    facts.push(Fact::Overflowed { missed: dropped });
    backlog.try_push(chunk).expect("a backlog has room for a chunk once it is cleared");
    missed.saturating_add(u64::from(dropped))
}

/// Sending, delivered: what waits goes out, or the watcher is caught up.
fn caught_up(
    token: Token,
    backlog: &mut Queue<Chunk>,
    missed: u64,
    facts: &mut Facts,
    out: &mut Queue<Request>,
) -> Stream {
    if backlog.is_empty() {
        return Stream::Idle;
    }
    deliver(token, backlog, missed, facts, out);
    Stream::Sending { missed: 0 }
}

/// Draining, delivered: what waits goes out, or the watch is over.
fn drained(
    token: Token,
    backlog: &mut Queue<Chunk>,
    missed: u64,
    facts: &mut Facts,
    out: &mut Queue<Request>,
) -> Stream {
    if backlog.is_empty() {
        return close(token, End::Finished, facts, out);
    }
    deliver(token, backlog, missed, facts, out);
    Stream::Draining { missed: 0 }
}

/// A delivery in flight, unwatched: what waits is dropped, and the watch
/// ends once the delivery has.
fn abandon(backlog: &mut Queue<Chunk>) -> Stream {
    clear(backlog);
    Stream::Closing
}

/// The watch ends, for `end`.
fn close(token: Token, end: End, facts: &mut Facts, out: &mut Queue<Request>) -> Stream {
    facts.push(Fact::Ended { end });
    out.push(Request::Ended { watcher: token, end });
    Stream::Closed
}

/// Sends the whole backlog in one delivery, told what was missed before it.
fn deliver(token: Token, backlog: &mut Queue<Chunk>, missed: u64, facts: &mut Facts, out: &mut Queue<Request>) {
    let count = backlog.len();
    let mut chunks = List::with_capacity(count);
    for _ in 0..count {
        if let Some(chunk) = backlog.pop() {
            chunks.push(chunk).expect("room for the backlog");
        }
    }
    facts.push(Fact::Delivered { chunks: count });
    out.push(Request::Deliver { watcher: token, missed, chunks: chunks.into_boxed() });
}

/// Drops what waits in `backlog`.
fn clear(backlog: &mut Queue<Chunk>) {
    for _ in 0..backlog.len() {
        if backlog.pop().is_none() {
            return;
        }
    }
}

/// What the watcher's new state implies: a closed watcher is unnamed and
/// retired, its token the parent's again.
fn conclude(model: &mut Model, id: Id<Watcher>) {
    let watcher = model.watchers.get(id).expect("a watcher concluded is live");
    match watcher.state {
        Stream::Closed => {
            let token = watcher.token;
            let named = model.names.remove(&token);
            assert!(named == Some(id), "a watcher's name is its own");
            model.watchers.retire(id);
        }
        Stream::Idle | Stream::Sending { .. } | Stream::Draining { .. } | Stream::Closing => {}
    }
}
