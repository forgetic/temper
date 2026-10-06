//! Values owned by the connector top and passed to the change policy
//! (domain/forge.md, sections 8–10).
//!
//! The top keeps `Change`, including its pending phase, counters and clean
//! predecessor heads. This module keeps nothing and knows no API tokens.
//! `Facts` and `Heard` form a current snapshot, not a wake event.
use alloc::boxed::Box;
use skein_lib::{Duration, Wall};

/// A forge commit identifier, translated at the connector top.
pub type Commit = [u8; 32];

/// The status of a fact or requirement at the named head.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Status {
    /// No answer has been observed.
    Unknown,
    /// The answer remains in flight.
    Pending,
    /// The requirement holds.
    Passed,
    /// The requirement failed.
    Failed,
}

/// Whether a gate needs the current head or accepts clean predecessors.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Freshness {
    /// The verdict must name the current head.
    Exact,
    /// A verdict at a clean predecessor head remains valid.
    Clean,
}

/// How the top obtains a gate verdict.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum GateKind {
    /// An agent reviews the change.
    Agent,
    /// A person approves the change.
    Person,
    /// A configured check supplies a verdict.
    Check,
}

/// One gate the change or its project asks for.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Gate {
    pub number: u64,
    pub kind: GateKind,
    pub blocking: bool,
    pub freshness: Freshness,
    pub eager: bool,
}

/// A gate's current verdict and the head it names.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct GateReport {
    pub number: u64,
    pub head: Commit,
    pub status: Status,
}

/// Why a repair delegate is needed.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Repair {
    /// CI failed on the change's head.
    Ci,
    /// A blocking gate requested changes.
    Gate(u64),
    /// CI failed after a clean update from the base.
    Semantic,
}

/// Why a change is held for its requester or a person.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Hold {
    /// A forge wait exceeded its stall bound.
    Stalled,
    /// Another repair would exceed the configured bound.
    Repairs,
    /// Another conflict resolution would exceed its bound.
    Resolutions,
    /// Another mechanical update would exceed its bound.
    Updates,
    /// The held branch moved outside its writer.
    BranchMoved,
    /// The held branch disappeared.
    BranchMissing,
    /// The pull request closed without a merge.
    PullClosed,
    /// The pull request was retargeted.
    Retargeted,
    /// A delegate or effect failed permanently.
    Failed,
    /// A person rejected a required gate.
    Rejected,
}

/// A change's committed place on its way to landing.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum State {
    /// A producing delegate has yet to push the branch.
    Producing { requested: bool },
    /// A pushed branch needs a pull request.
    Opening { requested: bool },
    /// A released change is recreating its deleted branch.
    Recreating { head: Commit },
    /// A released change is reopening or retargeting its pull request.
    Reopening { requested: bool, retarget: bool },
    /// CI is read on the pushed head.
    Checking { head: Commit },
    /// Required gates are sought on the checked head.
    Gating { head: Commit, asked: Box<[u64]> },
    /// The change is ready, waiting its turn.
    Queued { head: Commit, ready_since: Wall },
    /// The change owns the queue through an update and landing.
    First { head: Commit, ready_since: Wall },
    /// A mechanical update of the branch is outstanding.
    Updating { from: Commit, base: Commit, ready_since: Wall },
    /// A conflict resolution delegate is outstanding.
    Resolving { requested: bool, base: Commit },
    /// A repair delegate is outstanding.
    Repairing { requested: bool, why: Repair },
    /// A merge at the named head is outstanding.
    Landing { head: Commit },
    /// The pull request merged at this commit.
    Landed { merge: Commit },
    /// No write is made until a release lifts this cause.
    Held { was: Box<State>, why: Hold },
}

/// The top-owned procedure record for one change task.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Change {
    pub task: u64,
    pub state: State,
    pub gates: Box<[Gate]>,
    pub clean: Box<[Commit]>,
    pub repairs: u32,
    pub resolutions: u32,
    pub updates: u32,
    pub since: Wall,
    pub ready_since: Option<Wall>,
    pub owns_turn: bool,
    pub last_head: Option<Commit>,
}

/// Observed state of the change's pull request.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Pull {
    /// No pull request for the branch pair was found.
    Missing,
    /// The open pull request names this head and base.
    Open { head: Commit, base: Commit },
    /// Someone closed the pull request without merging it.
    Closed,
    /// The pull request merged at this commit.
    Merged { commit: Commit },
}

/// CI observed on a specific head.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Ci {
    pub head: Commit,
    pub status: Status,
}

/// Fresh forge and queue facts gathered by the top.
#[expect(clippy::struct_excessive_bools, reason = "independent root-supplied facts")]
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Facts {
    pub branch: Option<Commit>,
    pub expected_base: Commit,
    pub pull: Pull,
    pub ci: Ci,
    pub base_ci: Ci,
    pub base_tip: Commit,
    pub contains_base: Status,
    pub mergeable: Status,
    pub gates: Box<[GateReport]>,
    pub writer_taken: bool,
    pub drift: Option<Hold>,
    pub first: bool,
    pub may_merge: bool,
    pub queue_repair_active: bool,
}

/// Committed delegate and effect results supplied by the top.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum EffectResult {
    /// No effect result is outstanding.
    None,
    /// The requested effect is still outstanding.
    Pending,
    /// The effect was made or found again.
    Made,
    /// A mechanical update conflicted with the base.
    Conflict,
    /// An effect failed permanently.
    Failed,
}

/// Current task and effect standing, independent of the wake trigger.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Heard {
    pub delegate: Status,
    pub effect: EffectResult,
    pub released: bool,
    pub cancelled: bool,
}

/// The delegate the top should create in the same decision.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Delegate {
    /// Produce and push the change's branch.
    Produce,
    /// Repair CI or a gate failure on the branch.
    Repair(Repair),
    /// Resolve a conflicting update from a merge in progress.
    Resolve { base: Commit },
    /// Obtain a gate's verdict at the current head.
    Gate { number: u64, head: Commit },
}

/// The effect the top should commit after checking authority.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Effect {
    /// Open a pull request for the pushed branch.
    Open,
    /// Recreate a deleted held branch at its last known head.
    CreateBranch { head: Commit },
    /// Reopen a pull request closed by another person.
    Reopen,
    /// Restore the change's landing branch after a retarget.
    Retarget,
    /// Merge the landing branch into the change's branch.
    Update { head: Commit, base: Commit },
    /// Merge the pull request at its checked head.
    Merge { head: Commit, base: Commit },
}

/// What the top does with the procedure's one decision.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Decision {
    /// Nothing is due in this snapshot.
    None,
    /// Wait for a fact or deadline to change.
    Wait { until: Option<Wall> },
    /// Create one bounded delegate.
    Delegate(Delegate),
    /// Commit one effect to the top's outbox.
    Effect(Effect),
    /// Tell the top this change is ready to queue.
    Ready,
    /// Ask the top for the branch's one queue repair.
    QueueRepair,
    /// Finish with the observed merge commit.
    Finish { merge: Commit },
    /// Finish cancellation after the top settles pending effects.
    Cancel,
    /// Hold the task and tell its requester why.
    Hold(Hold),
}

/// A next state and its decision from one coherent snapshot.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Stepped {
    pub change: Change,
    pub decision: Decision,
}

/// Bounds on delegates, updates, stalls and supplied gate facts.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    pub repairs: u32,
    pub resolutions: u32,
    pub updates: u32,
    pub stall: Duration,
    pub gates: u32,
    pub clean_heads: u32,
}

/// One change ready for a landing branch's derived queue.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Ready {
    pub task: u64,
    pub priority: i32,
    pub since: Wall,
}
