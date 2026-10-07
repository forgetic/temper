//! Owned questions and answers (domain/authority.md, sections 8 and 9).

use alloc::boxed::Box;

use skein_lib::Wall;

use crate::{Authority, Executor, Gate, Name, Numbers, ProposalKind, Scopes, Tools};

/// Ordered from least to most strict: deciding never clears a fact failure.
/// Terminal result of one pure check, ordered by severity; it neither executes the action nor
/// commits a durable decision. (domain/authority.md, section 8).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Answer {
    /// Checks permit the action; the caller still owns execution and atomic commitment.
    Allow,
    /// Required facts, budget, account or writer condition are not ready; no action is authorized
    /// yet.
    Wait,
    /// Task authority is insufficient but hard ceilings permit seeking an eligible accepter.
    Propose,
    /// A hard requirement, admission bound or permission failed.
    Refuse,
}

/// Authority layer that supplied a ceiling or permission for a finding. (domain/authority.md,
/// sections 6 and 8).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Source {
    /// The acting or creating task's authority.
    Task,
    /// The root-verified person's project role.
    Role,
    /// The live project's ceiling.
    Project,
    /// Immutable deployment rules.
    Deployment,
}

/// Fixed-size reason emitted by a pure check into caller-reserved bounded room; multiple
/// independent reasons may accompany one answer. (domain/authority.md, sections 8–10).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Finding {
    /// Input shape or payload exceeds admission limits.
    Oversized,
    /// No live policy exists for the named project.
    UnknownProject,
    /// The live policy has no named role.
    UnknownRole,
    /// Componentwise authority deficits against this source.
    Authority {
        /** Authority layer against which this condition failed. */
        source: Source,
        /** Independent component deficits against that source. */
        lacks: crate::Lacks,
    },
    /// A child executor is not permitted by this source.
    Executor {
        /** Authority layer against which this condition failed. */
        source: Source,
    },
    /// Aggregate lifetime task capacity does not fit this source.
    Tasks {
        /** Authority layer against which this condition failed. */
        source: Source,
    },
    /// Aggregate reserved spend does not fit this source.
    Spend {
        /** Authority layer against which this condition failed. */
        source: Source,
    },
    /// Required accounting or aggregate arithmetic is unrepresentable.
    Arithmetic,
    /// Offered run budget is not above the minimum or available funding is insufficient.
    RunBudget,
    /// Offered run budget exceeds deployment or task authority cap.
    RunCap,
    /// Task deadline is past the supplied wall time.
    Deadline,
    /// A required model account is unusable.
    Account,
    /// A written resource is held by another task or has a pending hold.
    Writer,
    /// Configured family is absent from an applicable authority value.
    Tool,
    /// This source has no grant covering the requested operation.
    Grant {
        /** Authority layer against which this condition failed. */
        source: Source,
    },
    /// The caller has no verified standing to message the task.
    Reference,
    /// The note scope is absent from this source.
    Scope {
        /** Authority layer against which this condition failed. */
        source: Source,
    },
    /// A pinned required fact is missing, unknown or pending.
    Required {
        /** Connector whose required pinned fact was checked. */
        connector: u16,

        fact: u16,
    },
    /// A matching pinned required fact failed.
    Failed {
        /** Connector whose required pinned fact was checked. */
        connector: u16,

        fact: u16,
    },
    /// The person's role does not allow this request kind.
    Unpermitted,
    /// The person's role may not accept this proposal kind.
    Undecidable,
    /// A funding request's pool budget exceeds the role's period ceiling.
    PeriodSpend,
    /// An applicable rule needs a landing snapshot that was not supplied.
    LandingMissing,
    /// Landing head differs from the effect's exact state pin.
    LandingPin,
    /// Required exact-head CI is not passed.
    Ci { status: Status },
    /// Required exact-head containment of the branch tip is not passed.
    Behind { status: Status },
    /// A blocking gate lacks a passed valid-head verdict.
    Gate {
        /** Blocking gate number resolved by the root. */
        number: u32,

        status: Status,
    },
    /// Distinct valid approvals of the required role are fewer than requested.
    Approval {
        role: u32,
        /** Distinct eligible passed reviewers at valid heads. */
        have: u32,
        /** Required distinct-person count. */
        want: u32,
    },
    /// A valid reviewer of the required role requested changes.
    ReviewFailed { role: u32 },
}

