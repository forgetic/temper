//! Root-owned store shapes and concrete child wrappers (domain/engine.md, 5.4).
//! The root gathers these atomically; the store protocol encodes them.
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
    Turn {
        task: u64,
        attempt: u64,
        turn: u32,
    },
    /// Store key for the tasks child's record, including funding and receipts (domain/tasks.md, 2).
    Tasks(
        /// Tasks-issued key, preserved under the root wrapper without reinterpretation (domain/tasks.md, 2).
        temper_engine_domain_tasks::Key,
    ),
    /// Store key for people identities, sign-ins, roles and keyed replies (domain/people.md, 11).
    People(
        /// People-issued key for its durable secret-free records (domain/people.md, 11).
        temper_engine_domain_people::Key,
    ),
}
/// Root-owned page ranges (domain/engine.md, section 5.3).
/// The root asks the store for strictly ordered rows from one range; child
/// ranges join here with their record wrappers when routing is implemented.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Range {
    /// The singleton durable deployment header; it has no continuation.
    Deployment,
    /// Root startup reads live task rows, funding and replay admissions (domain/engine.md, 6).
    Tasks,
    /// Root startup reads every people child row before accepting people (domain/engine.md, 6).
    People,
    /// Root reads one historical ended task for an authenticated result page (domain/people.md, 6).
    TaskResult {
        /// Positive root-issued ended task key; terminal page has at most one row (domain/people.md, 6).
        task: u64,
    },
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
                Key::Turn { .. } | Key::Tasks(_) | Key::People(_) => false,
            },
            Range::Tasks => match key {
                Key::Tasks(child) => match child {
                    temper_engine_domain_tasks::Key::Live(_)
                    | temper_engine_domain_tasks::Key::Stub(_)
                    | temper_engine_domain_tasks::Key::Message(_)
                    | temper_engine_domain_tasks::Key::Receipt(_)
                    | temper_engine_domain_tasks::Key::Offer(_)
                    | temper_engine_domain_tasks::Key::Question(_)
                    | temper_engine_domain_tasks::Key::Subscription(_)
                    | temper_engine_domain_tasks::Key::Ledger(_)
                    | temper_engine_domain_tasks::Key::Admission(_) => true,
                    temper_engine_domain_tasks::Key::Ended(_)
                    | temper_engine_domain_tasks::Key::ArchivedMessage(_)
                    | temper_engine_domain_tasks::Key::History { .. }
                    | temper_engine_domain_tasks::Key::Closure { .. }
                    | temper_engine_domain_tasks::Key::Funding(_) => false,
                },
                Key::Deployment | Key::Turn { .. } | Key::People(_) => false,
            },
            Range::People => match key {
                Key::People(_) => true,
                Key::Deployment | Key::Turn { .. } | Key::Tasks(_) => false,
            },
            Range::TaskResult { task } => match key {
                Key::Tasks(temper_engine_domain_tasks::Key::Ended(number)) => task == number,
                Key::Tasks(_) | Key::Deployment | Key::Turn { .. } | Key::People(_) => false,
            },
            Range::Turns { task, attempt } => match key {
                Key::Deployment | Key::Tasks(_) | Key::People(_) => false,
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
    /// Child's durable task/funding/admission record; saved atomically by the root (domain/engine.md, 5.6).
    Tasks(
        /// Owned child row, deep bytes checked before journal or load retention (domain/engine.md, 5.3 and 5.6).
        temper_engine_domain_tasks::Stored,
    ),
    /// Child's durable identity/session/keyed reply; saved atomically by the root (domain/engine.md, 5.6).
    People(
        /// Owned secret-free child row, deep bytes checked before retention (domain/engine.md, 5.3 and 5.6).
        temper_engine_domain_people::Stored,
    ),
}
impl Record {
    #[must_use]
    pub const fn key(&self) -> Key {
        match self {
            Record::Deployment(_) => Key::Deployment,
            Record::Turn(row) => Key::Turn { task: row.task, attempt: row.attempt, turn: row.turn },
            Record::Tasks(row) => Key::Tasks(row.key()),
            Record::People(row) => Key::People(row.key()),
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

/// Deep owned allocation bytes retained by a store row (domain/engine.md, 5.3).
/// The root validates the whole decoded page before restoring or cloning.
#[must_use]
pub fn record_bytes(record: &Record) -> Option<u64> {
    match record {
        Record::Deployment(_) => Some(0),
        Record::Turn(turn) => u64::try_from(turn.transcript.len()).ok(),
        Record::Tasks(row) => temper_engine_domain_tasks::stored_bytes(row),
        Record::People(row) => match row {
            temper_engine_domain_people::Stored::Person { identity, .. } => {
                u64::try_from(identity.login.len()).ok()?.checked_add(u64::try_from(identity.name.len()).ok()?)
            }
            temper_engine_domain_people::Stored::Roles { holdings, .. } => u64::try_from(holdings.len())
                .ok()?
                .checked_mul(u64::try_from(size_of::<temper_engine_domain_people::Holding>()).ok()?),
            temper_engine_domain_people::Stored::Answer { ask, .. } => match ask {
                temper_engine_domain_people::Ask::StartChat { words, .. } => u64::try_from(words.len()).ok(),
            },
            temper_engine_domain_people::Stored::SignIn { .. } => Some(0),
        },
    }
}
pub(crate) fn owned_bytes(write: &Write) -> Option<u64> {
    match write {
        Write::Save(record) => record_bytes(record),
        Write::Erase(_) => Some(0),
    }
}
