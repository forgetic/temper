//! The root's typed durable store vocabulary (domain/engine.md, 5.3–5.6).
//! It carries deployment counters, accepted transcript turns and owned child
//! records/keys. The root gathers writes into atomic decisions; the store
//! protocol encodes versions and supplies ordered, bounded pages.
//!
//! These values keep no IO state and know no store format, transport or secret
//! bytes. [`Range::contains`], [`Record::key`], [`Write::key`] and [`record_bytes`]
//! are pure value helpers: they neither issue IO nor create terminal events.
//! Range membership and deep-byte accounting are only parts of admission;
//! [`crate::loads`] also checks whole-page row bounds, order and continuation.
use alloc::boxed::Box;
pub use jig_core::{
    CallKey, Deployment, EscalationDecisionRecord, Family, ProposalDecisionRecord, RunProof, TerminalRecord, TurnProof,
    TurnRecord,
};

/// Exact typed answer kept for replay across a lost channel or root restart.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum CallAnswer {
    /// A forge write joined the named call's decision; later attempts can reask for its settled result.
    ForgeEffect { entry: u64, outcome: Option<temper_engine_domain_forge_client::Outcome> },
    /// A forge write was refused before an outbox entry existed.
    ForgeEffectRefused(temper_engine_domain_forge_client::api::Error),
    /// The pure policy check declined a forge write, retaining its bounded reasons.
    ForgeEffectDenied { answer: jig_core_authority::Answer, findings: Box<[jig_core_authority::Finding]> },
    /// One fresh bounded forge read, retained for named-call replay.
    ForgeRead(
        Box<Result<temper_engine_domain_forge_client::api::Answer, temper_engine_domain_forge_client::api::Error>>,
    ),
    /// One held descendant decision reached its semantic terminal.
    EscalationDecided { task: u64, revision: u64, outcome: jig_core_tasks::EscalationOutcome },
    /// A task holder's held-decision call was refused before mutation.
    EscalationRefused(jig_core_tasks::Problem),
    /// Proposal entered its proposer's durable pending state.
    Proposed { proposal: u64 },
    /// Proposal was accepted, rejected, passed, or withdrawn.
    ProposalDecided { proposal: u64, outcome: jig_core_tasks::ProposalOutcome },
    /// Proposal action or current standing failed before mutation.
    ProposalRefused(jig_core_tasks::Problem),
    /// A named task control change entered the same durable decision.
    Controlled,
    /// A named task control change was refused without mutation.
    ControlRefused(jig_core_tasks::Problem),
    /// A requested widening needs a holder or violates a hard ceiling.
    ControlDenied { answer: jig_core_authority::Answer },
    /// Message entered its target's inbox at this commit position.
    Sent { message: u64 },
    /// Reciprocal task references were installed.
    Introduced,
    /// A message or introduction failed its live reference or inbox check.
    MessageRefused(jig_core_tasks::Problem),
    /// One standing interest was installed.
    Subscribed { subscription: u64 },
    /// One standing interest was removed.
    Unsubscribed,
    /// A standing interest failed admission.
    SubscriptionRefused(jig_core_tasks::Problem),
    /// All members of an authorized batch, in call order.
    Delegated(Box<[u64]>),
    /// A whole batch declined by authority, with its independent findings.
    DelegationDenied { answer: jig_core_authority::Answer, findings: Box<[jig_core_authority::Finding]> },
    /// A whole batch refused by structural or finite-funding admission.
    DelegationRefused(jig_core_tasks::Problem),
    /// The named tool is deferred to a later engine route.
    Unavailable,
}