/// Pure check result and optional replacement funding snapshot; the root commits allowed numbers
/// together with the action once. (domain/authority.md, sections 7–8).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Checked {
    /// Strictest answer after all applicable checks.
    pub answer: Answer,
    /// Replacement funding numbers only on `Allow`; other answers carry `None` and authorize no
    /// debit.
    pub numbers: Option<Numbers>,
}

/// Root-translated directly created task's executor and complete authority, admitted under batch
/// and authority limits. (domain/authority.md, section 8.1).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Delegate {
    /// Executor permission that the creator must have.
    pub executor: Executor,
    /// Complete child authority, bounded by the configured authority limits.
    pub authority: Authority,
}

/// Root-gathered creation question over one coherent creator and funding snapshot; no tasks or
/// reservations are retained here. (domain/authority.md, section 8.1).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct BatchAsk {
    pub project: u32,
    /// Creator's current authority, bounded by `Limits`.
    pub creator: Authority,
    /// Creator's current allotment snapshot, not a copy of its static ceiling.
    pub numbers: Numbers,
    /// Lifetime task capacity still available, kept separately by tasks. Current lifetime task
    /// capacity available from the tasks ledger.
    pub tasks_left: u32,
    /// Whole all-or-nothing batch, bounded by `Limits::batch`; each child costs one slot plus its
    /// descendant capacity.
    pub tasks: Box<[Delegate]>,
}

/// A read or effect of one connector, pinned to the state it names.
/// Root-translated connector read or effect with a literal resource and exact state pin, bounded on
/// admission. (domain/authority.md, section 8.2).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Effect {
    pub connector: u16,
    /// Requested exact kind; grant implications do not remove requirements on this kind.
    pub kind: u16,
    /// Full literal resource name, bounded by segment and byte limits.
    pub name: Name,
    pub state: [u8; 32],
}

/// Root-gathered permission and authentic-fact question for one pinned effect; this value makes no
/// connector call. (domain/authority.md, section 8.2).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct EffectAsk {
    pub project: u32,
    /// Acting task or accepter authority, admitted under `Limits`.
    pub authority: Authority,
    /// Single pinned connector operation being checked.
    pub effect: Effect,
    /// Coherent landing snapshot when available; absence waits if applicable landing rules require
    /// it.
    pub landing: Option<Landing>,
}

/// Opaque fixed-width head/state identity supplied by the root; authority neither parses nor
/// resolves it. (domain/authority.md, section 10).
pub type Head = [u8; 32];

/// CI report supplied by the root for the exact landing head; CI never carries over clean
/// predecessors. (domain/authority.md, section 10).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Ci {
    pub head: Head,
    /// CI outcome; missing or pending waits and failed refuses when CI is required.
    pub status: Status,
}

/// Root-resolved gate report for the named change, with head freshness checked against the landing
/// snapshot. (domain/authority.md, section 10).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Verdict {
    pub gate: u32,
    pub head: Head,
    /// Verdict outcome, interpreted only at an accepted head.
    pub status: Status,
}

/// Only authenticated people's latest reviews, with root-verified roles.
/// Agent reviews use gate verdicts and cannot count as human approvals.
/// Authenticated latest human review with root-verified project role; duplicate people count once
/// and agent gates never count as people. (domain/authority.md, section 10).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Review {
    /// Authenticated person number, used to count distinct reviewers.
    pub person: u64,
    pub role: u32,
    /// Head reviewed, checked against required freshness.
    pub head: Head,
    /// Passed approval or failed request for changes; pending provides no approval.
    pub status: Status,
}

