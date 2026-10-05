use crate::{Authority, Class, Funder, Numbers, Tries};
use alloc::boxed::Box;
use skein_lib::{ReplyTo, Wall};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Party {
    Task(u64,),
    Person(u64,),
    Deployment { project: u32 ,},
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Executor {
    Agent { charter: u32 ,},
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Parameter {
    Number { name: u32, value: u64 ,},
    Bytes { name: u32, value: Box<[u8]> ,},
    Resource { name: u32, connector: u16, resource: u64 ,},
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Spec {
    pub words: Box<[u8]>,
    pub parameters: Box<[Parameter]>,
    pub inputs: Box<[u64]>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Verdict {
    pub code: u32,
    pub words: u32,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Contract {
    Report { words: u32 ,},
    Verdict { choices: Box<[Verdict]> ,},
    Change { connector: u16, kind: u16, words: u32 ,},
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum TaskResult {
    Report { words: Box<[u8]> ,},
    Verdict { code: u32, words: Box<[u8]> ,},
    Change { connector: u16, kind: u16, resource: u64, words: Box<[u8]> ,},
    Failure { reason: Box<[u8]> ,},
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Ending {
    Done(TaskResult,),
    Failed { reason: Box<[u8]> ,},
    Cancelled { reason: Box<[u8]>, result: Option<TaskResult> ,},
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Status {
    Done,
    Failed,
    Cancelled,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum End {
    Finished { result: TaskResult, cancel_delegates: bool ,},
    Parked,
    Failed(Class,),
    Refused,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Hold {
    Failures(Class),
    Dependency(u64,),
    Stopped,
    Drift,
    Effects,
    Budget,
    Deadline,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Active {
    Idle,
    Due,
    Preparing,
    Claimed { attempt: u64 ,},
    Running { attempt: u64 ,},
    BackingOff { until: Wall ,},
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Stage {
    Run { attempt: u64 ,},
    Delegates,
    Effects,
    Settled,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Closing {
    pub stage: Stage,
    pub ending: Ending,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Was {
    Waiting,
    Active(Active,),
    Closing(Closing),
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Phase {
    Waiting,
    Active(Active),
    Closing(Closing),
    Held { was: Was, why: Hold ,},
    Ended(Ending,),
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct New {
    pub number: u64,
    pub project: u32,
    pub executor: Executor,
    pub spec: Spec,
    pub contract: Contract,
    pub authority: Authority,
    pub numbers: Numbers,
    pub funder: Funder,
    pub dependencies: Box<[u64]>,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct TaskRecord {
    pub number: u64,
    pub project: u32,
    pub requester: Party,
    /// Current durable allotment generation; replacement never resets `run_spent`.
    pub allotment: u64,
    pub historical_spend: u64,
    pub run_spent: u64,
    pub root: u64,
    pub depth: u32,
    pub executor: Executor,
    pub spec: Spec,
    pub contract: Contract,
    pub authority: Authority,
    pub numbers: Numbers,
    pub funder: Funder,
    pub dependencies: Box<[u64]>,
    /// Remaining live dependency identities, a subset of `dependencies`;
    /// tasks removes each once its dependency ends in the same decision.
    /// Bounded by dependencies; this is task readiness, not a historical
    /// root stub cache (domain/tasks.md, 4 and 5.1).
    pub waiting_on: Box<[u64]>,
    pub delegates: Box<[u64]>,
    pub turn: u32,
    /// Tasks made in this subtree over its life, including itself.
    pub made: u32,
    pub attempt: u64,
    pub last_answer: Option<u64>,
    pub tries: Tries,
    pub refusals: u32,
    pub phase: Phase,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Key {
    Live(u64),
    Ended(u64,),
    Ledger(Funder),
    Closure { task: u64, generation: u64 ,},
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Stored {
    Live(Box<TaskRecord>,),
    Ended(Box<TaskRecord>,),
    Ledger(crate::FundingRecord,),
    Closure(crate::Closure,),
}

impl Stored {
    #[must_use]
    pub const fn key(&self) -> Key {
        match self {
            Stored::Live(record) => Key::Live(record.number),
            Stored::Ended(record) => Key::Ended(record.number),
            Stored::Ledger(record) => Key::Ledger(record.funder),
            Stored::Closure(record) => Key::Closure { task: record.task, generation: record.generation },
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Refusal {
    NotReady,
    Unknown,
    Busy,
    Duplicate,
    Empty,
    Batch,
    Live,
    Project,
    Tree,
    Depth,
    Delegates,
    Dependencies,
    Cycle,
    Executor,
    Spec,
    Contract,
    AuthorityShape,
    Inputs,
    State,
    Attempt,
    LiveDelegates,
    Restore,
    Read,
    Turn,
    Funding,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Problem {
    pub task: Option<u64>,
    pub why: Refusal,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Accepted {
    New,
    Already,
}

#[derive(PartialEq, Eq, Debug)]
pub enum Event {
    OpenPeriod {
        /// Root-issued reply destination; exactly one terminal reply (domain/tasks.md, 5).
        reply_to: ReplyTo,
        /// Authenticated project identity checked by root in this decision (domain/tasks.md, 2).
        project: u32,
        /// Fresh period identity for opening; the actual original period for carving (domain/tasks.md, 2).
        period: u64,
        /// Finite authority-approved allotment in the deployment spending unit (domain/tasks.md, 2).
        budget: u64,
    },
    CarvePool {
        /// Root-issued reply destination; exactly one terminal reply (domain/tasks.md, 5).
        reply_to: ReplyTo,
        /// Authenticated project identity checked by root in this decision (domain/tasks.md, 2).
        project: u32,
        /// Person whose current project role root authenticated (domain/tasks.md, 2).
        person: u64,
        /// Fresh period identity for opening; the actual original period for carving (domain/tasks.md, 2).
        period: u64,
        /// Finite authority-approved allotment in the deployment spending unit (domain/tasks.md, 2).
        budget: u64,
    },
    Turn {
        /// Root-issued reply destination; exactly one terminal reply (domain/tasks.md, 5).
        reply_to: ReplyTo,
        /// Nonzero root-issued task identity (domain/tasks.md, 5).
        task: u64,
        /// Nonzero claimed attempt fence; stale attempts cannot charge (domain/tasks.md, 5).
        attempt: u64,
        /// Nonzero next consecutive turn; root/fleet fences accepted replay (domain/tasks.md, 5).
        turn: u32,
        /// Root supplies no message fence in this slice; Some is refused until a real inbox route (domain/tasks.md, 5).
        read: Option<u64>,
        /// Whole priced attempt spend; only its new delta is posted (domain/tasks.md, 5).
        cumulative: u64,
    },
    Make {
        reply_to: ReplyTo,
        creator: Party,
        batch: Box<[New]>,
    },
    Prepare {
        reply_to: ReplyTo,
        task: u64,
    },
    Claim {
        reply_to: ReplyTo,
        task: u64,
        attempt: u64,
    },
    Started {
        task: u64,
        attempt: u64,
    },
    Activation {
        reply_to: ReplyTo,
        task: u64,
        attempt: u64,
        end: End,
        /// Root distinguishes priced worker spend from unpriced topology/failure;
        /// one Acknowledged or Refused terminal, with proof room reserved first
        /// (domain/tasks.md, 14.1; domain/engine.md, 7.5).
        cause: Cause,
    },
    PreparationFailed {
        task: u64,
    },
    Hold {
        task: u64,
        why: Hold,
    },
    Settled {
        task: u64,
    },
    Restore {
        record: Stored,
    },
    Restored,
}

#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    Made {
        reply_to: ReplyTo,
        tasks: Box<[u64]>,
    },
    Refused {
        reply_to: ReplyTo,
        problem: Problem,
    },
    Done {
        reply_to: ReplyTo,
    },
    Acknowledged {
        reply_to: ReplyTo,
        task: u64,
        attempt: u64,
        accepted: Accepted,
    },
    TurnAcknowledged {
        reply_to: ReplyTo,
        task: u64,
        attempt: u64,
        turn: u32,
        accepted: Accepted,
    },
    Activate {
        /// Child supplies one due task's bounded semantic preparation context;
        /// root consumes authority/brief readiness and drops it at claim/failure
        /// (domain/tasks.md, 14.2; domain/engine.md, 7.1 and 9).
        context: Box<RunContext>,
    },
    Stop {
        task: u64,
        attempt: u64,
    },
    Adopt {
        /// Child names the root-issued positive live task identity (domain/tasks.md, 14.2).
        task: u64,
        /// Child's current positive durable claim; root validates proof before fleet adoption (domain/engine.md, 6).
        attempt: u64,
        /// Latest committed consecutive turn, or zero; fleet fences older bodies before child admission (domain/engine.md, 7.2 and 7.5).
        kept: u32,
    },
    Close {
        task: u64,
        ending: Ending,
    },
    Ended {
        task: u64,
        requester: Party,
        ending: Ending,
    },
    Save {
        record: Stored,
    },
    Erase {
        key: Key,
    },
    RestoreRefused {
        problem: Problem,
    },
}

/// Root explains whether this lifecycle terminal carries an admitted priced
/// worker expense, or an unpriced topology/readiness failure. Root reserves
/// replay proof room before sending either (domain/engine.md, 7.2 and 7.4).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Cause {
    /// Root's priced worker answer; tasks applies only its new delta together
    /// with lifecycle admission. Refusal changes neither (domain/tasks.md, 5).
    Priced {
        /// Whole attempt expense, monotonic and bounded by representability of
        /// authentic financial links (domain/authority.md, 7).
        cumulative: u64,
    },
    /// Root's loss, fleet refusal or invalid-answer normalization; this cause
    /// spends nothing, and still ends in one terminal reply (domain/tasks.md, 5).
    Unpriced,
}

/// Tasks to root: bounded semantic values for one actual requested activation.
/// Root owns this temporary preparation context, never a second mutable task
/// or funding ledger. It drops it on claim or failure (domain/engine.md, 7.1 and 9).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RunContext {
    /// Root-issued task identity for this due activation (domain/tasks.md, 3).
    pub task: u64,
    /// Configured project whose policy root checks (domain/authority.md, 8.3).
    pub project: u32,
    /// Domain executor; root selects its actual route (domain/tasks.md, 2).
    pub executor: Executor,
    /// Bounded spec owned by this preparation (domain/tasks.md, 3 and 10).
    pub spec: Spec,
    /// Bounded result contract rendered in the brief (domain/tasks.md, 3).
    pub contract: Contract,
    /// Actual requester rendered in the brief (domain/tasks.md, 3).
    pub requester: Party,
    /// Exact current task authority supplied to pure policy (domain/tasks.md, 2).
    pub authority: Authority,
    /// Exact current task financial snapshot for this preparation; root never
    /// mutates or persists it as a second ledger (domain/authority.md, 2).
    pub numbers: Numbers,
}
