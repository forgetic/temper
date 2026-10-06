//! Root-to-tasks semantic inputs and tasks-to-root persistence/lifecycle outputs
//! (domain/tasks.md, sections 2–5).
//! Inputs have no protocol bytes or connector events. Root authorizes them,
//! owns exact transport replay and commits related rows/effects atomically.
use crate::{Authority, Class, Funder, Numbers, Tries};
use alloc::boxed::Box;
use skein_lib::{ReplyTo, Wall};

/// Root-verified requester/creator identity; requester topology and actual financial source are
/// separate values. (domain/tasks.md, sections 2–3).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Party {
    /// Task requester/creator whose delegation topology this child owns.
    Task(
        /** Deployment task number of the creator/requester; task requesters must remain live until their delegates end. */
         u64,
    ),
    /// Person requester/creator authenticated by the root.
    Person(/** Authenticated deployment person number supplied by the root. */ u64),
    /// Deployment-owned work for one project, authorized by the root.
    Deployment {
        /** Project on whose behalf deployment configuration starts the task. */
        project: u32,
    },
}

/// Implemented task executor selected by the root; the contracted boundary exposes agent charters
/// only. (domain/tasks.md, section 2).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Executor {
    /// Configured agent charter; procedure/person execution routes are outside the contracted API.
    Agent {
        /** Configured agent charter number, checked against `Domain`'s admitted charter table. */
        charter: u32,
    },
}

/// Bounded typed semantic parameter carrier; tasks does not interpret names or resources, and the
/// current root chat constructor supplies no parameters. (domain/tasks.md, section 3).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Parameter {
    /// Typed numeric specification carrier; current chat construction supplies no parameters.
    Number {
        /** Opaque semantic parameter name, carried without interpretation by tasks. */
        name: u32,
        /** Numeric parameter value, carried without interpretation here. */
        value: u64,
    },
    /// Typed bounded byte specification carrier; current chat construction supplies no parameters.
    Bytes {
        /** Opaque semantic parameter name, carried without interpretation by tasks. */
        name: u32,
        /** Owned parameter bytes; these and `Spec::words` share `Limits::spec_bytes`. */
        value: Box<[u8]>,
    },
    /// Typed connector/resource carrier; no connector lookup or effect is performed by tasks.
    Resource {
        /** Opaque semantic parameter name, carried without interpretation by tasks. */
        name: u32,
        /** Connector identity carried without lookup or interpretation here. */
        connector: u16,
        /** Opaque semantic resource identifier; tasks does not load connector data. */
        resource: u64,
    },
}

/// Owned bounded task description supplied by root; current admission requires empty historical
/// inputs. (domain/tasks.md, section 3).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Spec {
    /// Owned specification words; combined with byte-valued parameters, bounded by
    /// `Limits::spec_bytes`.
    pub words: Box<[u8]>,
    /// Typed parameters bounded by `Limits::parameters`; tasks does not interpret their names, and
    /// current root chat construction supplies an empty slice.
    pub parameters: Box<[Parameter]>,
    /// Typed historical input identities, bounded by `Limits::inputs` for shape measurement;
    /// current `Make` and live restore require this slice empty.
    pub inputs: Box<[u64]>,
}

/// One admitted result choice with a unique code and a per-choice word-byte bound.
/// (domain/tasks.md, sections 3 and 5.6).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Verdict {
    /// Distinct permitted choice code within its contract.
    pub code: u32,
    /// Maximum word bytes for this choice, no greater than `Limits::result_bytes`.
    pub words: u32,
}

/// Root-supplied bounded result shape; tasks validates returned results against it before terminal
/// admission. (domain/tasks.md, sections 3 and 5.6).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Contract {
    /// Bounded report result.
    Report {
        /** Maximum report bytes, no greater than `Limits::result_bytes`. */
        words: u32,
    },
    /// One of a nonempty admitted list of distinct verdict codes and byte caps.
    Verdict {
        /** Nonempty distinct-code choices, bounded by `Limits::contract_choices`, each with a result-byte bound. */
        choices: Box<[Verdict]>,
    },
    /// Connector/kind-matching result with an opaque resource and bounded report.
    Change {
        /** Required connector identity in a successful change result. */
        connector: u16,

        kind: u16,
        /** Maximum change-report bytes, no greater than `Limits::result_bytes`. */
        words: u32,
    },
}

