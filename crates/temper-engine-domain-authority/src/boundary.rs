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
    /// (domain/authority.md, section 8).
    Allow,
    /// Required facts, budget, account or writer condition are not ready; no action is authorized
    /// yet. (domain/authority.md, section 8).
    Wait,
    /// Task authority is insufficient but hard ceilings permit seeking an eligible accepter.
    /// (domain/authority.md, section 8).
    Propose,
    /// A hard requirement, admission bound or permission failed. (domain/authority.md, section 8).
    Refuse,
}

/// Authority layer that supplied a ceiling or permission for a finding. (domain/authority.md,
/// sections 6 and 8).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Source {
    /// The acting or creating task's authority. (domain/authority.md, sections 6 and 8).
    Task,
    /// The root-verified person's project role. (domain/authority.md, sections 6 and 8).
    Role,
    /// The live project's ceiling. (domain/authority.md, sections 6 and 8).
    Project,
    /// Immutable deployment rules. (domain/authority.md, sections 6 and 8).
    Deployment,
}

/// Fixed-size reason emitted by a pure check into caller-reserved bounded room; multiple
/// independent reasons may accompany one answer. (domain/authority.md, sections 8–10).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Finding {
    /// Input shape or payload exceeds admission limits. (domain/authority.md, sections 8–10).
    Oversized,
    /// No live policy exists for the named project. (domain/authority.md, sections 8–10).
    UnknownProject,
    /// The live policy has no named role. (domain/authority.md, sections 8–10).
    UnknownRole,
    /// Componentwise authority deficits against this source. (domain/authority.md, sections 8–10).
    Authority {
        /** Authority layer against which this condition failed. (domain/authority.md, sections 8–10). */
        source: Source,
        /** Independent component deficits against that source. (domain/authority.md, sections 8–10). */
        lacks: crate::Lacks,
    },
    /// A child executor is not permitted by this source. (domain/authority.md, sections 8–10).
    Executor {
        /** Authority layer against which this condition failed. (domain/authority.md, sections 8–10). */
        source: Source,
    },
    /// Aggregate lifetime task capacity does not fit this source. (domain/authority.md, sections
    /// 8–10).
    Tasks {
        /** Authority layer against which this condition failed. (domain/authority.md, sections 8–10). */
        source: Source,
    },
    /// Aggregate reserved spend does not fit this source. (domain/authority.md, sections 8–10).
    Spend {
        /** Authority layer against which this condition failed. (domain/authority.md, sections 8–10). */
        source: Source,
    },
    /// Required accounting or aggregate arithmetic is unrepresentable. (domain/authority.md,
    /// sections 8–10).
    Arithmetic,
    /// Offered run budget is not above the minimum or available funding is insufficient.
    /// (domain/authority.md, sections 8–10).
    RunBudget,
    /// Offered run budget exceeds deployment or task authority cap. (domain/authority.md, sections
    /// 8–10).
    RunCap,
    /// Task deadline is past the supplied wall time. (domain/authority.md, sections 8–10).
    Deadline,
    /// A required model account is unusable. (domain/authority.md, sections 8–10).
    Account,
    /// A written resource is held by another task or has a pending hold. (domain/authority.md,
    /// sections 8–10).
    Writer,
    /// Configured family is absent from an applicable authority value. (domain/authority.md,
    /// sections 8–10).
    Tool,
    /// This source has no grant covering the requested operation. (domain/authority.md, sections
    /// 8–10).
    Grant {
        /** Authority layer against which this condition failed. (domain/authority.md, sections 8–10). */
        source: Source,
    },
    /// The caller has no verified standing to message the task. (domain/authority.md, sections
    /// 8–10).
    Reference,
    /// The note scope is absent from this source. (domain/authority.md, sections 8–10).
    Scope {
        /** Authority layer against which this condition failed. (domain/authority.md, sections 8–10). */
        source: Source,
    },
    /// A pinned required fact is missing, unknown or pending. (domain/authority.md, sections 8–10).
    Required {
        /** Connector whose required pinned fact was checked. (domain/authority.md, sections 8–10). */
        connector: u16,
        /** Required connector fact kind. (domain/authority.md, sections 8–10). */
        fact: u16,
    },
    /// A matching pinned required fact failed. (domain/authority.md, sections 8–10).
    Failed {
        /** Connector whose required pinned fact was checked. (domain/authority.md, sections 8–10). */
        connector: u16,
        /** Required connector fact kind. (domain/authority.md, sections 8–10). */
        fact: u16,
    },
    /// The person's role does not allow this request kind. (domain/authority.md, sections 8–10).
    Unpermitted,
    /// The person's role may not accept this proposal kind. (domain/authority.md, sections 8–10).
    Undecidable,
    /// A funding request's pool budget exceeds the role's period ceiling. (domain/authority.md,
    /// sections 8–10).
    PeriodSpend,
    /// An applicable rule needs a landing snapshot that was not supplied. (domain/authority.md,
    /// sections 8–10).
    LandingMissing,
    /// Landing head differs from the effect's exact state pin. (domain/authority.md, sections
    /// 8–10).
    LandingPin,
    /// Required exact-head CI is not passed. (domain/authority.md, sections 8–10).
    Ci {
        /** Observed condition preventing allowance. (domain/authority.md, sections 8–10). */
        status: Status,
    },
    /// Required exact-head containment of the branch tip is not passed. (domain/authority.md,
    /// sections 8–10).
    Behind {
        /** Observed condition preventing allowance. (domain/authority.md, sections 8–10). */
        status: Status,
    },
    /// A blocking gate lacks a passed valid-head verdict. (domain/authority.md, sections 8–10).
    Gate {
        /** Blocking gate number resolved by the root. (domain/authority.md, sections 8–10). */
        number: u32,
        /** Observed condition preventing allowance. (domain/authority.md, sections 8–10). */
        status: Status,
    },
    /// Distinct valid approvals of the required role are fewer than requested.
    /// (domain/authority.md, sections 8–10).
    Approval {
        /** Required root-verified project role. (domain/authority.md, sections 8–10). */
        role: u32,
        /** Distinct eligible passed reviewers at valid heads. (domain/authority.md, sections 8–10). */
        have: u32,
        /** Required distinct-person count. (domain/authority.md, sections 8–10). */
        want: u32,
    },
    /// A valid reviewer of the required role requested changes. (domain/authority.md, sections
    /// 8–10).
    ReviewFailed {
        /** Required root-verified project role. (domain/authority.md, sections 8–10). */
        role: u32,
    },
}

