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
use skein_lib::Wall;

/// Root to store, then store to root at startup: one fixed-size deployment
/// header with allocated high-water marks. Every writing decision saves it
/// atomically with its other rows (domain/engine.md, 5.1 and 5.4).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Deployment {
    /// Shell-supplied identity committed on the deployment's first start and restored thereafter;
    /// exactly sixteen bytes.
    pub id: [u8; 16],
    /// Last task number allocated by the root; zero before its first allocation, never reset or
    /// reused.
    pub tasks: u64,
    /// Last candidate person number allocated for sign-in; refused or existing identities may leave
    /// gaps.
    pub people: u64,
    /// Last sign-in candidate allocated by the root; persisted gaps are allowed and no secret
    /// session bytes are held here.
    pub sign_ins: u64,
    /// Last root-issued result position in commit order.
    pub messages: u64,
    /// Last activation number allocated for a claim; the candidate may leave a gap if preparation
    /// fails.
    pub runs: u64,
    /// Last root-allocated call number; current 06a routes no tool calls.
    pub calls: u64,
    /// Last issued ordered commit. A restored header is already durable; a live journal may await
    /// this number's store terminal.
    pub commits: u64,
}

/// Root-selected counter for `fresh`; each is a separate checked `u64` high-water
/// mark, not a channel token or child-owned handle (domain/engine.md, 5.4).
/// Selecting a family is pure; allocating it dirties the journal header.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Family {
    /// Task identities saved with the task row.
    Task,
    /// Person candidates supplied to people sign-in admission.
    Person,
    /// Secret-free sign-in candidates supplied to people.
    SignIn,
    /// Result positions issued as tasks end.
    Message,
    /// Fresh attempt identities saved at claims.
    Run,
    /// Count of distinct named calls newly decided by the engine.
    Call,
}

/// Stable call identity supplied by a run and scoped by the root's task and
/// claim (domain/engine.md, sections 5.4 and 7.3).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct CallKey {
    pub task: u64,
    pub attempt: u64,
    /// One-based assistant completion.
    pub completion: u32,
    /// Zero-based assistant block position.
    pub position: u32,
}

/// Exact typed answer kept for replay across a lost channel or root restart.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum CallAnswer {
    /// One held descendant decision reached its semantic terminal.
    EscalationDecided { task: u64, revision: u64, outcome: temper_engine_domain_tasks::EscalationOutcome },
    /// A task holder's held-decision call was refused before mutation.
    EscalationRefused(temper_engine_domain_tasks::Problem),
    /// Proposal entered its proposer's durable pending state.
    Proposed { proposal: u64 },
    /// Proposal was accepted, rejected, passed, or withdrawn.
    ProposalDecided { proposal: u64, outcome: temper_engine_domain_tasks::ProposalOutcome },
    /// Proposal action or current standing failed before mutation.
    ProposalRefused(temper_engine_domain_tasks::Problem),
    /// A named task control change entered the same durable decision.
    Controlled,
    /// A named task control change was refused without mutation.
    ControlRefused(temper_engine_domain_tasks::Problem),
    /// A requested widening needs a holder or violates a hard ceiling.
    ControlDenied { answer: temper_engine_domain_authority::Answer },
    /// Message entered its target's inbox at this commit position.
    Sent { message: u64 },
    /// Reciprocal task references were installed.
    Introduced,
    /// A message or introduction failed its live reference or inbox check.
    MessageRefused(temper_engine_domain_tasks::Problem),
    /// One standing interest was installed.
    Subscribed { subscription: u64 },
    /// One standing interest was removed.
    Unsubscribed,
    /// A standing interest failed admission.
    SubscriptionRefused(temper_engine_domain_tasks::Problem),
    /// All members of an authorized batch, in call order.
    Delegated(Box<[u64]>),
    /// A whole batch declined by authority, with its independent findings.
    DelegationDenied {
        answer: temper_engine_domain_authority::Answer,
        findings: Box<[temper_engine_domain_authority::Finding]>,
    },
    /// A whole batch refused by structural or finite-funding admission.
    DelegationRefused(temper_engine_domain_tasks::Problem),
    /// The named tool is deferred to a later engine route.
    Unavailable,
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
        temper_engine_domain_tasks::Key,
    ),
    /// Store key for people identities, sign-ins, roles and keyed replies.
    People(
        /// People-issued key for its durable secret-free records.
        temper_engine_domain_people::Key,
    ),
}