impl CallAnswer {
    /// Retain the core's part of a named answer. The forge alone retains
    /// connector payloads; authority findings belong to the core.
    #[must_use]
    pub fn core_part(&self, connector: u16) -> jig_core::CallPart {
        match self {
            CallAnswer::ForgeEffect { .. } | CallAnswer::ForgeEffectRefused(_) | CallAnswer::ForgeRead(_) => {
                jig_core::CallPart::Connector { connector }
            }
            CallAnswer::ForgeEffectDenied { answer, findings } => {
                jig_core::CallPart::EffectDenied { answer: *answer, findings: findings.clone() }
            }
            CallAnswer::EscalationDecided { task, revision, outcome } => {
                jig_core::CallPart::EscalationDecided { task: *task, revision: *revision, outcome: *outcome }
            }
            CallAnswer::EscalationRefused(problem) => jig_core::CallPart::EscalationRefused(problem.clone()),
            CallAnswer::Proposed { proposal } => jig_core::CallPart::Proposed { proposal: *proposal },
            CallAnswer::ProposalDecided { proposal, outcome } => {
                jig_core::CallPart::ProposalDecided { proposal: *proposal, outcome: *outcome }
            }
            CallAnswer::ProposalRefused(problem) => jig_core::CallPart::ProposalRefused(problem.clone()),
            CallAnswer::Controlled => jig_core::CallPart::Controlled,
            CallAnswer::ControlRefused(problem) => jig_core::CallPart::ControlRefused(problem.clone()),
            CallAnswer::ControlDenied { answer } => jig_core::CallPart::ControlDenied { answer: *answer },
            CallAnswer::Sent { message } => jig_core::CallPart::Sent { message: *message },
            CallAnswer::Introduced => jig_core::CallPart::Introduced,
            CallAnswer::MessageRefused(problem) => jig_core::CallPart::MessageRefused(problem.clone()),
            CallAnswer::Subscribed { subscription } => jig_core::CallPart::Subscribed { subscription: *subscription },
            CallAnswer::Unsubscribed => jig_core::CallPart::Unsubscribed,
            CallAnswer::SubscriptionRefused(problem) => jig_core::CallPart::SubscriptionRefused(problem.clone()),
            CallAnswer::Delegated(tasks) => jig_core::CallPart::Delegated(tasks.clone()),
            CallAnswer::DelegationDenied { answer, findings } => {
                jig_core::CallPart::DelegationDenied { answer: *answer, findings: findings.clone() }
            }
            CallAnswer::DelegationRefused(problem) => jig_core::CallPart::DelegationRefused(problem.clone()),
            CallAnswer::Unavailable => jig_core::CallPart::Unavailable,
        }
    }

    /// Assemble the core's answer when it owns the complete value. A
    /// connector marker is assembled from that connector's own table.
    #[must_use]
    pub fn from_core(part: &jig_core::CallPart) -> Option<CallAnswer> {
        Some(match part {
            jig_core::CallPart::Connector { .. } => return None,
            jig_core::CallPart::EffectDenied { answer, findings } => {
                CallAnswer::ForgeEffectDenied { answer: *answer, findings: findings.clone() }
            }
            jig_core::CallPart::EscalationDecided { task, revision, outcome } => {
                CallAnswer::EscalationDecided { task: *task, revision: *revision, outcome: *outcome }
            }
            jig_core::CallPart::EscalationRefused(problem) => CallAnswer::EscalationRefused(problem.clone()),
            jig_core::CallPart::Proposed { proposal } => CallAnswer::Proposed { proposal: *proposal },
            jig_core::CallPart::ProposalDecided { proposal, outcome } => {
                CallAnswer::ProposalDecided { proposal: *proposal, outcome: *outcome }
            }
            jig_core::CallPart::ProposalRefused(problem) => CallAnswer::ProposalRefused(problem.clone()),
            jig_core::CallPart::Controlled => CallAnswer::Controlled,
            jig_core::CallPart::ControlRefused(problem) => CallAnswer::ControlRefused(problem.clone()),
            jig_core::CallPart::ControlDenied { answer } => CallAnswer::ControlDenied { answer: *answer },
            jig_core::CallPart::Sent { message } => CallAnswer::Sent { message: *message },
            jig_core::CallPart::Introduced => CallAnswer::Introduced,
            jig_core::CallPart::MessageRefused(problem) => CallAnswer::MessageRefused(problem.clone()),
            jig_core::CallPart::Subscribed { subscription } => CallAnswer::Subscribed { subscription: *subscription },
            jig_core::CallPart::Unsubscribed => CallAnswer::Unsubscribed,
            jig_core::CallPart::SubscriptionRefused(problem) => CallAnswer::SubscriptionRefused(problem.clone()),
            jig_core::CallPart::Delegated(tasks) => CallAnswer::Delegated(tasks.clone()),
            jig_core::CallPart::DelegationDenied { answer, findings } => {
                CallAnswer::DelegationDenied { answer: *answer, findings: findings.clone() }
            }
            jig_core::CallPart::DelegationRefused(problem) => CallAnswer::DelegationRefused(problem.clone()),
            jig_core::CallPart::Unavailable => CallAnswer::Unavailable,
        })
    }
}

