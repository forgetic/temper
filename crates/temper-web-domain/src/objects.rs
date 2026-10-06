//! One page's objects, held once by key and named by typed handles.
use crate::{Choice, Person, Refusal};
use alloc::boxed::Box;
use skein_lib::{Id, Map, Slab, Time, Token, Wall};

/// Stable object identity in one page's working set.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum ObjectKey {
    Task(u64),
    Escalation { task: u64 },
    Result { task: u64 },
}

/// Typed phase sufficient for the first chat page.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TaskPhase {
    Starting,
    Running,
    Parked,
    Held(HoldReason),
    Ended(EndKind),
}

/// Why a task is waiting for a person's decision.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HoldReason {
    Tries,
    Budget,
    Stopped,
}

/// How a task ended.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EndKind {
    Done,
    Failed,
    Cancelled,
}

/// A small task summary in a page header or card.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Chip {
    pub task: u64,
    pub project: u32,
    pub title: Box<[u8]>,
    pub phase: TaskPhase,
    pub spent: u64,
    pub budget: u64,
    pub revision: u64,
}

/// Actions the engine currently offers on a held task.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Offers {
    pub release: bool,
    pub leave_held: bool,
    pub pass_up: bool,
}

/// A held task's decision card.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Escalation {
    pub task: u64,
    pub reason: HoldReason,
    pub waiting_since: Wall,
    pub offers: Offers,
    pub revision: u64,
}

/// An ended task's report or failure, with words read by the view.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TaskResult {
    pub task: u64,
    pub kind: EndKind,
    pub words: Box<[u8]>,
    pub revision: u64,
}

/// The typed body of a page object.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Body {
    Chip(Chip),
    Escalation(Escalation),
    Ended(TaskResult),
}

/// Why an object is leaving after a short linger.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Why {
    Decided { by: Person, choice: Choice, at: Wall },
    Withdrawn,
    Ended,
}

/// Where a card stands with the person.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Card {
    Open,
    Deciding { request: Token },
    Refused { refusal: Refusal },
    Leaving { why: Why, until: Time },
}

/// One page object; `sources` is a bitset of live watch and read owners.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Object {
    pub key: ObjectKey,
    pub revision: u64,
    pub body: Body,
    pub card: Card,
    pub sources: u8,
}

/// Bounded keyed working set, cleared when its page leaves.
#[derive(Debug)]
pub(crate) struct Objects {
    pub slab: Slab<Object>,
    pub index: Map<ObjectKey, Id<Object>>,
}

impl Objects {
    pub fn new(capacity: u32) -> Objects {
        Objects { slab: Slab::with_capacity(capacity), index: Map::with_capacity(capacity) }
    }

    pub fn get(&self, id: Id<Object>) -> Option<&Object> {
        let object = self.slab.get(id)?;
        if self.index.get(&object.key) == Some(&id) { Some(object) } else { None }
    }

    pub fn get_mut(&mut self, id: Id<Object>) -> Option<&mut Object> {
        let key = self.slab.get(id)?.key;
        if self.index.get(&key) == Some(&id) { self.slab.get_mut(id) } else { None }
    }

    pub fn find(&self, key: ObjectKey) -> Option<Id<Object>> {
        self.index.get(&key).copied()
    }

    pub fn put(&mut self, key: ObjectKey, revision: u64, body: Body, source: u8) -> Option<Id<Object>> {
        if let Some(id) = self.find(key) {
            let object = self.slab.get_mut(id).expect("indexed object exists");
            if object.revision != revision {
                match object.card {
                    Card::Deciding { .. } => {}
                    Card::Open | Card::Refused { .. } | Card::Leaving { .. } => object.card = Card::Open,
                }
            }
            object.revision = revision;
            object.body = body;
            object.sources |= source;
            return Some(id);
        }
        let object = Object { key, revision, body, card: Card::Open, sources: source };
        let Ok(id) = self.slab.insert(object) else {
            return None;
        };
        self.index.insert(key, id).expect("object index matches slab capacity");
        Some(id)
    }

    pub fn remove(&mut self, key: ObjectKey) {
        if let Some(id) = self.index.remove(&key) {
            self.slab.retire(id);
        }
    }

    pub fn clear(&mut self) {
        let mut keys = skein_lib::List::with_capacity(self.index.capacity());
        for (key, _) in &self.index {
            keys.push(*key).expect("keys fit index capacity");
        }
        for key in &keys {
            self.remove(*key);
        }
    }

    pub fn clear_source(&mut self, source: u8) {
        for (_, id) in &self.index {
            let object = self.slab.get_mut(*id).expect("indexed object exists");
            object.sources &= !source;
        }
    }

    pub fn prune(&mut self) {
        let mut keys = skein_lib::List::with_capacity(self.index.capacity());
        for (key, id) in &self.index {
            let object = self.slab.get(*id).expect("indexed object exists");
            if object.sources == 0 {
                match object.card {
                    Card::Open | Card::Refused { .. } => keys.push(*key).expect("keys fit index capacity"),
                    Card::Deciding { .. } | Card::Leaving { .. } => {}
                }
            }
        }
        for key in &keys {
            self.remove(*key);
        }
    }
}
