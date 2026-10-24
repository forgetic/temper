//! The root's typed durable store vocabulary (domain/root.md, 8).
//! It wraps core and forge records and keys. The root gathers owner-produced
//! writes into atomic decisions; the store
//! protocol encodes versions and supplies ordered, bounded pages.
//!
//! These values keep no IO state and know no store format, transport or secret
//! bytes. [`CoreRange::contains`], [`Record::key`], [`Write::key`] and [`record_bytes`]
//! are pure value helpers: they neither issue IO nor create terminal events.
//! Range membership and deep-byte accounting are only parts of admission;
//! [`crate::loads`] also checks whole-page row bounds, order and continuation.
use alloc::boxed::Box;
pub use jig_domain_core::{
    CallKey, Deployment, EscalationDecisionRecord, Family, ProposalDecisionRecord, RunProof, TerminalRecord, TurnProof,
    TurnRecord,
};

/// Fields named by a decoded amendment (domain/agent.md, 4.3).
/// Joined at the host flight; the core retains only its revision reply.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Changed {
    pub tracked: Option<jig_domain_tasks::Tracking>,
    pub fields: u8,
}

impl Changed {
    pub const SPEC: u8 = 1;
    pub const WAKE: u8 = 2;
    pub const DEPENDENCIES: u8 = 4;
    pub const AUTHORITY: u8 = 8;
    pub const INSTRUCTIONS: u8 = 16;
    pub const LLM: u8 = 32;

    #[must_use]
    pub fn amendment(amendment: &jig_domain_tasks::Amendment) -> Self {
        let mut fields = 0;
        for (named, bit) in [
            (amendment.spec.is_some(), Self::SPEC),
            (amendment.wake.is_some(), Self::WAKE),
            (amendment.dependencies.is_some(), Self::DEPENDENCIES),
            (amendment.authority.is_some(), Self::AUTHORITY),
            (amendment.instructions.is_some(), Self::INSTRUCTIONS),
            (amendment.llm.is_some(), Self::LLM),
        ] {
            if named {
                fields |= bit;
            }
        }
        Self { tracked: amendment.tracked, fields }
    }
}

/// Typed call feedback; amendment details are joined from the decoded tool.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum CallAnswer {
    /// A release must name again exactly when its effect is uncertain.
    Again { task: u64, required: bool },
    /// The caller exceeded its authority and passed the escalation up.
    PassedUp { task: u64 },
    /// A control committed at the spec revision its next amendment names.
    Revised { task: u64, revision: u64 },
    /// An amendment committed, naming the controls it changed.
    Amended { task: u64, revision: u64, changed: Changed },
    /// The revision named by the caller moved to this value.
    Moved { task: u64, now: jig_domain_people::Moved },
    /// A forge write joined the named call's decision; later attempts can reask for its settled result.
    ForgeEffect { entry: u64, deadline: skein_lib::Wall, outcome: Option<skein_forge_client::Outcome> },
    /// A forge write was refused before an outbox entry existed.
    ForgeEffectRefused(skein_forge::api::Error),
    /// The pure policy check declined a forge write, retaining its bounded reasons.
    ForgeEffectDenied { answer: jig_domain_authority::Answer, findings: Box<[jig_domain_authority::Finding]> },
    /// An engine tool was refused with the authority findings.
    ToolDenied { answer: jig_domain_authority::Answer, findings: Box<[jig_domain_authority::Finding]> },
    /// One fresh bounded forge read, retained for named-call replay.
    ForgeRead(Box<Result<skein_forge::api::Answer, skein_forge::api::Error>>),
    /// One held descendant decision reached its semantic terminal.
    EscalationDecided { task: u64, revision: u64, outcome: jig_domain_tasks::EscalationOutcome },
    /// A task holder's held-decision call was refused before mutation.
    EscalationRefused(jig_domain_tasks::Problem),
    /// Proposal entered its proposer's durable pending state.
    Proposed { proposal: u64, holder: jig_domain_tasks::ProposalHolder },
    /// Proposal was accepted, rejected, passed, or withdrawn.
    ProposalDecided { proposal: u64, outcome: jig_domain_tasks::ProposalOutcome },
    /// Proposal action or current standing failed before mutation.
    ProposalRefused(jig_domain_tasks::Problem),
    /// A named task control change entered the same durable decision.
    Controlled,
    /// A named task control change was refused without mutation.
    ControlRefused(jig_domain_tasks::Problem),
    /// An effect was refused before mutation.
    EffectRefused(jig_domain_tasks::Problem),
    /// A requested widening needs a holder or violates a hard ceiling.
    ControlDenied { answer: jig_domain_authority::Answer },
    /// Message entered its target's inbox at this commit position.
    Sent { message: u64 },
    /// Reciprocal task references were installed.
    Introduced,
    /// A message or introduction failed its live reference or inbox check.
    MessageRefused(jig_domain_tasks::Problem),
    /// One standing interest was installed.
    Subscribed { subscription: u64 },
    /// One standing interest was removed.
    Unsubscribed,
    /// A standing interest failed admission.
    SubscriptionRefused(jig_domain_tasks::Problem),
    /// All members of an authorized batch, in call order.
    Delegated(Box<[jig_domain_core::Made]>),
    /// A whole batch declined by authority, with its independent findings.
    DelegationDenied { answer: jig_domain_authority::Answer, findings: Box<[jig_domain_authority::Finding]> },
    /// A whole batch refused by structural or finite-funding admission.
    DelegationRefused(jig_domain_tasks::Problem),
    /// Render-only repository choice feedback; only the underlying refusal is stored.
    /// Box the problem to keep the existing answer slots within their allocation budget.
    ChangeRefused { problem: Box<jig_domain_tasks::Problem>, named: Option<Box<[u8]>>, repositories: Box<[Box<[u8]>]> },
    /// A note write's durable name and revision.
    NoteWritten { name: u64, revision: u32 },
    /// One bounded notes recall page.
    NoteRecalled { entries: Box<[jig_domain_notes::Entry]>, more: bool },
    /// A note request changed nothing.
    NoteRefused(jig_domain_notes::Refusal),
    /// The named tool is deferred to a later engine route.
    Unavailable,
}