/// One durable root call decision. The key, rather than a generated receipt,
/// makes a repeated worker request find the same answer.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct CallRecord {
    pub key: CallKey,
    pub answer: CallAnswer,
}

/// Ordered fixed-size address of a durable row. Root-issued and child-issued
/// identities stay in their own variants; keys contain no payload bytes or IO
/// handles (domain/engine.md, 5.3–5.6).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Key {
    /// Immutable outcome of a person-decided proposal, loaded by its root-issued number.
    ProposalDecision(u64),
    /// Root's immutable decided held-chat revision; read only by named race
    /// replay, never restored into live state.
    EscalationDecision {
        /// Positive root-issued task.
        task: u64,
        /// Positive semantic revision, unique within task.
        revision: u64,
    },
    /// A run's durable named host-tool decision, retained until its answer is in a turn.
    Call(/** Task, attempt, completion and block position. */ CallKey),
    /// Singleton deployment header key.
    Deployment,
    /// Root-accepted transcript turn under one task and attempt.
    Turn {
        /// Positive durable task number, validated for turn writes.
        task: u64,
        /// Positive root-issued activation number.
        attempt: u64,
        /// Positive turn sequence number, represented by `u32`.
        turn: u32,
    },
    /// Root-issued live claim replay key; at most one per live task, erased on end.
    RunProof {
        /// Positive task number issued by root; current claim replaces this row.
        task: u64,
    },
    /// Root's historical typed terminal evidence; never restored into the live proof table.
    Terminal {
        /// Positive root-issued task number.
        task: u64,
        /// Positive root-issued claim number; immutable archive identity.
        attempt: u64,
    },
    /// Child semantic task/financial record key; transport proofs have root keys and no child
    /// receipt family exists.
    Tasks(
        /// Tasks-issued key, preserved under the root wrapper without reinterpretation.
        jig_core_tasks::Key,
    ),
    /// Store key for people identities, sign-ins, roles and keyed replies.
    People(
        /// People-issued key for its durable secret-free records.
        jig_core_people::Key,
    ),
    /// Stable root store address for a connector row.
    Forge(u64),
}

