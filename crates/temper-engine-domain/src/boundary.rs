//! The engine boundary, wrapping its typed peers (jig's domain/root.md, 2).
use crate::route::{HostMessage, HostRequest, escalation, forge_route, inbox, proposals, results};
use crate::{CallAnswer, CallKey, Delivery, Key, Range, Record, Write};
use alloc::boxed::Box;
use jig_core::{Delegate, HistoricalResult, ProcedureAction, RunCharter};
use jig_core_accounts as accounts;
use jig_core_brief as brief;
use jig_core_fleet as fleet;
use jig_core_people as people;
use jig_core_tasks as tasks;
use jig_core_views as views;
use skein_lib::{List, ReplyTo, Token};
use temper_engine_domain_forge as forge;
use temper_engine_domain_forge_client as forge_client;

/// A complete claim's assignment, root to worker after durability; its
/// brief sections are bounded by brief limits (domain/engine.md, 7.1 and 9).
/// Its attempt ends through the worker answer route; the worker keeps its
/// terminal body until a durable ACK, with channel loss handled by fleet grace.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Assignment {
    /// Root-issued durable task number.
    pub task: u64,
    pub attempt: u64,
    /// Configured charter, never selected by the worker.
    pub charter: u32,
    /// Root-owned charter policy and task result contract for this activation.
    pub run: Box<RunCharter>,
    /// Owned task, attempt and transcript-tail sections, at most brief `sections` and
    /// `brief_bytes`. Task/deployment lineage awaits its root route.
    pub sections: Box<[BriefSection]>,
    /// Whole unread words offered to this attempt, oldest first.
    pub inbox: Box<[tasks::Word]>,
    /// Writable repository tags whose workspace starts at the task's saved-work branch.
    pub saved: Box<[u32]>,
    /// Concrete forge checkouts and their claimed writer branches.
    pub workspace: ForgeWorkspace,
    /// Ordered opaque committed turn bodies of this task, empty for a fresh run.
    pub transcript: Box<[Box<[u8]>]>,
    /// Named answers committed after the task's last accepted turn. The
    /// worker gives these to the new attempt before it can decide new calls.
    pub answered: Box<[crate::CallRecord]>,
    pub settled: Box<[jig_core::SettledCall]>,
    /// Secret-free account grant; token bytes stay in the protocol.
    pub grant: accounts::Grant,
}

/// One root-owned section in a worker assignment (jig's domain/engine.md, section 9).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BriefSection {
    pub kind: BriefKind,
    pub body: BriefBody,
}

/// The core's typed section or a section owned by temper's forge connector.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BriefKind {
    Core(brief::Core),
    Forge(ForgeBriefKind),
}

/// The forge connector's three brief section kinds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ForgeBriefKind {
    Ci,
    Reviews,
    Pull,
}

/// Bytes handed to the worker, or why a section was unavailable.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum BriefBody {
    Text(Box<[u8]>),
    Missing(brief::GatherMissing),
}

/// A checkout's authoritative forge start, prepared afresh by the worker.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ForgeStart {
    Base(Box<[u8]>),
    Branch(Box<[u8]>),
    Saved(Box<[u8]>),
    Merge { branch: Box<[u8]>, base: forge_client::api::Commit },
}

/// One adopted repository translated for the worker's workspace.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ForgeRepository {
    pub tag: u32,
    pub provider: forge_client::api::Repository,
    pub name: Box<[u8]>,
    pub host: Box<[u8]>,
    pub owner: Box<[u8]>,
    pub start: ForgeStart,
    pub push: Option<Box<[u8]>>,
    pub identity: u32,
}

/// Concrete checkout roots and cache ownership for one assignment.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ForgeWorkspace {
    pub key: Box<[u8]>,
    pub repositories: Box<[ForgeRepository]>,
}

/// Worker to root: one numbered transcript and cumulative spend, retained
/// by its sender until the matching ACK (domain/engine.md, 7.2).
#[derive(PartialEq, Eq, Debug)]
pub struct Turn {
    /// Positive consecutive worker turn; fleet fences accepted/pending duplicates before child
    /// admission using the restored kept turn. Root owns durable latest-turn evidence.
    pub number: u32,
    /// Worker-priced cumulative spend for this attempt, not a new delta. Tasks validates
    /// monotonicity and charges the accepted delta atomically.
    pub cumulative: u64,
    /// Last offered inbox message read in this turn; taking joins the transcript and charge commit.
    pub read: Option<u64>,
    /// Owned transcript, bounded by journal transcript bytes before fleet admission.
    pub transcript: Box<[u8]>,
}

