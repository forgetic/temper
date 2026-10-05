//! Owned questions and answers (domain/authority.md, sections 8 and 9).

use alloc::boxed::Box;

use skein_lib::Wall;

use crate::{Authority, Executor, Gate, Name, Numbers, ProposalKind, Scopes, Tools};

/// Ordered from least to most strict: deciding never clears a fact failure.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Answer {
    Allow,
    Wait,
    Propose,
    Refuse,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Source {
    Task,
    Role,
    Project,
    Deployment,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Finding {
    Oversized,
    UnknownProject,
    UnknownRole,
    Authority { source: Source, lacks: crate::Lacks },
    Executor { source: Source },
    Tasks { source: Source },
    Spend { source: Source },
    Arithmetic,
    RunBudget,
    RunCap,
    Deadline,
    Account,
    Writer,
    Tool,
    Grant { source: Source },
    Reference,
    Scope { source: Source },
    Required { connector: u16, fact: u16 },
    Failed { connector: u16, fact: u16 },
    Unpermitted,
    Undecidable,
    PeriodSpend,
    LandingMissing,
    LandingPin,
    Ci { status: Status },
    Behind { status: Status },
    Gate { number: u32, status: Status },
    Approval { role: u32, have: u32, want: u32 },
    ReviewFailed { role: u32 },
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Checked {
    pub answer: Answer,
    pub numbers: Option<Numbers>,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Delegate {
    pub executor: Executor,
    pub authority: Authority,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct BatchAsk {
    pub project: u32,
    pub creator: Authority,
    pub numbers: Numbers,
    /// Lifetime task capacity still available, kept separately by tasks.
    pub tasks_left: u32,
    pub tasks: Box<[Delegate]>,
}

/// A read or effect of one connector, pinned to the state it names.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Effect {
    pub connector: u16,
    pub kind: u16,
    pub name: Name,
    pub state: [u8; 32],
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct EffectAsk {
    pub project: u32,
    pub authority: Authority,
    pub effect: Effect,
    pub landing: Option<Landing>,
}

pub type Head = [u8; 32];

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Ci {
    pub head: Head,
    pub status: Status,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Verdict {
    pub gate: u32,
    pub head: Head,
    pub status: Status,
}

/// Only authenticated people's latest reviews, with root-verified roles.
/// Agent reviews use gate verdicts and cannot count as human approvals.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Review {
    pub person: u64,
    pub role: u32,
    pub head: Head,
    pub status: Status,
}

/// Facts read together for this exact head and landing branch's tip.
/// All verdicts and authenticated latest reviews belong to the effect's
/// named change and branch; the root resolves gate identities and roles.
/// `clean` contains only predecessor heads linked to `head` entirely by
/// temper's clean updates since the last repair or conflict resolution.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Landing {
    pub head: Head,
    pub tip: Head,
    pub contains_tip: Status,
    pub ci: Ci,
    pub clean: Box<[Head]>,
    pub gates: Box<[Gate]>,
    pub verdicts: Box<[Verdict]>,
    pub reviews: Box<[Review]>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Status {
    Unknown,
    Pending,
    Passed,
    Failed,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Fact {
    pub connector: u16,
    pub kind: u16,
    pub name: Name,
    pub state: [u8; 32],
    pub status: Status,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Writer {
    Task,
    Ancestor,
    Pending,
    Other,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Write {
    pub effect: Effect,
    pub held: Writer,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct RunAsk {
    pub project: u32,
    pub authority: Authority,
    pub numbers: Numbers,
    pub budget: u64,
    pub wall: Wall,
    /// One status for each model account its charter will use.
    pub accounts: Box<[bool]>,
    /// Every resource its workspace will write, including saved work.
    pub writes: Box<[Write]>,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum PersonRequest {
    Create(Box<[Delegate]>),
    /// The complete future authority/allotment to fund, not an unchecked delta.
    Allot(Authority),
    Accept(Action),
    /// What the amendment must newly give; a narrowing gives empty authority.
    Amend(Authority),
    Move(Authority),
    Cancel,
    Release,
    Watch,
    Policy,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct PersonAsk {
    pub project: u32,
    pub role: u32,
    pub pool: Numbers,
    pub tasks_left: u32,
    pub request: PersonRequest,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Call {
    Tool,
    Read(Effect),
    Message { referenced: bool },
    Note(Scopes),
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct CallAsk {
    pub project: u32,
    pub authority: Authority,
    /// Exactly one family bit, assigned by configuration.
    pub family: Tools,
    pub call: Call,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Action {
    Batch(Box<[Delegate]>),
    Effect(Effect),
    Widen(Authority),
    Amend(Authority),
    /// Topology grants standing; release may additionally need authority.
    Escalate {
        release: Option<Authority>,
    },
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Holder {
    /// The caller has verified that this task stands above the proposer.
    Task {
        project: u32,
        authority: Authority,
        numbers: Numbers,
        tasks_left: u32,
    },
    Person {
        project: u32,
        role: u32,
        proposal: ProposalKind,
        pool: Numbers,
        tasks_left: u32,
    },
}