/// Owned executor outcome checked against the admitted task contract before lifecycle admission;
/// connector interpretation and outward delivery remain with root. (domain/tasks.md, section 5.6).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum TaskResult {
    /// Successful bounded report matching a report contract.
    Report {
        /** Report bytes, bounded by both the admitted contract and `Limits::result_bytes`. */
        words: Box<[u8]>,
    },
    /// Successful admitted verdict code with bounded explanation.
    Verdict {
        code: u32,
        /** Explanation bytes bounded by that choice's word cap and `Limits::result_bytes`. */
        words: Box<[u8]>,
    },
    /// Successful matching connector/kind result; tasks performs no connector effect.
    Change {
        /** Connector identity that must equal the admitted change contract. */
        connector: u16,
        /** Connector-defined kind that must equal the admitted change contract. */
        kind: u16,
        /** Opaque resulting resource identifier; root/connector interpretation remains outside tasks. */
        resource: u64,
        /** Change-report bytes bounded by the admitted contract and `Limits::result_bytes`. */
        words: Box<[u8]>,
    },
    /// Bounded failure result allowed by every contract; produces an `Ending::Failed`.
    Failure {
        /** Failure-result reason, allowed by every contract and bounded by `Limits::result_bytes`. */
        reason: Box<[u8]>,
    },
}

/// Durable final outcome, or a pending closing outcome; requester notification follows actual
/// settlement. (domain/tasks.md, sections 5.1 and 5.6).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Ending {
    /// Checked successful result proposed for final ending; retained while closing and made
    /// historical only after settlement.
    Done(/** Successful checked result retained in pending closing or in the historical ended record. */ TaskResult),
    /// Failure ending with its bounded reason, pending while closing or final in the historical
    /// record.
    Failed {
        /** Owned failure reason bounded by `Limits::result_bytes`. */
        reason: Box<[u8]>,
    },
    /// Cancellation of descendant work, optionally preserving an already-completed result; no
    /// arbitrary public cancel route is exposed.
    Cancelled {
        /** Owned cancellation reason bounded by `Limits::result_bytes`. */
        reason: Box<[u8]>,
        /** Optional already-completed result retained with cancellation, separately bounded and checked against the contract. */
        result: Option<TaskResult>,
    },
}

/// Content-free terminal classification used in observations and dependency readiness.
/// (domain/tasks.md, section 5.1).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Status {
    /// Task completed successfully; dependents can become due after settlement.
    Done,
    /// Task failed; waiting dependents are held.
    Failed,
    /// Task was cancelled; waiting dependents are held.
    Cancelled,
}

/// Root-to-tasks activation terminal, fenced by task/attempt; a terminal may park or retry without
/// ending the task. (domain/tasks.md, sections 5.2, 5.5 and 5.6).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum End {
    /// Proposed final result; valid finishing with live delegates requires cancelling them or is
    /// refused without admission.
    Finished {
        /** Owned proposed result; invalid bounded shapes consume an `Invalid` failure, while oversized priced terminals refuse before charging. */
        result: TaskResult,
        /** Whether to cancel live descendant work; otherwise a valid finish with live delegates is refused and its attempt remains live. */
        cancel_delegates: bool,
    },
    /// Successful activation park; clears retry/refusal counters and leaves the task idle, without
    /// implementing a wake route here.
    Parked,
    /// Count one classified failure, then back off or hold beyond the configured retry allowance.
    Failed(/** Reported failure category whose counter/backoff policy applies. */ Class),
    /// Pre-execution refusal pause using the transient delay policy, without spending a failure
    /// try.
    Refused,
}

/// Root-reported or task-derived reason to preserve a task's prior lifecycle while stopping its
/// live activation. (domain/tasks.md, section 5.5).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Hold {
    /// Configured retry allowance was exhausted.
    Failures(/** Failure class whose retry allowance was exhausted. */ Class),
    /// A required dependency failed or was cancelled.
    Dependency(/** `Dependency` that ended failed or cancelled, causing this dependent to be held. */ u64),
    /// Root reports a person's stop or another authorized stop decision.
    Stopped,
    /// Root reports resource drift requiring a decision; tasks owns no connector drift detection.
    Drift,
    /// Root reports a permanently failed effect requiring a decision.
    Effects,
    /// Actual spending exhausted available `funding`; a representable charge is still retained.
    Budget,
    /// Root reports that the permitted deadline prevents another activation.
    Deadline,
}