/// One typed engine tool request from the current worker claim
/// (domain/engine.md, section 7.3). Later routes extend this vocabulary.
#[derive(PartialEq, Eq, Debug)]
pub enum Tool {
    /// Write or correct one scoped note by the revision this run recalled.
    Note { entry: jig_core_notes::New, recalled: Option<u32> },
    /// Read one bounded page of notes by name or description.
    Recall { by: jig_core_notes::Recall, page: u32 },
    /// Queue one checked forge write with a root-derived idempotency key.
    EffectForge {
        repository: forge_client::api::Repository,
        resource: forge::What,
        write: Box<forge_client::api::Write>,
    },
    /// Read one adopted forge resource through the connector's bounded fresh lane.
    ReadForge { repository: forge_client::api::Repository, read: forge_client::api::Read },
    /// Ask a covering ancestor or person to carry one action the caller cannot take alone.
    Propose { action: ProposedAction, reason: Box<[u8]>, as_holder: bool },
    /// Decide one pending proposal currently addressed to this task.
    Decide { proposer: u64, proposal: u64, decision: ProposalChoice },
    /// Withdraw this task's still-pending proposal.
    Withdraw { proposal: u64 },
    /// Resolve one held delegate currently waiting at this task.
    DecideEscalation { task: u64, revision: u64, decision: EscalationChoice },
    /// Create one authorized batch of direct task delegates.
    Delegate { batch: Box<[Delegate]> },
    /// Amend one live delegate after authority fitting and finite source checks.
    Amend { target: u64, amendment: tasks::Amendment },
    /// Cancel one live delegate's whole subtree with a bounded reason.
    Cancel { target: u64, reason: Box<[u8]> },
    /// Release one held delegate.
    Release { target: u64 },
    /// Send whole bounded words to a task the caller references.
    Message { target: u64, form: MessageForm, words: Box<[u8]> },
    /// Give two tasks referenced by the caller reciprocal references.
    Introduce { left: u64, right: u64 },
    /// Add one bounded task-state or timer interest.
    Subscribe { kind: tasks::SubscriptionKind },
    /// Subscribe this task to typed connector news, with overlap paths for landings.
    SubscribeForge { topic: forge::Topic, own_change: Option<u64>, paths: Box<[Box<[u8]>]> },
    /// Remove one current interest by its root-issued number.
    Unsubscribe { subscription: u64 },
    /// Root-normalized whole-batch shape refusal after bounded ingress.
    Rejected(tasks::Refusal),
    /// Root-normalized message shape refusal before retaining its words.
    RejectedMessage(tasks::Refusal),
    /// Root-normalized control shape refusal before retaining its payload.
    RejectedControl(tasks::Refusal),
    /// Root-normalized proposal shape refusal before retaining its action.
    RejectedProposal(tasks::Refusal),
    /// A note or recall failed bounded ingress.
    RejectedNote(jig_core_notes::Refusal),
    /// The engine records an unavailable answer for a route not yet installed.
    Unavailable,
}

/// Action a worker can submit for an authority holder's decision.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ProposedAction {
    Effect { repository: forge_client::api::Repository, resource: forge::What, write: Box<forge_client::api::Write> },
    Batch(Box<[Delegate]>),
    Amend { task: u64, amendment: tasks::Amendment },
    Widen { task: u64, authority: tasks::Authority },
    Release { task: u64 },
}

/// A task holder's answer to one proposal.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ProposalChoice {
    Accept,
    Reject { reason: Box<[u8]> },
    Pass,
}

/// A task ancestor's decision on one held descendant.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum EscalationChoice {
    Release,
    Reject { reason: Box<[u8]> },
    Pass,
}

/// The sendable portion of the task inbox vocabulary.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MessageForm {
    Words,
    Question,
    Answer { question: u64 },
}

/// A run-named tool call. The root fills the task and attempt from the
/// worker envelope before looking up its durable decision.
#[derive(PartialEq, Eq, Debug)]
pub struct Call {
    pub completion: u32,
    pub position: u32,
    pub tool: Tool,
}

