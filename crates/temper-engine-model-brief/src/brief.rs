//! Briefs being gathered: a brief asks its parent to read every section at
//! once, takes each read's content as it arrives, and is rendered (see
//! `cut`) when the last has arrived, or with what it has when its time runs
//! out, the sections not read missing. A required section that cannot be
//! read fails the brief, at once: nothing would make it worth rendering.
//!
//! A brief that answers is retired at once. Its reads still in flight are
//! orphaned (5.3): each keeps its place in the reads until its terminal
//! arrives, which is dropped, and holds nothing else. A render is admitted
//! only while there is room for one more brief and for as many reads as a
//! brief may have, the orphans' included; a render refused as busy is owed
//! a notice, [`Request::Room`], once there is room again.
//!
//! The transition table. Every other cell is unreachable by the boundary's
//! contract and the loop's rules: a read's terminal only for a read in
//! flight, which an answered brief no longer names; and a deadline armed
//! only while a brief gathers.
//!
//! ```text
//! state      event or alarm                          next       requests
//! (none)     render, admitted                        Gathering  a read per section
//!            render, of no sections                  (none)     rendered
//!            render, busy or oversized               (none)     refused
//! Gathering  read, content, more in flight           Gathering
//!            read, content, the last                 Closed     rendered
//!            read failed or oversized, optional      as for content, the section missing
//!            read failed or oversized, required      Closed     failed
//!            deadline, a required section not read  Closed     failed (late)
//!            deadline, otherwise                     Closed     rendered, those not read missing (late)
//! (orphan)   read                                    (none)     (dropped)
//! ```
//!
//! Any of them may end with a room notice, when it frees room after a
//! refusal.

use alloc::boxed::Box;
use core::mem;

use temper_lib::{Env, Id, List, Queue, ReplyTo, Slab, Time, Token};

use crate::boundary::{Kind, Part, Read, Refusal, Request, Source, Unread, Wanted};
use crate::cut;
use crate::facts::{Fact, Facts, Gathered};
use crate::limits::{self, Limits};
use crate::model::Model;

/// A brief, from its render to its answer.
#[derive(Debug)]
pub(crate) struct Brief {
    pub(crate) state: State,
}

#[derive(Debug)]
pub(crate) enum State {
    /// Reads are in flight for `waiting` of its sections; its deadline,
    /// `until`, runs.
    Gathering { reply_to: ReplyTo, slots: List<Slot>, waiting: u32, until: Time },
    /// Terminal: answered, and retired. Holds nothing.
    Closed,
}

/// A section of a brief being gathered, in the order the parent asked for
/// it, and the items its source named past what a source may, cut at the
/// entrance.
#[derive(Debug)]
pub(crate) struct Slot {
    pub(crate) kind: Kind,
    pub(crate) required: bool,
    pub(crate) content: Content,
    pub(crate) items: u32,
}

#[derive(Debug)]
pub(crate) enum Content {
    /// Its read is in flight.
    Asked(Id<Reading>),
    Got(Box<[Part]>),
    Missing(Unread),
}

/// A read in flight: the brief it is for and the slot it fills, or none
/// once that brief has answered. Its `owner` token is its handle's.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Reading {
    pub(crate) brief: Option<Id<Brief>>,
    pub(crate) index: u32,
}

/// A render: refused at the entrance, answered at once if it has no
/// sections, or gathered, a read for each section.
pub(crate) fn render(
    model: &mut Model,
    env: &Env<Limits>,
    reply_to: ReplyTo,
    wanted: Box<[Wanted]>,
    out: &mut Queue<Request>,
) {
    let limits = &env.limits;
    let Ok(sections) = u32::try_from(wanted.len()) else {
        return refuse(model, reply_to, Refusal::Oversized, out);
    };
    if sections > limits.sections {
        return refuse(model, reply_to, Refusal::Oversized, out);
    }
    if sections == 0 {
        model.facts.push(Fact::Rendered { sections, cut: 0, missing: 0 });
        out.push(Request::Rendered { reply_to, sections: List::with_capacity(0).into_boxed() });
        return;
    }
    if !room(model, limits) {
        model.owed = true;
        return refuse(model, reply_to, Refusal::Busy, out);
    }
    let id = model.briefs.insert(Brief { state: State::Closed }).expect("room for a brief, checked at the entrance");
    model.gathering = model.gathering.checked_add(1).expect("no more briefs than the limits");
    let mut slots = List::with_capacity(sections);
    for (index, wanted) in wanted.into_iter().enumerate() {
        let index = u32::try_from(index).expect("no more sections than the limits");
        let Wanted { source, required } = wanted;
        let kind = source.kind();
        let (source, items) = bounded(source, limits.items);
        let reading = Reading { brief: Some(id), index };
        let reading = model.reads.insert(reading).expect("room for the reads of a brief, checked at the entrance");
        model.reading = model.reading.checked_add(1).expect("no more reads than the limits");
        let slot = Slot { kind, required, content: Content::Asked(reading), items };
        slots.push(slot).expect("room for each section");
        out.push(cut::read(reading.token(), source, limits));
    }
    model.facts.push(Fact::Gathering { sections });
    let until = env.now.saturating_add(limits.gather);
    let brief = model.briefs.get_mut(id).expect("inserted above");
    brief.state = State::Gathering { reply_to, slots, waiting: sections, until };
    follow(model, id, out, &env.limits);
}

