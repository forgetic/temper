//! Briefs being gathered: a brief asks its parent to read every section at
//! once, takes each read's content as it arrives, and is rendered (see
//! `cut`) when the last has arrived, or with what it has when its time runs
//! out, the sections not read missing. A required section that cannot be
//! read fails the brief, at once: nothing would make it worth rendering.
//!
//! A brief that has answered while reads are still in flight settles (5.3):
//! it keeps its slot, holding nothing but their count, until each read's
//! terminal has arrived, and drops what they bring. Its reads never outlive
//! it, and the parent's contract (one terminal per read) bounds how long it
//! settles.
//!
//! The transition table. Every other cell is unreachable by the boundary's
//! contract and the loop's rules: a read's terminal only for a read in
//! flight, whose brief lives until its last read has ended; and a deadline
//! armed only while a brief gathers.
//!
//! ```text
//! state      event or alarm                          next       requests
//! (none)     render, admitted                        Gathering  a read per section
//!            render, of no sections                  (none)     rendered
//!            render, busy or oversized               (none)     refused
//! Gathering  read, content, more in flight           Gathering
//!            read, content, the last                 Closed     rendered
//!            read failed or oversized, optional      as for content, the section missing
//!            read failed or oversized, required,
//!              more in flight                        Settling   failed
//!              the last                              Closed     failed
//!            deadline, a required section not read  Settling   failed
//!            deadline, otherwise                     Settling   rendered, those not read missing
//! Settling   read, more in flight                    Settling   (dropped)
//!            read, the last                          Closed     (dropped)
//! ```

use alloc::boxed::Box;
use core::mem;

use temper_lib::{Deadlines, Env, Id, List, Queue, ReplyTo, Slab, Time, Token};

use crate::boundary::{Kind, Part, Read, Refusal, Request, Source, Wanted};
use crate::cut;
use crate::facts::{Fact, Facts, Gathered};
use crate::limits::Limits;
use crate::model::Model;

/// A brief, from its render to its last read's end.
#[derive(Debug)]
pub(crate) struct Brief {
    pub(crate) state: State,
}

#[derive(Debug)]
pub(crate) enum State {
    /// Reads are in flight for `waiting` of its sections; its deadline,
    /// `until`, runs.
    Gathering { reply_to: ReplyTo, slots: List<Slot>, waiting: u32, until: Time },
    /// Answered, with `late` reads still in flight.
    Settling { late: u32 },
    /// Terminal: holds nothing.
    Closed,
}

/// A section of a brief being gathered, in the order the parent asked for
/// it.
#[derive(Debug)]
pub(crate) struct Slot {
    pub(crate) kind: Kind,
    pub(crate) required: bool,
    pub(crate) content: Content,
}

#[derive(Debug)]
pub(crate) enum Content {
    /// Its read is in flight.
    Asked,
    Got(Box<[Part]>),
    /// Its read failed, brought more than a read may, or did not end in
    /// time.
    Missing,
}

/// A read in flight: the brief it is for, and the slot it fills. Its `owner`
/// token is its handle's.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Reading {
    pub(crate) brief: Id<Brief>,
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
    if let Some(refusal) = refusal(&model.briefs, limits, &wanted) {
        model.facts.push(Fact::Refused { refusal });
        out.push(Request::Refused { reply_to, refusal });
        return;
    }
    let sections = u32::try_from(wanted.len()).expect("no more sections than the limits");
    if sections == 0 {
        model.facts.push(Fact::Rendered { sections, cut: 0, missing: 0 });
        out.push(Request::Rendered { reply_to, sections: List::with_capacity(0).into_boxed() });
        return;
    }
    let id = model.briefs.insert(Brief { state: State::Closed }).expect("room for a brief, checked at the entrance");
    let mut slots = List::with_capacity(sections);
    for (index, wanted) in wanted.into_iter().enumerate() {
        let index = u32::try_from(index).expect("no more sections than the limits");
        let Wanted { source, required } = wanted;
        let kind = source.kind();
        let reading = Reading { brief: id, index };
        let reading = model.reads.insert(reading).expect("room for a read per section of each brief");
        slots.push(Slot { kind, required, content: Content::Asked }).expect("room for each section");
        out.push(ask(reading.token(), source, kind, limits));
    }
    model.facts.push(Fact::Gathering { sections });
    let until = env.now.saturating_add(limits.gather);
    let brief = model.briefs.get_mut(id).expect("inserted above");
    brief.state = State::Gathering { reply_to, slots, waiting: sections, until };
    follow(&mut model.briefs, &mut model.alarms, id);
}