/// Root-owned page ranges (domain/engine.md, section 5.3).
/// The root asks the store for strictly ordered rows from one range; child
/// records already have concrete task/people wrappers. These values issue no
/// request on their own and promise no cross-page snapshot (domain/engine.md, 5.3).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Range {
    /// One immutable decision for a later authenticated proposal caller.
    ProposalDecision { proposal: u64 },
    /// Exactly one root-owned decision archive for a stale authenticated
    /// decision; no unbounded history restore.
    EscalationDecision {
        /// Positive named task.
        task: u64,
        /// Positive decided semantic revision.
        revision: u64,
    },
    /// Named call decisions for currently live tasks, restored before attempts are adopted.
    Calls,
    /// Singleton header read by the root at startup; at most one row and no continuation.
    Deployment,
    /// Root startup pages only child Live/Ledger state; historical Ended rows stay outside this
    /// range. Actual root-supported task shapes validate before child restoration.
    Tasks,
    /// Historical ended tasks, ordered by task key for bounded scans; each row carries its result position.
    EndedResults,
    /// Root pages current claims only, at most tasks `tasks` rows; every proof must correlate with
    /// its loaded live row before tasks Restored or fleet adoption. Historical
    /// transcripts/terminals are excluded.
    RunProofs,
    /// Root startup reads every people child row before accepting people.
    People,
    /// Root startup pages the forge connector's durable working set.
    Forge,
    /// Root reads one historical ended task for an authenticated result page.
    TaskResult {
        /// Positive root-issued ended task key; terminal page has at most one row.
        task: u64,
    },
    /// Historical committed transcript turns for one task and attempt.
    Turns {
        /// Positive durable task number; admitted ranges reject zero.
        task: u64,
        /// Positive root-issued activation; other attempts cannot match.
        attempt: u64,
    },
    /// All committed turns of one task, in attempt and turn order, for preparing its next run.
    TaskTranscript { task: u64 },
}