/// Current agent-task activation phase; task and attempt identity fence every external transition.
/// (domain/tasks.md, section 5.2).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Active {
    /// `Activation` parked; no current wake input is exposed by this contracted child.
    Idle,
    /// An activation is requested and root owns preparation.
    Due,
    /// Root is gathering the actual brief/resources; a claim or preparation failure follows.
    Preparing,
    /// Root committed the fenced attempt before assignment/start.
    Claimed {
        /** Committed root-issued strictly increasing nonzero attempt identity. */
        attempt: u64,
    },
    /// Root reported the exact claimed attempt started.
    Running {
        /** Current claimed attempt reported as started. */
        attempt: u64,
    },
    /// `Retry`/refusal pause with one projected deadline.
    BackingOff {
        /** Saved wall-time retry threshold, projected once to a monotonic deadline and reprojected on restore. */
        until: Wall,
    },
}

/// Ordered closing obligation; the root owns actual effect/resource settlement and returns
/// `Settled` after `Close`. (domain/tasks.md, section 5.1).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Stage {
    /// Wait for the exact live attempt's terminal.
    Run {
        /** `Live` attempt whose terminal must be heard before closing advances. */
        attempt: u64,
    },
    /// Wait for all live delegates and actual incoming financial allocations to settle.
    Delegates,
    /// `Close` output is owed settlement from root; tasks owns no connector obligations.
    Effects,
    /// Root closing obligations settled; final financial posting and historical end can commit.
    /// Unheld `Settled` is transient and refused on live restore; held prior closing may retain it.
    Settled,
}

/// Owned pending ending and closing stage retained until every obligation and financial allocation
/// settles. (domain/tasks.md, section 5.1).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Closing {
    /// Next unsettled closing obligation.
    pub stage: Stage,
    /// Bounded pending final outcome, preserved through holds and cancellation.
    pub ending: Ending,
}

/// Lifecycle preserved while held; terminal/settlement notifications can update it without lifting
/// the hold. (domain/tasks.md, section 5.1).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Was {
    /// Prior dependency-waiting phase.
    Waiting,
    /// Prior agent activation phase.
    Active(/** Prior agent activation phase, including its attempt/backoff fence. */ Active),
    /// Prior pending closing state.
    Closing(/** Prior closing stage and owned pending ending. */ Closing),
}

/// Task's complete lifecycle value; ended values occur in historical durable rows rather than the
/// live arena. (domain/tasks.md, section 5.1).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Phase {
    /// `Some` live dependency remains; readiness becomes `Due` when `waiting_on` empties.
    Waiting,
    /// `Agent` activation is idle, due, preparing, claimed, running or backing off.
    Active(Active),
    /// Pending ending waits for ordered activation/delegate/root settlement.
    Closing(/** Owned pending ending and next closing obligation. */ Closing),
    /// Prior lifecycle preserved for a decision; current contracted API exposes no general
    /// release/amend route.
    Held {
        /** Prior phase retained and updated while held; the hold is not lifted by a terminal. */
        was: Was,

        why: Hold,
    },
    /// Historical outcome outside the live arena; restoring it as live is refused.
    Ended(/** Historical final ending; this phase is excluded from the live restore arena. */ Ending),
}

/// Root-authorized member of an atomic creation batch; tasks preflights the whole graph, payloads,
/// capacity and actual finite reservations before mutation. (domain/tasks.md, sections 3–4).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct New {
    /// Fresh never-reused task number supplied by the root; live or within-batch duplicates refuse.
    pub number: u64,
    /// Policy project authenticated by root and checked against creator/project structural limits.
    pub project: u32,
    /// `Agent` charter executor, checked against admitted configuration.
    pub executor: Executor,
    /// Owned specification within configured counts/bytes, with empty historical inputs.
    pub spec: Spec,
    /// Admitted bounded result contract.
    pub contract: Contract,
    /// Complete root-approved authority carrier; tasks checks bounded shape, not policy inclusion.
    pub authority: Authority,
    /// Fresh allotment: budget equals authority spend, with all spend/reservation counters zero.
    pub numbers: Numbers,
    /// Actual finite source; task sources must be live ancestors of the task creator and external
    /// sources must belong to this project.
    pub funder: Funder,
    /// Distinct immutable dependencies, at most `Limits::dependencies`: members of this batch or
    /// the creator's current live delegates.
    pub dependencies: Box<[u64]>,
}

