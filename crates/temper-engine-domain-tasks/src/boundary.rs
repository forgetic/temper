//! Root-to-tasks semantic inputs and tasks-to-root persistence/lifecycle outputs
//! (domain/tasks.md, sections 2–5 and 14; domain/engine.md, section 7.5).
//! Inputs have no protocol bytes or connector events. Root authorizes them,
//! owns exact transport replay and commits related rows/effects atomically.
use crate::{Authority, Class, Funder, Numbers, Tries};
use alloc::boxed::Box;
use skein_lib::{ReplyTo, Wall};

/// Root-verified requester/creator identity; requester topology and actual financial source are
/// separate values. (domain/tasks.md, sections 2–3 and 14).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Party {
    /// Task requester/creator whose delegation topology this child owns. (domain/tasks.md, sections
    /// 2–3 and 14).
    Task(
        /** Deployment task number of the creator/requester; task requesters must remain live until their delegates end. (domain/tasks.md, sections 2–3 and 14). */
         u64,
    ),
    /// Person requester/creator authenticated by the root. (domain/tasks.md, sections 2–3 and 14).
    Person(
        /** Authenticated deployment person number supplied by the root. (domain/tasks.md, sections 2–3 and 14). */ u64,
    ),
    /// Deployment-owned work for one project, authorized by the root. (domain/tasks.md, sections
    /// 2–3 and 14).
    Deployment {
        /** Project on whose behalf deployment configuration starts the task. (domain/tasks.md, sections 2–3 and 14). */
        project: u32,
    },
}

/// Implemented task executor selected by the root; the contracted boundary exposes agent charters
/// only. (domain/tasks.md, sections 2 and 14).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Executor {
    /// Configured agent charter; procedure/person execution routes are outside the contracted API.
    /// (domain/tasks.md, sections 2 and 14).
    Agent {
        /** Configured agent charter number, checked against `Domain`'s admitted charter table. (domain/tasks.md, sections 2 and 14). */
        charter: u32,
    },
}

/// Bounded typed semantic parameter carrier; tasks does not interpret names or resources, and the
/// current root chat constructor supplies no parameters. (domain/tasks.md, sections 3 and 14).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Parameter {
    /// Typed numeric specification carrier; current chat construction supplies no parameters.
    /// (domain/tasks.md, sections 3 and 14).
    Number {
        /** Opaque semantic parameter name, carried without interpretation by tasks. (domain/tasks.md, sections 3 and 14). */
        name: u32,
        /** Numeric parameter value, carried without interpretation here. (domain/tasks.md, sections 3 and 14). */
        value: u64,
    },
    /// Typed bounded byte specification carrier; current chat construction supplies no parameters.
    /// (domain/tasks.md, sections 3 and 14).
    Bytes {
        /** Opaque semantic parameter name, carried without interpretation by tasks. (domain/tasks.md, sections 3 and 14). */
        name: u32,
        /** Owned parameter bytes; these and `Spec::words` share `Limits::spec_bytes`. (domain/tasks.md, sections 3 and 14). */
        value: Box<[u8]>,
    },
    /// Typed connector/resource carrier; no connector lookup or effect is performed by tasks.
    /// (domain/tasks.md, sections 3 and 14).
    Resource {
        /** Opaque semantic parameter name, carried without interpretation by tasks. (domain/tasks.md, sections 3 and 14). */
        name: u32,
        /** Connector identity carried without lookup or interpretation here. (domain/tasks.md, sections 3 and 14). */
        connector: u16,
        /** Opaque semantic resource identifier; tasks does not load connector data. (domain/tasks.md, sections 3 and 14). */
        resource: u64,
    },
}

/// Owned bounded task description supplied by root; current admission requires empty historical
/// inputs. (domain/tasks.md, sections 3 and 14).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Spec {
    /// Owned specification words; combined with byte-valued parameters, bounded by
    /// `Limits::spec_bytes`. (domain/tasks.md, sections 3 and 14).
    pub words: Box<[u8]>,
    /// Typed parameters bounded by `Limits::parameters`; tasks does not interpret their names, and
    /// current root chat construction supplies an empty slice. (domain/tasks.md, sections 3 and
    /// 14).
    pub parameters: Box<[Parameter]>,
    /// Typed historical input identities, bounded by `Limits::inputs` for shape measurement;
    /// current `Make` and live restore require this slice empty. (domain/tasks.md, sections 3 and
    /// 14).
    pub inputs: Box<[u64]>,
}

/// One admitted result choice with a unique code and a per-choice word-byte bound.
/// (domain/tasks.md, sections 3, 5.6 and 14).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Verdict {
    /// Distinct permitted choice code within its contract. (domain/tasks.md, sections 3, 5.6 and
    /// 14).
    pub code: u32,
    /// Maximum word bytes for this choice, no greater than `Limits::result_bytes`.
    /// (domain/tasks.md, sections 3, 5.6 and 14).
    pub words: u32,
}

/// Root-supplied bounded result shape; tasks validates returned results against it before terminal
/// admission. (domain/tasks.md, sections 3, 5.6 and 14).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Contract {
    /// Bounded report result. (domain/tasks.md, sections 3, 5.6 and 14).
    Report {
        /** Maximum report bytes, no greater than `Limits::result_bytes`. (domain/tasks.md, sections 3, 5.6 and 14). */
        words: u32,
    },
    /// One of a nonempty admitted list of distinct verdict codes and byte caps. (domain/tasks.md,
    /// sections 3, 5.6 and 14).
    Verdict {
        /** Nonempty distinct-code choices, bounded by `Limits::contract_choices`, each with a result-byte bound. (domain/tasks.md, sections 3, 5.6 and 14). */
        choices: Box<[Verdict]>,
    },
    /// Connector/kind-matching result with an opaque resource and bounded report. (domain/tasks.md,
    /// sections 3, 5.6 and 14).
    Change {
        /** Required connector identity in a successful change result. (domain/tasks.md, sections 3, 5.6 and 14). */
        connector: u16,
        /** Required connector-defined change kind. (domain/tasks.md, sections 3, 5.6 and 14). */
        kind: u16,
        /** Maximum change-report bytes, no greater than `Limits::result_bytes`. (domain/tasks.md, sections 3, 5.6 and 14). */
        words: u32,
    },
}