/// Facts read together for this exact head and landing branch's tip.
/// All verdicts and authenticated latest reviews belong to the effect's
/// named change and branch; the root resolves gate identities and roles.
/// `clean` contains only predecessor heads linked to `head` entirely by
/// temper's clean updates since the last repair or conflict resolution.
/// One coherent root-supplied snapshot for the named change and branch; boxed collections are
/// bounded by `Limits` on admission. (domain/authority.md, section 10).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Landing {
    /// Proposed landing head, which must equal the effect's exact state pin.
    pub head: Head,
    /// Landing-branch tip whose containment was checked in this snapshot.
    pub tip: Head,
    /// Containment result for this exact head and tip; unknown/pending waits and absence refuses
    /// when required.
    pub contains_tip: Status,
    /// CI head and status from the same named change snapshot.
    pub ci: Ci,
    /// CI availability is configured at repository adoption, never inferred from statuses.
    pub has_ci: bool,
    /// Configured checks required in place of CI; each must pass at this head.
    pub checks: Box<[Gate]>,
    /// Root-verified clean predecessor heads, bounded by `Limits::heads`; repair or conflict
    /// resolution breaks this lineage.
    pub clean: Box<[Head]>,
    /// Change gates added to policy gates, bounded by `Limits::gates`.
    pub gates: Box<[Gate]>,
    /// Gate reports for the change, bounded by `Limits::verdicts`.
    pub verdicts: Box<[Verdict]>,
    /// Authenticated latest human reviews, bounded by `Limits::reviews`.
    pub reviews: Box<[Review]>,
}

/// Reported fact state; unknown and pending wait, passed satisfies, and a relevant failure refuses.
/// (domain/authority.md, sections 8.2 and 10).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Status {
    /// No known outcome; required conditions wait.
    Unknown,
    /// Outcome remains in flight; required conditions wait.
    Pending,
    /// Reported condition is satisfied at its pin.
    Passed,
    /// Reported condition failed at its pin.
    Failed,
}

/// Authentic root-supplied connector report; matching requires the full resource name, exact state
/// pin and required fact kind. (domain/authority.md, section 8.2).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Fact {
    pub connector: u16,
    /// Reported connector fact kind, not the effect kind.
    pub kind: u16,
    /// Full resource name the report concerns, bounded by segment and byte limits.
    pub name: Name,
    pub state: [u8; 32],
    /// Reported outcome; contradictory pending/failure reports cannot be cleared by a passed
    /// report.
    pub status: Status,
}

/// Root-verified writer-hold relationship for a run's written resource; authority performs no
/// ownership discovery. (domain/authority.md, section 8.3).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Writer {
    /// Hold belongs to the running task.
    Task,
    /// Hold belongs to a root-verified ancestor of the running task.
    Ancestor,
    /// Hold is unresolved; run admission waits.
    Pending,
    /// Another task holds the writer; run admission waits.
    Other,
}

/// One workspace resource and its root-verified writer relationship, admitted under run write/name
/// limits. (domain/authority.md, section 8.3).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Write {
    /// Written resource and required connector kind, including saved workspace writes.
    pub effect: Effect,
    /// Root-verified current writer hold.
    pub held: Writer,
}

/// Root-gathered run admission snapshot; no claim, worker, clock or account state is held by this
/// question. (domain/authority.md, section 8.3).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct RunAsk {
    pub project: u32,
    /// Current task authority, bounded by `Limits`.
    pub authority: Authority,
    /// Current funding snapshot used to compute available run spend.
    pub numbers: Numbers,
    /// Offered run budget in the deployment unit; must exceed the minimum and fit all caps.
    pub budget: u64,
    /// Root-supplied current wall time for the deadline check.
    pub wall: Wall,
    /// One status for each model account its charter will use. One usability status per model
    /// account used by the charter, bounded by `Limits::accounts`.
    pub accounts: Box<[bool]>,
    /// Every resource its workspace will write, including saved work. Every written resource,
    /// including saved work, bounded by `Limits::writes`.
    pub writes: Box<[Write]>,
}