/// Pure check result and optional replacement funding snapshot; the root commits allowed numbers
/// together with the action once. (domain/authority.md, sections 7–8).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Checked {
    /// Strictest answer after all applicable checks. (domain/authority.md, sections 7–8).
    pub answer: Answer,
    /// Replacement funding numbers only on `Allow`; other answers carry `None` and authorize no
    /// debit. (domain/authority.md, sections 7–8).
    pub numbers: Option<Numbers>,
}

/// Root-translated directly created task's executor and complete authority, admitted under batch
/// and authority limits. (domain/authority.md, section 8.1).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Delegate {
    /// Executor permission that the creator must have. (domain/authority.md, section 8.1).
    pub executor: Executor,
    /// Complete child authority, bounded by the configured authority limits. (domain/authority.md,
    /// section 8.1).
    pub authority: Authority,
}

/// Root-gathered creation question over one coherent creator and funding snapshot; no tasks or
/// reservations are retained here. (domain/authority.md, section 8.1).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct BatchAsk {
    /// Live policy project for this decision. (domain/authority.md, section 8.1).
    pub project: u32,
    /// Creator's current authority, bounded by `Limits`. (domain/authority.md, section 8.1).
    pub creator: Authority,
    /// Creator's current allotment snapshot, not a copy of its static ceiling.
    /// (domain/authority.md, section 8.1).
    pub numbers: Numbers,
    /// Lifetime task capacity still available, kept separately by tasks.
    /// Current lifetime task capacity available from the tasks ledger. (domain/authority.md,
    /// section 8.1).
    pub tasks_left: u32,
    /// Whole all-or-nothing batch, bounded by `Limits::batch`; each child costs one slot plus its
    /// descendant capacity. (domain/authority.md, section 8.1).
    pub tasks: Box<[Delegate]>,
}