fn refuse(model: &mut Model, reply_to: ReplyTo, refusal: Refusal, out: &mut Queue<Request>) {
    model.facts.push(Fact::Refused { refusal });
    out.push(Request::Refused { reply_to, refusal });
}

/// Whether a render may be admitted: room for one more brief, and for as
/// many reads as a brief may have.
pub(crate) fn room(model: &Model, limits: &Limits) -> bool {
    let reads = limits::reads(limits).expect("worst_case accepted the limits");
    model.gathering < limits.briefs && model.reading.saturating_add(limits.sections) <= reads
}

/// `source`, naming no more items than a source may, and how many items it
/// named past them, which are cut.
fn bounded(source: Source, most: u32) -> (Source, u32) {
    match source {
        Source::Dependencies(items) => {
            let named = u32::try_from(items.len()).unwrap_or(u32::MAX);
            if named <= most {
                return (Source::Dependencies(items), 0);
            }
            let mut kept = List::with_capacity(most);
            for item in items.iter().take(usize::try_from(most).expect("a u32 fits a usize")) {
                kept.push(*item).expect("room for the items kept");
            }
            (Source::Dependencies(kept.into_boxed()), named.saturating_sub(most))
        }
        source @ (Source::Item(_)
        | Source::Comments { .. }
        | Source::Ci { .. }
        | Source::Reviews { .. }
        | Source::Pull { .. }
        | Source::Attempts(_)
        | Source::Plan { .. }
        | Source::Notes { .. }
        | Source::Template(_)) => (source, 0),
    }
}

/// A read's terminal: its content fills its slot, or, if its brief has
/// answered, it is dropped.
pub(crate) fn read(model: &mut Model, env: &Env<Limits>, owner: Token, answer: Read, out: &mut Queue<Request>) {
    let reading = Id::from_token(owner);
    let Reading { brief, index } = *model.reads.get(reading).expect("a read's terminal names a read in flight");
    model.reads.retire(reading);
    model.reading = model.reading.checked_sub(1).expect("a read in flight is counted");
    let Some(id) = brief else {
        model.facts.push(Fact::Read { read: Gathered::Late });
        return notice(model, &env.limits, out);
    };
    let brief = model.briefs.get_mut(id).expect("a brief names its reads until it answers");
    let state = mem::replace(&mut brief.state, State::Closed);
    brief.state = match state {
        State::Gathering { reply_to, slots, waiting, until } => {
            let gathering = Gathering { reply_to, slots, waiting, until };
            filled(gathering, index, answer, &env.limits, &mut model.reads, &mut model.facts, out)
        }
        State::Closed => unreachable!("a closed brief names no read"),
    };
    follow(model, id, out, &env.limits);
}

/// A brief's deadline: it answers with what it has.
pub(crate) fn expire(model: &mut Model, env: &Env<Limits>, id: Id<Brief>, out: &mut Queue<Request>) {
    let brief = model.briefs.get_mut(id).expect("an alarm names a brief that lives");
    let state = mem::replace(&mut brief.state, State::Closed);
    brief.state = match state {
        State::Gathering { reply_to, slots, waiting, until } => {
            let gathering = Gathering { reply_to, slots, waiting, until };
            expired(gathering, &env.limits, &mut model.reads, &mut model.facts, out)
        }
        State::Closed => unreachable!("only a gathering brief has its deadline armed"),
    };
    follow(model, id, out, &env.limits);
}

/// What a gathering brief holds, moved out of its state.
struct Gathering {
    reply_to: ReplyTo,
    slots: List<Slot>,
    waiting: u32,
    until: Time,
}

// Cell handlers: each takes the source state's data by value and returns the
// target state.

/// Gathering, a read ended: its slot is filled; the brief fails if it was
/// required and is missing, and is rendered once it was the last.
fn filled(
    gathering: Gathering,
    index: u32,
    answer: Read,
    limits: &Limits,
    reads: &mut Slab<Reading>,
    facts: &mut Facts,
    out: &mut Queue<Request>,
) -> State {
    let Gathering { reply_to, mut slots, waiting, until } = gathering;
    let slot = slots.get_mut(index).expect("a read fills a section of its brief");
    let kind = slot.kind;
    let required = slot.required;
    let (content, gathered) = take(kind, limits, answer);
    facts.push(Fact::Read { read: gathered });
    slot.content = content;
    let waiting = waiting.checked_sub(1).expect("a brief waits for each read in flight");
    if let Some(why) = unread(gathered)
        && required
    {
        return failed(reply_to, &slots, kind, why, reads, facts, out);
    }
    if waiting > 0 {
        return State::Gathering { reply_to, slots, waiting, until };
    }
    rendered(reply_to, &slots, limits, facts, out);
    State::Closed
}