/// Why a render is refused at the entrance, if it is: past the limits, or
/// with no room for one more brief.
fn refusal(briefs: &Slab<Brief>, limits: &Limits, wanted: &[Wanted]) -> Option<Refusal> {
    let sections = u32::try_from(wanted.len()).unwrap_or(u32::MAX);
    if sections > limits.sections {
        return Some(Refusal::Oversized);
    }
    for wanted in wanted {
        let items = match &wanted.source {
            Source::Dependencies(items) => u32::try_from(items.len()).unwrap_or(u32::MAX),
            Source::Item(_)
            | Source::Comments { .. }
            | Source::Ci { .. }
            | Source::Reviews { .. }
            | Source::Pull { .. }
            | Source::Attempts(_)
            | Source::Plan { .. }
            | Source::Notes { .. }
            | Source::Template(_) => 0,
        };
        if items > limits.items {
            return Some(Refusal::Oversized);
        }
    }
    if sections > 0 && briefs.is_full() {
        return Some(Refusal::Busy);
    }
    None
}

/// The read of `source`, for a section of `kind`, within the limits.
fn ask(owner: Token, source: Source, kind: Kind, limits: &Limits) -> Request {
    Request::Read { owner, source, keep: cut::keep(kind), parts: limits.parts, bytes: limits.read_bytes }
}

/// A read's terminal: its content fills its slot, or its brief, settling,
/// drops it.
pub(crate) fn read(model: &mut Model, env: &Env<Limits>, owner: Token, read: Read, out: &mut Queue<Request>) {
    let reading = Id::from_token(owner);
    let Reading { brief: id, index } = *model.reads.get(reading).expect("a read's terminal names a read in flight");
    model.reads.retire(reading);
    let (content, gathered) = take(&env.limits, read);
    let brief = model.briefs.get_mut(id).expect("a brief lives until its last read has ended");
    let state = mem::replace(&mut brief.state, State::Closed);
    brief.state = match state {
        State::Gathering { reply_to, slots, waiting, until } => {
            model.facts.push(Fact::Read { read: gathered });
            let gathering = Gathering { reply_to, slots, waiting, until };
            filled(gathering, index, content, &env.limits, &mut model.facts, out)
        }
        State::Settling { late } => {
            model.facts.push(Fact::Read { read: Gathered::Late });
            settled(late)
        }
        State::Closed => unreachable!("a closed brief has no read in flight"),
    };
    follow(&mut model.briefs, &mut model.alarms, id);
}

/// A brief's deadline: it answers with what it has.
pub(crate) fn expire(model: &mut Model, env: &Env<Limits>, id: Id<Brief>, out: &mut Queue<Request>) {
    let brief = model.briefs.get_mut(id).expect("an alarm names a brief that lives");
    let state = mem::replace(&mut brief.state, State::Closed);
    brief.state = match state {
        State::Gathering { reply_to, slots, waiting, until } => {
            let gathering = Gathering { reply_to, slots, waiting, until };
            expired(gathering, &env.limits, &mut model.facts, out)
        }
        State::Settling { .. } | State::Closed => unreachable!("only a gathering brief has its deadline armed"),
    };
    follow(&mut model.briefs, &mut model.alarms, id);
}

/// What a read brought: content within the limits, or a missing section.
fn take(limits: &Limits, read: Read) -> (Content, Gathered) {
    match read {
        Read::Got(parts) => {
            if within(limits, &parts) {
                (Content::Got(parts), Gathered::Got)
            } else {
                (Content::Missing, Gathered::Oversized)
            }
        }
        Read::Failed => (Content::Missing, Gathered::Failed),
    }
}