/// Owned executor outcome checked against the admitted task contract before lifecycle admission;
/// connector interpretation and outward delivery remain with root. (domain/tasks.md, sections 5.6
/// and 14).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum TaskResult {
    /// Successful bounded report matching a report contract. (domain/tasks.md, sections 5.6 and
    /// 14).
    Report {
        /** Report bytes, bounded by both the admitted contract and `Limits::result_bytes`. (domain/tasks.md, sections 5.6 and 14). */
        words: Box<[u8]>,
    },
    /// Successful admitted verdict code with bounded explanation. (domain/tasks.md, sections 5.6
    /// and 14).
    Verdict {
        /** One admitted verdict choice code. (domain/tasks.md, sections 5.6 and 14). */
        code: u32,
        /** Explanation bytes bounded by that choice's word cap and `Limits::result_bytes`. (domain/tasks.md, sections 5.6 and 14). */
        words: Box<[u8]>,
    },
    /// Successful matching connector/kind result; tasks performs no connector effect.
    /// (domain/tasks.md, sections 5.6 and 14).
    Change {
        /** Connector identity that must equal the admitted change contract. (domain/tasks.md, sections 5.6 and 14). */
        connector: u16,
        /** Connector-defined kind that must equal the admitted change contract. (domain/tasks.md, sections 5.6 and 14). */
        kind: u16,
        /** Opaque resulting resource identifier; root/connector interpretation remains outside tasks. (domain/tasks.md, sections 5.6 and 14). */
        resource: u64,
        /** Change-report bytes bounded by the admitted contract and `Limits::result_bytes`. (domain/tasks.md, sections 5.6 and 14). */
        words: Box<[u8]>,
    },
    /// Bounded failure result allowed by every contract; produces an `Ending::Failed`.
    /// (domain/tasks.md, sections 5.6 and 14).
    Failure {
        /** Failure-result reason, allowed by every contract and bounded by `Limits::result_bytes`. (domain/tasks.md, sections 5.6 and 14). */
        reason: Box<[u8]>,
    },
}

/// Durable final outcome, or a pending closing outcome; requester notification follows actual
/// settlement. (domain/tasks.md, sections 5.1, 5.6 and 14).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Ending {
    /// Checked successful result proposed for final ending; retained while closing and made
    /// historical only after settlement. (domain/tasks.md, sections 5.1, 5.6 and 14).
    Done(
        /** Successful checked result retained in pending closing or in the historical ended record. (domain/tasks.md, sections 5.1, 5.6 and 14). */
         TaskResult,
    ),
    /// Failure ending with its bounded reason, pending while closing or final in the historical
    /// record. (domain/tasks.md, sections 5.1, 5.6 and 14).
    Failed {
        /** Owned failure reason bounded by `Limits::result_bytes`. (domain/tasks.md, sections 5.1, 5.6 and 14). */
        reason: Box<[u8]>,
    },
    /// Cancellation of descendant work, optionally preserving an already-completed result; no
    /// arbitrary public cancel route is exposed. (domain/tasks.md, sections 5.1, 5.6 and 14).
    Cancelled {
        /** Owned cancellation reason bounded by `Limits::result_bytes`. (domain/tasks.md, sections 5.1, 5.6 and 14). */
        reason: Box<[u8]>,
        /** Optional already-completed result retained with cancellation, separately bounded and checked against the contract. (domain/tasks.md, sections 5.1, 5.6 and 14). */
        result: Option<TaskResult>,
    },
}

/// Content-free terminal classification used in observations and dependency readiness.
/// (domain/tasks.md, sections 5.1 and 14).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Status {
    /// Task completed successfully; dependents can become due after settlement. (domain/tasks.md,
    /// sections 5.1 and 14).
    Done,
    /// Task failed; waiting dependents are held. (domain/tasks.md, sections 5.1 and 14).
    Failed,
    /// Task was cancelled; waiting dependents are held. (domain/tasks.md, sections 5.1 and 14).
    Cancelled,
}

/// Root-to-tasks activation terminal, fenced by task/attempt; a terminal may park or retry without
/// ending the task. (domain/tasks.md, sections 5.2, 5.5, 5.6 and 14).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum End {
    /// Proposed final result; valid finishing with live delegates requires cancelling them or is
    /// refused without admission. (domain/tasks.md, sections 5.2, 5.5, 5.6 and 14).
    Finished {
        /** Owned proposed result; invalid bounded shapes consume an `Invalid` failure, while oversized priced terminals refuse before charging. (domain/tasks.md, sections 5.2, 5.5, 5.6 and 14). */
        result: TaskResult,
        /** Whether to cancel live descendant work; otherwise a valid finish with live delegates is refused and its attempt remains live. (domain/tasks.md, sections 5.2, 5.5, 5.6 and 14). */
        cancel_delegates: bool,
    },
    /// Successful activation park; clears retry/refusal counters and leaves the task idle, without
    /// implementing a wake route here. (domain/tasks.md, sections 5.2, 5.5, 5.6 and 14).
    Parked,
    /// Count one classified failure, then back off or hold beyond the configured retry allowance.
    /// (domain/tasks.md, sections 5.2, 5.5, 5.6 and 14).
    Failed(
        /** Reported failure category whose counter/backoff policy applies. (domain/tasks.md, sections 5.2, 5.5, 5.6 and 14). */
         Class,
    ),
    /// Pre-execution refusal pause using the transient delay policy, without spending a failure
    /// try. (domain/tasks.md, sections 5.2, 5.5, 5.6 and 14).
    Refused,
}

/// Root-reported or task-derived reason to preserve a task's prior lifecycle while stopping its
/// live activation. (domain/tasks.md, sections 5.5 and 14).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Hold {
    /// Configured retry allowance was exhausted. (domain/tasks.md, sections 5.5 and 14).
    Failures(/** Failure class whose retry allowance was exhausted. (domain/tasks.md, sections 5.5 and 14). */ Class),
    /// A required dependency failed or was cancelled. (domain/tasks.md, sections 5.5 and 14).
    Dependency(
        /** `Dependency` that ended failed or cancelled, causing this dependent to be held. (domain/tasks.md, sections 5.5 and 14). */
         u64,
    ),
    /// Root reports a person's stop or another authorized stop decision. (domain/tasks.md, sections
    /// 5.5 and 14).
    Stopped,
    /// Root reports resource drift requiring a decision; tasks owns no connector drift detection.
    /// (domain/tasks.md, sections 5.5 and 14).
    Drift,
    /// Root reports a permanently failed effect requiring a decision. (domain/tasks.md, sections
    /// 5.5 and 14).
    Effects,
    /// Actual spending exhausted available `funding`; a representable charge is still retained.
    /// (domain/tasks.md, sections 5.5 and 14).
    Budget,
    /// Root reports that the permitted deadline prevents another activation. (domain/tasks.md,
    /// sections 5.5 and 14).
    Deadline,
}

/// Current agent-task activation phase; task and attempt identity fence every external transition.
/// (domain/tasks.md, sections 5.2 and 14).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Active {
    /// `Activation` parked; no current wake input is exposed by this contracted child.
    /// (domain/tasks.md, sections 5.2 and 14).
    Idle,
    /// An activation is requested and root owns preparation. (domain/tasks.md, sections 5.2 and
    /// 14).
    Due,
    /// Root is gathering the actual brief/resources; a claim or preparation failure follows.
    /// (domain/tasks.md, sections 5.2 and 14).
    Preparing,
    /// Root committed the fenced attempt before assignment/start. (domain/tasks.md, sections 5.2
    /// and 14).
    Claimed {
        /** Committed root-issued strictly increasing nonzero attempt identity. (domain/tasks.md, sections 5.2 and 14). */
        attempt: u64,
    },
    /// Root reported the exact claimed attempt started. (domain/tasks.md, sections 5.2 and 14).
    Running {
        /** Current claimed attempt reported as started. (domain/tasks.md, sections 5.2 and 14). */
        attempt: u64,
    },
    /// `Retry`/refusal pause with one projected deadline. (domain/tasks.md, sections 5.2 and 14).
    BackingOff {
        /** Saved wall-time retry threshold, projected once to a monotonic deadline and reprojected on restore. (domain/tasks.md, sections 5.2 and 14). */
        until: Wall,
    },
}