impl CallAnswer {
    /// Retain the core's part of a named answer. The forge alone retains
    /// connector payloads; authority findings belong to the core.
    #[must_use]
    pub fn core_part(&self, connector: u16) -> jig_domain_core::CallPart {
        match self {
            CallAnswer::Again { task, required } => {
                jig_domain_core::CallPart::Again { task: *task, required: *required }
            }
            CallAnswer::PassedUp { task } => jig_domain_core::CallPart::PassedUp { task: *task },
            CallAnswer::Revised { task, revision } | CallAnswer::Amended { task, revision, .. } => {
                jig_domain_core::CallPart::Revised { task: *task, revision: *revision }
            }
            CallAnswer::Moved { task, now } => jig_domain_core::CallPart::Moved { task: *task, now: *now },
            CallAnswer::ForgeEffect { entry, deadline, outcome } => {
                let outcome = match outcome {
                    Some(skein_forge_client::Outcome::Made { .. }) => {
                        Some(jig_domain_core::connector::OutboxOutcome::Made)
                    }
                    Some(skein_forge_client::Outcome::Failed(_) | skein_forge_client::Outcome::Raced { .. }) => {
                        Some(jig_domain_core::connector::OutboxOutcome::Failed)
                    }
                    Some(skein_forge_client::Outcome::Withdrawn) => {
                        Some(jig_domain_core::connector::OutboxOutcome::Withdrawn)
                    }
                    Some(skein_forge_client::Outcome::Uncertain | skein_forge_client::Outcome::Queued) => {
                        Some(jig_domain_core::connector::OutboxOutcome::Uncertain)
                    }
                    Some(skein_forge_client::Outcome::Held) => {
                        Some(jig_domain_core::connector::OutboxOutcome::Held { entry: *entry })
                    }
                    None => None,
                };
                jig_domain_core::CallPart::Effect { connector, entry: *entry, deadline: *deadline, outcome }
            }
            CallAnswer::ForgeEffectRefused(_) | CallAnswer::ForgeRead(_) => {
                jig_domain_core::CallPart::Connector { connector }
            }
            CallAnswer::ForgeEffectDenied { answer, findings } => {
                jig_domain_core::CallPart::EffectDenied { answer: *answer, findings: findings.clone() }
            }
            CallAnswer::ToolDenied { answer, findings } => {
                jig_domain_core::CallPart::ToolDenied { answer: *answer, findings: findings.clone() }
            }
            CallAnswer::EscalationDecided { task, revision, outcome } => {
                jig_domain_core::CallPart::EscalationDecided { task: *task, revision: *revision, outcome: *outcome }
            }
            CallAnswer::EscalationRefused(problem) => jig_domain_core::CallPart::EscalationRefused(problem.clone()),
            CallAnswer::Proposed { proposal, holder } => {
                jig_domain_core::CallPart::Proposed { proposal: *proposal, holder: *holder }
            }
            CallAnswer::ProposalDecided { proposal, outcome } => {
                jig_domain_core::CallPart::ProposalDecided { proposal: *proposal, outcome: *outcome }
            }
            CallAnswer::ProposalRefused(problem) => jig_domain_core::CallPart::ProposalRefused(problem.clone()),
            CallAnswer::Controlled => jig_domain_core::CallPart::Controlled,
            CallAnswer::EffectRefused(problem) => jig_domain_core::CallPart::EffectRefused(problem.clone()),
            CallAnswer::ControlRefused(problem) => jig_domain_core::CallPart::ControlRefused(problem.clone()),
            CallAnswer::ControlDenied { answer } => jig_domain_core::CallPart::ControlDenied { answer: *answer },
            CallAnswer::Sent { message } => jig_domain_core::CallPart::Sent { message: *message },
            CallAnswer::Introduced => jig_domain_core::CallPart::Introduced,
            CallAnswer::MessageRefused(problem) => jig_domain_core::CallPart::MessageRefused(problem.clone()),
            CallAnswer::Subscribed { subscription } => {
                jig_domain_core::CallPart::Subscribed { subscription: *subscription }
            }
            CallAnswer::Unsubscribed => jig_domain_core::CallPart::Unsubscribed,
            CallAnswer::SubscriptionRefused(problem) => jig_domain_core::CallPart::SubscriptionRefused(problem.clone()),
            CallAnswer::Delegated(tasks) => jig_domain_core::CallPart::Delegated(tasks.clone()),
            CallAnswer::DelegationDenied { answer, findings } => {
                jig_domain_core::CallPart::DelegationDenied { answer: *answer, findings: findings.clone() }
            }
            CallAnswer::DelegationRefused(problem) => jig_domain_core::CallPart::DelegationRefused(problem.clone()),
            CallAnswer::ChangeRefused { problem, .. } => {
                jig_domain_core::CallPart::DelegationRefused(problem.as_ref().clone())
            }
            CallAnswer::NoteWritten { name, revision } => {
                jig_domain_core::CallPart::NoteWritten { name: *name, revision: *revision }
            }
            CallAnswer::NoteRecalled { entries, more } => {
                jig_domain_core::CallPart::NoteRecalled { entries: entries.clone(), more: *more }
            }
            CallAnswer::NoteRefused(why) => jig_domain_core::CallPart::NoteRefused(*why),
            CallAnswer::Unavailable => jig_domain_core::CallPart::Unavailable,
        }
    }

