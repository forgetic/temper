//! Root-owned store shapes (domain/engine.md, 5.4). Children's shapes join
//! this vocabulary when their routing is implemented; the protocol encodes it.
use alloc::boxed::Box;
use skein_lib::Wall;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Deployment {
    pub id: [u8; 16],
    pub tasks: u64,
    pub people: u64,
    pub sign_ins: u64,
    pub messages: u64,
    pub runs: u64,
    pub calls: u64,
    pub commits: u64,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Family {
    Task,
    Person,
    SignIn,
    Message,
    Run,
    Call,
}
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Key {
    Deployment,
    Turn { task: u64, attempt: u64, turn: u32 },
}
/// Root-owned page ranges (domain/engine.md, section 5.3).
/// The root asks the store for strictly ordered rows from one range; child
/// ranges join here with their record wrappers when routing is implemented.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Range {
    /// The singleton durable deployment header; it has no continuation.
    Deployment,
    /// Committed transcript turns for one nonzero task and attempt.
    Turns {
        /// Durable task number allocated by the root's deployment header.
        task: u64,
        /// Nonzero activation number; turns from other attempts cannot match.
        attempt: u64,
    },
}
impl Range {
    /// Check membership before accepting a store row (domain/engine.md, 5.3).
    /// Turn zero is invalid; continuation ordering is checked by the load owner.
    #[must_use]
    pub const fn contains(self, key: Key) -> bool {
        match self {
            Range::Deployment => match key {
                Key::Deployment => true,
                Key::Turn { .. } => false,
            },
            Range::Turns { task, attempt } => match key {
                Key::Deployment => false,
                Key::Turn { task: found, attempt: run, turn } => found == task && run == attempt && turn != 0,
            },
        }
    }
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct TurnRecord {
    pub task: u64,
    pub attempt: u64,
    pub turn: u32,
    pub spent: u64,
    pub read: Option<u64>,
    pub at: Wall,
    pub transcript: Box<[u8]>,
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Record {
    Deployment(Deployment),
    Turn(TurnRecord),
}
impl Record {
    #[must_use]
    pub const fn key(&self) -> Key {
        match self {
            Record::Deployment(_) => Key::Deployment,
            Record::Turn(row) => Key::Turn { task: row.task, attempt: row.attempt, turn: row.turn },
        }
    }
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Write {
    Save(Record),
    Erase(Key),
}
impl Write {
    #[must_use]
    pub const fn key(&self) -> Key {
        match self {
            Write::Save(row) => row.key(),
            Write::Erase(key) => *key,
        }
    }
}