/// Ordered closing obligation; the root owns actual effect/resource settlement and returns
/// `Settled` after `Close`. (domain/tasks.md, sections 5.1 and 14).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Stage {
    /// Wait for the exact live attempt's terminal. (domain/tasks.md, sections 5.1 and 14).
    Run {
        /** `Live` attempt whose terminal must be heard before closing advances. (domain/tasks.md, sections 5.1 and 14). */
        attempt: u64,
    },
    /// Wait for all live delegates and actual incoming financial allocations to settle.
    /// (domain/tasks.md, sections 5.1 and 14).
    Delegates,
    /// `Close` output is owed settlement from root; tasks owns no connector obligations.
    /// (domain/tasks.md, sections 5.1 and 14).
    Effects,
    /// Root closing obligations settled; final financial posting and historical end can commit.
    /// Unheld `Settled` is transient and refused on live restore; held prior closing may retain it.
    /// (domain/tasks.md, sections 5.1 and 14).
    Settled,
}

/// Owned pending ending and closing stage retained until every obligation and financial allocation
/// settles. (domain/tasks.md, sections 5.1 and 14).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Closing {
    /// Next unsettled closing obligation. (domain/tasks.md, sections 5.1 and 14).
    pub stage: Stage,
    /// Bounded pending final outcome, preserved through holds and cancellation. (domain/tasks.md,
    /// sections 5.1 and 14).
    pub ending: Ending,
}

/// Lifecycle preserved while held; terminal/settlement notifications can update it without lifting
/// the hold. (domain/tasks.md, sections 5.1 and 14).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Was {
    /// Prior dependency-waiting phase. (domain/tasks.md, sections 5.1 and 14).
    Waiting,
    /// Prior agent activation phase. (domain/tasks.md, sections 5.1 and 14).
    Active(
        /** Prior agent activation phase, including its attempt/backoff fence. (domain/tasks.md, sections 5.1 and 14). */
         Active,
    ),
    /// Prior pending closing state. (domain/tasks.md, sections 5.1 and 14).
    Closing(/** Prior closing stage and owned pending ending. (domain/tasks.md, sections 5.1 and 14). */ Closing),
}

/// Task's complete lifecycle value; ended values occur in historical durable rows rather than the
/// live arena. (domain/tasks.md, sections 5.1 and 14).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Phase {
    /// `Some` live dependency remains; readiness becomes `Due` when `waiting_on` empties.
    /// (domain/tasks.md, sections 5.1 and 14).
    Waiting,
    /// `Agent` activation is idle, due, preparing, claimed, running or backing off.
    /// (domain/tasks.md, sections 5.1 and 14).
    Active(/** Engaged agent activation phase. (domain/tasks.md, sections 5.1 and 14). */ Active),
    /// Pending ending waits for ordered activation/delegate/root settlement. (domain/tasks.md,
    /// sections 5.1 and 14).
    Closing(/** Owned pending ending and next closing obligation. (domain/tasks.md, sections 5.1 and 14). */ Closing),
    /// Prior lifecycle preserved for a decision; current contracted API exposes no general
    /// release/amend route. (domain/tasks.md, sections 5.1 and 14).
    Held {
        /** Prior phase retained and updated while held; the hold is not lifted by a terminal. (domain/tasks.md, sections 5.1 and 14). */
        was: Was,
        /** Reason for the hold. (domain/tasks.md, sections 5.1 and 14). */
        why: Hold,
    },
    /// Historical outcome outside the live arena; restoring it as live is refused.
    /// (domain/tasks.md, sections 5.1 and 14).
    Ended(
        /** Historical final ending; this phase is excluded from the live restore arena. (domain/tasks.md, sections 5.1 and 14). */
         Ending,
    ),
}

/// Root-authorized member of an atomic creation batch; tasks preflights the whole graph, payloads,
/// capacity and actual finite reservations before mutation. (domain/tasks.md, sections 3–4 and 14).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct New {
    /// Fresh never-reused task number supplied by the root; live or within-batch duplicates refuse.
    /// (domain/tasks.md, sections 3–4 and 14).
    pub number: u64,
    /// Policy project authenticated by root and checked against creator/project structural limits.
    /// (domain/tasks.md, sections 3–4 and 14).
    pub project: u32,
    /// `Agent` charter executor, checked against admitted configuration. (domain/tasks.md, sections
    /// 3–4 and 14).
    pub executor: Executor,
    /// Owned specification within configured counts/bytes, with empty historical inputs.
    /// (domain/tasks.md, sections 3–4 and 14).
    pub spec: Spec,
    /// Admitted bounded result contract. (domain/tasks.md, sections 3–4 and 14).
    pub contract: Contract,
    /// Complete root-approved authority carrier; tasks checks bounded shape, not policy inclusion.
    /// (domain/tasks.md, sections 3–4 and 14).
    pub authority: Authority,
    /// Fresh allotment: budget equals authority spend, with all spend/reservation counters zero.
    /// (domain/tasks.md, sections 3–4 and 14).
    pub numbers: Numbers,
    /// Actual finite source; task sources must be live ancestors of the task creator and external
    /// sources must belong to this project. (domain/tasks.md, sections 3–4 and 14).
    pub funder: Funder,
    /// Distinct immutable dependencies, at most `Limits::dependencies`: members of this batch or
    /// the creator's current live delegates. (domain/tasks.md, sections 3–4 and 14).
    pub dependencies: Box<[u64]>,
}

