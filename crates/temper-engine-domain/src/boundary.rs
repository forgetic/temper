//! The engine boundary, wrapping its typed peers (jig's domain/root.md, 2).
use crate::route::{HostRequest, escalation, forge_route, proposals};
use crate::{CallKey, Delivery, Key, Range, Record, Write};
use alloc::boxed::Box;
use jig_domain_accounts as accounts;
use jig_domain_brief as brief;
use jig_domain_core as core;
use jig_domain_core::{Delegate, ProcedureStep, RunCharter};
use jig_domain_fleet as fleet;
use jig_domain_host as host;
use jig_domain_people as people;
use jig_domain_tasks as tasks;
use jig_domain_views as views;
use skein_lib::{List, ReplyTo, Token, Wall};
use temper_engine_domain_forge as forge;

/// A complete claim's assignment, root to worker after durability; its
/// brief sections are bounded by brief limits (domain/root.md, 7.1 and 7.2).
/// The protocol layer encodes it as the hub's assignment, with the forge
/// workspace beside it as temper's application part (jig's
/// `protocol/channel.md`, 6.2).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Assignment {
    /// Root-issued durable task number.
    pub task: u64,
    pub attempt: u64,
    /// The conversation's first attempt, shared by its resumes.
    pub conversation: u64,
    /// The core's ask for live content (jig's `domain/engine.md`, 7.1).
    pub live: host::LiveAsk,
    /// Configured charter, never selected by the worker.
    pub charter: u32,
    /// Root-owned charter policy and task result contract for this activation.
    pub run: Box<RunCharter>,
    /// Owned task, attempt and transcript-tail sections, at most brief `sections` and
    /// `brief_bytes`. Task/deployment lineage awaits its root route.
    pub sections: Box<[BriefSection]>,
    /// Whole unread words offered to this attempt, oldest first, at most the run's inbox.
    pub messages: Box<[tasks::Word]>,
    /// Writable repository tags whose workspace starts at the task's saved-work branch.
    pub saved: Box<[u64]>,
    /// The task holding the workspace, whose runs may reuse it: the hub's workstream.
    pub workstream: u64,
    /// Concrete forge checkouts and their claimed writer branches.
    pub workspace: ForgeWorkspace,
    /// Ordered opaque committed turn bodies of this task, empty for a fresh run.
    pub transcript: Box<[Box<[u8]>]>,
    /// Named answers committed after the task's last accepted turn. The
    /// worker gives these to the new attempt before it can decide new calls.
    pub answered: Box<[crate::CallRecord]>,
    pub settled: Box<[jig_domain_core::SettledCall]>,
    /// Secret-free account and application credential grants; token bytes stay in the protocol.
    pub grants: Box<[accounts::Grant]>,
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

/// The forge connector's brief section kinds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ForgeBriefKind {
    Ci,
    Reviews,
    Pull,
    Home,
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
    Merge { branch: Box<[u8]>, base: skein_forge::api::Commit },
}

/// One adopted repository translated for the worker's workspace.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ForgeRepository {
    pub tag: u64,
    pub provider: skein_forge::api::Repository,
    pub name: Box<[u8]>,
    pub host: Box<[u8]>,
    pub owner: Box<[u8]>,
    pub start: ForgeStart,
    pub push: Option<Box<[u8]>>,
    pub identity: u32,
}

/// Concrete checkout roots for one assignment; the workstream that owns
/// their cache is the hub's (jig's `domain/hosts.md`, 2).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ForgeWorkspace {
    pub repositories: Box<[ForgeRepository]>,
}

