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
/// Root-owned ranges. Children's ranges join with their record wrappers.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Range {
    Deployment,
    Turns { task: u64, attempt: u64 },
}
impl Range {
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