    /// Assemble the core's part. The host flight adds decoded amendment
    /// fields; a connector marker uses that connector's own table.
    #[must_use]
    pub fn from_core(part: &jig_domain_core::CallPart) -> Option<CallAnswer> {
        Some(match part {
            jig_domain_core::CallPart::Again { task, required } => {
                CallAnswer::Again { task: *task, required: *required }
            }
            jig_domain_core::CallPart::PassedUp { task } => CallAnswer::PassedUp { task: *task },
            jig_domain_core::CallPart::Revised { task, revision } => {
                CallAnswer::Revised { task: *task, revision: *revision }
            }
            jig_domain_core::CallPart::Moved { task, now } => CallAnswer::Moved { task: *task, now: *now },
            jig_domain_core::CallPart::Effect { entry, deadline, .. } => {
                CallAnswer::ForgeEffect { entry: *entry, deadline: *deadline, outcome: None }
            }
            jig_domain_core::CallPart::Connector { .. } => return None,
            jig_domain_core::CallPart::EffectDenied { answer, findings } => {
                CallAnswer::ForgeEffectDenied { answer: *answer, findings: findings.clone() }
            }
            jig_domain_core::CallPart::ToolDenied { answer, findings } => {
                CallAnswer::ToolDenied { answer: *answer, findings: findings.clone() }
            }
            jig_domain_core::CallPart::EscalationDecided { task, revision, outcome } => {
                CallAnswer::EscalationDecided { task: *task, revision: *revision, outcome: *outcome }
            }
            jig_domain_core::CallPart::EscalationRefused(problem) => CallAnswer::EscalationRefused(problem.clone()),
            jig_domain_core::CallPart::Proposed { proposal, holder } => {
                CallAnswer::Proposed { proposal: *proposal, holder: *holder }
            }
            jig_domain_core::CallPart::ProposalDecided { proposal, outcome } => {
                CallAnswer::ProposalDecided { proposal: *proposal, outcome: *outcome }
            }
            jig_domain_core::CallPart::ProposalRefused(problem) => CallAnswer::ProposalRefused(problem.clone()),
            jig_domain_core::CallPart::Controlled => CallAnswer::Controlled,
            jig_domain_core::CallPart::EffectRefused(problem) => CallAnswer::EffectRefused(problem.clone()),
            jig_domain_core::CallPart::ControlRefused(problem) => CallAnswer::ControlRefused(problem.clone()),
            jig_domain_core::CallPart::ControlDenied { answer } => CallAnswer::ControlDenied { answer: *answer },
            jig_domain_core::CallPart::Sent { message } => CallAnswer::Sent { message: *message },
            jig_domain_core::CallPart::Introduced => CallAnswer::Introduced,
            jig_domain_core::CallPart::MessageRefused(problem) => CallAnswer::MessageRefused(problem.clone()),
            jig_domain_core::CallPart::Subscribed { subscription } => {
                CallAnswer::Subscribed { subscription: *subscription }
            }
            jig_domain_core::CallPart::Unsubscribed => CallAnswer::Unsubscribed,
            jig_domain_core::CallPart::SubscriptionRefused(problem) => CallAnswer::SubscriptionRefused(problem.clone()),
            jig_domain_core::CallPart::Delegated(tasks) => CallAnswer::Delegated(tasks.clone()),
            jig_domain_core::CallPart::DelegationDenied { answer, findings } => {
                CallAnswer::DelegationDenied { answer: *answer, findings: findings.clone() }
            }
            jig_domain_core::CallPart::DelegationRefused(problem) => CallAnswer::DelegationRefused(problem.clone()),
            jig_domain_core::CallPart::NoteWritten { name, revision } => {
                CallAnswer::NoteWritten { name: *name, revision: *revision }
            }
            jig_domain_core::CallPart::NoteRecalled { entries, more } => {
                CallAnswer::NoteRecalled { entries: entries.clone(), more: *more }
            }
            jig_domain_core::CallPart::NoteRefused(why) => CallAnswer::NoteRefused(*why),
            jig_domain_core::CallPart::Unavailable => CallAnswer::Unavailable,
        })
    }
}

/// One durable root call decision. The key, rather than a generated receipt,
/// makes a repeated worker request find the same answer.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct CallRecord {
    pub key: CallKey,
    pub answer: CallAnswer,
    /// The exact protocol-rendered host answer and any opaque delivery evidence.
    pub settled: Option<jig_domain_core::SettledCall>,
}

/// Ordered address of a durable row. Root and child identities keep their own
/// variants; a scoped note index includes its bounded resource pattern
/// (jig's domain/engine.md, 5.3–5.6).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Key {
    Core(jig_domain_core::Key),
    Forge(temper_engine_domain_forge::Key),
}