/// A read or effect of one connector, pinned to the state it names.
/// Root-translated connector read or effect with a literal resource and exact state pin, bounded on
/// admission. (domain/authority.md, section 8.2).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Effect {
    /// Connector owning the resource and effect-kind vocabulary. (domain/authority.md, section
    /// 8.2).
    pub connector: u16,
    /// Requested exact kind; grant implications do not remove requirements on this kind.
    /// (domain/authority.md, section 8.2).
    pub kind: u16,
    /// Full literal resource name, bounded by segment and byte limits. (domain/authority.md,
    /// section 8.2).
    pub name: Name,
    /// Exact state pin carried into conditional execution by the root. (domain/authority.md,
    /// section 8.2).
    pub state: [u8; 32],
}

/// Root-gathered permission and authentic-fact question for one pinned effect; this value makes no
/// connector call. (domain/authority.md, section 8.2).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct EffectAsk {
    /// Live policy project for the effect. (domain/authority.md, section 8.2).
    pub project: u32,
    /// Acting task or accepter authority, admitted under `Limits`. (domain/authority.md, section
    /// 8.2).
    pub authority: Authority,
    /// Single pinned connector operation being checked. (domain/authority.md, section 8.2).
    pub effect: Effect,
    /// Coherent landing snapshot when available; absence waits if applicable landing rules require
    /// it. (domain/authority.md, section 8.2).
    pub landing: Option<Landing>,
}

/// Opaque fixed-width head/state identity supplied by the root; authority neither parses nor
/// resolves it. (domain/authority.md, section 10).
pub type Head = [u8; 32];

/// CI report supplied by the root for the exact landing head; CI never carries over clean
/// predecessors. (domain/authority.md, section 10).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Ci {
    /// Exact head on which CI was reported. (domain/authority.md, section 10).
    pub head: Head,
    /// CI outcome; missing or pending waits and failed refuses when CI is required.
    /// (domain/authority.md, section 10).
    pub status: Status,
}

/// Root-resolved gate report for the named change, with head freshness checked against the landing
/// snapshot. (domain/authority.md, section 10).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Verdict {
    /// Gate number resolved by the root. (domain/authority.md, section 10).
    pub gate: u32,
    /// Head on which this verdict was made. (domain/authority.md, section 10).
    pub head: Head,
    /// Verdict outcome, interpreted only at an accepted head. (domain/authority.md, section 10).
    pub status: Status,
}