/// Owned durable task state emitted to root storage; root uses `RunContext` for preparation and
/// does not maintain a mutable copy of this ledger. (domain/tasks.md, sections 3, 5 and 14).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct TaskRecord {
    /// One bounded semantic held-chat decision and checked revision. Root owns
    /// authentication, routing and historical receipts (domain/tasks.md, 15).
    pub escalation: crate::Escalation,
    /// Stable never-reused deployment task identity. (domain/tasks.md, sections 3, 5 and 14).
    pub number: u64,
    /// Project whose policy applies to this task. (domain/tasks.md, sections 3, 5 and 14).
    pub project: u32,
    /// Original current requester topology, distinct from the actual financial funder.
    /// (domain/tasks.md, sections 3, 5 and 14).
    pub requester: Party,
    /// Committed cumulative expense of the current attempt; a `new` `Claim` resets only this
    /// attempt baseline, not total direct spend. (domain/tasks.md, sections 3, 5 and 14).
    pub run_spent: u64,
    /// Requester-tree root identity, equal to number for a top-level task. (domain/tasks.md,
    /// sections 3, 5 and 14).
    pub root: u64,
    /// Structural depth below that root, bounded by `Limits::depth`. (domain/tasks.md, sections 3,
    /// 5 and 14).
    pub depth: u32,
    /// Configured agent charter executor. (domain/tasks.md, sections 3, 5 and 14).
    pub executor: Executor,
    /// Owned bounded specification; live state has empty historical inputs. (domain/tasks.md,
    /// sections 3, 5 and 14).
    pub spec: Spec,
    /// Owned admitted result contract used at terminal validation. (domain/tasks.md, sections 3, 5
    /// and 14).
    pub contract: Contract,
    /// Owned bounded permission carrier checked by root policy, not by tasks. (domain/tasks.md,
    /// sections 3, 5 and 14).
    pub authority: Authority,
    /// Authentic current-allotment accounting, updated atomically with lifecycle admission.
    /// (domain/tasks.md, sections 3, 5 and 14).
    pub numbers: Numbers,
    /// Actual source of the current allotment, kept separate from requester ancestry.
    /// (domain/tasks.md, sections 3, 5 and 14).
    pub funder: Funder,
    /// Immutable admitted dependency identities, bounded by `Limits::dependencies`.
    /// (domain/tasks.md, sections 3, 5 and 14).
    pub dependencies: Box<[u64]>,
    /// `Exact` still-live subset of dependencies, bounded by `Limits::dependencies`; removed once
    /// per dependency end in the same decision. `Restore` checks count/uniqueness/subset before
    /// retention and complete live coverage after all pages. (domain/tasks.md, sections 3, 5 and
    /// 14).
    pub waiting_on: Box<[u64]>,
    /// Current live direct delegates, bounded by `Limits::delegates`; requester ending waits until
    /// they are gone. (domain/tasks.md, sections 3, 5 and 14).
    pub delegates: Box<[u64]>,
    /// Latest newly admitted contiguous turn in the current attempt; `new` `Claim` starts at zero.
    /// (domain/tasks.md, sections 3, 5 and 14).
    pub turn: u32,
    /// Lifetime tasks made in this subtree, including itself, bounded by `Limits::tree_tasks`;
    /// ending delegates does not return capacity. (domain/tasks.md, sections 3, 5 and 14).
    pub made: u32,
    /// Latest root-issued attempt identity; `new` claims must strictly increase it.
    /// (domain/tasks.md, sections 3, 5 and 14).
    pub attempt: u64,
    /// Most recently accepted activation-terminal attempt while the task remains live; not an exact
    /// transport replay receipt. (domain/tasks.md, sections 3, 5 and 14).
    pub last_answer: Option<u64>,
    /// Per-class failure history used with the configured retry policy. (domain/tasks.md, sections
    /// 3, 5 and 14).
    pub tries: Tries,
    /// Saturating pre-execution refusal counter used for pauses rather than failure tries.
    /// (domain/tasks.md, sections 3, 5 and 14).
    pub refusals: u32,
    /// Current lifecycle or the ended historical outcome. (domain/tasks.md, sections 3, 5 and 14).
    pub phase: Phase,
}

/// Typed logical store `key` for task state or financial history, with no byte encoding performed
/// in this child. (domain/tasks.md, sections 2 and 14).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Key {
    /// Logical mutable live-task row. (domain/tasks.md, sections 2 and 14).
    Live(/** `Live` task identity whose row is replaced or erased. (domain/tasks.md, sections 2 and 14). */ u64),
    /// Logical historical ended-task row. (domain/tasks.md, sections 2 and 14).
    Ended(
        /** Historical ended task identity retained in the store, outside the live arena. (domain/tasks.md, sections 2 and 14). */
         u64,
    ),
    /// Logical finite period/pool row. (domain/tasks.md, sections 2 and 14).
    Ledger(/** Actual period/pool source identity. (domain/tasks.md, sections 2 and 14). */ Funder),
}

/// Tasks-to-root persistence row or root-to-tasks live restore input; only `Live` and `Ledger`
/// belong to startup restoration. (domain/tasks.md, sections 2 and 14).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Stored {
    /// Current bounded live task state, emitted on mutation and admitted once at startup.
    /// (domain/tasks.md, sections 2 and 14).
    Live(
        /** Owned boxed bounded live task row; startup validates its shape and later validates graph/financial links. (domain/tasks.md, sections 2 and 14). */
         Box<TaskRecord>,
    ),
    /// Historical final task state; root loads it for actual authenticated result reads, never as
    /// live startup state. (domain/tasks.md, sections 2 and 14).
    Ended(
        /** Owned boxed historical ending retained in root storage; never a live restore input or a replayed result delivery. (domain/tasks.md, sections 2 and 14). */
         Box<TaskRecord>,
    ),
    /// Authentic finite external source accounting; restored links are checked with live
    /// reservations. (domain/tasks.md, sections 2 and 14).
    Ledger(
        /** Finite period/pool accounting owned by tasks and admitted at startup under `Limits::funders`. (domain/tasks.md, sections 2 and 14). */
         crate::FundingRecord,
    ),
}

impl Stored {
    /// Pure fixed-size projection of this row's logical `key`; no allocation, mutation, persistence
    /// or lifecycle output. (domain/tasks.md, sections 2 and 14).
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
/// (domain/tasks.md, sections 4–5 and 14).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Refusal {
    /// Startup is restoring or failed; no mutation is admitted. (domain/tasks.md, sections 4–5 and
    /// 14).
    NotReady,
    /// Named creator/task is not in the live arena. (domain/tasks.md, sections 4–5 and 14).
    Unknown,
    /// Finite source table has no room before source creation/carving. (domain/tasks.md, sections
    /// 4–5 and 14).
    Busy,
    /// Task/source identity is already retained or a task number repeats within the batch.
    /// (domain/tasks.md, sections 4–5 and 14).
    Duplicate,
    /// `Make` batch is empty. (domain/tasks.md, sections 4–5 and 14).
    Empty,
    /// `Batch` exceeds the configured direct-child count. (domain/tasks.md, sections 4–5 and 14).
    Batch,
    /// `Batch` cannot fit the live task slab/name set, including slots not yet reclaimed.
    /// (domain/tasks.md, sections 4–5 and 14).
    Live,
    /// Creator/project identity or per-project live task count is invalid. (domain/tasks.md,
    /// sections 4–5 and 14).
    Project,
    /// Lifetime requester-tree task count would exceed its configured bound. (domain/tasks.md,
    /// sections 4–5 and 14).
    Tree,
    /// A `new` child's structural depth would exceed the configured bound. (domain/tasks.md,
    /// sections 4–5 and 14).
    Depth,
    /// Creator's live delegate list cannot fit the batch. (domain/tasks.md, sections 4–5 and 14).
    Delegates,
    /// `Dependency` count, uniqueness or current-live/same-batch identity is invalid.
    /// (domain/tasks.md, sections 4–5 and 14).
    Dependencies,
    /// Combined live delegation waits and immutable dependencies would cycle. (domain/tasks.md,
    /// sections 4–5 and 14).
    Cycle,
    /// `Agent` charter is absent from the admitted configured table. (domain/tasks.md, sections 4–5
    /// and 14).
    Executor,
    /// Specification count or aggregate owned bytes are invalid. (domain/tasks.md, sections 4–5 and
    /// 14).
    Spec,
    /// `TaskResult`-contract shape or a priced terminal's result-byte admission is invalid.
    /// (domain/tasks.md, sections 4–5 and 14).
    Contract,
    /// Carried authority collections or aggregate bytes exceed the configured shape bounds.
    /// (domain/tasks.md, sections 4–5 and 14).
    AuthorityShape,
    /// Historical inputs are nonempty; no current root historical-input creation route is
    /// implemented. (domain/tasks.md, sections 4–5 and 14).
    Inputs,
    /// Current task phase does not permit the requested admission. (domain/tasks.md, sections 4–5
    /// and 14).
    State,
    /// `Attempt` is stale, zero or incompatible with the current phase. (domain/tasks.md, sections
    /// 4–5 and 14).
    Attempt,
    /// Valid finishing declines to cancel still-live delegates; activation and charge remain
    /// unadmitted. (domain/tasks.md, sections 4–5 and 14).
    LiveDelegates,
    /// `Live` startup row shape, graph or actual financial links are invalid; historical rows
    /// cannot be restored live. (domain/tasks.md, sections 4–5 and 14).
    Restore,
    /// `Turn` carries a read fence although this contracted API has no inbox/read route.
    /// (domain/tasks.md, sections 4–5 and 14).
    Read,
    /// `Turn` is not the next contiguous current-attempt turn, or cumulative expense decreases.
    /// (domain/tasks.md, sections 4–5 and 14).
    Turn,
    /// Authentic finite source/reservation or eventual actual-chain arithmetic cannot admit the
    /// whole decision. (domain/tasks.md, sections 4–5 and 14).
    Funding,
}