impl Range {
    /// Check membership before accepting a store row. Pure value predicate, with no allocation,
    /// effect or terminal. Turn zero is invalid; continuation ordering is checked by the load
    /// owner. It does not validate a child row's payload or its task links.
    #[must_use]
    #[expect(clippy::too_many_lines, reason = "all store families are checked exhaustively in one range matcher")]
    pub const fn contains(self, key: Key) -> bool {
        match self {
            Range::Forge => match key {
                Key::Forge(id) => id != 0,
                Key::Call(_)
                | Key::EscalationDecision { .. }
                | Key::ProposalDecision(_)
                | Key::Deployment
                | Key::Turn { .. }
                | Key::RunProof { .. }
                | Key::Terminal { .. }
                | Key::Tasks(_)
                | Key::People(_) => false,
            },
            Range::ProposalDecision { proposal } => match key {
                Key::ProposalDecision(number) => proposal != 0 && number == proposal,
                Key::Call(_)
                | Key::EscalationDecision { .. }
                | Key::Deployment
                | Key::Turn { .. }
                | Key::RunProof { .. }
                | Key::Terminal { .. }
                | Key::Tasks(_)
                | Key::People(_)
                | Key::Forge(_) => false,
            },
            Range::Calls => match key {
                Key::Call(call) => call.task != 0 && call.attempt != 0 && call.completion != 0,
                Key::EscalationDecision { .. }
                | Key::ProposalDecision(_)
                | Key::Deployment
                | Key::Turn { .. }
                | Key::RunProof { .. }
                | Key::Terminal { .. }
                | Key::Tasks(_)
                | Key::People(_)
                | Key::Forge(_) => false,
            },
            Range::EscalationDecision { task, revision } => match key {
                Key::EscalationDecision { task: found, revision: current } => {
                    task != 0 && revision != 0 && task == found && revision == current
                }
                Key::Call(_)
                | Key::ProposalDecision(_)
                | Key::Deployment
                | Key::Turn { .. }
                | Key::RunProof { .. }
                | Key::Terminal { .. }
                | Key::Tasks(_)
                | Key::People(_)
                | Key::Forge(_) => false,
            },
            Range::Deployment => match key {
                Key::Deployment => true,
                Key::Call(_)
                | Key::EscalationDecision { .. }
                | Key::ProposalDecision(_)
                | Key::Turn { .. }
                | Key::RunProof { .. }
                | Key::Terminal { .. }
                | Key::Tasks(_)
                | Key::People(_)
                | Key::Forge(_) => false,
            },
            Range::Tasks => match key {
                Key::Tasks(child) => match child {
                    jig_core_tasks::Key::Live(_)
                    | jig_core_tasks::Key::Ledger(_)
                    | jig_core_tasks::Key::Stub(_)
                    | jig_core_tasks::Key::Writer(_)
                    | jig_core_tasks::Key::Pool(_)
                    | jig_core_tasks::Key::PersonProposal(_) => true,
                    jig_core_tasks::Key::Ended(_) | jig_core_tasks::Key::History { .. } => false,
                },
                Key::Call(_)
                | Key::EscalationDecision { .. }
                | Key::ProposalDecision(_)
                | Key::Deployment
                | Key::Turn { .. }
                | Key::RunProof { .. }
                | Key::Terminal { .. }
                | Key::People(_)
                | Key::Forge(_) => false,
            },
            Range::EndedResults => match key {
                Key::Tasks(jig_core_tasks::Key::Ended(number)) => number != 0,
                Key::Call(_)
                | Key::EscalationDecision { .. }
                | Key::ProposalDecision(_)
                | Key::Deployment
                | Key::Turn { .. }
                | Key::RunProof { .. }
                | Key::Terminal { .. }
                | Key::Tasks(_)
                | Key::People(_)
                | Key::Forge(_) => false,
            },
            Range::People => match key {
                Key::People(_) => true,
                Key::Call(_)
                | Key::EscalationDecision { .. }
                | Key::ProposalDecision(_)
                | Key::Deployment
                | Key::Turn { .. }
                | Key::RunProof { .. }
                | Key::Terminal { .. }
                | Key::Tasks(_)
                | Key::Forge(_) => false,
            },
            Range::RunProofs => match key {
                Key::RunProof { task } => task != 0,
                Key::Call(_)
                | Key::EscalationDecision { .. }
                | Key::ProposalDecision(_)
                | Key::Deployment
                | Key::Turn { .. }
                | Key::Terminal { .. }
                | Key::Tasks(_)
                | Key::People(_)
                | Key::Forge(_) => false,
            },
            Range::TaskResult { task } => match key {
                Key::Tasks(jig_core_tasks::Key::Ended(number)) => task == number,
                Key::Call(_)
                | Key::EscalationDecision { .. }
                | Key::ProposalDecision(_)
                | Key::Tasks(_)
                | Key::Deployment
                | Key::Turn { .. }
                | Key::RunProof { .. }
                | Key::Terminal { .. }
                | Key::People(_)
                | Key::Forge(_) => false,
            },
            Range::Turns { task, attempt } => match key {
                Key::Turn { task: found, attempt: run, turn } => found == task && run == attempt && turn != 0,
                Key::Call(_)
                | Key::EscalationDecision { .. }
                | Key::ProposalDecision(_)
                | Key::Deployment
                | Key::RunProof { .. }
                | Key::Terminal { .. }
                | Key::Tasks(_)
                | Key::People(_)
                | Key::Forge(_) => false,
            },
            Range::TaskTranscript { task } => match key {
                Key::Turn { task: found, attempt, turn } => task != 0 && found == task && attempt != 0 && turn != 0,
                Key::Call(_)
                | Key::EscalationDecision { .. }
                | Key::ProposalDecision(_)
                | Key::Deployment
                | Key::RunProof { .. }
                | Key::Terminal { .. }
                | Key::Tasks(_)
                | Key::People(_)
                | Key::Forge(_) => false,
            },
        }
    }
}