/// Root-owned page ranges (domain/root.md, section 8).
/// The root asks the store for strictly ordered rows from one range; child
/// records already have concrete task/people wrappers. These values issue no
/// request on their own and promise no cross-page snapshot (domain/root.md, 8).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum CoreRange {
    /// One immutable decision for a later authenticated proposal caller.
    ProposalDecision { proposer: jig_domain_people::Proposer, proposal: u64 },
    /// One task owner and the retained history position of its proposal.
    TaskProposal { task: u64, proposal: u64 },
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
    /// Restored and checked before accounts and every other child.
    Settings,
    /// The accounts' counter, accounts and sources, after the header and before the children
    /// (jig's `RestoreRange::Accounts`).
    Accounts,
    /// Root startup pages only child Live/Ledger state; historical Ended rows stay outside this
    /// range. Actual root-supported task shapes validate before child restoration.
    Tasks,
    /// Root pages current claims only, at most tasks `tasks` rows; every proof must correlate with
    /// its loaded live row before tasks Restored or fleet adoption. Historical
    /// transcripts/terminals are excluded.
    RunProofs,
    /// Root startup reads every people child row before accepting people.
    People,
    /// Root reads one historical ended task for an authenticated result page.
    EndedRead { task: u64, party: u64 },
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
    /// The committed turns of one task's current window, from its first
    /// (attempt, turn), in key order, for preparing its next run.
    TaskTranscript { task: u64, from: (u64, u32) },
    /// One bounded notes-child load by name or by scope and cursor.
    Notes(jig_domain_notes::Range),
}

/// Child-contiguous live or historical store range.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Range {
    Core(CoreRange),
    /// Ended owners in jig's census index-walk order, with repeats. The
    /// cursor is the last index key walked, rather than an owner row's key
    /// (protocol/store.md, section 5).
    Census(jig_domain_core::Census),
    Forge,
}

impl Range {
    /// Check membership before accepting a store row. Pure value predicate, with no allocation,
    /// effect or terminal. Turn zero is invalid; continuation ordering is checked by the load
    /// owner. It does not validate a child row's payload or its task links.
    #[must_use]
    pub fn contains(&self, key: &Key) -> bool {
        let core = match key {
            Key::Core(core) => core,
            Key::Forge(_) => return *self == Range::Forge,
        };
        match self {
            Range::Forge => false,
            // The store joins the census's admitted index rows to the ended
            // owners they name, and pages those owners in key order.
            Range::Census(census) => match core {
                jig_domain_core::Key::Tasks(jig_domain_tasks::Key::Ended(task)) => *task != 0,
                jig_domain_core::Key::People(jig_domain_people::Key::EndedRead { .. }) => census.admits(core),
                jig_domain_core::Key::Core(_)
                | jig_domain_core::Key::Tasks(_)
                | jig_domain_core::Key::People(_)
                | jig_domain_core::Key::Notes(_)
                | jig_domain_core::Key::Accounts(_) => false,
            },
            Range::Core(range) => range.contains(core),
        }
    }
}

impl CoreRange {
    /// Membership of one core or child key; live families follow jig's own predicates.
    #[must_use]
    pub fn contains(&self, key: &jig_domain_core::Key) -> bool {
        use jig_domain_core::{CoreKey, Key as Core};
        match self {
            CoreRange::TaskProposal { task, proposal } => {
                let Core::Tasks(
                    jig_domain_tasks::Key::Live(found) | jig_domain_tasks::Key::History { task: found, .. },
                ) = key
                else {
                    return false;
                };
                *task != 0 && *proposal != 0 && found == task
            }
            CoreRange::ProposalDecision { proposer, proposal } => {
                let Core::Core(CoreKey::ProposalDecision { proposer: found, proposal: number }) = key else {
                    return false;
                };
                *proposal != 0 && found == proposer && number == proposal
            }
            CoreRange::Calls => {
                let Core::Core(CoreKey::Call(call)) = key else { return false };
                call.task != 0 && call.attempt != 0 && call.completion != 0
            }
            CoreRange::EscalationDecision { task, revision } => {
                let Core::Core(CoreKey::EscalationDecision { task: found, revision: current }) = key else {
                    return false;
                };
                *task != 0 && *revision != 0 && task == found && revision == current
            }
            CoreRange::Deployment => {
                let Core::Core(CoreKey::Deployment) = key else { return false };
                true
            }
            CoreRange::Settings => jig_domain_core::RestoreRange::Settings.admits(key),
            CoreRange::Accounts => {
                let Core::Accounts(_) = key else { return false };
                true
            }
            CoreRange::Tasks => match key {
                Core::Core(CoreKey::Projection(_)) => true,
                Core::Tasks(child) => child.live(),
                Core::Core(_) | Core::People(_) | Core::Notes(_) | Core::Accounts(_) => false,
            },
            CoreRange::People => {
                let Core::People(child) = key else { return false };
                child.live()
            }
            CoreRange::RunProofs => {
                let Core::Core(CoreKey::RunProof(task)) = key else { return false };
                *task != 0
            }
            CoreRange::EndedRead { task, party } => {
                *key == jig_domain_core::Key::People(jig_domain_people::Key::EndedRead { task: *task, party: *party })
            }
            CoreRange::TaskResult { task } => {
                let Core::Tasks(jig_domain_tasks::Key::Ended(number)) = key else { return false };
                task == number
            }
            CoreRange::Turns { task, attempt } => {
                let Core::Core(CoreKey::Turn { task: found, attempt: run, turn }) = key else { return false };
                found == task && run == attempt && *turn != 0
            }
            CoreRange::TaskTranscript { task, from } => {
                let Core::Core(CoreKey::Turn { task: found, attempt, turn }) = key else { return false };
                *task != 0 && found == task && *attempt != 0 && *turn != 0 && (*attempt, *turn) >= *from
            }
            CoreRange::Notes(range) => {
                let Core::Notes(child) = key else { return false };
                match range {
                    jig_domain_notes::Range::Entry { name } => match child {
                        jig_domain_notes::Key::Entry { name: found } => name == found,
                        jig_domain_notes::Key::Line { .. } => false,
                    },
                    jig_domain_notes::Range::Lines { scope, after } => match child {
                        jig_domain_notes::Key::Line { scope: found, name } => {
                            let past_cursor = match after {
                                Some(cursor) => name > cursor,
                                None => true,
                            };
                            scope == found && past_cursor
                        }
                        jig_domain_notes::Key::Entry { .. } => false,
                    },
                }
            }
        }
    }
}