/// `Refusal` location and reason returned to the root; batch-wide failures may have no single task
/// identity. (domain/tasks.md, sections 4–5 and 14).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Problem {
    /// Offending task identity when one exists; `None` denotes a batch/source/global failure.
    /// (domain/tasks.md, sections 4–5 and 14).
    pub task: Option<u64>,
    /// Bounded structural, lifecycle, financial or restore admission reason. (domain/tasks.md,
    /// sections 4–5 and 14).
    pub why: Refusal,
}

/// Lifecycle acknowledgement classification, not an exact payload replay proof; exact transport
/// replay is the root's responsibility. (domain/tasks.md, sections 5.6 and 14). (domain/engine.md,
/// section 7.5).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Accepted {
    /// This event newly admitted the task transition or turn. (domain/tasks.md, sections 5.6 and
    /// 14). (domain/engine.md, section 7.5).
    New,
    /// `Unpriced` terminal attempt equals the live task's recorded `last_answer`; root remains
    /// responsible for exact payload proof. (domain/tasks.md, sections 5.6 and 14).
    /// (domain/engine.md, section 7.5).
    Already,
}

/// Root-issued semantic task/finance inputs after authorization and transport fencing;
/// notifications have no reply destination and may be ignored if stale. (domain/tasks.md, sections
/// 4–5 and 14). (domain/engine.md, section 7.5).
#[derive(PartialEq, Eq, Debug)]
pub enum Event {
    /// Root `SetRoles` preflight: inspect only current person-requested `Waiting`
    /// contexts without mutation; one typed terminal even before readiness (domain/tasks.md, 16; domain/engine.md, 7.8).
    InspectEscalations {
        /// Stage-local root right consumed once by `EscalationsInspected`
        /// (domain/tasks.md, 16).
        reply_to: ReplyTo,
        /// Root-validated project whose current `Waiting` tasks are inspected (domain/tasks.md, 16).
        project: u32,
    },
    /// Root after successful `ApplyRoles` in the same synchronous decision; emit
    /// existing `EscalationNeeded` for `Waiting` only and one completion (domain/tasks.md, 16).
    RecheckEscalations {
        /// Stage-local root right consumed by `EscalationsRechecked`
        /// (domain/tasks.md, 16).
        reply_to: ReplyTo,
        /// Same preflight project; rejected/currently unheld tasks stay inert
        /// (domain/tasks.md, 16).
        project: u32,
    },
    /// Root queries one bounded held view for actual named reads/decisions;
    /// returns one `EscalationInspected`, including absent (domain/tasks.md, 15).
    InspectEscalation {
        /// Root-owned synchronous correlation, echoed once (domain/tasks.md, 15).
        reply_to: ReplyTo,
        /// Named task; this query mutates nothing (domain/tasks.md, 15).
        task: u64,
    },
    /// Root resolves the just-created routing obligation before commitment;
    /// stale notification changes nothing (domain/tasks.md, 15).
    RoutedEscalation {
        /// Held person chat (domain/tasks.md, 15).
        task: u64,
        /// Exact positive semantic revision (domain/tasks.md, 15).
        revision: u64,
        /// Root-verified eligible requester or final policy role (domain/tasks.md, 15).
        holder: crate::EscalationHolder,
    },
    /// Root has authenticated the current recipient and checked authority;
    /// one `EscalationDecided` terminal follows (domain/tasks.md, 15).
    DecideEscalation {
        /// Root-owned correlation owed one semantic terminal (domain/tasks.md, 15).
        reply_to: ReplyTo,
        /// Exact held task (domain/tasks.md, 15).
        task: u64,
        /// Waiting revision, never a root transport receipt (domain/tasks.md, 15).
        revision: u64,
        /// Positive authenticated deciding person (domain/tasks.md, 15).
        by: u64,
        /// Bounded authorized release/reject/pass choice (domain/tasks.md, 15).
        decision: crate::EscalationDecision,
    },
    /// `Open` one finite monotonic project period after root authorization; emits ledger `Save`
    /// plus `Done`, or one `Refused` with no mutation. (domain/tasks.md, sections 4–5 and 14).
    /// (domain/engine.md, section 7.5).
    OpenPeriod {
        /// Root-issued destination owed one `Done` or `Refused` terminal. (domain/tasks.md,
        /// sections 4–5 and 14). (domain/engine.md, section 7.5).
        reply_to: ReplyTo,
        /// Project authenticated/authorized by the root for this finite source. (domain/tasks.md,
        /// sections 4–5 and 14). (domain/engine.md, section 7.5).
        project: u32,
        /// Fresh period identity, strictly greater than retained periods for this project.
        /// (domain/tasks.md, sections 4–5 and 14). (domain/engine.md, section 7.5).
        period: u64,
        /// Finite root-authorized deployment-unit budget; source capacity is checked before
        /// mutation. (domain/tasks.md, sections 4–5 and 14). (domain/engine.md, section 7.5).
        budget: u64,
    },
    /// Reserve one finite person's pool from its original period after root authorization; emits
    /// both ledger Saves plus `Done`, or `Refused` atomically. (domain/tasks.md, sections 4–5 and
    /// 14). (domain/engine.md, section 7.5).
    CarvePool {
        /// Root-issued destination owed one `Done` or `Refused` terminal. (domain/tasks.md,
        /// sections 4–5 and 14). (domain/engine.md, section 7.5).
        reply_to: ReplyTo,
        /// Project authorized by the root for this person's finite pool. (domain/tasks.md, sections
        /// 4–5 and 14). (domain/engine.md, section 7.5).
        project: u32,
        /// Person whose current role and allotment root authenticated and checked.
        /// (domain/tasks.md, sections 4–5 and 14). (domain/engine.md, section 7.5).
        person: u64,
        /// Existing original project period whose available budget is reserved for this pool.
        /// (domain/tasks.md, sections 4–5 and 14). (domain/engine.md, section 7.5).
        period: u64,
        /// Complete finite pool budget, atomically reserved from the original period.
        /// (domain/tasks.md, sections 4–5 and 14). (domain/engine.md, section 7.5).
        budget: u64,
    },
    /// Admit only the next contiguous turn and its checked expense delta atomically; nonempty read
    /// fences, stale turns/attempts and arithmetic failures refuse without debit. (domain/tasks.md,
    /// sections 4–5 and 14). (domain/engine.md, section 7.5).
    Turn {
        /// Root-issued destination owed one `TurnAcknowledged` or `Refused` terminal.
        /// (domain/tasks.md, sections 4–5 and 14). (domain/engine.md, section 7.5).
        reply_to: ReplyTo,
        /// `Live` task whose claimed attempt and next turn are being admitted. (domain/tasks.md,
        /// sections 4–5 and 14). (domain/engine.md, section 7.5).
        task: u64,
        /// Nonzero current attempt fence; stale attempts cannot charge or advance turns.
        /// (domain/tasks.md, sections 4–5 and 14). (domain/engine.md, section 7.5).
        attempt: u64,
        /// Exactly the next contiguous nonzero turn in this attempt; root handles exact replay
        /// before this child input. (domain/tasks.md, sections 4–5 and 14). (domain/engine.md,
        /// section 7.5).
        turn: u32,
        /// Must be `None` in this contracted boundary; no inbox/read-fence route is implemented
        /// here. (domain/tasks.md, sections 4–5 and 14). (domain/engine.md, section 7.5).
        read: Option<u64>,
        /// Whole priced attempt expense; tasks posts only the checked delta above `run_spent`,
        /// including representable overruns. (domain/tasks.md, sections 4–5 and 14).
        /// (domain/engine.md, section 7.5).
        cumulative: u64,
    },
    /// Create an authorized whole batch and actual reservations atomically, producing `Made` or
    /// `Refused` plus bounded persistence/lifecycle outputs. (domain/tasks.md, sections 4–5 and
    /// 14). (domain/engine.md, section 7.5).
    Make {
        /// Root-issued destination owed one `Made` or `Refused` terminal for the whole batch.
        /// (domain/tasks.md, sections 4–5 and 14). (domain/engine.md, section 7.5).
        reply_to: ReplyTo,
        /// Root-verified creator/requester of every `new` task; task creator and actual `funding`
        /// ancestry are checked separately. (domain/tasks.md, sections 4–5 and 14).
        /// (domain/engine.md, section 7.5).
        creator: Party,
        /// Owned nonempty batch bounded by `Limits::batch`; all graph, payload and reservation
        /// checks precede mutation. (domain/tasks.md, sections 4–5 and 14). (domain/engine.md,
        /// section 7.5).
        batch: Box<[New]>,
    },
    /// `Due`-to-`Preparing` admission, producing `Done` or `Refused`. (domain/tasks.md, sections
    /// 4–5 and 14). (domain/engine.md, section 7.5).
    Prepare {
        /// Destination owed one `Done` or `Refused` terminal. (domain/tasks.md, sections 4–5 and
        /// 14). (domain/engine.md, section 7.5).
        reply_to: ReplyTo,
        /// `Live` `Due` task to enter `Preparing`; other phases refuse without mutation.
        /// (domain/tasks.md, sections 4–5 and 14). (domain/engine.md, section 7.5).
        task: u64,
    },
    /// `Preparing`-to-`Claimed` admission with a strictly increasing root attempt, producing `Done`
    /// or `Refused`. (domain/tasks.md, sections 4–5 and 14). (domain/engine.md, section 7.5).
    Claim {
        /// Destination owed one `Done` or `Refused` terminal. (domain/tasks.md, sections 4–5 and
        /// 14). (domain/engine.md, section 7.5).
        reply_to: ReplyTo,
        /// `Live` `Preparing` task whose attempt is being claimed. (domain/tasks.md, sections 4–5
        /// and 14). (domain/engine.md, section 7.5).
        task: u64,
        /// Root-issued nonzero identity strictly greater than the latest claim; success resets
        /// attempt turn/expense baselines. (domain/tasks.md, sections 4–5 and 14).
        /// (domain/engine.md, section 7.5).
        attempt: u64,
    },
    /// Notification changing only the matching `Claimed` attempt to `Running`; absent/stale inputs
    /// are ignored and no reply is owed. (domain/tasks.md, sections 4–5 and 14). (domain/engine.md,
    /// section 7.5).
    Started {
        /// `Live` claimed task the root reports as started. (domain/tasks.md, sections 4–5 and 14).
        /// (domain/engine.md, section 7.5).
        task: u64,
        /// Matching claimed attempt; absent tasks and stale phases/attempts are ignored.
        /// (domain/tasks.md, sections 4–5 and 14). (domain/engine.md, section 7.5).
        attempt: u64,
    },
    /// Admit a root-fenced activation terminal with explicit expense provenance; emits one
    /// `Acknowledged` or `Refused` and atomic resulting saves. (domain/tasks.md, sections 4–5 and
    /// 14). (domain/engine.md, section 7.5).
    Activation {
        /// Destination owed one `Acknowledged` or `Refused` terminal; root delays exposure until
        /// the decision is durable. (domain/tasks.md, sections 4–5 and 14). (domain/engine.md,
        /// section 7.5).
        reply_to: ReplyTo,
        /// `Live` task whose activation terminal is admitted. (domain/tasks.md, sections 4–5 and
        /// 14). (domain/engine.md, section 7.5).
        task: u64,
        /// Root-fenced current attempt, including a stopped run preserved while held or closing.
        /// (domain/tasks.md, sections 4–5 and 14). (domain/engine.md, section 7.5).
        attempt: u64,
        /// Bounded root-supplied activation outcome; tasks normalizes invalid outcomes. A park/retry need not end the
        /// task. (domain/tasks.md, sections 4–5 and 14). (domain/engine.md, section 7.5).
        end: End,
        /// `Priced` worker cumulative expense or unpriced topology/readiness cause; root owns exact
        /// replay fencing. (domain/tasks.md, sections 4–5 and 14). (domain/engine.md, section 7.5).
        cause: Cause,
    },
    /// Notification pausing only a `Preparing` task; does not consume a failure try and has no
    /// reply. (domain/tasks.md, sections 4–5 and 14). (domain/engine.md, section 7.5).
    PreparationFailed {
        /// `Preparing` task to pause without consuming an activation failure try; absent or other-
        /// phase tasks are ignored. (domain/tasks.md, sections 4–5 and 14). (domain/engine.md,
        /// section 7.5).
        task: u64,
    },
    /// Authorized notification preserving prior state and issuing `Stop` for any live run;
    /// absent/already-held inputs have no output. (domain/tasks.md, sections 4–5 and 14).
    /// (domain/engine.md, section 7.5).
    Hold {
        /// `Live` task to hold while retaining its prior phase; absent/already-held tasks are
        /// ignored. (domain/tasks.md, sections 4–5 and 14). (domain/engine.md, section 7.5).
        task: u64,
        /// Authorized or root-classified hold reason; tasks judges no policy here.
        /// (domain/tasks.md, sections 4–5 and 14). (domain/engine.md, section 7.5).
        why: Hold,
    },
    /// Root notification after `Close` obligations finish; matching `Effects` stages advance,
    /// including held prior closing, without lifting a hold. (domain/tasks.md, sections 4–5 and
    /// 14). (domain/engine.md, section 7.5).
    Settled {
        /// Task whose root-owned `Close` obligations settled; only `Effects` stages advance,
        /// including preserved closing while held. (domain/tasks.md, sections 4–5 and 14).
        /// (domain/engine.md, section 7.5).
        task: u64,
    },
    /// Admit one `Live`/`Ledger` row during restoring; invalid input permanently fails restoration
    /// with `RestoreRefused`. (domain/tasks.md, sections 4–5 and 14). (domain/engine.md, section
    /// 7.5).
    Restore {
        /// One bounded `Live` or `Ledger` row while restoring; historical `Ended` and
        /// invalid/duplicate rows fail restoration. (domain/tasks.md, sections 4–5 and 14).
        /// (domain/engine.md, section 7.5).
        record: Stored,
    },
    /// Validate complete requester/dependency/financial links and combined acyclicity, then emit
    /// bounded activations/adoptions/closing outputs. Repetition after ready/failed is ignored.
    /// (domain/tasks.md, sections 4–5 and 14). (domain/engine.md, section 7.5).
    Restored,
}