/// Owned typed row sent root to store in a commit or returned store to root in
/// a bounded page. The transaction/page, not each row, has the store terminal
/// (domain/engine.md, 5.1, 5.3 and 5.6).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Record {
    /// The first committed final proposal decision, read by later callers.
    ProposalDecision(ProposalDecisionRecord),
    /// Named host-tool decision committed with the action it caused.
    Call(CallRecord),
    /// Root's immutable semantic decision evidence; not task-owned transport
    /// state.
    EscalationDecision(
        /// Exact bounded accepted choice.
        EscalationDecisionRecord,
    ),
    /// Fixed deployment counters, saved only by the journal.
    Deployment(Deployment),
    /// Accepted transcript turn owned by the root.
    Turn(/** Owned transcript and acceptance metadata, byte-bounded at admission. */ TurnRecord),
    /// Current bounded replay evidence; proof and all financial writes share one commit.
    RunProof(
        /// Root-owned proof, bounded by tasks.tasks, one latest turn and journal bytes.
        RunProof,
    ),
    /// Immutable typed terminal archive; startup never retains the historical family.
    Terminal(
        /// Root-owned exact bounded worker offer or canonical root-translated topology/Invalid
        /// terminal at accepted expense, within result admission bounds.
        TerminalRecord,
    ),
    /// Child's authentic task or funding row; root transport proofs have their own variants. All
    /// writes share the root decision.
    Tasks(
        /// Owned child row, deep bytes checked before journal or load retention.
        jig_core_tasks::Stored,
    ),
    /// Child's durable identity/session/keyed reply; saved atomically by the root.
    People(
        /// Owned secret-free child row, deep bytes checked before retention.
        jig_core_people::Stored,
    ),
    /// One connector row with its root allocated store identity.
    Forge { id: u64, row: Box<temper_engine_domain_forge::Stored> },
}

impl Record {
    /// Pure fixed-size key projection; it emits no request and neither copies payloads nor
    /// validates their bounds.
    #[must_use]
    pub const fn key(&self) -> Key {
        match self {
            Record::ProposalDecision(row) => Key::ProposalDecision(row.proposal),
            Record::Call(row) => Key::Call(row.key),
            Record::EscalationDecision(row) => Key::EscalationDecision { task: row.task, revision: row.revision },
            Record::Deployment(_) => Key::Deployment,
            Record::Turn(row) => Key::Turn { task: row.task, attempt: row.attempt, turn: row.turn },
            Record::RunProof(row) => Key::RunProof { task: row.task },
            Record::Terminal(row) => Key::Terminal { task: row.task, attempt: row.attempt },
            Record::Tasks(row) => Key::Tasks(row.key()),
            Record::People(row) => Key::People(row.key()),
            Record::Forge { id, .. } => Key::Forge(*id),
        }
    }
}

/// Root to store: one unique-key operation within an atomic commit. Child
/// saves/erases acquire no separate terminal; the ordered commit answers once
/// (domain/engine.md, 5.1 and 5.6).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Write {
    /// Replace the row at its own key.
    Save(/** Owned typed row, byte-bounded by journal admission. */ Record),
    /// Remove the named row, with no owned payload.
    Erase(/** Fixed-size row key; journal callers cannot erase the header. */ Key),
}

impl Write {
    /// Pure fixed-size key projection for replacement/erase coalescing; it emits no effect or
    /// terminal.
    #[must_use]
    pub const fn key(&self) -> Key {
        match self {
            Write::Save(row) => row.key(),
            Write::Erase(key) => *key,
        }
    }
}

