//! Whether a session without a live run is woken by what is in its inbox
//! (engine-model.md, 4.3 and 5.4): now, later (at a time its batch or its
//! timer says, unless more events come first), or not until something else
//! comes. A wake rule lets through the events of the sources it names, once
//! there are at least as many as its batch counts or the oldest is as old as
//! its batch allows; a person's message it names is let through at once,
//! since a person waits for the answer. Its timer wakes it on its own, a
//! while after its last turn.
//!
//! Only sessions have wake rules: every other step is asked what is due
//! whenever what it reads changes. Every event given is read, however many
//! come from sources the rule does not name, so no message is lost behind
//! them; the parent bounds the inbox it keeps.

use temper_lib::{Env, Time};

use crate::limits::Limits;
use crate::plan::Wake;

/// An event in an item's inbox: where it comes from, and when it happened.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Inbound {
    pub source: Source,
    pub at: Time,
}

/// Where an inbox event comes from.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Source {
    /// The item's own pull request: CI, a review, a push.
    Own,
    /// One of the steps it comes after, finishing.
    Dependency,
    /// One of the steps it added, finishing or held.
    Child,
    /// An item it subscribes to, changing.
    Subscribed,
    /// A person's message on it.
    Message,
}

/// Whether an item is woken.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Woken {
    Now,
    /// At this time, unless more events wake it first.
    At(Time),
    /// Not until more events come.
    No,
}

/// Whether `inbox`, the events since the item's inbox position, wakes a
/// session whose wake rule is `rule` and whose last turn was at `last` (or
/// which was made then). It stops reading once it knows the session wakes
/// now.
#[must_use]
pub fn wake(env: &Env<Limits>, rule: &Wake, inbox: &[Inbound], last: Time) -> Woken {
    let on = rule.on;
    let mut count: u32 = 0;
    let mut oldest: Option<Time> = None;
    for event in inbox {
        let (wakes, message) = match event.source {
            Source::Own => (on.own, false),
            Source::Dependency | Source::Child => (on.related, false),
            Source::Subscribed => (on.subscribed, false),
            Source::Message => (on.messages, true),
        };
        if !wakes {
            continue;
        }
        count = count.saturating_add(1);
        if message || count >= rule.batch.count {
            return Woken::Now;
        }
        oldest = Some(match oldest {
            Some(earlier) => earlier.min(event.at),
            None => event.at,
        });
    }
    let mut at: Option<Time> = None;
    if let Some(oldest) = oldest
        && let Some(age) = rule.batch.age
    {
        at = Some(oldest.saturating_add(age));
    }
    if let Some(every) = rule.every {
        let timer = last.saturating_add(every);
        at = Some(match at {
            Some(batch) => batch.min(timer),
            None => timer,
        });
    }
    match at {
        Some(at) if at <= env.now => Woken::Now,
        Some(at) => Woken::At(at),
        None => Woken::No,
    }
}