/// Tasks-to-root typed persistence, lifecycle and terminal reply outputs; root groups resulting
/// saves/erases with effects and delays outward replies until durability. (domain/tasks.md,
/// sections 5 and 14). (domain/engine.md, section 7.5).
#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    /// Terminal read-only `SetRoles` preflight; snapshot is owned transient root
    /// context, not retained state (domain/tasks.md, 16; domain/engine.md, 7.8).
    EscalationsInspected {
        /// Echoed stage-local right (domain/tasks.md, 16).
        reply_to: ReplyTo,
        /// At most `Limits::tasks` `Waiting` contexts without owned rejection
        /// reasons; only `NotReady` can refuse (domain/tasks.md, 16).
        result: Result<Box<[crate::EscalationContext]>, Refusal>,
    },
    /// Terminal for `RecheckEscalations` after its existing `EscalationNeeded` outputs; root
    /// completes the keyed role flight after routing them (domain/tasks.md, 16).
    EscalationsRechecked {
        /// Echoed stage-local right, consumed once (domain/tasks.md, 16).
        reply_to: ReplyTo,
        /// Success or `NotReady` before any recheck output
        /// (domain/tasks.md, 16).
        result: Result<(), Refusal>,
    },
    /// Tasks asks root to resolve its new held person chat in this atomic
    /// decision; no outward notice precedes commitment (domain/tasks.md, 15).
    EscalationNeeded {
        /// Temporary bounded held context; root drops it after routing
        /// (domain/tasks.md, 15).
        context: Box<crate::EscalationContext>,
    },
    /// Terminal for one `InspectEscalation`; query owns no durable task copy
    /// (domain/tasks.md, 15).
    EscalationInspected {
        /// Echoed root correlation, consumed once (domain/tasks.md, 15).
        reply_to: ReplyTo,
        /// Owned bounded view, or absent/non-person/non-held task
        /// (domain/tasks.md, 15).
        context: Option<Box<crate::EscalationContext>>,
    },
    /// One semantic decision terminal; root commits accepted child writes,
    /// its typed history and people's keyed answer together (domain/tasks.md, 15).
    EscalationDecided {
        /// Echoed root correlation, consumed once (domain/tasks.md, 15).
        reply_to: ReplyTo,
        /// Decision task (domain/tasks.md, 15).
        task: u64,
        /// Exact requested semantic revision (domain/tasks.md, 15).
        revision: u64,
        /// Accepted semantic transition or unchanged refusal (domain/tasks.md, 15).
        outcome: crate::EscalationOutcome,
    },
    /// One successful `Make` terminal reporting the complete batch. (domain/tasks.md, sections 5
    /// and 14). (domain/engine.md, section 7.5).
    Made {
        /** Original `Make` destination for its one successful terminal reply. (domain/tasks.md, sections 5 and 14). (domain/engine.md, section 7.5). */
        reply_to: ReplyTo,
        /** All `new` task numbers in input order, bounded by `Limits::batch`; no partial creation is reported. (domain/tasks.md, sections 5 and 14). (domain/engine.md, section 7.5). */
        tasks: Box<[u64]>,
    },
    /// One reply-bearing entrance/lifecycle/financial refusal; no requested mutation was admitted.
    /// (domain/tasks.md, sections 5 and 14). (domain/engine.md, section 7.5).
    Refused {
        /** Original reply-bearing event destination, consumed once on refusal. (domain/tasks.md, sections 5 and 14). (domain/engine.md, section 7.5). */
        reply_to: ReplyTo,
        /** Admission location/reason; requested mutation was not made. (domain/tasks.md, sections 5 and 14). (domain/engine.md, section 7.5). */
        problem: Problem,
    },
    /// One successful `funding`, preparation or claim terminal. (domain/tasks.md, sections 5 and
    /// 14). (domain/engine.md, section 7.5).
    Done {
        /** Original successful `funding`/preparation/claim destination, consumed once. (domain/tasks.md, sections 5 and 14). (domain/engine.md, section 7.5). */
        reply_to: ReplyTo,
    },
    /// One activation-terminal reply; durability and exact transport replay proof belong to root.
    /// (domain/tasks.md, sections 5 and 14). (domain/engine.md, section 7.5).
    Acknowledged {
        /** Original activation-terminal destination; root durability barrier precedes outward acknowledgement. (domain/tasks.md, sections 5 and 14). (domain/engine.md, section 7.5). */
        reply_to: ReplyTo,
        /** Task whose lifecycle terminal was admitted or already recorded. (domain/tasks.md, sections 5 and 14). (domain/engine.md, section 7.5). */
        task: u64,
        /** Fenced activation attempt. (domain/tasks.md, sections 5 and 14). (domain/engine.md, section 7.5). */
        attempt: u64,
        /** New lifecycle admission or existing last-answer identity; exact payload replay is separately verified by root. (domain/tasks.md, sections 5 and 14). (domain/engine.md, section 7.5). */
        accepted: Accepted,
    },
    /// One newly admitted turn reply; root saves its transcript with the charged task state before
    /// outward acknowledgement. (domain/tasks.md, sections 5 and 14). (domain/engine.md, section
    /// 7.5).
    TurnAcknowledged {
        /** Original `new`-turn destination; root commits transcript and charged task row together before acknowledging. (domain/tasks.md, sections 5 and 14). (domain/engine.md, section 7.5). */
        reply_to: ReplyTo,
        /** Task whose turn and expense delta were admitted. (domain/tasks.md, sections 5 and 14). (domain/engine.md, section 7.5). */
        task: u64,
        /** Current claimed attempt identity. (domain/tasks.md, sections 5 and 14). (domain/engine.md, section 7.5). */
        attempt: u64,
        /** New contiguous turn committed in task state. (domain/tasks.md, sections 5 and 14). (domain/engine.md, section 7.5). */
        turn: u32,
        /** Current charged-turn route emits `Accepted::New`; root owns replay acknowledgement without re-entering this route. (domain/tasks.md, sections 5 and 14). (domain/engine.md, section 7.5). */
        accepted: Accepted,
    },
    /// Actual bounded semantic preparation request to root, not a rendered brief or a raw mutable
    /// task peek. (domain/tasks.md, sections 5 and 14). (domain/engine.md, section 7.5).
    Activate {
        /** Owned bounded semantic preparation snapshot; root selects the actual route and drops the temporary snapshot on claim/failure. (domain/tasks.md, sections 5 and 14). (domain/engine.md, section 7.5). */
        context: Box<RunContext>,
    },
    /// Ask root/fleet to stop one exact live attempt; its lifecycle terminal is still owed.
    /// (domain/tasks.md, sections 5 and 14). (domain/engine.md, section 7.5).
    Stop {
        /** Task whose live activation must be stopped by root/fleet. (domain/tasks.md, sections 5 and 14). (domain/engine.md, section 7.5). */
        task: u64,
        /** `Exact` live attempt to stop; its terminal remains owed before closing can advance. (domain/tasks.md, sections 5 and 14). (domain/engine.md, section 7.5). */
        attempt: u64,
    },
    /// Ask root/fleet to reconcile a restored actual claim and highest committed turn.
    /// (domain/tasks.md, sections 5 and 14). (domain/engine.md, section 7.5).
    Adopt {
        /** `Restored` live task whose actual claim the root must reconcile with fleet. (domain/tasks.md, sections 5 and 14). (domain/engine.md, section 7.5). */
        task: u64,
        /** `Restored` committed attempt identity to adopt; no `new` attempt is minted here. (domain/tasks.md, sections 5 and 14). (domain/engine.md, section 7.5). */
        attempt: u64,
        /** Highest committed turn in the restored attempt, supplied for fleet reconciliation. (domain/tasks.md, sections 5 and 14). (domain/engine.md, section 7.5). */
        kept: u32,
    },
    /// Ask root to complete its actual closing obligations, then return `Settled`; emitted only
    /// after delegates and funded allocations settle. (domain/tasks.md, sections 5 and 14).
    /// (domain/engine.md, section 7.5).
    Close {
        /** Task whose delegates/financial allocations have settled and root closing obligations must finish. (domain/tasks.md, sections 5 and 14). (domain/engine.md, section 7.5). */
        task: u64,
        /** Bounded pending final ending; root returns `Settled` only after its actual closing obligations finish. (domain/tasks.md, sections 5 and 14). (domain/engine.md, section 7.5). */
        ending: Ending,
    },
    /// One newly ended requester-identified notification emitted with the ended record and exact
    /// financial posting; current root exposes person result notices, with no task-requester inbox
    /// route or child delivery credit. (domain/tasks.md, sections 5 and 14). (domain/engine.md,
    /// section 7.5).
    Ended {
        /** Task removed from the live arena and saved as a historical ended row. (domain/tasks.md, sections 5 and 14). (domain/engine.md, section 7.5). */
        task: u64,
        /** Actual requester identifying this notification; current root consumes person result notices, and tasks retains no delivery credit or inbox. (domain/tasks.md, sections 5 and 14). (domain/engine.md, section 7.5). */
        requester: Party,
        /** Bounded final result/reason; emitted with ended/actual-funder writes in one root decision. (domain/tasks.md, section 5.6). (domain/engine.md, section 7.2). */
        ending: Ending,
    },
    /// Typed owned persistence output for the parent's current atomic decision. (domain/tasks.md,
    /// sections 5 and 14). (domain/engine.md, section 7.5).
    Save {
        /** Owned typed row joining the root's atomic decision; not an `IO` submission or a durable acknowledgement by itself. (domain/tasks.md, sections 5 and 14). (domain/engine.md, section 7.5). */
        record: Stored,
    },
    /// Typed logical-row removal for the parent's current atomic decision. (domain/tasks.md,
    /// sections 5 and 14). (domain/engine.md, section 7.5).
    Erase {
        /** Typed row removal joining the same atomic decision as related saves and outputs. (domain/tasks.md, sections 5 and 14). (domain/engine.md, section 7.5). */
        key: Key,
    },
    /// Terminal startup failure; this instance cannot become ready through more
    /// `Restore`/`Restored` inputs. (domain/tasks.md, sections 5 and 14). (domain/engine.md,
    /// section 7.5).
    RestoreRefused {
        /** Terminal startup validation failure; this domain instance remains unready. (domain/tasks.md, sections 5 and 14). (domain/engine.md, section 7.5). */
        problem: Problem,
    },
}