/// Root-owned page ranges (domain/engine.md, section 5.3).
/// The root asks the store for strictly ordered rows from one range; child
/// records already have concrete task/people wrappers. These values issue no
/// request on their own and promise no cross-page snapshot (domain/engine.md, 5.3).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Range {
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
            Range::Calls => match key {
                Key::Call(call) => call.task != 0 && call.attempt != 0 && call.completion != 0,
                Key::EscalationDecision { .. }
                | Key::Deployment
                | Key::Turn { .. }
                | Key::RunProof { .. }
                | Key::Terminal { .. }
                | Key::Tasks(_)
                | Key::People(_) => false,
            },
            Range::EscalationDecision { task, revision } => match key {
                Key::EscalationDecision { task: found, revision: current } => {
                    task != 0 && revision != 0 && task == found && revision == current
                }
                Key::Call(_)
                | Key::Deployment
                | Key::Turn { .. }
                | Key::RunProof { .. }
                | Key::Terminal { .. }
                | Key::Tasks(_)
                | Key::People(_) => false,
            },
            Range::Deployment => match key {
                Key::Deployment => true,
                Key::Call(_)
                | Key::EscalationDecision { .. }
                | Key::Turn { .. }
                | Key::RunProof { .. }
                | Key::Terminal { .. }
                | Key::Tasks(_)
                | Key::People(_) => false,
            },
            Range::Tasks => match key {
                Key::Tasks(child) => match child {
                    temper_engine_domain_tasks::Key::Live(_) | temper_engine_domain_tasks::Key::Ledger(_) => true,
                    temper_engine_domain_tasks::Key::Ended(_) | temper_engine_domain_tasks::Key::History { .. } => {
                        false
                    }
                },
                Key::Call(_)
                | Key::EscalationDecision { .. }
                | Key::Deployment
                | Key::Turn { .. }
                | Key::RunProof { .. }
                | Key::Terminal { .. }
                | Key::People(_) => false,
            },
            Range::EndedResults => match key {
                Key::Tasks(temper_engine_domain_tasks::Key::Ended(number)) => number != 0,
                Key::Call(_)
                | Key::EscalationDecision { .. }
                | Key::Deployment
                | Key::Turn { .. }
                | Key::RunProof { .. }
                | Key::Terminal { .. }
                | Key::Tasks(_)
                | Key::People(_) => false,
            },
            Range::People => match key {
                Key::People(_) => true,
                Key::Call(_)
                | Key::EscalationDecision { .. }
                | Key::Deployment
                | Key::Turn { .. }
                | Key::RunProof { .. }
                | Key::Terminal { .. }
                | Key::Tasks(_) => false,
            },
            Range::RunProofs => match key {
                Key::RunProof { task } => task != 0,
                Key::Call(_)
                | Key::EscalationDecision { .. }
                | Key::Deployment
                | Key::Turn { .. }
                | Key::Terminal { .. }
                | Key::Tasks(_)
                | Key::People(_) => false,
            },
            Range::TaskResult { task } => match key {
                Key::Tasks(temper_engine_domain_tasks::Key::Ended(number)) => task == number,
                Key::Call(_)
                | Key::EscalationDecision { .. }
                | Key::Tasks(_)
                | Key::Deployment
                | Key::Turn { .. }
                | Key::RunProof { .. }
                | Key::Terminal { .. }
                | Key::People(_) => false,
            },
            Range::Turns { task, attempt } => match key {
                Key::Turn { task: found, attempt: run, turn } => found == task && run == attempt && turn != 0,
                Key::Call(_)
                | Key::EscalationDecision { .. }
                | Key::Deployment
                | Key::RunProof { .. }
                | Key::Terminal { .. }
                | Key::Tasks(_)
                | Key::People(_) => false,
            },
            Range::TaskTranscript { task } => match key {
                Key::Turn { task: found, attempt, turn } => task != 0 && found == task && attempt != 0 && turn != 0,
                Key::Call(_)
                | Key::EscalationDecision { .. }
                | Key::Deployment
                | Key::RunProof { .. }
                | Key::Terminal { .. }
                | Key::Tasks(_)
                | Key::People(_) => false,
            },
        }
    }
}