/// Owned durable task state emitted to root storage; root uses `RunContext` for preparation and
/// does not maintain a mutable copy of this ledger. (domain/tasks.md, sections 3 and 5).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct TaskRecord {
    /// Root-issued commit order for an ended result; zero while live and until the root saves an ending.
    pub result_position: u64,
    /// One bounded semantic held-chat decision and checked revision. Root owns
    /// authentication, routing and historical receipts.
    pub escalation: crate::Escalation,
    /// Stable never-reused deployment task identity.
    pub number: u64,
    pub project: u32,
    /// Original current requester topology, distinct from the actual financial funder.
    pub requester: Party,
    /// Committed cumulative expense of the current attempt; a `new` `Claim` resets only this
    /// attempt baseline, not total direct spend.
    pub run_spent: u64,
    /// Requester-tree root identity, equal to number for a top-level task.
    pub root: u64,
    /// Structural depth below that root, bounded by `Limits::depth`.
    pub depth: u32,
    pub executor: Executor,
    /// Owned bounded specification; live state has empty historical inputs.
    pub spec: Spec,
    pub contract: Contract,
    /// Owned bounded permission carrier checked by root policy, not by tasks.
    pub authority: Authority,
    /// Authentic current-allotment accounting, updated atomically with lifecycle admission.
    pub numbers: Numbers,
    /// Actual source of the current allotment, kept separate from requester ancestry.
    pub funder: Funder,
    /// Immutable admitted dependency identities, bounded by `Limits::dependencies`.
    pub dependencies: Box<[u64]>,
    /// `Exact` still-live subset of dependencies, bounded by `Limits::dependencies`; removed once
    /// per dependency end in the same decision. `Restore` checks count/uniqueness/subset before
    /// retention and complete live coverage after all pages.
    pub waiting_on: Box<[u64]>,
    /// Current live direct delegates, bounded by `Limits::delegates`; requester ending waits until
    /// they are gone.
    pub delegates: Box<[u64]>,
    /// Latest newly admitted contiguous turn in the current attempt; `new` `Claim` starts at zero.
    pub turn: u32,
    /// Lifetime tasks made in this subtree, including itself, bounded by `Limits::tree_tasks`;
    /// ending delegates does not return capacity.
    pub made: u32,
    /// Latest root-issued attempt identity; `new` claims must strictly increase it.
    pub attempt: u64,
    /// Most recently accepted activation-terminal attempt while the task remains live; not an exact
    /// transport replay receipt.
    pub last_answer: Option<u64>,
    /// Per-class failure history used with the configured retry policy.
    pub tries: Tries,
    /// Saturating pre-execution refusal counter used for pauses rather than failure tries.
    pub refusals: u32,
    /// Current lifecycle or the ended historical outcome.
    pub phase: Phase,
}

/// Typed logical store `key` for task state or financial history, with no byte encoding performed
/// in this child. (domain/tasks.md, section 2).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Key {
    /// Logical mutable live-task row.
    Live(/** `Live` task identity whose row is replaced or erased. */ u64),
    /// Logical historical ended-task row.
    Ended(/** Historical ended task identity retained in the store, outside the live arena. */ u64),
    /// Logical finite period/pool row.
    Ledger(/** Actual period/pool source identity. */ Funder),
}

/// Tasks-to-root persistence row or root-to-tasks live restore input; only `Live` and `Ledger`
/// belong to startup restoration. (domain/tasks.md, section 2).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Stored {
    /// Current bounded live task state, emitted on mutation and admitted once at startup.
    Live(
        /** Owned boxed bounded live task row; startup validates its shape and later validates graph/financial links. */
         Box<TaskRecord>,
    ),
    /// Historical final task state; root loads it for actual authenticated result reads, never as
    /// live startup state.
    Ended(
        /** Owned boxed historical ending retained in root storage; never a live restore input or a replayed result delivery. */
         Box<TaskRecord>,
    ),
    /// Authentic finite external source accounting; restored links are checked with live
    /// reservations.
    Ledger(
        /** Finite period/pool accounting owned by tasks and admitted at startup under `Limits::funders`. */
        crate::FundingRecord,
    ),
}

impl Stored {
    /// Pure fixed-size projection of this row's logical `key`; no allocation, mutation, persistence
    /// or lifecycle output.
    #[must_use]
    pub const fn key(&self) -> Key {
        match self {
            Stored::Live(record) => Key::Live(record.number),
            Stored::Ended(record) => Key::Ended(record.number),
            Stored::Ledger(record) => Key::Ledger(record.funder),
        }
    }
}

