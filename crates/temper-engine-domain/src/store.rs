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
    /// Root-issued live claim replay key; at most one per live task, erased on end (domain/engine.md, 7.2).
    RunProof {
        /// Positive task number issued by root; current claim replaces this row (domain/engine.md, 5.4).
        task: u64,
    },
    /// Root's historical typed terminal evidence; never restored into the live proof table (domain/engine.md, 7.4).
    Terminal {
        /// Positive root-issued task number (domain/engine.md, 5.4).
        task: u64,
        /// Positive root-issued claim number; immutable archive identity (domain/engine.md, 7.1).
        attempt: u64,
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
    /// Root startup reads live task rows and authentic funding counters (domain/engine.md, 6).
    Tasks,
    /// Root pages only current claim proof rows; historical turns and terminals are excluded (domain/engine.md, 6).
    RunProofs,
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
    /// Root checks store membership before retaining a row; range continuations
    /// are separately ordered and bounded (domain/engine.md, 5.3).
    #[must_use]
    pub const fn contains(self, key: Key) -> bool {
        match self {
            Range::Deployment => match key {
                Key::Deployment => true,
                Key::Turn { .. } | Key::RunProof { .. } | Key::Terminal { .. } | Key::Tasks(_) | Key::People(_) => {
                    false
                }
            },
            Range::Tasks => match key {
                Key::Tasks(child) => match child {
                    temper_engine_domain_tasks::Key::Live(_) | temper_engine_domain_tasks::Key::Ledger(_) => true,
                    temper_engine_domain_tasks::Key::Ended(_) | temper_engine_domain_tasks::Key::Closure { .. } => {
                        false
                    }
                },
                Key::Deployment | Key::Turn { .. } | Key::RunProof { .. } | Key::Terminal { .. } | Key::People(_) => {
                    false
                }
            },
            Range::People => match key {
                Key::People(_) => true,
                Key::Deployment | Key::Turn { .. } | Key::RunProof { .. } | Key::Terminal { .. } | Key::Tasks(_) => {
                    false
                }
            },
            Range::RunProofs => match key {
                Key::RunProof { task } => task != 0,
                Key::Deployment | Key::Turn { .. } | Key::Terminal { .. } | Key::Tasks(_) | Key::People(_) => false,
            },
            Range::TaskResult { task } => match key {
                Key::Tasks(temper_engine_domain_tasks::Key::Ended(number)) => task == number,
                Key::Tasks(_)
                | Key::Deployment
                | Key::Turn { .. }
                | Key::RunProof { .. }
                | Key::Terminal { .. }
                | Key::People(_) => false,
            },
            Range::Turns { task, attempt } => match key {
                Key::Turn { task: found, attempt: run, turn } => found == task && run == attempt && turn != 0,
                Key::Deployment | Key::RunProof { .. } | Key::Terminal { .. } | Key::Tasks(_) | Key::People(_) => false,
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

/// Root's accepted terminal identity (worker offer or actual root-translated
/// unpriced terminal), saved atomically with task and
/// financial writes before ACK; result bytes obey root admission (domain/engine.md, 7.4).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct TerminalRecord {
    /// Root-issued positive durable task identity (domain/engine.md, 5.4).
    pub task: u64,
    /// Root-issued positive attempt identity; unchanged on replay (domain/engine.md, 7.1).
    pub attempt: u64,
    /// Accepted cumulative expense for a priced worker offer or unchanged expense for an unpriced root terminal; child counters change once (domain/engine.md, 7.4).
    pub cumulative: u64,
    /// Exact bounded worker terminal, even when child lifecycle normalizes it; a refused answer archives root's unpriced Invalid normalization, and topology routes archive their actual Lost/Refused terminal (domain/engine.md, 7.4).
    pub end: temper_engine_domain_tasks::End,
}

/// Root's latest accepted turn metadata; the full transcript stays only in
/// the immutable turn archive. Fleet fences earlier bodies (domain/engine.md, 7.2).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TurnProof {
    /// Positive consecutive accepted turn, supplied by the worker (domain/engine.md, 7.2).
    pub turn: u32,
    /// Accepted cumulative priced spend; tasks owns all financial counters (domain/engine.md, 7.2).
    pub cumulative: u64,
    /// Admitted message fence; currently none until an actual root inbox route (domain/engine.md, 7.2).
    pub read: Option<u64>,
}

/// Root's current claim evidence, store to root on paged startup. At most
/// tasks.tasks rows are kept; each has one latest turn and one terminal.
/// Claim replaces the row; ending erases it (domain/engine.md, 6 and 7.2).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct RunProof {
    /// Positive task identity allocated by root; unique in the live proof table (domain/engine.md, 5.4).
    pub task: u64,
    /// Positive current claim identity allocated by root (domain/engine.md, 7.1).
    pub attempt: u64,
    /// Latest accepted turn, or none before the first turn; no historical body is retained (domain/engine.md, 7.2).
    pub turn: Option<TurnProof>,
    /// Latest typed terminal for this claim, or none; cleared on replacement/end (domain/engine.md, 7.4).
    pub terminal: Option<TerminalRecord>,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Record {
    Deployment(Deployment),
    Turn(TurnRecord),
    /// Current bounded replay evidence; proof and all financial writes share one commit (domain/engine.md, 7.2).
    RunProof(
        /// Root-owned proof, bounded by tasks.tasks, one latest turn and journal bytes (domain/engine.md, 6 and 7.2).
        RunProof,
    ),
    /// Immutable typed terminal archive; startup never retains the historical family (domain/engine.md, 7.4).
    Terminal(
        /// Root-owned accepted worker terminal, within result admission bounds (domain/engine.md, 7.4).
        TerminalRecord,
    ),
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
            Record::RunProof(row) => Key::RunProof { task: row.task },
            Record::Terminal(row) => Key::Terminal { task: row.task, attempt: row.attempt },
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
        Record::RunProof(row) => match &row.terminal {
            Some(terminal) => terminal_bytes(terminal),
            None => Some(0),
        },
        Record::Terminal(row) => terminal_bytes(row),
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

fn terminal_bytes(row: &TerminalRecord) -> Option<u64> {
    match &row.end {
        temper_engine_domain_tasks::End::Finished { result, .. } => match result {
            temper_engine_domain_tasks::TaskResult::Report { words }
            | temper_engine_domain_tasks::TaskResult::Verdict { words, .. }
            | temper_engine_domain_tasks::TaskResult::Change { words, .. } => u64::try_from(words.len()).ok(),
            temper_engine_domain_tasks::TaskResult::Failure { reason } => u64::try_from(reason.len()).ok(),
        },
        temper_engine_domain_tasks::End::Parked
        | temper_engine_domain_tasks::End::Failed(_)
        | temper_engine_domain_tasks::End::Refused => Some(0),
    }
}