/// One typed engine tool request from the current worker claim
/// (domain/root.md, sections 3 and 5.2). Later routes extend this vocabulary.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Tool {
    /// Write or correct one scoped note by the revision this run recalled.
    Note { entry: jig_domain_notes::New, recalled: Option<u32> },
    /// Read one bounded page of notes by name or description.
    Recall { by: jig_domain_notes::Recall, page: u32 },
    /// Queue one checked forge write with a root-derived idempotency key.
    EffectForge { repository: skein_forge::api::Repository, resource: forge::What, write: Box<skein_forge::api::Write> },
    /// Read one adopted forge resource through the connector's bounded fresh lane.
    ReadForge { repository: skein_forge::api::Repository, read: skein_forge::api::Read },
    /// Ask a covering ancestor or person to carry one action the caller cannot take alone.
    Propose { action: ProposedAction, reason: Box<[u8]>, as_holder: bool },
    /// Decide one pending proposal currently addressed to this task.
    Decide { proposer: u64, proposal: u64, decision: ProposalChoice },
    /// Withdraw this task's still-pending proposal.
    Withdraw { proposal: u64 },
    /// Resolve one held delegate currently waiting at this task.
    DecideEscalation { task: u64, revision: u64, decision: EscalationChoice },
    /// Expand a project repository change into one ordinary named delegation.
    Change { words: Box<[u8]>, repository: Option<Box<[u8]>> },
    /// Create one authorized batch of direct task delegates.
    Delegate { batch: Box<[Delegate]> },
    /// Amend one live delegate after authority fitting and finite source checks.
    Amend { target: u64, revision: u64, amendment: tasks::Amendment },
    /// Cancel one live delegate's whole subtree with a bounded reason.
    Cancel { target: u64, reason: Box<[u8]> },
    /// Release one held delegate.
    Release { target: u64, revision: u64, again: Option<bool> },
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
    RejectedNote(jig_domain_notes::Refusal),
    /// The engine records an unavailable answer for a route not yet installed.
    Unavailable,
}

/// Action a worker can submit for an authority holder's decision.
#[derive(Clone, PartialEq, Eq, Debug)]
#[expect(clippy::large_enum_variant, reason = "the worker boundary carries jig's bounded amendment whole")]
pub enum ProposedAction {
    Effect { repository: skein_forge::api::Repository, resource: forge::What, write: Box<skein_forge::api::Write> },
    Batch(Box<[Delegate]>),
    Amend { task: u64, revision: u64, amendment: tasks::Amendment },
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
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Call {
    pub completion: u32,
    pub position: u32,
    pub tool: Tool,
}

/// A setup result contains no credential (protocol/credentials.md, 1).
#[derive(Debug)]
pub enum SetupOutcome {
    Done,
    Manifest { state: Box<[u8]> },
    Install { state: Box<[u8]>, url: Box<[u8]> },
}
#[derive(Debug)]
pub enum SetupEnd {
    Done { forge: u16, outcome: SetupOutcome },
    Stopped { forge: u16, reason: Box<[u8]> },
}
/// External inputs to the walking root (domain/root.md, 3). Admission is
/// `try_step`'s: an input the journal cannot take returns to its owner, and
/// store answers are always taken (jig's `domain/root.md`, 3).
/// The input union, with one variant for each typed peer.
#[derive(Debug)]
pub enum Event {
    /// Terminal of the protocol-owned plan named by `run`; temper-07 consumes it.
    Setup {
        run: Token,
        end: SetupEnd,
    },
    Store(StoreInput),
    Worker(WorkerInput),
    Party(PartyInput),
    Account(accounts::Event),
    /// A generation named by temper's own credential source for one numbered
    /// application credential, a repository's git identity (domain/root.md, 7.1).
    Credential {
        number: u32,
        generation: u64,
        valid: skein_lib::Duration,
    },
    Forge(Box<forge::Event>),
    Released(Released),
    Restart,
    /// Iterate offers progress requested by ready owners.
    Resume,
    /// The service begins its drain (jig's `domain/engine.md`, 6.1).
    Stop,
}
/// Optional protocol renderings in the same order as a transcript load's rows.
pub type TranscriptTexts = Box<[Option<Box<[u8]>>]>;

#[derive(Debug)]
pub enum StoreInput {
    Committed {
        /// Store-echoed positive commit number, at most the latest issued commit; older cumulative
        /// answers are inert.
        number: u64,
        /// The store's map bytes after this commit (jig's `protocol/store.md`, 11.2).
        fill: u64,
    },
    Uncommitted {
        /// Store-echoed issued commit that failed; failures at/before durable progress or after
        /// stop are inert.
        number: u64,
    },
    Loaded {
        /// Protocol-rendered text in row order, only for transcript window loads.
        /// Each text is at most the core tail budget; None marks an unreadable turn.
        texts: Option<TranscriptTexts>,
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
    /// The one terminal of a core read load (jig's `client/domain/contract.md`, 6).
    ReadLoaded { load: Token, result: core::reads::Loaded },
}

/// The three inputs of a worker channel, in the hub's vocabulary (jig's
/// `domain/hosts.md`, 2 and 6.4).
#[derive(Debug)]
pub enum Link {
    Hello {
        worker: u32,
        hello: host::Hello,
    },
    /// One hub record; temper's parts ride beside an answer and are empty otherwise.
    Up {
        up: host::LinkUp,
        parts: Parts,
    },
    Lost,
}

/// Temper's application parts beside a hub answer, named by its work tokens
/// (jig's `protocol/channel.md`, 6.2).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Parts {
    /// Full set of repository tags with saved work after this terminal, when the worker made
    /// a save; absent preserves the previous set.
    pub saved: Option<Box<[u64]>>,
    /// Last heads actually pushed by the worker, indexed by deployment repository tag.
    pub pushed: Box<[forge::Pushed]>,
    /// A permanent failure to prepare, naming its repository by tag.
    pub preparation: Option<Preparation>,
}

impl Parts {
    /// A record that carries no application part.
    #[must_use]
    pub fn none() -> Parts {
        Parts { saved: None, pushed: Box::new([]), preparation: None }
    }
}

/// Why a repository could not be prepared, permanently (protocol/channel.md, 5).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Preparation {
    /// The forge lacks what the assignment names for this repository.
    Missing { tag: u64, missing: Missing },
    /// The forge refused this repository's identity.
    Refused { tag: u64 },
}