/// Terminal bounded admission reason; refusal makes no requested lifecycle or financial mutation.
/// (domain/tasks.md, sections 4–5).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Refusal {
    /// Startup is restoring or failed; no mutation is admitted.
    NotReady,
    /// Named creator/task is not in the live arena.
    Unknown,
    /// Finite source table has no room before source creation/carving.
    Busy,
    /// Task/source identity is already retained or a task number repeats within the batch.
    Duplicate,
    /// `Make` batch is empty.
    Empty,
    /// `Batch` exceeds the configured direct-child count.
    Batch,
    /// `Batch` cannot fit the live task slab/name set, including slots not yet reclaimed.
    Live,
    /// Creator/project identity or per-project live task count is invalid.
    Project,
    /// Lifetime requester-tree task count would exceed its configured bound.
    Tree,
    /// A `new` child's structural depth would exceed the configured bound.
    Depth,
    /// Creator's live delegate list cannot fit the batch.
    Delegates,
    /// `Dependency` count, uniqueness or current-live/same-batch identity is invalid.
    Dependencies,
    /// Combined live delegation waits and immutable dependencies would cycle.
    Cycle,
    /// `Agent` charter is absent from the admitted configured table.
    Executor,
    /// Specification count or aggregate owned bytes are invalid.
    Spec,
    /// `TaskResult`-contract shape or a priced terminal's result-byte admission is invalid.
    Contract,
    /// Carried authority collections or aggregate bytes exceed the configured shape bounds.
    AuthorityShape,
    /// Historical inputs are nonempty; no current root historical-input creation route is
    /// implemented.
    Inputs,
    /// Current task phase does not permit the requested admission.
    State,
    /// `Attempt` is stale, zero or incompatible with the current phase.
    Attempt,
    /// Valid finishing declines to cancel still-live delegates; activation and charge remain
    /// unadmitted.
    LiveDelegates,
    /// `Live` startup row shape, graph or actual financial links are invalid; historical rows
    /// cannot be restored live.
    Restore,
    /// `Turn` carries a read fence although this contracted API has no inbox/read route.
    Read,
    /// `Turn` is not the next contiguous current-attempt turn, or cumulative expense decreases.
    Turn,
    /// Authentic finite source/reservation or eventual actual-chain arithmetic cannot admit the
    /// whole decision.
    Funding,
}

/// `Refusal` location and reason returned to the root; batch-wide failures may have no single task
/// identity. (domain/tasks.md, sections 4–5).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Problem {
    /// Offending task identity when one exists; `None` denotes a batch/source/global failure.
    pub task: Option<u64>,
    /// Bounded structural, lifecycle, financial or restore admission reason.
    pub why: Refusal,
}

/// Lifecycle acknowledgement classification, not an exact payload replay proof; exact transport
/// replay is the root's responsibility. (domain/tasks.md, section 5.6).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Accepted {
    /// This event newly admitted the task transition or turn.
    New,
    /// `Unpriced` terminal attempt equals the live task's recorded `last_answer`; root remains
    /// responsible for exact payload proof.
    Already,
}