/// External inputs to the walking root; store terminals always enter,
/// worker bodies are bounded before retention (domain/engine.md, 5–7).
/// Web calls own one terminal reply right. Valid current worker bodies are
/// acknowledged after durability or refused for retry; stale worker input may
/// be dropped by fleet. Store and refresh variants are terminals, not new calls.
/// The input union, with one variant for each typed peer.
#[derive(Debug)]
pub enum Event {
    Store(StoreInput),
    Worker(WorkerInput),
    Party(PartyInput),
    Account(accounts::Event),
    Forge(Box<forge::Event>),
    Released(Released),
    Restart,
    /// Iterate offers progress requested by ready owners.
    Resume,
}
#[derive(Debug)]
pub enum StoreInput {
    Committed {
        /// Store-echoed positive commit number, at most the latest issued commit; older cumulative
        /// answers are inert.
        number: u64,
    },
    Uncommitted {
        /// Store-echoed issued commit that failed; failures at/before durable progress or after
        /// stop are inert.
        number: u64,
    },
    Loaded {
        /// Echoed generational load token, fenced after its one terminal and reclaim.
        owner: Token,
        /// Owned decoded rows, at most `loads::Limits::rows` and `loads::Limits::reply_bytes`
        /// before restoration.
        rows: Box<[Record]>,
        /// Exclusive last-row continuation in the requested range, or terminal page.
        next: Option<Key>,
    },
    Unloaded {
        /// Echoed generational load token, fenced after its one terminal and reclaim.
        owner: Token,
    },
}
#[derive(Debug)]
pub enum WorkerInput {
    HostCall {
        channel: Token,
        task: u64,
        attempt: u64,
        call: fleet::Call,
    },
    DecodedHostCall {
        to: ReplyTo,
        body: Call,
    },
    RenderedHostCall {
        to: ReplyTo,
        answer: jig_core::SettledAnswer,
    },
    Inbound {
        task: u64,
        attempt: u64,
        message: HostMessage,
    },
    Call {
        channel: Token,
        task: u64,
        attempt: u64,
        call: Token,
        body: Call,
    },
    Hello {
        /// Protocol-issued opaque channel identity; at most fleet `workers` are retained cold, and
        /// fleet admits the live channel.
        channel: Token,
        /// Worker slot/host/workstream report, bounded by fleet limits before cold retention.
        hello: fleet::Hello,
    },
    Lost {
        /// Protocol-issued identity of the channel that closed; duplicates/unknown
        /// channels consume no queued handoff room.
        channel: Token,
    },
    Turn {
        /// Worker protocol sender; fleet validates it against the current attempt before handing
        /// the owned body to tasks.
        channel: Token,
        /// Positive durable task number owning this turn; fleet treats it as an opaque run
        /// identity.
        task: u64,
        /// Positive root-issued activation number; fleet fences stale worker bodies.
        attempt: u64,
        /// Owned numbered transcript and cumulative charge; the root bounds bytes before payload
        /// retention.
        turn: Turn,
    },
    Answer {
        /// Worker protocol sender checked by fleet against the hosted attempt.
        channel: Token,
        /// Durable task number for the answered attempt; it is not a fresh task allocation.
        task: u64,
        /// Positive root-issued activation number; fleet fences stale worker bodies.
        attempt: u64,
        /// Whole priced spend for this attempt; tasks checks monotonicity and semantic admission
        /// atomically, while root/fleet owns transport replay evidence and fencing.
        cumulative: u64,
        /// Owned task terminal; root retained payload bytes are at most twice tasks `result_bytes`,
        /// then tasks checks result/contract admission against its stricter result bound.
        end: tasks::End,
        /// Full set of repository tags with saved work after this terminal, when the worker made
        /// a save; absent preserves the previous set.
        saved: Option<Box<[u32]>>,
        /// Last heads actually pushed by the worker, indexed by deployment repository tag.
        pushed: Box<[forge::Pushed]>,
    },
}
#[derive(Debug)]
pub enum PartyInput {
    Watch {
        watcher: Token,
        sign_in: u64,
        key: [u8; 16],
        subject: views::Subject,
    },
    Unwatch {
        watcher: Token,
    },
    ViewDelivered {
        watcher: Token,
        done: bool,
    },
    StartRecurring {
        project: u32,
        authority: tasks::Authority,
        template: tasks::RecurringTemplate,
    },
    Period {
        project: u32,
        period: u64,
        budget: u64,
    },
    ProcedureStep {
        task: u64,
        step: u64,
        connector: u16,
        code: u32,
        action: ProcedureAction,
    },
    ReadEscalation {
        /// One web-issued reply right.
        reply_to: ReplyTo,
        /// Root-issued session, checked against both clocks.
        sign_in: u64,
        /// Positive chat task known from Started.
        task: u64,
    },
    SignedIn {
        /// Web-issued right to this one sign-in reply, returned busy immediately or moved through
        /// people to a durable terminal.
        reply_to: ReplyTo,
        /// Protocol-authenticated forge/user key and display bytes, bounded by people
        /// `identity_bytes`.
        identity: people::Identity,
    },
    Ask {
        /// Web-issued right to one keyed request terminal; duplicates can join bounded people
        /// waiters, each still replied to once.
        reply_to: ReplyTo,
        /// Root-issued durable secret-free session number; authenticated and expired by people.
        sign_in: u64,
        /// Web-issued fixed request key, scoped by person and persisted with the exact typed ask.
        key: [u8; 16],
        /// Typed chat request with words bounded by people words before routing.
        ask: people::Ask,
    },
    ReadResult {
        /// Web-issued right moved into one bounded result waiter; consumes one terminal
        /// result/refusal reply.
        reply_to: ReplyTo,
        /// Root-issued durable secret-free session number; authenticated and expired by people.
        sign_in: u64,
        /// Positive ended task number selected by the reader; its stored requester must match the
        /// authenticated person.
        task: u64,
    },
    ReadInbox {
        reply_to: ReplyTo,
        sign_in: u64,
        /// Positive maximum entries, capped by the configured per-person inbox limit.
        most: u32,
    },
    ViewInbox {
        reply_to: ReplyTo,
        sign_in: u64,
        most: u32,
        before: Option<crate::InboxCursor>,
    },
}

