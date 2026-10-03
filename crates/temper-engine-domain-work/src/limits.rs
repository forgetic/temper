use temper_lib::{Deadlines, Duration, Id, Map, Queue, Slab, Token};

use crate::boundary::{Class, Item};
use crate::facts::Fact;
use crate::tracked::Tracked;

/// The work hub's limits (programming-model.md, section 7), handed by its
/// parent to every step read-only.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    /// Items tracked at once: the working set, as the hub sees it. An item
    /// beyond them is refused at the entrance, and waits there.
    pub items: u32,
    /// How each failure class is retried. A run refused before anything ran
    /// is claimed again as a transient failure is retried, counting none.
    pub retries: Retries,
    /// Inbound events kept for a run until it is placed, for each item. One
    /// beyond them stays in the item's inbox, for the next run.
    pub undelivered: u32,
    /// Facts kept until the parent drains them. Beyond them, facts are
    /// dropped and counted.
    pub facts: u32,
}

/// How each failure class is retried.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Retries {
    pub transient: Retry,
    pub permanent: Retry,
    pub run: Retry,
    pub agent: Retry,
    pub lost: Retry,
    pub invalid: Retry,
}

impl Retries {
    /// How `class` is retried.
    #[must_use]
    pub const fn of(&self, class: Class) -> Retry {
        match class {
            Class::Transient => self.transient,
            Class::Permanent => self.permanent,
            Class::Run => self.run,
            Class::Agent => self.agent,
            Class::Lost => self.lost,
            Class::Invalid => self.invalid,
        }
    }
}

/// How a failure class is retried: after a backoff that doubles with each
/// failure, from `base` up to `max`, half of it drawn at random.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Retry {
    /// The failures of the class an item is retried after. One more holds it
    /// for a person.
    pub retries: u32,
    pub base: Duration,
    pub max: Duration,
}

/// The most memory the domain holds under `limits`, in bytes (6.3), or `None`
/// if it does not fit a `u64`.
///
/// It counts the containers, their bookkeeping included, and not allocator
/// overhead. An item holds no bytes: what it passes on is named by its
/// parent's tokens, and its record's part is plain data; it keeps the tokens
/// of the inbound events its run has not taken yet.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    let tracked = Slab::<Tracked>::worst_case(limits.items)?;
    let names = Map::<Item, Id<Tracked>>::worst_case(limits.items)?;
    let alarms = Deadlines::<Id<Tracked>>::worst_case(limits.items)?;
    let facts = Queue::<Fact>::worst_case(limits.facts)?;
    let undelivered = u64::from(limits.items).checked_mul(Queue::<Token>::worst_case(limits.undelivered)?)?;
    tracked.checked_add(names)?.checked_add(alarms)?.checked_add(facts)?.checked_add(undelivered)
}
