//! What a plan is made of (engine-domain.md, 5.1 and 5.2): steps of a few
//! primitives, their dependencies, the gates they add, what wakes a session,
//! and the envelope an accepted plan grows within. Names are labels, compared
//! byte for byte and never interpreted.

use alloc::boxed::Box;

use temper_lib::Duration;

/// One of the deployment's repositories, named by its index in the list the
/// deployment configures. A plan writes to no other.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Repository(pub u32);

/// A commit, as the forge names it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Commit(pub [u8; 32]);

/// A graph of steps under a goal, as a run proposes it.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Plan {
    pub steps: Box<[Step]>,
    /// How far it may grow once accepted, without asking again.
    pub envelope: Envelope,
    /// The tokens its steps' runs may spend together, as estimated from
    /// their budgets.
    pub budget: u64,
}

/// One step of a plan: the work an item carries.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Step {
    /// Unique in its plan: what dependencies and keys name it by, and the
    /// title of its item.
    pub name: Box<[u8]>,
    /// The repository its item is in, and a change's pull request with it.
    pub repository: Repository,
    pub work: Work,
    /// The steps it comes after, by name. Each of them is done first, and so
    /// are the steps each of them adds.
    pub after: Box<[Box<[u8]>]>,
    /// What it adds to the rules' conditions, never taking from them.
    pub gates: Box<[Gate]>,
}

/// A step's primitive, with its spec (5.1).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Work {
    /// A run, whose outcome is a report, and may be steps added to its plan.
    Agent(AgentSpec),
    /// A change through the pull-request lifecycle.
    Change(ChangeSpec),
    /// Nothing to run: it waits.
    Wait(WaitSpec),
    /// An agent with an inbox, which yields and parks instead of ending.
    Session(SessionSpec),
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct AgentSpec {
    pub charter: Charter,
    /// Whether it may finish by adding steps to its plan, within the plan's
    /// envelope, which makes the steps it adds its children.
    pub grows: bool,
}

/// A change to the step's repository: a run produces it on the item's own
/// branch, the engine opens its pull request into `base`, CI runs on its
/// head, it is reviewed, runs repair it, and the engine lands it once its
/// gates hold.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ChangeSpec {
    /// The branch it lands into.
    pub base: Box<[u8]>,
    /// The run that produces it, and every run that repairs it.
    pub produce: Charter,
    /// Whether the repository's checks must pass before a run pushes it
    /// (agent-domain.md, 4.4).
    pub checks: bool,
    pub review: Review,
}

/// Who reviews a change's head, never below what the rules ask.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Review {
    /// A person approves its exact head, and nobody asks for changes on it.
    Person,
    /// A run with this charter gives its verdict on the exact head.
    Agent(Charter),
}

/// What a wait waits for, once its dependencies are done.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum WaitSpec {
    /// Its dependencies, and nothing more.
    Steps,
    /// A person's decision on its item: accepting it ends the wait, rejecting
    /// it holds the item for a person.
    Decision,
    /// This long after its last dependency was done, or after its item was
    /// made when it has none.
    Time(Duration),
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct SessionSpec {
    pub charter: Charter,
    pub resume: Resume,
    /// What wakes it for a turn, after its first. Only sessions have wake
    /// rules: every other step is asked what is due whenever what it reads
    /// changes.
    pub wake: Wake,
}

/// Which of a session's wakes resume its parked snapshot, when the engine
/// holds one, rather than start fresh (section 6).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Resume {
    /// Resume while chatting; start fresh while supervising a goal.
    Default,
    Always,
    Never,
}

/// What a step's runs are given (agent-domain.md, 4.1), less what the step's
/// primitive and the moment decide: the brief's sections and the outcome
/// spec.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Charter {
    /// The guidance the brief carries, in the plan's words.
    pub instructions: Box<[u8]>,
    /// A template from the deployment's configuration whose guidance the
    /// brief carries too, by name.
    pub template: Option<Box<[u8]>>,
    pub grants: Grants,
    pub budget: Budget,
}

/// What a run may do besides finishing (agent-domain.md, 4.1 and 4.3).
#[expect(clippy::struct_excessive_bools, reason = "each grant is given on its own: a set of flags")]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Grants {
    /// Write to its checkout.
    pub modify: bool,
    /// Run commands in its checkout.
    pub shell: bool,
    /// Read the forge beyond its brief, and recall notes.
    pub forge: bool,
    /// Run sub-agents.
    pub subagents: bool,
    /// Write notes, through the `note` outlet.
    pub note: bool,
}

/// What a run may spend: tokens, LLM turns and time.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Budget {
    pub tokens: u64,
    pub turns: u32,
    pub time: Duration,
}

impl Budget {
    /// Whether every dimension of `self` is within `most`'s.
    #[must_use]
    pub fn within(self, most: Budget) -> bool {
        self.tokens <= most.tokens && self.turns <= most.turns && self.time <= most.time
    }
}

/// A condition a step adds before it lands (a change) or starts (any other
/// primitive). A change always waits for CI on its exact head and for its
/// review; these come on top.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Gate {
    /// At least this many people approve a change's exact head.
    Approvals(u32),
    /// A person accepts the step on its item.
    Accepted,
}

/// What wakes a session that has no live run (5.4).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Wake {
    pub on: Sources,
    /// Wake it this long after its last turn, if nothing else has.
    pub every: Option<Duration>,
    pub batch: Batch,
}

/// Which inbox events wake an item.
#[expect(clippy::struct_excessive_bools, reason = "each source is chosen on its own: a set of flags")]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Sources {
    /// Its own changes: CI, reviews and pushes on its pull request.
    pub own: bool,
    /// Its dependencies or its children finishing.
    pub related: bool,
    /// The items it subscribes to changing.
    pub subscribed: bool,
    /// A person's message. Messages are never batched: a person waits for
    /// the answer.
    pub messages: bool,
}

/// Events are let through once there are at least `count` of them, or once
/// the oldest is `age` old. A count of one lets each through as it comes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Batch {
    pub count: u32,
    pub age: Option<Duration>,
}

/// How far an accepted plan may grow without a person accepting it again:
/// how many steps of each primitive, in which repositories, and where
/// changes may land. Growth a person accepts beyond it widens it.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Envelope {
    pub agents: u32,
    pub changes: u32,
    pub waits: u32,
    pub sessions: u32,
    /// The repositories the items of steps added to the plan may be in.
    pub repositories: Box<[Repository]>,
    /// The branches changes added to the plan may land into.
    pub into: Box<[Target]>,
}

/// A branch of one of the deployment's repositories.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Target {
    pub repository: Repository,
    pub base: Box<[u8]>,
}

/// Steps of each primitive: added to a plan so far, or about to be.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Growth {
    pub agents: u32,
    pub changes: u32,
    pub waits: u32,
    pub sessions: u32,
}

impl Growth {
    pub const NONE: Growth = Growth { agents: 0, changes: 0, waits: 0, sessions: 0 };
}