/// Root-translated person action after membership verification; authority returns a decision over
/// values, not task mutations. (domain/authority.md, section 8.4).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum PersonRequest {
    /// Check role permission and all-or-nothing batch funding.
    Create(
        /** Whole directly created batch, bounded by `Limits::batch` and each child's authority limits. */
        Box<[Delegate]>,
    ),
    /// The complete future authority/allotment to fund, not an unchecked delta. Check role
    /// permission and the complete future allotment.
    Allot(/** Complete future authority/allotment to reserve, bounded by authority limits. */ Authority),
    /// Check role acceptance and rights for one proposed action.
    Accept(/** Single proposed action; effect acceptance still needs its pinned-fact check. */ Action),
    /// What the amendment must newly give; a narrowing gives empty authority. Check role permission
    /// and authority newly given by the amendment.
    Amend(
        /** Authority the amendment newly gives, bounded by authority limits; narrowing supplies empty authority. */
        Authority,
    ),
    /// Check role permission and the authority/allotment funding the move.
    Move(/** Complete authority/allotment to fund for the moved task, bounded by authority limits. */ Authority),
    /// Check specific cancellation permission; does not cancel here.
    Cancel,
    /// Check specific release permission; does not release here.
    Release,
    /// Check specific watch permission; does not create a view here.
    Watch,
    /// Check specific policy-change permission; does not replace policy here.
    Policy,
}

/// Root-gathered person-role question with the actual funding snapshot and separate available
/// lifetime capacity. (domain/authority.md, section 8.4).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct PersonAsk {
    pub project: u32,
    pub role: u32,
    /// Actual person-period pool snapshot; funding requests check its budget against role period
    /// spend.
    pub pool: Numbers,
    /// Current lifetime task capacity available to this funding decision.
    pub tasks_left: u32,
    /// Single translated action checked for specific role permission.
    pub request: PersonRequest,
}

/// Root-translated tool-call requirements beyond its configured family bit; serving the call
/// remains with the root. (domain/authority.md, section 8.5).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Call {
    /// Family permission alone is checked here.
    Tool,
    /// Family permission and resource read grant are checked.
    Read(/** Pinned resource read requiring a covered grant and bounded name. */ Effect),
    /// Family permission and root-verified task reference are checked.
    Message {
        /** Whether the root verified a reference giving standing to message this task. */
        referenced: bool,
    },
    /// Family permission and one supported scope are checked.
    Note(/** Exactly one supported note scope; other scope shapes refuse. */ Scopes),
}

/// Root-gathered call permission question over one configured family and admitted
/// authority/resource values. (domain/authority.md, section 8.5).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct CallAsk {
    pub project: u32,
    /// Current caller authority, admitted under `Limits`.
    pub authority: Authority,
    /// Exactly one family bit, assigned by configuration. Exactly one tool-family bit selected by
    /// the root's registry; other shapes refuse.
    pub family: Tools,
    pub call: Call,
}

/// Owned proposed action used for needs and acceptance checks; caller bounds its payloads and
/// commits acceptance with execution. (domain/authority.md, section 9).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Action {
    /// Propose an all-or-nothing creation batch.
    Batch(/** Whole delegated batch, bounded by batch and authority limits. */ Box<[Delegate]>),
    /// Propose one exact pinned connector effect.
    Effect(/** One pinned connector effect with a bounded resource name. */ Effect),
    /// Propose wider authority for a task.
    Widen(/** Complete requested wider authority, bounded at admission. */ Authority),
    /// Propose authority newly needed by an amendment.
    Amend(/** Authority newly needed by the amendment, bounded at admission. */ Authority),
    /// Topology grants standing; release may additionally need authority. Propose release based on
    /// standing and any additionally needed authority.
    Escalate {
        /// Authority newly needed for release, or `None` when standing alone suffices.
        release: Option<Authority>,
    },
}

/// Root-verified eligible proposal accepter with current funding and task capacity; standing and
/// distance are not discovered here. (domain/authority.md, section 9).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Holder {
    /// The caller has verified that this task stands above the proposer. Ancestor task whose
    /// standing the caller has verified.
    Task {
        project: u32,
        /// Ancestor's admitted current authority.
        authority: Authority,
        /// Ancestor's current actual funding snapshot.
        numbers: Numbers,
        /// Ancestor's remaining lifetime task capacity.
        tasks_left: u32,
    },
    /// Person whose role membership and proposal standing the root has verified.
    Person {
        project: u32,
        role: u32,
        /// Proposal kind this role must be allowed to decide.
        proposal: ProposalKind,
        /// Actual person-period funding snapshot.
        pool: Numbers,
        /// Current lifetime task capacity available to acceptance.
        tasks_left: u32,
    },
}