/// Only authenticated people's latest reviews, with root-verified roles.
/// Agent reviews use gate verdicts and cannot count as human approvals.
/// Authenticated latest human review with root-verified project role; duplicate people count once
/// and agent gates never count as people. (domain/authority.md, section 10).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Review {
    /// Authenticated person number, used to count distinct reviewers. (domain/authority.md, section
    /// 10).
    pub person: u64,
    /// Project role verified by the root for this review. (domain/authority.md, section 10).
    pub role: u32,
    /// Head reviewed, checked against required freshness. (domain/authority.md, section 10).
    pub head: Head,
    /// Passed approval or failed request for changes; pending provides no approval.
    /// (domain/authority.md, section 10).
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
    /// Proposed landing head, which must equal the effect's exact state pin. (domain/authority.md,
    /// section 10).
    pub head: Head,
    /// Landing-branch tip whose containment was checked in this snapshot. (domain/authority.md,
    /// section 10).
    pub tip: Head,
    /// Containment result for this exact head and tip; unknown/pending waits and absence refuses
    /// when required. (domain/authority.md, section 10).
    pub contains_tip: Status,
    /// CI head and status from the same named change snapshot. (domain/authority.md, section 10).
    pub ci: Ci,
    /// Root-verified clean predecessor heads, bounded by `Limits::heads`; repair or conflict
    /// resolution breaks this lineage. (domain/authority.md, section 10).
    pub clean: Box<[Head]>,
    /// Change gates added to policy gates, bounded by `Limits::gates`. (domain/authority.md,
    /// section 10).
    pub gates: Box<[Gate]>,
    /// Gate reports for the change, bounded by `Limits::verdicts`. (domain/authority.md, section
    /// 10).
    pub verdicts: Box<[Verdict]>,
    /// Authenticated latest human reviews, bounded by `Limits::reviews`. (domain/authority.md,
    /// section 10).
    pub reviews: Box<[Review]>,
}

/// Reported fact state; unknown and pending wait, passed satisfies, and a relevant failure refuses.
/// (domain/authority.md, sections 8.2 and 10).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Status {
    /// No known outcome; required conditions wait. (domain/authority.md, sections 8.2 and 10).
    Unknown,
    /// Outcome remains in flight; required conditions wait. (domain/authority.md, sections 8.2 and
    /// 10).
    Pending,
    /// Reported condition is satisfied at its pin. (domain/authority.md, sections 8.2 and 10).
    Passed,
    /// Reported condition failed at its pin. (domain/authority.md, sections 8.2 and 10).
    Failed,
}

/// Authentic root-supplied connector report; matching requires the full resource name, exact state
/// pin and required fact kind. (domain/authority.md, section 8.2).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Fact {
    /// Connector that authenticated this report. (domain/authority.md, section 8.2).
    pub connector: u16,
    /// Reported connector fact kind, not the effect kind. (domain/authority.md, section 8.2).
    pub kind: u16,
    /// Full resource name the report concerns, bounded by segment and byte limits.
    /// (domain/authority.md, section 8.2).
    pub name: Name,
    /// Exact resource state at which the report holds. (domain/authority.md, section 8.2).
    pub state: [u8; 32],
    /// Reported outcome; contradictory pending/failure reports cannot be cleared by a passed
    /// report. (domain/authority.md, section 8.2).
    pub status: Status,
}

/// Root-verified writer-hold relationship for a run's written resource; authority performs no
/// ownership discovery. (domain/authority.md, section 8.3).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Writer {
    /// Hold belongs to the running task. (domain/authority.md, section 8.3).
    Task,
    /// Hold belongs to a root-verified ancestor of the running task. (domain/authority.md, section
    /// 8.3).
    Ancestor,
    /// Hold is unresolved; run admission waits. (domain/authority.md, section 8.3).
    Pending,
    /// Another task holds the writer; run admission waits. (domain/authority.md, section 8.3).
    Other,
}

/// One workspace resource and its root-verified writer relationship, admitted under run write/name
/// limits. (domain/authority.md, section 8.3).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Write {
    /// Written resource and required connector kind, including saved workspace writes.
    /// (domain/authority.md, section 8.3).
    pub effect: Effect,
    /// Root-verified current writer hold. (domain/authority.md, section 8.3).
    pub held: Writer,
}