/// Root-issued semantic task/finance inputs after authorization and transport fencing;
/// notifications have no reply destination and may be ignored if stale. (domain/tasks.md, sections 4–5).
#[derive(PartialEq, Eq, Debug)]
pub enum Event {
    /// Root `SetRoles` preflight: inspect only current person-requested `Waiting`
    /// contexts without mutation; one typed terminal even before readiness.
    InspectEscalations {
        /// Stage-local root right consumed once by `EscalationsInspected`
        reply_to: ReplyTo,
        /// Root-validated project whose current `Waiting` tasks are inspected.
        project: u32,
    },
    /// Root after successful `ApplyRoles` in the same synchronous decision; emit
    /// existing `EscalationNeeded` for `Waiting` only and one completion.
    RecheckEscalations {
        /// Stage-local root right consumed by `EscalationsRechecked`
        reply_to: ReplyTo,
        /// Same preflight project; rejected/currently unheld tasks stay inert
        project: u32,
    },
    /// Root queries one bounded held view for actual named reads/decisions;
    /// returns one `EscalationInspected`, including absent.
    InspectEscalation {
        /// Root-owned synchronous correlation, echoed once.
        reply_to: ReplyTo,
        /// Named task; this query mutates nothing.
        task: u64,
    },
    /// Root resolves the just-created routing obligation before commitment;
    /// stale notification changes nothing.
    RoutedEscalation {
        /// Held person chat.
        task: u64,
        /// Exact positive semantic revision.
        revision: u64,
        /// Root-verified eligible requester or final policy role.
        holder: crate::EscalationHolder,
    },
    /// Root has authenticated the current recipient and checked authority;
    /// one `EscalationDecided` terminal follows.
    DecideEscalation {
        /// Root-owned correlation owed one semantic terminal.
        reply_to: ReplyTo,
        task: u64,
        /// Waiting revision, never a root transport receipt.
        revision: u64,
        /// Positive authenticated deciding person.
        by: u64,
        /// Bounded authorized release/reject/pass choice.
        decision: crate::EscalationDecision,
    },
    /// `Open` one finite monotonic project period after root authorization; emits ledger `Save`
    /// plus `Done`, or one `Refused` with no mutation.
    OpenPeriod {
        /// Root-issued destination owed one `Done` or `Refused` terminal.
        reply_to: ReplyTo,
        project: u32,
        /// Fresh period identity, strictly greater than retained periods for this project.
        period: u64,
        /// Finite root-authorized deployment-unit budget; source capacity is checked before
        /// mutation.
        budget: u64,
    },
    /// Reserve one finite person's pool from its original period after root authorization; emits
    /// both ledger Saves plus `Done`, or `Refused` atomically.
    CarvePool {
        /// Root-issued destination owed one `Done` or `Refused` terminal.
        reply_to: ReplyTo,
        project: u32,
        /// Person whose current role and allotment root authenticated and checked.
        person: u64,
        /// Existing original project period whose available budget is reserved for this pool.
        period: u64,
        /// Complete finite pool budget, atomically reserved from the original period.
        budget: u64,
    },
    /// Admit only the next contiguous turn and its checked expense delta atomically; nonempty read
    /// fences, stale turns/attempts and arithmetic failures refuse without debit.
    Turn {
        /// Root-issued destination owed one `TurnAcknowledged` or `Refused` terminal.
        reply_to: ReplyTo,
        /// `Live` task whose claimed attempt and next turn are being admitted.
        task: u64,
        /// Nonzero current attempt fence; stale attempts cannot charge or advance turns.
        attempt: u64,
        /// Exactly the next contiguous nonzero turn in this attempt; root handles exact replay
        /// before this child input.
        turn: u32,
        /// Must be `None` in this contracted boundary; no inbox/read-fence route is implemented
        /// here.
        read: Option<u64>,
        /// Whole priced attempt expense; tasks posts only the checked delta above `run_spent`,
        /// including representable overruns.
        cumulative: u64,
    },
    /// Create an authorized whole batch and actual reservations atomically, producing `Made` or
    /// `Refused` plus bounded persistence/lifecycle outputs.
    Make {
        /// Root-issued destination owed one `Made` or `Refused` terminal for the whole batch.
        reply_to: ReplyTo,
        /// Root-verified creator/requester of every `new` task; task creator and actual `funding`
        /// ancestry are checked separately.
        creator: Party,
        /// Owned nonempty batch bounded by `Limits::batch`; all graph, payload and reservation
        /// checks precede mutation.
        batch: Box<[New]>,
    },
    /// `Due`-to-`Preparing` admission, producing `Done` or `Refused`.
    Prepare {
        /// Destination owed one `Done` or `Refused` terminal.
        reply_to: ReplyTo,
        /// `Live` `Due` task to enter `Preparing`; other phases refuse without mutation.
        task: u64,
    },
    /// `Preparing`-to-`Claimed` admission with a strictly increasing root attempt, producing `Done`
    /// or `Refused`.
    Claim {
        /// Destination owed one `Done` or `Refused` terminal.
        reply_to: ReplyTo,
        /// `Live` `Preparing` task whose attempt is being claimed.
        task: u64,
        /// Root-issued nonzero identity strictly greater than the latest claim; success resets
        /// attempt turn/expense baselines.
        attempt: u64,
    },
    /// Notification changing only the matching `Claimed` attempt to `Running`; absent/stale inputs
    /// are ignored and no reply is owed.
    Started {
        /// `Live` claimed task the root reports as started.
        task: u64,
        /// Matching claimed attempt; absent tasks and stale phases/attempts are ignored.
        attempt: u64,
    },
    /// Admit a root-fenced activation terminal with explicit expense provenance; emits one
    /// `Acknowledged` or `Refused` and atomic resulting saves.
    Activation {
        /// Destination owed one `Acknowledged` or `Refused` terminal; root delays exposure until
        /// the decision is durable.
        reply_to: ReplyTo,
        /// `Live` task whose activation terminal is admitted.
        task: u64,
        /// Root-fenced current attempt, including a stopped run preserved while held or closing.
        attempt: u64,
        /// Bounded root-supplied activation outcome; tasks normalizes invalid outcomes. A
        /// park/retry need not end the task.
        end: End,
        /// `Priced` worker cumulative expense or unpriced topology/readiness cause; root owns exact
        /// replay fencing.
        cause: Cause,
    },
    /// Notification pausing only a `Preparing` task; does not consume a failure try and has no
    /// reply.
    PreparationFailed {
        /// `Preparing` task to pause without consuming an activation failure try; absent or other-
        /// phase tasks are ignored.
        task: u64,
    },
    /// Authorized notification preserving prior state and issuing `Stop` for any live run;
    /// absent/already-held inputs have no output.
    Hold {
        /// `Live` task to hold while retaining its prior phase; absent/already-held tasks are
        /// ignored.
        task: u64,
        /// Authorized or root-classified hold reason; tasks judges no policy here.
        why: Hold,
    },
    /// Root notification after `Close` obligations finish; matching `Effects` stages advance,
    /// including held prior closing, without lifting a hold.
    Settled {
        /// Task whose root-owned `Close` obligations settled; only `Effects` stages advance,
        /// including preserved closing while held.
        task: u64,
    },
    /// Admit one `Live`/`Ledger` row during restoring; invalid input permanently fails restoration
    /// with `RestoreRefused`.
    Restore {
        /// One bounded `Live` or `Ledger` row while restoring; historical `Ended` and
        /// invalid/duplicate rows fail restoration.
        record: Stored,
    },
    /// Validate complete requester/dependency/financial links and combined acyclicity, then emit
    /// bounded activations/adoptions/closing outputs. Repetition after ready/failed is ignored.
    Restored,
}