/// Owned typed row sent root to store in a commit or returned store to root in
/// a bounded page. The transaction/page, not each row, has the store terminal
/// (domain/root.md, 8).
#[derive(Clone, PartialEq, Eq, Debug)]
#[expect(clippy::large_enum_variant, reason = "jig's core records move whole through the store's pages")]
pub enum Record {
    Core(jig_domain_core::Record),
    Forge { row: Box<temper_engine_domain_forge::Stored> },
}

impl Record {
    /// Pure fixed-size key projection; it emits no request and neither copies payloads nor
    /// validates their bounds.
    #[must_use]
    pub fn key(&self) -> Key {
        match self {
            Record::Core(row) => Key::Core(row.key()),
            Record::Forge { row } => Key::Forge(temper_engine_domain_forge::stored_key(row)),
        }
    }
}

/// Root to store: one unique-key operation within an atomic commit. Child
/// saves/erases acquire no separate terminal; the ordered commit answers once
/// (domain/root.md, 8).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Write {
    /// Copy a held revision to its history destination before its held row is erased.
    CopySettingsRevision(u32),
    /// Replace the row at its own key.
    Save(/** Owned typed row, byte-bounded by journal admission. */ Record),
    /// Remove the named row, with no owned payload.
    Erase(/** Fixed-size row key; journal callers cannot erase the header. */ Key),
}