/// What the forge lacks for a preparation (protocol/channel.md, 5).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Missing {
    Repository,
    Branch,
    Commit,
}

/// An observation of the root, which decides nothing (domain/root.md, 3 and 7.4).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Fact {
    /// A fenced answer reported a permanent preparation failure.
    PreparationFailed { task: u64, attempt: u64, preparation: Preparation },
    /// The core refused a procedure's step; the root decides nothing (domain/root.md, 3 and 7.4).
    StepRefused { task: u64, step: u64, problem: jig_domain_tasks::Problem },
}

#[derive(Debug)]
#[expect(clippy::large_enum_variant, reason = "decoded calls stay inline in the bounded input queue priced at startup")]
pub enum WorkerInput {
    /// A worker channel's hello, record or loss.
    Link { channel: Token, link: Link },
    /// The protocol decoded a relayed call into the engine's tool vocabulary.
    DecodedHostCall { to: ReplyTo, body: Call },
    /// The protocol rendered a decided call's answer for its host.
    RenderedHostCall { to: ReplyTo, answer: jig_domain_core::SettledAnswer },
    /// The protocol read an ended answer's opaque outcome after the fleet fenced it.
    ResultRead { payload: Token, end: tasks::End },
}
/// A keyed request owned by the root, or forwarded to the forge (client/README.md, 8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Application {
    Forge(forge::Ask),
    Return { source: u32, path: Box<[u8]> },
}
/// The family's terminal answer; root returns are volatile, like forge setup answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApplicationAnswer {
    Adopt(forge::ApplicationOutcome),
    SetUp(forge::SetUpAnswer),
    StartDemo(forge::StartAnswer),
    Connect(forge::ConnectAnswer),
    Install(forge::InstallAnswer),
    Reachable(forge::ReachableAnswer),
    Returning,
}
#[derive(Debug)]
pub enum PartyInput {
    /// Resolve a sign-in through the people child for provider authorization.
    Whose {
        reply_to: ReplyTo,
        sign_in: u64,
    },
    /// A direct owner settings change, decided by jig.
    ChangeSettings {
        reply_to: ReplyTo,
        sign_in: u64,
        key: [u8; 16],
        digest: [u8; 32],
        change: Box<core::settings::Change>,
    },
    /// End one authenticated sign-in through the people child (ruling 116).
    SignOut {
        reply_to: ReplyTo,
        sign_in: u64,
    },
    Application {
        scope: people::ApplicationScope,
        reply_to: ReplyTo,
        sign_in: u64,
        key: [u8; 16],
        digest: [u8; 32],
        request: Application,
        /// The number and digest of a value the request carried, already kept
        /// in the secrets' store (jig's `protocol/client.md`, 5).
        carried: Option<core::Carried>,
    },
    StartRecurring {
        project: u32,
        authority: tasks::Authority,
        template: tasks::RecurringTemplate,
    },
    /// A project's period begins, from outside the root (jig's
    /// `protocol/configuration.md`, 5).
    Period {
        project: u32,
        period: u64,
        budget: u64,
        next: Option<Wall>,
    },
    ProcedureStep {
        task: u64,
        step: u64,
        connector: u16,
        code: u32,
        decision: ProcedureStep,
    },
    SignedIn {
        /// Authenticated sign-in offered beside the printed link.
        held: Option<u64>,
        /// Web-issued right to this one sign-in reply, moved through people to a durable terminal.
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
        models: Box<[Box<[u8]>]>,
    },
    /// A keyed account ask whose value is already kept in the secrets store.
    Key {
        reply_to: ReplyTo,
        sign_in: u64,
        key: [u8; 16],
        ask: Box<people::Ask>,
        carried: core::Carried,
    },
    /// One unkeyed read on a live sign-in, answered after every preceding commit.
    Read {
        reply_to: ReplyTo,
        sign_in: u64,
        read: core::reads::Read,
    },
    /// Watch one subject's whole document on a live sign-in.
    /// A forge panel uses `Watched::Application` in its project's scope, the
    /// configured forge connector, and the change task's number as its token
    /// (domain/root.md, 5.1; client/forge.md, 9.2). The core admits its project role.
    Watch {
        watcher: Token,
        sign_in: u64,
        subject: core::documents::Watched,
        window: core::documents::Window,
    },
    /// Follow a task's live run from the client's acknowledged turn and offset.
    WatchRun {
        watcher: Token,
        sign_in: u64,
        task: u64,
        turn: Option<views::TurnName>,
        offset: u32,
    },
    Unwatch {
        watcher: Token,
    },
    /// The protocol took a watch's held document.
    Delivered {
        watcher: Token,
    },
    /// The protocol took one provisional stream delivery.
    StreamDelivered {
        watcher: Token,
    },
}