/// Tasks-to-root typed persistence, lifecycle and terminal reply outputs; root groups resulting
/// saves/erases with effects and delays outward replies until durability. (domain/tasks.md, section 5).
#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    /// Terminal read-only `SetRoles` preflight; snapshot is owned transient root
    /// context, not retained state.
    EscalationsInspected {
        /// Echoed stage-local right.
        reply_to: ReplyTo,
        /// At most `Limits::tasks` `Waiting` contexts without owned rejection
        /// reasons; only `NotReady` can refuse.
        result: Result<Box<[crate::EscalationContext]>, Refusal>,
    },
    /// Terminal for `RecheckEscalations` after its existing `EscalationNeeded` outputs; root
    /// completes the keyed role flight after routing them.
    EscalationsRechecked {
        /// Echoed stage-local right, consumed once.
        reply_to: ReplyTo,
        /// Success or `NotReady` before any recheck output
        result: Result<(), Refusal>,
    },
    /// Tasks asks root to resolve its new held person chat in this atomic
    /// decision; no outward notice precedes commitment.
    EscalationNeeded {
        /// Temporary bounded held context; root drops it after routing
        context: Box<crate::EscalationContext>,
    },
    /// Terminal for one `InspectEscalation`; query owns no durable task copy
    EscalationInspected {
        /// Echoed root correlation, consumed once.
        reply_to: ReplyTo,
        /// Owned bounded view, or absent/non-person/non-held task
        context: Option<Box<crate::EscalationContext>>,
    },
    /// One semantic decision terminal; root commits accepted child writes,
    /// its typed history and people's keyed answer together.
    EscalationDecided {
        /// Echoed root correlation, consumed once.
        reply_to: ReplyTo,
        task: u64,
        revision: u64,
        /// Accepted semantic transition or unchanged refusal.
        outcome: crate::EscalationOutcome,
    },
    /// One successful `Make` terminal reporting the complete batch.
    Made {
        /** Original `Make` destination for its one successful terminal reply. */
        reply_to: ReplyTo,
        /** All `new` task numbers in input order, bounded by `Limits::batch`; no partial creation is reported. */
        tasks: Box<[u64]>,
    },
    /// One reply-bearing entrance/lifecycle/financial refusal; no requested mutation was admitted.
    Refused {
        /** Original reply-bearing event destination, consumed once on refusal. */
        reply_to: ReplyTo,
        /** Admission location/reason; requested mutation was not made. */
        problem: Problem,
    },
    /// One successful `funding`, preparation or claim terminal.
    Done {
        /** Original successful `funding`/preparation/claim destination, consumed once. */
        reply_to: ReplyTo,
    },
    /// One activation-terminal reply; durability and exact transport replay proof belong to root.
    Acknowledged {
        /** Original activation-terminal destination; root durability barrier precedes outward acknowledgement. */
        reply_to: ReplyTo,
        /** Task whose lifecycle terminal was admitted or already recorded. */
        task: u64,
        /** Fenced activation attempt. */
        attempt: u64,
        /** New lifecycle admission or existing last-answer identity; exact payload replay is separately verified by root. */
        accepted: Accepted,
    },
    /// One newly admitted turn reply; root saves its transcript with the charged task state before
    /// outward acknowledgement.
    TurnAcknowledged {
        /** Original `new`-turn destination; root commits transcript and charged task row together before acknowledging. */
        reply_to: ReplyTo,
        /** Task whose turn and expense delta were admitted. */
        task: u64,
        /** Current claimed attempt identity. */
        attempt: u64,
        /** New contiguous turn committed in task state. */
        turn: u32,
        /** Current charged-turn route emits `Accepted::New`; root owns replay acknowledgement without re-entering this route. */
        accepted: Accepted,
    },
    /// Actual bounded semantic preparation request to root, not a rendered brief or a raw mutable
    /// task peek.
    Activate {
        /** Owned bounded semantic preparation snapshot; root selects the actual route and drops the temporary snapshot on claim/failure. */
        context: Box<RunContext>,
    },
    /// Ask root/fleet to stop one exact live attempt; its lifecycle terminal is still owed.
    Stop {
        /** Task whose live activation must be stopped by root/fleet. */
        task: u64,
        /** `Exact` live attempt to stop; its terminal remains owed before closing can advance. */
        attempt: u64,
    },
    /// Ask root/fleet to reconcile a restored actual claim and highest committed turn.
    Adopt {
        /** `Restored` live task whose actual claim the root must reconcile with fleet. */
        task: u64,
        /** `Restored` committed attempt identity to adopt; no `new` attempt is minted here. */
        attempt: u64,
        /** Highest committed turn in the restored attempt, supplied for fleet reconciliation. */
        kept: u32,
    },
    /// Ask root to complete its actual closing obligations, then return `Settled`; emitted only
    /// after delegates and funded allocations settle.
    Close {
        /** Task whose delegates/financial allocations have settled and root closing obligations must finish. */
        task: u64,
        /** Bounded pending final ending; root returns `Settled` only after its actual closing obligations finish. */
        ending: Ending,
    },
    /// One newly ended requester-identified notification emitted with the ended record and exact
    /// financial posting; current root exposes person result notices, with no task-requester inbox
    /// route or child delivery credit.
    Ended {
        /** Task removed from the live arena and saved as a historical ended row. */
        task: u64,
        /** Actual requester identifying this notification; current root consumes person result notices, and tasks retains no delivery credit or inbox. */
        requester: Party,
        /** Bounded final result/reason; emitted with ended/actual-funder writes in one root decision. */
        ending: Ending,
    },
    /// Typed owned persistence output for the parent's current atomic decision.
    Save {
        /** Owned typed row joining the root's atomic decision; not an `IO` submission or a durable acknowledgement by itself. */
        record: Stored,
    },
    /// Typed logical-row removal for the parent's current atomic decision.
    Erase {
        /** Typed row removal joining the same atomic decision as related saves and outputs. */
        key: Key,
    },
    /// Terminal startup failure; this instance cannot become ready through more
    /// `Restore`/`Restored` inputs.
    RestoreRefused {
        /** Terminal startup validation failure; this domain instance remains unready. */
        problem: Problem,
    },
}