/// Gathering, its deadline: the sections not read are missing, late; it
/// fails if one is required, and is rendered otherwise.
fn expired(
    gathering: Gathering,
    limits: &Limits,
    reads: &mut Slab<Reading>,
    facts: &mut Facts,
    out: &mut Queue<Request>,
) -> State {
    let Gathering { reply_to, mut slots, waiting, until: _ } = gathering;
    facts.push(Fact::Expired { waiting });
    orphan(&slots, reads);
    let mut missing = None;
    for index in 0..slots.len() {
        let slot = slots.get_mut(index).expect("within the slots");
        let unread = match &slot.content {
            Content::Asked(_) => true,
            Content::Got(_) | Content::Missing(_) => false,
        };
        if unread {
            slot.content = Content::Missing(Unread::Late);
            if slot.required && missing.is_none() {
                missing = Some(slot.kind);
            }
        }
    }
    if let Some(kind) = missing {
        facts.push(Fact::Failed { missing: kind });
        out.push(Request::Failed { reply_to, missing: kind, why: Unread::Late });
        return State::Closed;
    }
    rendered(reply_to, &slots, limits, facts, out);
    State::Closed
}

/// What a read brought for a section of `kind`: content within what the
/// read may bring, or a missing section.
fn take(kind: Kind, limits: &Limits, answer: Read) -> (Content, Gathered) {
    match answer {
        Read::Got(parts) => {
            if within(kind, limits, &parts) {
                (Content::Got(parts), Gathered::Got)
            } else {
                (Content::Missing(Unread::Oversized), Gathered::Oversized)
            }
        }
        Read::Failed => (Content::Missing(Unread::Failed), Gathered::Failed),
    }
}

/// Why a read that ended so left its section missing, if it did.
fn unread(gathered: Gathered) -> Option<Unread> {
    match gathered {
        Gathered::Got => None,
        Gathered::Failed => Some(Unread::Failed),
        Gathered::Oversized => Some(Unread::Oversized),
        Gathered::Late => unreachable!("a read that fills a slot is in time"),
    }
}

/// Whether `parts` are within what a read for a section of `kind` may
/// bring.
fn within(kind: Kind, limits: &Limits, parts: &[Part]) -> bool {
    let Ok(count) = u32::try_from(parts.len()) else {
        return false;
    };
    let mut bytes = 0_usize;
    for part in parts {
        bytes = bytes.saturating_add(part.bytes.len());
    }
    let most = usize::try_from(cut::most(kind, limits)).expect("a u32 fits a usize");
    count <= limits.parts && bytes <= most
}

/// The brief fails for want of the required section of `kind`; its reads
/// still in flight are orphaned.
fn failed(
    reply_to: ReplyTo,
    slots: &List<Slot>,
    kind: Kind,
    why: Unread,
    reads: &mut Slab<Reading>,
    facts: &mut Facts,
    out: &mut Queue<Request>,
) -> State {
    orphan(slots, reads);
    facts.push(Fact::Failed { missing: kind });
    out.push(Request::Failed { reply_to, missing: kind, why });
    State::Closed
}

/// The brief is rendered from `slots`.
fn rendered(reply_to: ReplyTo, slots: &List<Slot>, limits: &Limits, facts: &mut Facts, out: &mut Queue<Request>) {
    let brief = cut::brief(slots.as_slice(), limits);
    facts.push(Fact::Rendered { sections: slots.len(), cut: brief.cut, missing: brief.missing });
    out.push(Request::Rendered { reply_to, sections: brief.sections });
}

/// The reads of `slots` still in flight no longer name their brief, which
/// has answered.
fn orphan(slots: &List<Slot>, reads: &mut Slab<Reading>) {
    for slot in slots {
        match &slot.content {
            Content::Asked(reading) => {
                let reading = reads.get_mut(*reading).expect("a read asked for is in flight");
                reading.brief = None;
            }
            Content::Got(_) | Content::Missing(_) => {}
        }
    }
}

/// What a brief's state implies, applied after every transition: whether
/// its deadline runs, or it has answered and is retired, which may make
/// room owed a notice.
fn follow(model: &mut Model, id: Id<Brief>, out: &mut Queue<Request>, limits: &Limits) {
    let brief = model.briefs.get(id).expect("a brief lives until it is retired");
    match &brief.state {
        State::Gathering { until, .. } => {
            model.alarms.arm(id, *until).expect("the alarm table has room for a deadline per brief");
        }
        State::Closed => {
            model.alarms.cancel(id);
            model.briefs.retire(id);
            model.gathering = model.gathering.checked_sub(1).expect("a brief gathering is counted");
            notice(model, limits, out);
        }
    }
}

/// Tells the parent there is room again, if a render was refused as busy
/// since it last heard so, and there is.
fn notice(model: &mut Model, limits: &Limits, out: &mut Queue<Request>) {
    if model.owed && room(model, limits) {
        model.owed = false;
        out.push(Request::Room);
    }
}