impl Write {
    /// Pure fixed-size key projection for replacement/erase coalescing; it emits no effect or
    /// terminal.
    #[must_use]
    pub fn key(&self) -> Key {
        match self {
            Write::CopySettingsRevision(number) => Key::Core(jig_domain_core::Key::Core(
                jig_domain_core::CoreKey::Settings(jig_domain_core::settings::Key::History(*number)),
            )),
            Write::Save(row) => row.key(),
            Write::Erase(key) => key.clone(),
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
        Record::Core(jig_domain_core::Record::Core(jig_domain_core::CoreRecord::Settings(row))) => {
            settings_bytes::record(row)
        }
        Record::Core(jig_domain_core::Record::Core(jig_domain_core::CoreRecord::Projection(row))) => {
            jig_domain_core::projection_record_bytes(row)
        }
        Record::Core(jig_domain_core::Record::Core(jig_domain_core::CoreRecord::EscalationDecision(row))) => {
            decision_bytes(&row.decision)
        }
        Record::Core(jig_domain_core::Record::Core(jig_domain_core::CoreRecord::Call(row))) => {
            let answer = match CallAnswer::from_core(&row.part) {
                Some(answer) => call_answer_bytes(&answer)?,
                None => 0,
            };
            answer.checked_add(match &row.settled {
                Some(call) => call.owned_bytes()?,
                None => 0,
            })
        }
        Record::Core(jig_domain_core::Record::Core(
            jig_domain_core::CoreRecord::ProposalDecision(_) | jig_domain_core::CoreRecord::Deployment(_),
        )) => Some(0),
        Record::Core(jig_domain_core::Record::Core(jig_domain_core::CoreRecord::Turn(turn))) => {
            u64::try_from(turn.transcript.len()).ok()
        }
        Record::Core(jig_domain_core::Record::Core(jig_domain_core::CoreRecord::RunProof(row))) => {
            match &row.terminal {
                Some(terminal) => terminal_bytes(terminal),
                None => Some(0),
            }
        }
        Record::Core(jig_domain_core::Record::Core(jig_domain_core::CoreRecord::Terminal(row))) => terminal_bytes(row),
        Record::Core(jig_domain_core::Record::Tasks(row)) => jig_domain_tasks::stored_bytes(row),
        Record::Core(jig_domain_core::Record::Accounts(row)) => match row {
            jig_domain_accounts::Stored::Next { .. } => Some(0),
            jig_domain_accounts::Stored::Account { subject, display, uses, .. } => {
                u64::try_from(subject.len()).ok()?.checked_add(u64::try_from(display.len()).ok()?)?.checked_add(
                    u64::try_from(uses.as_deref().unwrap_or_default().len())
                        .ok()?
                        .checked_mul(u64::try_from(size_of::<jig_domain_accounts::Use>()).ok()?)?,
                )
            }
            jig_domain_accounts::Stored::Issued { name, .. } | jig_domain_accounts::Stored::Source { name, .. } => {
                u64::try_from(name.len()).ok()
            }
        },
        Record::Core(jig_domain_core::Record::Notes(row)) => match row {
            jig_domain_notes::Record::Entry(entry) => note_entry_bytes(entry),
            jig_domain_notes::Record::Line(line) => {
                note_scope_bytes(&line.scope)?.checked_add(u64::try_from(line.description.len()).ok()?)
            }
        },
        Record::Forge { row, .. } => temper_engine_domain_forge::stored_bytes(row),
        Record::Core(jig_domain_core::Record::People(row)) => match row {
            jig_domain_people::Stored::Person { identity, .. } => u64::try_from(identity.key.subject.len())
                .ok()?
                .checked_add(u64::try_from(identity.login.len()).ok()?)?
                .checked_add(u64::try_from(identity.name.len()).ok()?),
            jig_domain_people::Stored::Roles { holdings, .. } => u64::try_from(holdings.len())
                .ok()?
                .checked_mul(u64::try_from(size_of::<jig_domain_people::Holding>()).ok()?),
            jig_domain_people::Stored::Answer { ask, models, outcome, .. } => {
                let payload = match ask.as_ref() {
                    jig_domain_people::Ask::AccountChange { change, .. } => {
                        use jig_domain_people::AccountChange;
                        match change {
                            AccountChange::DeclareProgram { name, .. } => u64::try_from(name.len()).ok(),
                            AccountChange::SetUses { uses, .. } => {
                                u64::try_from(uses.as_deref().unwrap_or_default().len())
                                    .ok()?
                                    .checked_mul(u64::try_from(size_of::<jig_domain_people::Use>()).ok()?)
                            }
                            AccountChange::CheckProgram { .. }
                            | AccountChange::FinishSignIn { .. }
                            | AccountChange::AddKey { .. }
                            | AccountChange::ReplaceKey { .. }
                            | AccountChange::RemoveKey { .. }
                            | AccountChange::Resume { .. }
                            | AccountChange::Remove { .. }
                            | AccountChange::MoveAccount { .. }
                            | AccountChange::MoveSource { .. }
                            | AccountChange::MoveOut { .. }
                            | AccountChange::Written { .. }
                            | AccountChange::MoveIn { .. } => Some(0),
                        }
                    }
                    jig_domain_people::Ask::Compound { operations, .. } => people_bytes::compound(operations),
                    jig_domain_people::Ask::Create { batch, .. } => people_bytes::batch(batch),
                    jig_domain_people::Ask::Propose {
                        action: jig_domain_people::Action::Batch(batch), reason, ..
                    } => people_bytes::batch(batch)?.checked_add(u64::try_from(reason.len()).ok()?),
                    jig_domain_people::Ask::Propose {
                        action: jig_domain_people::Action::Amend { amendment, .. },
                        reason,
                        ..
                    } => jig_domain_people::amendment_bytes(amendment)?.checked_add(u64::try_from(reason.len()).ok()?),
                    jig_domain_people::Ask::EditNote { scope, change, .. } => {
                        let scope_bytes = match scope.as_ref() {
                            jig_domain_people::NoteScope::Deployment
                            | jig_domain_people::NoteScope::Project
                            | jig_domain_people::NoteScope::Goal { .. } => 0,
                            jig_domain_people::NoteScope::Resources { pattern, .. } => {
                                let mut bytes = u64::try_from(pattern.segments.len())
                                    .ok()?
                                    .checked_mul(u64::try_from(size_of::<Box<[u8]>>()).ok()?)?;
                                for segment in &pattern.segments {
                                    bytes = bytes.checked_add(u64::try_from(segment.len()).ok()?)?;
                                }
                                let last = match &pattern.last {
                                    jig_domain_people::Last::Exact(bytes) | jig_domain_people::Last::Open(bytes) => {
                                        bytes.len()
                                    }
                                };
                                bytes.checked_add(u64::try_from(last).ok()?)?
                            }
                        };
                        let change_bytes = match change.as_ref() {
                            jig_domain_people::NoteChange::Correct { description, body, references, .. } => {
                                u64::try_from(description.len())
                                    .ok()?
                                    .checked_add(u64::try_from(body.len()).ok()?)?
                                    .checked_add(u64::try_from(references.len()).ok()?.checked_mul(8)?)?
                            }
                            jig_domain_people::NoteChange::Delete { .. } => 0,
                        };
                        scope_bytes.checked_add(change_bytes)
                    }
                    jig_domain_people::Ask::MakeProject { name, .. } | jig_domain_people::Ask::Rename { name, .. } => {
                        u64::try_from(name.len()).ok()
                    }
                    jig_domain_people::Ask::MakeService { name, .. } => u64::try_from(name.len()).ok(),
                    jig_domain_people::Ask::Adopt { adoption, .. } => adoption_bytes(adoption),
                    jig_domain_people::Ask::SetRoles { holdings, .. } => u64::try_from(holdings.len())
                        .ok()?
                        .checked_mul(u64::try_from(size_of::<jig_domain_people::Holding>()).ok()?),
                    jig_domain_people::Ask::Amend { amendment, .. } => jig_domain_people::amendment_bytes(amendment),
                    jig_domain_people::Ask::ChangePolicy { change, .. } => {
                        jig_domain_people::policy_change_bytes(change)
                    }
                    jig_domain_people::Ask::Say { words, .. }
                    | jig_domain_people::Ask::AnswerQuestion { words, .. } => u64::try_from(words.len()).ok(),
                    jig_domain_people::Ask::Move { reason, .. } | jig_domain_people::Ask::Cancel { reason, .. } => {
                        u64::try_from(reason.len()).ok()
                    }
                    jig_domain_people::Ask::ChangeSettings { .. }
                    | jig_domain_people::Ask::Withdraw { .. }
                    | jig_domain_people::Ask::MarkSeen { .. }
                    | jig_domain_people::Ask::Application { .. }
                    | jig_domain_people::Ask::TakePerson { .. }
                    | jig_domain_people::Ask::SetPool { .. }
                    | jig_domain_people::Ask::HandBackPerson { .. }
                    | jig_domain_people::Ask::Stop { .. }
                    | jig_domain_people::Ask::Release { .. }
                    | jig_domain_people::Ask::AccountSignIn { .. }
                    | jig_domain_people::Ask::AccountSignOut { .. } => Some(0),
                    jig_domain_people::Ask::AnswerPerson { result, .. } => match result {
                        jig_domain_people::PersonResult::Report { words }
                        | jig_domain_people::PersonResult::Verdict { words, .. } => u64::try_from(words.len()).ok(),
                        jig_domain_people::PersonResult::Failure { reason } => u64::try_from(reason.len()).ok(),
                        jig_domain_people::PersonResult::Approved { state, words } => {
                            u64::try_from(state.len()).ok()?.checked_add(match words {
                                Some(words) => u64::try_from(words.len()).ok()?,
                                None => 0,
                            })
                        }
                    },
                    jig_domain_people::Ask::DecideEscalation { decision, .. } => decision_bytes(decision),
                    jig_domain_people::Ask::DecideProposal { decision, .. } => match decision {
                        jig_domain_people::ProposalDecision::Accept | jig_domain_people::ProposalDecision::Pass => {
                            Some(0)
                        }
                        jig_domain_people::ProposalDecision::Reject { reason } => u64::try_from(reason.len()).ok(),
                    },
                }?;
                model_context_bytes(models)?.checked_add(payload)?.checked_add(program_answer_bytes(outcome)?)
            }
            jig_domain_people::Stored::SignIn { .. }
            | jig_domain_people::Stored::ReadResults { .. }
            | jig_domain_people::Stored::EndedRead { .. }
            | jig_domain_people::Stored::ReadConversation { .. } => Some(0),
            jig_domain_people::Stored::Policy { name, value, .. } => {
                jig_domain_people::policy_bytes(value)?.checked_add(u64::try_from(name.len()).ok()?)
            }
            jig_domain_people::Stored::Acted { act, outcome, .. } => {
                let payload = match act {
                    jig_domain_people::Act::Close { reason, .. } => u64::try_from(reason.len()).ok()?,
                    jig_domain_people::Act::AddKey { .. } | jig_domain_people::Act::RemoveAccount { .. } => 0,
                    jig_domain_people::Act::Adopt { adoption, .. } => adoption_bytes(adoption)?,
                    jig_domain_people::Act::MakeProject { .. } => unreachable!("demo never makes a project"),
                };
                payload.checked_add(program_answer_bytes(outcome)?)
            }
        },
    }
}

fn note_scope_bytes(scope: &jig_domain_notes::Scope) -> Option<u64> {
    match scope {
        jig_domain_notes::Scope::Resources { pattern, .. } => {
            let mut bytes =
                u64::try_from(pattern.segments.len()).ok()?.checked_mul(u64::try_from(size_of::<Box<[u8]>>()).ok()?)?;
            for segment in &pattern.segments {
                bytes = bytes.checked_add(u64::try_from(segment.len()).ok()?)?;
            }
            let last = match &pattern.last {
                jig_domain_notes::Last::Exact(value) | jig_domain_notes::Last::Open(value) => value.len(),
            };
            bytes.checked_add(u64::try_from(last).ok()?)
        }
        jig_domain_notes::Scope::Deployment
        | jig_domain_notes::Scope::Project { .. }
        | jig_domain_notes::Scope::Goal { .. } => Some(0),
    }
}

fn note_entry_bytes(entry: &jig_domain_notes::Entry) -> Option<u64> {
    note_scope_bytes(&entry.scope)?
        .checked_add(u64::try_from(entry.description.len()).ok()?)?
        .checked_add(u64::try_from(entry.body.len()).ok()?)?
        .checked_add(u64::from(entry.references.len()).checked_mul(8)?)
}

/// Checked deep-byte projection for journal admission, including scoped erase keys.
pub(crate) fn owned_bytes(write: &Write) -> Option<u64> {
    match write {
        Write::CopySettingsRevision(_) => u64::try_from(size_of::<u32>()).ok(), // The store copies the payload; only the number crosses this boundary.
        Write::Save(record) => record_bytes(record),
        Write::Erase(Key::Core(jig_domain_core::Key::Notes(jig_domain_notes::Key::Line { scope, .. }))) => {
            note_scope_bytes(scope)
        }
        Write::Erase(
            Key::Core(
                jig_domain_core::Key::Notes(jig_domain_notes::Key::Entry { .. })
                | jig_domain_core::Key::Core(
                    jig_domain_core::CoreKey::Deployment
                    | jig_domain_core::CoreKey::Settings(_)
                    | jig_domain_core::CoreKey::Call(_)
                    | jig_domain_core::CoreKey::EscalationDecision { .. }
                    | jig_domain_core::CoreKey::ProposalDecision { .. }
                    | jig_domain_core::CoreKey::Turn { .. }
                    | jig_domain_core::CoreKey::RunProof(..)
                    | jig_domain_core::CoreKey::Terminal { .. }
                    | jig_domain_core::CoreKey::Projection(_),
                )
                | jig_domain_core::Key::Tasks(_)
                | jig_domain_core::Key::People(_)
                | jig_domain_core::Key::Accounts(_),
            )
            | Key::Forge(_),
        ) => Some(0),
    }
}

fn terminal_bytes(row: &TerminalRecord) -> Option<u64> {
    jig_domain_tasks::terminal_bytes(&row.end)
}

fn decision_bytes(decision: &jig_domain_people::EscalationDecision) -> Option<u64> {
    match decision {
        jig_domain_people::EscalationDecision::Release | jig_domain_people::EscalationDecision::Pass => Some(0),
        jig_domain_people::EscalationDecision::Reject { reason } => u64::try_from(reason.len()).ok(),
    }
}

pub(crate) fn call_answer_bytes(answer: &CallAnswer) -> Option<u64> {
    match answer {
        CallAnswer::ForgeRead(result) => match result.as_ref() {
            Ok(answer) => skein_forge::measure::answer(answer),
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
        | CallAnswer::Again { .. }
        | CallAnswer::PassedUp { .. }
        | CallAnswer::Revised { .. }
        | CallAnswer::Amended { .. }
        | CallAnswer::Moved { .. }
        | CallAnswer::Controlled
        | CallAnswer::EffectRefused(_)
        | CallAnswer::ControlRefused(_)
        | CallAnswer::ControlDenied { .. }
        | CallAnswer::Introduced
        | CallAnswer::Sent { .. }
        | CallAnswer::MessageRefused(_)
        | CallAnswer::Subscribed { .. }
        | CallAnswer::Unsubscribed
        | CallAnswer::SubscriptionRefused(_)
        | CallAnswer::DelegationRefused(_)
        | CallAnswer::NoteWritten { .. }
        | CallAnswer::NoteRefused(_) => Some(0),
        CallAnswer::ChangeRefused { named, repositories, .. } => {
            let mut bytes = u64::try_from(repositories.len())
                .ok()?
                .checked_mul(u64::try_from(size_of::<Box<[u8]>>()).ok()?)?
                .checked_add(u64::try_from(size_of::<jig_domain_tasks::Problem>()).ok()?)?;
            if let Some(named) = named {
                bytes = bytes.checked_add(u64::try_from(named.len()).ok()?)?;
            }
            for name in repositories {
                bytes = bytes.checked_add(u64::try_from(name.len()).ok()?)?;
            }
            Some(bytes)
        }
        CallAnswer::NoteRecalled { entries, .. } => {
            let mut bytes = u64::try_from(entries.len())
                .ok()?
                .checked_mul(u64::try_from(size_of::<jig_domain_notes::Entry>()).ok()?)?;
            for entry in entries {
                bytes = bytes.checked_add(note_entry_bytes(entry)?)?;
            }
            Some(bytes)
        }
        CallAnswer::Delegated(numbers) => {
            u64::try_from(numbers.len()).ok()?.checked_mul(u64::try_from(size_of::<jig_domain_core::Made>()).ok()?)
        }
        CallAnswer::DelegationDenied { findings, .. }
        | CallAnswer::ForgeEffectDenied { findings, .. }
        | CallAnswer::ToolDenied { findings, .. } => u64::try_from(findings.len())
            .ok()?
            .checked_mul(u64::try_from(size_of::<jig_domain_authority::Finding>()).ok()?),
    }
}

mod people_bytes;
mod settings_bytes;

fn model_context_bytes(models: &[Box<[u8]>]) -> Option<u64> {
    let mut bytes = u64::try_from(models.len()).ok()?.checked_mul(u64::try_from(size_of::<Box<[u8]>>()).ok()?)?;
    for name in models {
        bytes = bytes.checked_add(u64::try_from(name.len()).ok()?)?;
    }
    Some(bytes)
}

/// Newly owned program answer fields, beside the retained ask and model context.
pub(crate) fn program_answer_bytes(outcome: &jig_domain_people::Outcome) -> Option<u64> {
    use jig_domain_people::Outcome;
    match outcome {
        Outcome::Refused(jig_domain_people::Refusal::Full { tasks, .. }) => {
            u64::try_from(tasks.len()).ok()?.checked_mul(u64::try_from(size_of::<u64>()).ok()?)
        }
        Outcome::AccountProgramDeclared { name, .. } => u64::try_from(name.len()).ok(),
        Outcome::AccountRemoved { failed_hosts, .. } | Outcome::AccountSignedOut { failed_hosts, .. } => {
            let mut bytes = u64::try_from(failed_hosts.len())
                .ok()?
                .checked_mul(u64::try_from(size_of::<jig_domain_people::AccountHostFailure>()).ok()?)?;
            for host in &**failed_hosts {
                bytes = bytes.checked_add(u64::try_from(host.name.len()).ok()?)?;
            }
            Some(bytes)
        }
        Outcome::SettingsChanged { .. }
        | Outcome::Compound { .. }
        | Outcome::CompoundRefused { .. }
        | Outcome::ProjectMade { .. }
        | Outcome::Renamed { .. }
        | Outcome::Created { .. }
        | Outcome::MarkedSeen { .. }
        | Outcome::Applied { .. }
        | Outcome::Again(_)
        | Outcome::SignedOut
        | Outcome::NoteEdited { .. }
        | Outcome::ServiceMade { .. }
        | Outcome::Adopted { .. }
        | Outcome::Proposed { .. }
        | Outcome::Withdrawn { .. }
        | Outcome::Stopped { .. }
        | Outcome::Cancelled { .. }
        | Outcome::Released { .. }
        | Outcome::PersonTaken { .. }
        | Outcome::PersonHandedBack { .. }
        | Outcome::PersonAnswered { .. }
        | Outcome::Moved { .. }
        | Outcome::ProposalDecided { .. }
        | Outcome::Said { .. }
        | Outcome::QuestionAnswered { .. }
        | Outcome::Amended { .. }
        | Outcome::RolesSet { .. }
        | Outcome::PolicyChanged { .. }
        | Outcome::PoolSet { .. }
        | Outcome::EscalationDecided { .. }
        | Outcome::AccountSignInStarted { .. }
        | Outcome::AccountFinishing { .. }
        | Outcome::AccountKeyKept { .. }
        | Outcome::AccountChanged
        | Outcome::AccountMovingOut { .. }
        | Outcome::AccountMovedIn { .. }
        | Outcome::Refused(_) => Some(0),
    }
}

fn adoption_bytes(adoption: &jig_domain_people::Adoption) -> Option<u64> {
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