/// Root's expense provenance for a lifecycle terminal; tasks owns authentic delta/accounting
/// admission while root owns exact transport replay proofs.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Cause {
    /// Worker terminal with authentic cumulative expense: preflight lifecycle and eventual
    /// financial arithmetic before admitting the `new` delta together.
    Priced {
        /// Authentic whole attempt expense, monotonic relative to `run_spent` and preflighted
        /// against eventual actual-chain representability.
        cumulative: u64,
    },
    /// Loss, fleet refusal or invalid-answer normalization supplied by root; performs lifecycle
    /// admission without charging and still owes one terminal reply.
    Unpriced,
}

/// Temporary owned semantic preparation snapshot emitted to root, bounded by task limits; not a
/// second mutable `TaskRecord` or `funding` ledger. (domain/tasks.md, section 3).
/// (domain/engine.md, sections 7.1 and 9).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RunContext {
    pub task: u64,
    pub project: u32,
    /// Implemented task executor, currently an agent charter selected by root.
    pub executor: Executor,
    /// Bounded owned semantic description; not rendered brief bytes.
    pub spec: Spec,
    /// Bounded result contract root includes in the brief.
    pub contract: Contract,
    /// Actual requester included in preparation semantics.
    pub requester: Party,
    /// `Exact` carried permission value root translates to authority's independent vocabulary.
    pub authority: Authority,
    /// Borrowed-then-copied authentic financial snapshot for this preparation; root does not mutate
    /// or persist it as a shadow ledger.
    pub numbers: Numbers,
}