/// Root-gathered run admission snapshot; no claim, worker, clock or account state is held by this
/// question. (domain/authority.md, section 8.3).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct RunAsk {
    /// Live policy project for the run. (domain/authority.md, section 8.3).
    pub project: u32,
    /// Current task authority, bounded by `Limits`. (domain/authority.md, section 8.3).
    pub authority: Authority,
    /// Current funding snapshot used to compute available run spend. (domain/authority.md, section
    /// 8.3).
    pub numbers: Numbers,
    /// Offered run budget in the deployment unit; must exceed the minimum and fit all caps.
    /// (domain/authority.md, section 8.3).
    pub budget: u64,
    /// Root-supplied current wall time for the deadline check. (domain/authority.md, section 8.3).
    pub wall: Wall,
    /// One status for each model account its charter will use.
    /// One usability status per model account used by the charter, bounded by `Limits::accounts`.
    /// (domain/authority.md, section 8.3).
    pub accounts: Box<[bool]>,
    /// Every resource its workspace will write, including saved work.
    /// Every written resource, including saved work, bounded by `Limits::writes`.
    /// (domain/authority.md, section 8.3).
    pub writes: Box<[Write]>,
}

/// Root-translated person action after membership verification; authority returns a decision over
/// values, not task mutations. (domain/authority.md, section 8.4).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum PersonRequest {
    /// Check role permission and all-or-nothing batch funding. (domain/authority.md, section 8.4).
    Create(
        /** Whole directly created batch, bounded by `Limits::batch` and each child's authority limits. (domain/authority.md, section 8.4). */
         Box<[Delegate]>,
    ),
    /// The complete future authority/allotment to fund, not an unchecked delta.
    /// Check role permission and the complete future allotment. (domain/authority.md, section 8.4).
    Allot(
        /** Complete future authority/allotment to reserve, bounded by authority limits. (domain/authority.md, section 8.4). */
         Authority,
    ),
    /// Check role acceptance and rights for one proposed action. (domain/authority.md, section
    /// 8.4).
    Accept(
        /** Single proposed action; effect acceptance still needs its pinned-fact check. (domain/authority.md, section 8.4). */
         Action,
    ),
    /// What the amendment must newly give; a narrowing gives empty authority.
    /// Check role permission and authority newly given by the amendment. (domain/authority.md,
    /// section 8.4).
    Amend(
        /** Authority the amendment newly gives, bounded by authority limits; narrowing supplies empty authority. (domain/authority.md, section 8.4). */
         Authority,
    ),
    /// Check role permission and the authority/allotment funding the move. (domain/authority.md,
    /// section 8.4).
    Move(
        /** Complete authority/allotment to fund for the moved task, bounded by authority limits. (domain/authority.md, section 8.4). */
         Authority,
    ),
    /// Check specific cancellation permission; does not cancel here. (domain/authority.md, section
    /// 8.4).
    Cancel,
    /// Check specific release permission; does not release here. (domain/authority.md, section
    /// 8.4).
    Release,
    /// Check specific watch permission; does not create a view here. (domain/authority.md, section
    /// 8.4).
    Watch,
    /// Check specific policy-change permission; does not replace policy here. (domain/authority.md,
    /// section 8.4).
    Policy,
}

/// Root-gathered person-role question with the actual funding snapshot and separate available
/// lifetime capacity. (domain/authority.md, section 8.4).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct PersonAsk {
    /// Project of the root-verified person membership. (domain/authority.md, section 8.4).
    pub project: u32,
    /// Numbered role in the live project policy. (domain/authority.md, section 8.4).
    pub role: u32,
    /// Actual person-period pool snapshot; funding requests check its budget against role period
    /// spend. (domain/authority.md, section 8.4).
    pub pool: Numbers,
    /// Current lifetime task capacity available to this funding decision. (domain/authority.md,
    /// section 8.4).
    pub tasks_left: u32,
    /// Single translated action checked for specific role permission. (domain/authority.md, section
    /// 8.4).
    pub request: PersonRequest,
}

