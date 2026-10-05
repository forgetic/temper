use crate::{Authority, Class, Funder, Numbers, Tries};
use alloc::boxed::Box;
use skein_lib::{ReplyTo, Wall};
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Party {
    Task(u64),
    Person(u64),
    Deployment { project: u32 },
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Executor {
    Agent { charter: u32 },
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Parameter {
    Number { name: u32, value: u64 },
    Bytes { name: u32, value: Box<[u8]> },
    Resource { name: u32, connector: u16, resource: u64 },
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
    Report { words: u32 },
    Verdict { choices: Box<[Verdict]> },
    Change { connector: u16, kind: u16, words: u32 },
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Result {
    Report { words: Box<[u8]> },
    Verdict { code: u32, words: Box<[u8]> },
    Change { connector: u16, kind: u16, resource: u64, words: Box<[u8]> },
    Failure { reason: Box<[u8]> },
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Ending {
    Done(Result),
    Failed { reason: Box<[u8]> },
    Cancelled { reason: Box<[u8]>, result: Option<Result> },
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Status {
    Done,
    Failed,
    Cancelled,
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum End {
    Finished { result: Result, cancel_delegates: bool },
    Parked,
    Failed(Class),
    Refused,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Hold {
    Failures(Class),
    Dependency(u64),
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
    Claimed { attempt: u64 },
    Running { attempt: u64 },
    BackingOff { until: Wall },
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Stage {
    Run { attempt: u64 },
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
    Active(Active),
    Closing(Closing),
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Phase {
    Waiting,
    Active(Active),
    Closing(Closing),
    Held { was: Was, why: Hold },
    Ended(Ending),
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
    pub root: u64,
    pub depth: u32,
    pub executor: Executor,
    pub spec: Spec,
    pub contract: Contract,
    pub authority: Authority,
    pub numbers: Numbers,
    pub funder: Funder,
    pub dependencies: Box<[u64]>,
    pub delegates: Box<[u64]>,
    /// Tasks made in this subtree over its life, including itself.
    pub made: u32,
    pub attempt: u64,
    pub last_answer: Option<u64>,
    pub tries: Tries,
    pub refusals: u32,
    pub phase: Phase,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
/// A summary of `Key::Ended(number)`, retained while a live task names its
/// result; the root reads the complete historical result through that key.
pub struct Stub {
    pub number: u64,
    pub project: u32,
    pub status: Status,
    pub attempt: u64,
    pub last_answer: Option<u64>,
}
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Key {
    Live(u64),
    Ended(u64),
    Stub(u64),
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Stored {
    Live(Box<TaskRecord>),
    Ended(Box<TaskRecord>),
    Stub(Stub),
}
impl Stored {
    #[must_use]
    pub const fn key(&self) -> Key {
        match self {
            Stored::Live(record) => Key::Live(record.number),
            Stored::Ended(record) => Key::Ended(record.number),
            Stored::Stub(stub) => Key::Stub(stub.number),
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
    Unheld,
    Reason,
    Restore,
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
    /// Numbers are fresh root-issued candidates; authority already allowed.
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
    /// One reply acknowledges an accepted terminal or refuses a finish while
    /// delegates are live; that finish may be corrected and tried again.
    Activation {
        reply_to: ReplyTo,
        task: u64,
        attempt: u64,
        end: End,
    },
    PreparationFailed {
        task: u64,
    },
    Hold {
        task: u64,
        why: Hold,
    },
    Release {
        reply_to: ReplyTo,
        task: u64,
    },
    Cancel {
        reply_to: ReplyTo,
        task: u64,
        reason: Box<[u8]>,
    },
    Settled {
        task: u64,
    },
    /// Root loads only an authoritative ended-record summary, in the same
    /// decision as the Make which needs it.
    RememberStub {
        reply_to: ReplyTo,
        stub: Stub,
    },
    ForgetStub {
        reply_to: ReplyTo,
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
    Activate {
        task: u64,
        executor: Executor,
    },
    Stop {
        task: u64,
        attempt: u64,
    },
    /// Restore a committed claim/run through the parent before new work.
    Adopt {
        task: u64,
        attempt: u64,
    },
    Close {
        task: u64,
        ending: Ending,
    },
    /// The parent makes the requester's durable result message in this same
    /// decision; it is not a volatile notification to replay after a restart.
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