/// Root to shell/worker/web. Commit and load requests each have one store
/// terminal; deliveries are notices or consume a `ReplyTo` (domain/engine.md, 5).
/// Every output crosses the journal door in its peer's vocabulary.
#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    Store(StoreRequest),
    Worker(WorkerRequest),
    Party(PartyRequest),
    Forge {
        call: Token,
        repository: forge_client::api::Repository,
        op: forge_client::api::Op,
    },
    Account(
        /// Fixed-size accounts request/notice; token values are filled only by the protocol and
        /// never retained here.
        accounts::Request,
    ),
    ToChild(Released),
    Stop,
}
#[derive(PartialEq, Eq, Debug)]
pub enum StoreRequest {
    Commit {
        /// Positive ordered commit number allocated by the journal, echoed by the store terminal;
        /// checked `u64` exhaustion stops admission.
        number: u64,
        /// Owned unique-key transaction, at most journal writes including its deployment header;
        /// applied atomically.
        writes: Box<[Write]>,
    },
    Load {
        /// Fresh generational load identity issued by root loads; store echoes it once through
        /// `Loaded` or `Unloaded`.
        owner: Token,
        /// Closed store-key family; every page key is checked for membership.
        range: Range,
        /// Exclusive cursor in this range, or its beginning.
        after: Option<Key>,
        /// Positive page demand, at most the configured load row bound.
        most: u32,
        /// Hard decoded-page byte limit, enforced before and after protocol decoding.
        bytes: u32,
    },
}
#[derive(PartialEq, Eq, Debug)]
pub enum WorkerRequest {
    Host(Box<HostRequest>),
    CallBusy {
        channel: Token,
        task: u64,
        attempt: u64,
        call: Token,
    },
    TurnBusy {
        /// Worker protocol destination echoed from its refused turn; a fixed-size identity, without
        /// admitting another worker.
        channel: Token,
        /// Task number echoed from the refused turn; this notice does not validate or allocate the
        /// task.
        task: u64,
        /// Attempt number echoed for the worker to fence the retry; no activation is admitted by
        /// this notice.
        attempt: u64,
        /// Turn number echoed from the refused body, represented by `u32`; worker retains that body
        /// for retry.
        turn: u32,
    },
    AnswerBusy {
        /// Worker protocol destination echoed from the refused terminal; no new worker is retained.
        channel: Token,
        /// Task number echoed from the refused answer; no task or charge is admitted.
        task: u64,
        /// Attempt number echoed for the worker to fence its retained terminal and retry.
        attempt: u64,
    },
    Deliver(Delivery),
}
#[derive(PartialEq, Eq, Debug)]
pub enum PartyRequest {
    View(views::Request),
    WatchRefused { watcher: Token, refusal: people::Refusal },
    Deliver(Delivery),
}