/// Root's expense provenance for a lifecycle terminal; tasks owns authentic delta/accounting
/// admission while root owns exact transport replay proofs. (domain/tasks.md, section 14).
/// (domain/engine.md, section 7.5).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Cause {
    /// Worker terminal with authentic cumulative expense: preflight lifecycle and eventual
    /// financial arithmetic before admitting the `new` delta together. (domain/tasks.md, section
    /// 14). (domain/engine.md, section 7.5).
    Priced {
        /// Authentic whole attempt expense, monotonic relative to `run_spent` and preflighted
        /// against eventual actual-chain representability. (domain/tasks.md, section 14).
        /// (domain/engine.md, section 7.5).
        cumulative: u64,
    },
    /// Loss, fleet refusal or invalid-answer normalization supplied by root; performs lifecycle
    /// admission without charging and still owes one terminal reply. (domain/tasks.md, section 14).
    /// (domain/engine.md, section 7.5).
    Unpriced,
}

/// Temporary owned semantic preparation snapshot emitted to root, bounded by task limits; not a
/// second mutable `TaskRecord` or `funding` ledger. (domain/tasks.md, sections 3 and 14).
/// (domain/engine.md, sections 7.1 and 9).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RunContext {
    /// Task identity for one currently requested activation. (domain/tasks.md, sections 3 and 14).
    /// (domain/engine.md, sections 7.1 and 9).
    pub task: u64,
    /// Policy project root checks for this preparation. (domain/tasks.md, sections 3 and 14).
    /// (domain/engine.md, sections 7.1 and 9).
    pub project: u32,
    /// Implemented task executor, currently an agent charter selected by root. (domain/tasks.md,
    /// sections 3 and 14). (domain/engine.md, sections 7.1 and 9).
    pub executor: Executor,
    /// Bounded owned semantic description; not rendered brief bytes. (domain/tasks.md, sections 3
    /// and 14). (domain/engine.md, sections 7.1 and 9).
    pub spec: Spec,
    /// Bounded result contract root includes in the brief. (domain/tasks.md, sections 3 and 14).
    /// (domain/engine.md, sections 7.1 and 9).
    pub contract: Contract,
    /// Actual requester included in preparation semantics. (domain/tasks.md, sections 3 and 14).
    /// (domain/engine.md, sections 7.1 and 9).
    pub requester: Party,
    /// `Exact` carried permission value root translates to authority's independent vocabulary.
    /// (domain/tasks.md, sections 3 and 14). (domain/engine.md, sections 7.1 and 9).
    pub authority: Authority,
    /// Borrowed-then-copied authentic financial snapshot for this preparation; root does not mutate
    /// or persist it as a shadow ledger. (domain/tasks.md, sections 3 and 14). (domain/engine.md,
    /// sections 7.1 and 9).
    pub numbers: Numbers,
}