/// Root to store: an accepted numbered transcript and cumulative priced spend,
/// saved atomically with child task/funding and root latest-turn proof before
/// its worker ACK
/// (domain/engine.md, 5.1 and 7.2). Loads return the same owned shape.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct TurnRecord {
    /// Positive durable task number owning this transcript.
    pub task: u64,
    /// Positive root-issued activation fence for the transcript.
    pub attempt: u64,
    /// Positive accepted turn number; sequence admission belongs to tasks.
    pub turn: u32,
    /// Cumulative accepted spend for this attempt, not an additional charge; represented by `u64`.
    pub spent: u64,
    /// No message fence is admitted in this current route; tasks refuses `Some` before mutation
    /// until an actual inbox route joins.
    pub read: Option<u64>,
    /// Injected wall time when the root accepted the turn; it survives restart without depending on
    /// the process clock.
    pub at: Wall,
    /// Owned transcript bytes, bounded by journal `transcript_bytes` before writing and by load
    /// page budgets when read.
    pub transcript: Box<[u8]>,
}

/// Root's accepted terminal identity (worker offer or actual root-translated
/// unpriced terminal), saved atomically with task and
/// financial writes before ACK; result bytes obey root admission (domain/engine.md, 7.4).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct TerminalRecord {
    /// Root-issued positive durable task identity.
    pub task: u64,
    /// Root-issued positive attempt identity; unchanged on replay.
    pub attempt: u64,
    /// Accepted cumulative expense for a priced worker offer or unchanged expense for an unpriced
    /// root terminal; child counters change once.
    pub cumulative: u64,
    /// Exact bounded worker terminal, even when child lifecycle normalizes it; a refused answer
    /// archives root's unpriced Invalid normalization. A lost claim is `Failed(Lost)` and spends a
    /// try, whether or not a turn was kept.
    pub end: temper_engine_domain_tasks::End,
}

/// Root's latest accepted turn metadata; the full transcript stays only in
/// the immutable turn archive. Fleet fences earlier bodies (domain/engine.md, 7.2).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TurnProof {
    /// Positive consecutive accepted turn, supplied by the worker.
    pub turn: u32,
    /// Accepted cumulative priced spend; tasks owns all financial counters.
    pub cumulative: u64,
    /// Admitted message fence, taken with the turn and charge.
    pub read: Option<u64>,
}

/// Root's current claim evidence, store to root on paged startup. At most
/// tasks.tasks rows are kept; each has one latest turn and one terminal.
/// Claim reserves map room before child mutation and replaces this row. Ending
/// erases it atomically; immutable transcript/terminal archives never enter the
/// live map. Startup validates every row or stops, dropping none silently
/// (domain/engine.md, sections 6 and 7.2).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct RunProof {
    /// Positive task identity allocated by root; unique in the live proof table.
    pub task: u64,
    /// Positive current claim identity allocated by root.
    pub attempt: u64,
    /// Highest inbox message offered to this attempt in an assignment or committed relay.
    pub offered: Option<u64>,
    /// Latest accepted turn, or none before the first turn; no historical body is retained.
    pub turn: Option<TurnProof>,
    /// Typed accepted worker offer or actual root-translated unpriced terminal; present iff the
    /// child answered this current attempt. At most twice task result bytes before normalization;
    /// cleared on replacement/end.
    pub terminal: Option<TerminalRecord>,
}

/// Root-owned immutable first accepted decision for one held-chat revision.
/// Saved atomically with semantic task change and keyed people outcome; bounded
/// reason is never restored into a live archive map.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct EscalationDecisionRecord {
    /// Actual child's project, used to authenticate historical reads rather
    /// than trust the caller's project.
    pub project: u32,
    /// Actual person requester from the accepted bounded semantic context;
    /// current requester/policy-role standing controls replay privacy
    pub requester: u64,
    /// Positive task identity.
    pub task: u64,
    /// Positive checked semantic revision.
    pub revision: u64,
    /// Positive authenticated winning person.
    pub by: u64,
    /// Exact accepted bounded choice; rejection reason fits journal `result_bytes`/`transcript_bytes`
    /// and both child bounds before mutation.
    pub decision: temper_engine_domain_people::EscalationDecision,
}

/// Owned typed row sent root to store in a commit or returned store to root in
/// a bounded page. The transaction/page, not each row, has the store terminal
/// (domain/engine.md, 5.1, 5.3 and 5.6).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Record {
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
        temper_engine_domain_tasks::Stored,
    ),
    /// Child's durable identity/session/keyed reply; saved atomically by the root.
    People(
        /// Owned secret-free child row, deep bytes checked before retention.
        temper_engine_domain_people::Stored,
    ),
}