#[derive(Debug)]
pub(crate) enum Work {
    AdoptRestored,
    AdoptDone,
    Restart(jig_core::RestartRequest),
    TypedDecoded { to: ReplyTo, body: Call },
    Core(jig_core::Event),
    Tasks(tasks::Event),
    People(people::Event),
    Fleet(fleet::Event),
    Brief(brief::GatherEvent),
    StartBrief { task: u64 },
    Forge(forge::Event),
    ProjectGoal(Box<jig_core::ProjectionFeed>),
    GoalSubscribe(forge::Subscriber),
    Activate(Box<tasks::RunContext>),
    EscalationLoaded { waiter: Token, rows: Box<[Record]> },
    EscalationFailed { waiter: Token },
    ProposalLoaded { waiter: Token, rows: Box<[Record]> },
    ProposalFailed { waiter: Token },
}

#[derive(Debug)]
pub(crate) struct BriefConnector {
    pub(crate) task: u64,
    pub(crate) kind: ForgeBriefKind,
    pub(crate) cutting: bool,
}

#[derive(Debug)]
pub(crate) struct PreparedWorkspace {
    pub(crate) workspace: forge_route::RunWorkspace,
    pub(crate) claim_names: Box<[forge::Name]>,
}

#[derive(Debug)]
pub(crate) enum Payload {
    Call {
        key: CallKey,
        body: Call,
    },
    CallAnswer(CallAnswer),
    SettledCall(jig_core::SettledCall),
    Message(HostMessage),
    InboxWord(tasks::Word),
    Turn {
        task: u64,
        attempt: u64,
        body: Turn,
    },
    Answer {
        task: u64,
        attempt: u64,
        cumulative: u64,
        end: tasks::End,
        saved: Option<Box<[u32]>>,
        pushed: Box<[forge::Pushed]>,
    },
}

#[derive(Debug)]
pub(crate) enum Read {
    Result(results::Read),
    Inbox(inbox::Read),
    Escalation(escalation::Query),
    Proposal(proposals::Query),
    Transcript { task: u64 },
    Dependency(DependencyRead),
    InputCheck(InputCheck),
    Notes { owner: Token },
}

#[derive(Debug)]
pub(crate) struct InputCheck {
    pub(crate) to: Token,
    pub(crate) key: CallKey,
    pub(crate) batch: Box<[Delegate]>,
    pub(crate) ids: Box<[u64]>,
    pub(crate) at: u32,
    pub(crate) project: u32,
    pub(crate) stubs: List<tasks::Stub>,
}

#[derive(Debug)]
pub(crate) struct DependencyRead {
    pub(crate) task: u64,
    pub(crate) ids: Box<[u64]>,
    pub(crate) at: u32,
    pub(crate) results: List<HistoricalResult>,
}

#[derive(Debug)]
pub(crate) struct ResultPage {
    pub(crate) waiter: Token,
    pub(crate) rows: Box<[Record]>,
    pub(crate) next: Option<Key>,
}

/// A child continuation that has crossed its journal barrier. Its payload is
/// private; iterate may only return it as `Event::Released`.
#[derive(PartialEq, Eq, Debug)]
pub struct Released(pub(crate) Delivery);

/// Put an owner-produced held output in its destination envelope, by variant only.
pub(crate) fn delivery_output(delivery: Delivery) -> Request {
    match delivery {
        delivery @ (Delivery::ForgeCommitted { .. }
        | Delivery::Fleet(_)
        | Delivery::View(_)
        | Delivery::Relay { .. }
        | Delivery::Load { .. }
        | Delivery::ReadEscalationDecision { .. }
        | Delivery::ReadResult { .. }
        | Delivery::BeginInboxView { .. }) => Request::ToChild(Released(delivery)),
        delivery @ (Delivery::Host(_)
        | Delivery::Procedure { .. }
        | Delivery::CallAnswer { .. }
        | Delivery::Inbound { .. }
        | Delivery::AcknowledgeTurn { .. }
        | Delivery::Acknowledge { .. }
        | Delivery::Cancel { .. }
        | Delivery::Assigned { .. }
        | Delivery::Refuse { .. }
        | Delivery::TurnBusy { .. }) => Request::Worker(WorkerRequest::Deliver(delivery)),
        delivery @ (Delivery::InboxPage { .. }
        | Delivery::InboxView { .. }
        | Delivery::EscalationReply { .. }
        | Delivery::Reply { .. }
        | Delivery::WebReply { .. }
        | Delivery::ResultReply { .. }
        | Delivery::Result { .. }) => Request::Party(PartyRequest::Deliver(delivery)),
        Delivery::ForgeCall { call, repository, op } => Request::Forge { call, repository, op },
    }
}