/// Whether `parts` are within what a read may bring.
fn within(limits: &Limits, parts: &[Part]) -> bool {
    let Ok(count) = u32::try_from(parts.len()) else {
        return false;
    };
    let mut bytes = 0_usize;
    for part in parts {
        bytes = bytes.saturating_add(part.bytes.len());
    }
    let most = usize::try_from(limits.read_bytes).expect("a u32 fits a usize");
    count <= limits.parts && bytes <= most
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
    content: Content,
    limits: &Limits,
    facts: &mut Facts,
    out: &mut Queue<Request>,
) -> State {
    let Gathering { reply_to, mut slots, waiting, until } = gathering;
    let missing = match &content {
        Content::Missing => true,
        Content::Asked | Content::Got(_) => false,
    };
    let slot = slots.get_mut(index).expect("a read fills a section of its brief");
    let (kind, required) = (slot.kind, slot.required);
    slot.content = content;
    let waiting = waiting.checked_sub(1).expect("a brief waits for each read in flight");
    if missing && required {
        return failed(reply_to, kind, waiting, facts, out);
    }
    if waiting > 0 {
        return State::Gathering { reply_to, slots, waiting, until };
    }
    rendered(reply_to, &slots, limits, facts, out);
    State::Closed
}

/// Gathering, its deadline: the sections not read are missing; it fails if
/// one is required, and is rendered otherwise.
fn expired(gathering: Gathering, limits: &Limits, facts: &mut Facts, out: &mut Queue<Request>) -> State {
    let Gathering { reply_to, mut slots, waiting, until: _ } = gathering;
    facts.push(Fact::Expired { waiting });
    let mut missing = None;
    for index in 0..slots.len() {
        let slot = slots.get_mut(index).expect("within the slots");
        let unread = match &slot.content {
            Content::Asked => true,
            Content::Got(_) | Content::Missing => false,
        };
        if unread {
            slot.content = Content::Missing;
            if slot.required && missing.is_none() {
                missing = Some(slot.kind);
            }
        }
    }
    if let Some(kind) = missing {
        return failed(reply_to, kind, waiting, facts, out);
    }
    rendered(reply_to, &slots, limits, facts, out);
    settled_after(waiting)
}

/// Settling, a late read ended: its content is dropped.
fn settled(late: u32) -> State {
    settled_after(late.checked_sub(1).expect("a settling brief waits for each read in flight"))
}

/// A brief that has answered: settling while `late` reads are in flight.
fn settled_after(late: u32) -> State {
    if late > 0 { State::Settling { late } } else { State::Closed }
}

/// The brief fails for want of the required section of `kind`.
fn failed(reply_to: ReplyTo, kind: Kind, waiting: u32, facts: &mut Facts, out: &mut Queue<Request>) -> State {
    facts.push(Fact::Failed { missing: kind });
    out.push(Request::Failed { reply_to, missing: kind });
    settled_after(waiting)
}

/// The brief is rendered from `slots`.
fn rendered(reply_to: ReplyTo, slots: &List<Slot>, limits: &Limits, facts: &mut Facts, out: &mut Queue<Request>) {
    let brief = cut::brief(slots.as_slice(), limits);
    facts.push(Fact::Rendered { sections: slots.len(), cut: brief.cut, missing: brief.missing });
    out.push(Request::Rendered { reply_to, sections: brief.sections });
}

/// What a brief's state implies, applied after every transition: whether
/// its deadline runs, and whether it is retired.
fn follow(briefs: &mut Slab<Brief>, alarms: &mut Deadlines<Id<Brief>>, id: Id<Brief>) {
    let brief = briefs.get(id).expect("a brief lives until it is retired");
    let (deadline, closed) = match &brief.state {
        State::Gathering { until, .. } => (Some(*until), false),
        State::Settling { .. } => (None, false),
        State::Closed => (None, true),
    };
    match deadline {
        Some(at) => alarms.arm(id, at).expect("the alarm table has room for a deadline per brief"),
        None => alarms.cancel(id),
    }
    if closed {
        briefs.retire(id);
    }
}