impl Record {
    /// Pure fixed-size key projection; it emits no request and neither copies payloads nor
    /// validates their bounds.
    #[must_use]
    pub const fn key(&self) -> Key {
        match self {
            Record::Call(row) => Key::Call(row.key),
            Record::EscalationDecision(row) => Key::EscalationDecision { task: row.task, revision: row.revision },
            Record::Deployment(_) => Key::Deployment,
            Record::Turn(row) => Key::Turn { task: row.task, attempt: row.attempt, turn: row.turn },
            Record::RunProof(row) => Key::RunProof { task: row.task },
            Record::Terminal(row) => Key::Terminal { task: row.task, attempt: row.attempt },
            Record::Tasks(row) => Key::Tasks(row.key()),
            Record::People(row) => Key::People(row.key()),
        }
    }
}

/// Root to store: one unique-key operation within an atomic commit. Child
/// saves/erases acquire no separate terminal; the ordered commit answers once
/// (domain/engine.md, 5.1 and 5.6).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
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
pub fn record_bytes(record: &Record) -> Option<u64> {
    match record {
        Record::EscalationDecision(row) => decision_bytes(&row.decision),
        Record::Call(row) => match &row.answer {
            CallAnswer::Unavailable
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
            CallAnswer::DelegationDenied { findings, .. } => u64::try_from(findings.len())
                .ok()?
                .checked_mul(u64::try_from(size_of::<temper_engine_domain_authority::Finding>()).ok()?),
        },
        Record::Deployment(_) => Some(0),
        Record::Turn(turn) => u64::try_from(turn.transcript.len()).ok(),
        Record::RunProof(row) => match &row.terminal {
            Some(terminal) => terminal_bytes(terminal),
            None => Some(0),
        },
        Record::Terminal(row) => terminal_bytes(row),
        Record::Tasks(row) => temper_engine_domain_tasks::stored_bytes(row),
        Record::People(row) => match row {
            temper_engine_domain_people::Stored::Person { identity, .. } => {
                u64::try_from(identity.login.len()).ok()?.checked_add(u64::try_from(identity.name.len()).ok()?)
            }
            temper_engine_domain_people::Stored::Roles { holdings, .. } => u64::try_from(holdings.len())
                .ok()?
                .checked_mul(u64::try_from(size_of::<temper_engine_domain_people::Holding>()).ok()?),
            temper_engine_domain_people::Stored::Answer { ask, .. } => match ask {
                temper_engine_domain_people::Ask::SetRoles { holdings, .. } => u64::try_from(holdings.len())
                    .ok()?
                    .checked_mul(u64::try_from(size_of::<temper_engine_domain_people::Holding>()).ok()?),
                temper_engine_domain_people::Ask::StartChat { words, .. }
                | temper_engine_domain_people::Ask::Say { words, .. } => u64::try_from(words.len()).ok(),
                temper_engine_domain_people::Ask::DecideEscalation { decision, .. } => decision_bytes(decision),
                temper_engine_domain_people::Ask::DecideProposal { decision, .. } => match decision {
                    temper_engine_domain_people::ProposalDecision::Accept
                    | temper_engine_domain_people::ProposalDecision::Pass => Some(0),
                    temper_engine_domain_people::ProposalDecision::Reject { reason } => {
                        u64::try_from(reason.len()).ok()
                    }
                },
            },
            temper_engine_domain_people::Stored::SignIn { .. }
            | temper_engine_domain_people::Stored::ReadPosition { .. } => Some(0),
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
    match &row.end {
        temper_engine_domain_tasks::End::Finished { result, .. } => match result {
            temper_engine_domain_tasks::TaskResult::Report { words }
            | temper_engine_domain_tasks::TaskResult::Verdict { words, .. }
            | temper_engine_domain_tasks::TaskResult::Change { words, .. } => u64::try_from(words.len()).ok(),
            temper_engine_domain_tasks::TaskResult::Failure { reason } => u64::try_from(reason.len()).ok(),
        },
        temper_engine_domain_tasks::End::Parked
        | temper_engine_domain_tasks::End::Failed(_)
        | temper_engine_domain_tasks::End::Refused => Some(0),
    }
}

fn decision_bytes(decision: &temper_engine_domain_people::EscalationDecision) -> Option<u64> {
    match decision {
        temper_engine_domain_people::EscalationDecision::Release
        | temper_engine_domain_people::EscalationDecision::Pass => Some(0),
        temper_engine_domain_people::EscalationDecision::Reject { reason } => u64::try_from(reason.len()).ok(),
    }
}