/// Deep owned allocation bytes retained by a store row. Pure checked calculation: sums heap
/// payloads and nested boxed-record/slice allocations, excluding the outer `Record` slot and
/// allocator overhead. It returns `None` on arithmetic overflow and emits no request or terminal.
/// The root validates the whole decoded page before restoring or cloning. Payload shape admission
/// remains the child's responsibility.
#[must_use]
#[expect(clippy::too_many_lines, reason = "the record byte projection covers each stored variant")]
pub fn record_bytes(record: &Record) -> Option<u64> {
    match record {
        Record::EscalationDecision(row) => decision_bytes(&row.decision),
        Record::Call(row) => match &row.answer {
            CallAnswer::ForgeRead(result) => match result.as_ref() {
                Ok(answer) => temper_engine_domain_forge_client::answer_bytes_unbounded(answer),
                Err(_) => Some(0),
            },
            CallAnswer::ForgeEffect { .. }
            | CallAnswer::ForgeEffectRefused(_)
            | CallAnswer::Unavailable
            | CallAnswer::Proposed { .. }
            | CallAnswer::ProposalDecided { .. }
            | CallAnswer::ProposalRefused(_)
            | CallAnswer::EscalationDecided { .. }
            | CallAnswer::EscalationRefused(_)
            | CallAnswer::Controlled
            | CallAnswer::ControlRefused(_)
            | CallAnswer::ControlDenied { .. }
            | CallAnswer::Introduced
            | CallAnswer::Sent { .. }
            | CallAnswer::MessageRefused(_)
            | CallAnswer::Subscribed { .. }
            | CallAnswer::Unsubscribed
            | CallAnswer::SubscriptionRefused(_)
            | CallAnswer::DelegationRefused(_) => Some(0),
            CallAnswer::Delegated(numbers) => u64::try_from(numbers.len()).ok()?.checked_mul(8),
            CallAnswer::DelegationDenied { findings, .. } | CallAnswer::ForgeEffectDenied { findings, .. } => {
                u64::try_from(findings.len())
                    .ok()?
                    .checked_mul(u64::try_from(size_of::<jig_core_authority::Finding>()).ok()?)
            }
        },
        Record::ProposalDecision(_) | Record::Deployment(_) => Some(0),
        Record::Turn(turn) => u64::try_from(turn.transcript.len()).ok(),
        Record::RunProof(row) => match &row.terminal {
            Some(terminal) => terminal_bytes(terminal),
            None => Some(0),
        },
        Record::Terminal(row) => terminal_bytes(row),
        Record::Tasks(row) => jig_core_tasks::stored_bytes(row),
        Record::Forge { row, .. } => temper_engine_domain_forge::stored_bytes(row),
        Record::People(row) => match row {
            jig_core_people::Stored::Person { identity, .. } => u64::try_from(identity.key.subject.len())
                .ok()?
                .checked_add(u64::try_from(identity.login.len()).ok()?)?
                .checked_add(u64::try_from(identity.name.len()).ok()?),
            jig_core_people::Stored::Roles { holdings, .. } => u64::try_from(holdings.len())
                .ok()?
                .checked_mul(u64::try_from(size_of::<jig_core_people::Holding>()).ok()?),
            jig_core_people::Stored::Answer { ask, .. } => match ask.as_ref() {
                jig_core_people::Ask::EditNote { scope, change, .. } => {
                    let scope_bytes = match scope.as_ref() {
                        jig_core_people::NoteScope::Deployment
                        | jig_core_people::NoteScope::Project
                        | jig_core_people::NoteScope::Goal { .. } => 0,
                        jig_core_people::NoteScope::Resources { pattern, .. } => {
                            let mut bytes = u64::try_from(pattern.segments.len())
                                .ok()?
                                .checked_mul(u64::try_from(size_of::<Box<[u8]>>()).ok()?)?;
                            for segment in &pattern.segments {
                                bytes = bytes.checked_add(u64::try_from(segment.len()).ok()?)?;
                            }
                            let last = match &pattern.last {
                                jig_core_people::Last::Exact(bytes) | jig_core_people::Last::Open(bytes) => bytes.len(),
                            };
                            bytes.checked_add(u64::try_from(last).ok()?)?
                        }
                    };
                    let change_bytes = match change.as_ref() {
                        jig_core_people::NoteChange::Correct { description, body, references, .. } => {
                            u64::try_from(description.len())
                                .ok()?
                                .checked_add(u64::try_from(body.len()).ok()?)?
                                .checked_add(u64::try_from(references.len()).ok()?.checked_mul(8)?)?
                        }
                        jig_core_people::NoteChange::Delete { .. } => 0,
                    };
                    scope_bytes.checked_add(change_bytes)
                }
                jig_core_people::Ask::MakeService { name, .. } => u64::try_from(name.len()).ok(),
                jig_core_people::Ask::Adopt { adoption, .. } => {
                    let mut bytes = u64::try_from(adoption.resource.path.len().checked_add(adoption.options.len())?)
                        .ok()?
                        .checked_mul(u64::try_from(size_of::<Box<[u8]>>()).ok()?)?;
                    for segment in &adoption.resource.path {
                        bytes = bytes.checked_add(u64::try_from(segment.len()).ok()?)?;
                    }
                    for option in &adoption.options {
                        bytes = bytes.checked_add(u64::try_from(option.len()).ok()?)?;
                    }
                    Some(bytes)
                }
                jig_core_people::Ask::SetGoal { spec, .. } => u64::try_from(spec.len()).ok(),
                jig_core_people::Ask::SetRoles { holdings, .. } => u64::try_from(holdings.len())
                    .ok()?
                    .checked_mul(u64::try_from(size_of::<jig_core_people::Holding>()).ok()?),
                jig_core_people::Ask::Prioritise { goals, .. } => {
                    u64::try_from(goals.len()).ok()?.checked_mul(u64::try_from(size_of::<(u64, u32)>()).ok()?)
                }
                jig_core_people::Ask::Amend { amendment, .. } => jig_core_people::amendment_bytes(amendment),
                jig_core_people::Ask::ChangePolicy { change, .. } => jig_core_people::policy_change_bytes(change),
                jig_core_people::Ask::StartChat { words, .. }
                | jig_core_people::Ask::Say { words, .. }
                | jig_core_people::Ask::AnswerQuestion { words, .. } => u64::try_from(words.len()).ok(),
                jig_core_people::Ask::Move { reason, .. } | jig_core_people::Ask::Cancel { reason, .. } => {
                    u64::try_from(reason.len()).ok()
                }
                jig_core_people::Ask::Watch { .. }
                | jig_core_people::Ask::TakePerson { .. }
                | jig_core_people::Ask::SetPool { .. }
                | jig_core_people::Ask::HandBackPerson { .. }
                | jig_core_people::Ask::Stop { .. }
                | jig_core_people::Ask::Release { .. } => Some(0),
                jig_core_people::Ask::AnswerPerson { result, .. } => match result {
                    jig_core_people::PersonResult::Report { words }
                    | jig_core_people::PersonResult::Verdict { words, .. } => u64::try_from(words.len()).ok(),
                    jig_core_people::PersonResult::Failure { reason } => u64::try_from(reason.len()).ok(),
                },
                jig_core_people::Ask::DecideEscalation { decision, .. } => decision_bytes(decision),
                jig_core_people::Ask::DecideProposal { decision, .. } => match decision {
                    jig_core_people::ProposalDecision::Accept | jig_core_people::ProposalDecision::Pass => Some(0),
                    jig_core_people::ProposalDecision::Reject { reason } => u64::try_from(reason.len()).ok(),
                },
            },
            jig_core_people::Stored::SignIn { .. } | jig_core_people::Stored::ReadPosition { .. } => Some(0),
            jig_core_people::Stored::Policy { value, .. } => jig_core_people::policy_bytes(value),
        },
    }
}

/// Pure checked deep-byte projection for journal admission; erases own no payload and saves use
/// `record_bytes`. No request or terminal is emitted.
pub(crate) fn owned_bytes(write: &Write) -> Option<u64> {
    match write {
        Write::Save(record) => record_bytes(record),
        Write::Erase(_) => Some(0),
    }
}

fn terminal_bytes(row: &TerminalRecord) -> Option<u64> {
    jig_core_tasks::terminal_bytes(&row.end)
}

fn decision_bytes(decision: &jig_core_people::EscalationDecision) -> Option<u64> {
    match decision {
        jig_core_people::EscalationDecision::Release | jig_core_people::EscalationDecision::Pass => Some(0),
        jig_core_people::EscalationDecision::Reject { reason } => u64::try_from(reason.len()).ok(),
    }
}