/// Root-translated tool-call requirements beyond its configured family bit; serving the call
/// remains with the root. (domain/authority.md, section 8.5).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Call {
    /// Family permission alone is checked here. (domain/authority.md, section 8.5).
    Tool,
    /// Family permission and resource read grant are checked. (domain/authority.md, section 8.5).
    Read(
        /** Pinned resource read requiring a covered grant and bounded name. (domain/authority.md, section 8.5). */
        Effect,
    ),
    /// Family permission and root-verified task reference are checked. (domain/authority.md,
    /// section 8.5).
    Message {
        /** Whether the root verified a reference giving standing to message this task. (domain/authority.md, section 8.5). */
        referenced: bool,
    },
    /// Family permission and one supported scope are checked. (domain/authority.md, section 8.5).
    Note(
        /** Exactly one supported note scope; other scope shapes refuse. (domain/authority.md, section 8.5). */ Scopes,
    ),
}

/// Root-gathered call permission question over one configured family and admitted
/// authority/resource values. (domain/authority.md, section 8.5).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct CallAsk {
    /// Live policy project for the call. (domain/authority.md, section 8.5).
    pub project: u32,
    /// Current caller authority, admitted under `Limits`. (domain/authority.md, section 8.5).
    pub authority: Authority,
    /// Exactly one family bit, assigned by configuration.
    /// Exactly one tool-family bit selected by the root's registry; other shapes refuse.
    /// (domain/authority.md, section 8.5).
    pub family: Tools,
    /// Additional requirement of the translated call. (domain/authority.md, section 8.5).
    pub call: Call,
}

/// Owned proposed action used for needs and acceptance checks; caller bounds its payloads and
/// commits acceptance with execution. (domain/authority.md, section 9).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Action {
    /// Propose an all-or-nothing creation batch. (domain/authority.md, section 9).
    Batch(
        /** Whole delegated batch, bounded by batch and authority limits. (domain/authority.md, section 9). */
        Box<[Delegate]>,
    ),
    /// Propose one exact pinned connector effect. (domain/authority.md, section 9).
    Effect(/** One pinned connector effect with a bounded resource name. (domain/authority.md, section 9). */ Effect),
    /// Propose wider authority for a task. (domain/authority.md, section 9).
    Widen(/** Complete requested wider authority, bounded at admission. (domain/authority.md, section 9). */ Authority),
    /// Propose authority newly needed by an amendment. (domain/authority.md, section 9).
    Amend(
        /** Authority newly needed by the amendment, bounded at admission. (domain/authority.md, section 9). */
        Authority,
    ),
    /// Topology grants standing; release may additionally need authority.
    /// Propose release based on standing and any additionally needed authority.
    /// (domain/authority.md, section 9).
    Escalate {
        /// Authority newly needed for release, or `None` when standing alone suffices.
        /// (domain/authority.md, section 9).
        release: Option<Authority>,
    },
}

/// Root-verified eligible proposal accepter with current funding and task capacity; standing and
/// distance are not discovered here. (domain/authority.md, section 9).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Holder {
    /// The caller has verified that this task stands above the proposer.
    /// Ancestor task whose standing the caller has verified. (domain/authority.md, section 9).
    Task {
        /// Live policy project of the ancestor task. (domain/authority.md, section 9).
        project: u32,
        /// Ancestor's admitted current authority. (domain/authority.md, section 9).
        authority: Authority,
        /// Ancestor's current actual funding snapshot. (domain/authority.md, section 9).
        numbers: Numbers,
        /// Ancestor's remaining lifetime task capacity. (domain/authority.md, section 9).
        tasks_left: u32,
    },
    /// Person whose role membership and proposal standing the root has verified.
    /// (domain/authority.md, section 9).
    Person {
        /// Live policy project of the eligible person. (domain/authority.md, section 9).
        project: u32,
        /// Role verified by the root for acceptance. (domain/authority.md, section 9).
        role: u32,
        /// Proposal kind this role must be allowed to decide. (domain/authority.md, section 9).
        proposal: ProposalKind,
        /// Actual person-period funding snapshot. (domain/authority.md, section 9).
        pool: Numbers,
        /// Current lifetime task capacity available to acceptance. (domain/authority.md, section
        /// 9).
        tasks_left: u32,
    },
}