/// Root to shell/worker/web. Commit and load requests each have one store
/// terminal; deliveries are notices or consume a `ReplyTo` (domain/root.md, 3
/// and 4). Every output crosses the journal door in its peer's vocabulary.
#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    Bound {
        request: Token,
        answer: forge::BindAnswer,
    },
    /// Stop the ended demo's box group; no request waits for its answer.
    StopGroup {
        forge: u16,
    },
    /// Start the demo's box group.
    SetUpGroup {
        forge: u16,
        request: Token,
    },
    /// Give this sign-in its checked return path (client/README.md, 8).
    Return {
        source: u32,
        path: Box<[u8]>,
    },
    /// Routed to the protocol layer's box link; nothing committed (domain/root.md, 5.1).
    SetUp {
        forge: u16,
        request: Token,
        org: Box<[u8]>,
        repository: Box<[u8]>,
    },
    Connect {
        forge: u16,
        request: Token,
        how: forge::How,
        carried: jig_domain_core::Carried,
    },
    Manifest {
        forge: u16,
        request: Token,
        back: Option<Box<[u8]>>,
    },
    Reachable {
        forge: u16,
        request: Token,
        installation: Option<u64>,
        page: u32,
    },
    Install {
        forge: u16,
        request: Token,
        back: Option<Box<[u8]>>,
    },
    EraseCarried {
        number: u64,
    },
    /// Run the staged plan named by this token; credential values stay in routing.
    Setup {
        run: Token,
    },
    Store(StoreRequest),
    Worker(WorkerRequest),
    Party(PartyRequest),
    Forge {
        call: Token,
        repository: skein_forge::api::Repository,
        op: skein_forge::api::Op,
    },
    Account(
        /// Fixed-size accounts request/notice; token values are filled only by the protocol and
        /// never retained here.
        accounts::Request,
    ),
    ToChild(Released),
    /// A forge input the drain does not decide, returned to its owner (jig's
    /// `domain/engine.md`, 6.1).
    Busy(Box<forge::Event>),
    /// Every commit before the drain is durable (jig's `domain/engine.md`, 6.1).
    Drained,
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
    /// One core read load after every preceding commit, answered by `ReadLoaded`.
    Read { load: Token, range: Box<core::reads::Range> },
}
#[derive(PartialEq, Eq, Debug)]
pub enum WorkerRequest {
    Host(Box<HostRequest>),
    /// Read an ended answer's opaque outcome in the protocol routing.
    ReadResult {
        payload: Token,
        outcome: Box<[u8]>,
    },
    Deliver(Delivery),
}
#[derive(PartialEq, Eq, Debug)]
pub enum PartyRequest {
    /// A sign-in the core refused before any decision: a counter is
    /// exhausted, or the store is at its fill mark.
    SignInRefused {
        to: ReplyTo,
    },
    /// A watch refused before any durable decision.
    WatchRefused {
        watcher: Token,
        why: core::WatchRefusal,
    },
    /// A run's provisional suffix for one follower, with no commit.
    RunText {
        watcher: Token,
        turn: views::TurnName,
        from: u32,
        to: u32,
        text: Box<[u8]>,
        cut: bool,
        call: Option<views::Call>,
    },
    Deliver(Delivery),
}

#[derive(Debug)]
pub(crate) enum Work {
    AdoptRestored,
    AdoptDone,
    Restart(jig_domain_core::RestartRequest),
    Decoded { to: ReplyTo, body: Call },
    Core(jig_domain_core::Event),
    Tasks(tasks::Event),
    People(people::Event),
    Fleet(fleet::Event),
    Brief(brief::GatherEvent),
    StartBrief { task: u64 },
    Forge(forge::Event),
    ProjectGoal(Box<jig_domain_core::ProjectionFeed>),
    GoalSubscribe(forge::Subscriber),
    Activate(Box<tasks::RunContext>),
    EscalationLoaded { waiter: Token, rows: Box<[Record]> },
    EscalationFailed { waiter: Token },
    ProposalLoaded { waiter: Token, rows: Box<[Record]> },
    ProposalFailed { waiter: Token },
    TaskProposalLoaded { waiter: Token, rows: Option<Box<[Record]>> },
    EndedLoaded { waiter: Token, rows: Box<[Record]> },
    EndedFailed { waiter: Token },
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

/// Bodies the fleet names by token while it routes them (jig's `domain/hosts.md`, 6.4).
#[derive(Debug)]
#[expect(clippy::large_enum_variant, reason = "bounded call ownership moves whole through the fleet handoff")]
pub(crate) enum Payload {
    /// The drain's answer to a relayed call.
    Busy,
    Call {
        key: CallKey,
        body: Call,
    },
    SettledCall(jig_domain_core::SettledCall),
    InboxWord(tasks::Word),
    Turn {
        task: u64,
        attempt: u64,
        turn: u32,
        cumulative: u64,
        read: Option<u64>,
        transcript: Box<[u8]>,
        opens_window: bool,
        refused: Box<[u64]>,
    },
    Answer {
        task: u64,
        attempt: u64,
        cumulative: u64,
        ending: host::Ending,
        read: Option<u64>,
        refused: Box<[u64]>,
        parts: Parts,
    },
}

/// An ended owner row the core asked the root to load.
#[derive(Debug)]
pub(crate) enum EndedRead {
    EndedTask { request: Token, task: u64 },
    SeenConversation { request: Token, task: u64 },
    Position { request: Token, task: u64, party: u64 },
    PartyInput { request: Token, task: u64 },
    PersonTask { request: Token, person: u64, project: u32, task: u64 },
}

#[derive(Debug)]
pub(crate) enum Read {
    Escalation(escalation::Query),
    Proposal(proposals::Query),
    TaskProposal { to: ReplyTo, key: CallKey, proposal: u64 },
    Ended(EndedRead),
    Transcript { task: u64 },
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

/// A child continuation that has crossed its journal barrier. Its payload is
/// private; iterate may only return it as `Event::Released`.
#[derive(PartialEq, Eq, Debug)]
pub struct Released(pub(crate) Delivery);

/// Put an owner-produced held output in its destination envelope, by variant only.
pub(crate) fn delivery_output(delivery: Delivery) -> Request {
    match delivery {
        delivery @ (Delivery::ProjectMade { .. }
        | Delivery::DemoActed { .. }
        | Delivery::ForgeCommitted { .. }
        | Delivery::Fleet(_)
        | Delivery::Relay { .. }
        | Delivery::Load { .. }
        | Delivery::ReadEscalationDecision { .. }) => Request::ToChild(Released(delivery)),
        delivery @ (Delivery::Host(_)
        | Delivery::Link { .. }
        | Delivery::Procedure { .. }
        | Delivery::Assigned { .. }
        | Delivery::Refuse { .. }
        | Delivery::TurnBusy { .. }) => Request::Worker(WorkerRequest::Deliver(delivery)),
        delivery @ (Delivery::DemoNote { .. }
        | Delivery::Forges { .. }
        | Delivery::Panel { .. }
        | Delivery::Application { .. }
        | Delivery::WebReply { .. }
        | Delivery::CarriedUnused { .. }
        | Delivery::Read { .. }
        | Delivery::Document { .. }
        | Delivery::WatchEnded { .. }
        | Delivery::SourceSignedOut { .. }
        | Delivery::Result { .. }) => Request::Party(PartyRequest::Deliver(delivery)),
        Delivery::ReadLoad { load, range } => Request::Store(StoreRequest::Read { load, range }),
        Delivery::Drained => Request::Drained,
        Delivery::Bound { request, answer } => Request::Bound { request, answer },
        Delivery::ForgeCall { call, repository, op } => Request::Forge { call, repository, op },
    }
}
