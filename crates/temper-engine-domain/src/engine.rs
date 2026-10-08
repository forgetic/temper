//! The concrete 06a root: bounded child ownership and synchronous decision
//! routing for a charged person chat (domain/engine.md, sections 3–7).
//! [`Domain`] keeps task/funding state in tasks, policy in authority, identities
//! and keyed requests in people, placement in fleet, task text gathering in
//! brief and secret-free credential lifetimes in accounts. Its own state is
//! commits, loads, candidate numbers, bounded current-claim replay evidence and
//! unfinished handoffs between children.
//!
//! The shell/protocol supplies [`Event`]s to [`step`], drains durable effects
//! through [`release`], fires child timers through [`fire`] and reclaims at the
//! iteration boundary. Every writing route closes into one atomic commit;
//! store terminals are accepted under pressure and a failed commit stops
//! release. Startup reads the header first, pages child state and current root
//! proofs, validates their identities/expense/terminal correlation, then permits
//! child restoration consequences. It adopts every restored claim before fleet
//! `Loaded` allows new placement (domain/engine.md, section 6).
//!
//! The root never knows file descriptors, wire encodings, kernel races,
//! repository contents or secret credential bytes (domain/engine.md, 2 and 5.5).
//! Its closed input vocabulary has no blanket child-event pass-through.
//! Named call replay is rooted here; individual tool routes, connectors and
//! broader people routes follow in later increments.
//! Actual keyed role administration preflights Waiting recipients, then commits
//! membership, semantic rerouting and keyed completion together without IO.
//! Root candidate/snapshot carriers are transient.
//! Child facts are disposable observations; [`Domain::quiescent`] reports
//! internal idleness, while an external referee establishes final story results.
mod escalation;
mod forge_route;
mod inbox;
mod landing;
mod policy_translate;
mod proposals;
mod results;

use crate::{
    CallAnswer, CallKey, Decision, Delivery, Family, Journal, JournalLimits, Key, Output, Range, Record, RunProof,
    Write, loads,
};
use alloc::boxed::Box;
use jig_core::{
    Core, HistoricalResult, PendingRelay, PersonProposalRoute, PersonTaskRoute, RestoringProof, RoutedCall, Transcript,
};
pub use jig_core::{Delegate, Dependency, Model, ProcedureAction, RunCharter, RunPolicy};
use jig_core_accounts as accounts;
use jig_core_authority as authority;
use jig_core_brief as brief;
use jig_core_fleet as fleet;
use jig_core_notes as notes;
use jig_core_people as people;
use jig_core_tasks as tasks;
use jig_core_views as views;
use skein_lib::{Env, Id, List, Map, Queue, ReplyTo, Slab, Token};
use temper_engine_domain_forge as forge;
use temper_engine_domain_forge_change as forge_change;
use temper_engine_domain_forge_client as forge_client;
use temper_engine_domain_forge_issues as forge_issues;

pub use forge_route::adopt_repository_ask;
pub use landing::{Approval, Freshness, Gate, LandingRule};

/// Root startup bounds, supplied by configuration and immutable at every step
/// (domain/engine.md, 4–5). `worst_case` checks the cross-child route, page,
/// journal and payload capacities before `Domain::new` allocates fixed room.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Limits {
    /// Ordered commit/write/delivery room, including all synchronous child routes.
    pub journal: JournalLimits,
    /// Paged store reads; every startup page must fit whole.
    pub loads: loads::Limits,
    /// Authentic task/funding room, also bounding root current-claim proof slots; transport
    /// evidence belongs to root.
    pub tasks: tasks::Limits,
    /// Deployment/project policy and finding room, supplied at startup.
    pub authority: authority::Limits,
    /// Identity, session and keyed request room.
    pub people: people::Limits,
    /// Worker attempts and unacknowledged turns.
    pub fleet: fleet::Limits,
    /// Maximum named decisions awaiting a turn or task end across live tasks.
    pub call_records: u32,
    /// Required task section gathering and cuts.
    pub brief: BriefLimits,
    /// Secret-free credential policy.
    pub accounts: accounts::Limits,
    /// Live watches, backlogs and expendable trace room.
    pub views: views::Limits,
    /// Bounded notes indexes and pages kept by the core.
    pub notes: notes::Limits,
    /// Forge connector subtree and its bounded outbox.
    pub forge: forge::Limits,
}

/// Root brief configuration, including temper's source and forge budgets.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BriefLimits {
    pub briefs: u32,
    pub sections: u32,
    pub parts: u32,
    pub read_bytes: u32,
    pub budgets: BriefBudgets,
    pub brief_bytes: u32,
    pub gather: skein_lib::Duration,
}

/// Byte ceilings for the root's core sections and its forge connector.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BriefBudgets {
    pub task: u32,
    pub dependencies: u32,
    pub ci: u32,
    pub reviews: u32,
    pub pull: u32,
    pub attempts: u32,
    pub plan: u32,
}

/// Startup configuration owned by the root, bounded by the corresponding
/// child limits; the shell supplies it once (domain/engine.md, 4).
#[derive(Debug)]
pub struct Config {
    /// Identity committed at the first start; ignored when a header exists.
    pub deployment: [u8; 16],
    pub seed: u64,
    /// Bootstrap project/identity owners supplied by deployment configuration, at most people
    /// `initial_owners`; sign-in authentication matches their identity keys rather than trusting a
    /// request's role.
    pub owners: Box<[people::InitialOwner]>,
    /// Provider number reserved for services made by this deployment; forge sign-in uses 0.
    pub deployment_provider: u16,
    /// Validated deployment and project policy; no duplicated policy values.
    pub authority: authority::Domain,
    /// Forge landing requirements and their connector-owned judge parameters.
    pub landing: LandingPolicy,
    /// Project policy mappings from connector permissions to roles at adoption.
    pub permission_roles: Map<u32, Box<[people::PermissionRole]>>,
    /// Connector number assigned to this application's forge adapter.
    pub forge_connector: u16,
    /// Procedure namespace assigned to the root's recurring goal executor.
    pub recurring_connector: u16,
    /// Charter selected for chats; admitted by tasks as the configured executor.
    pub charter: u32,
    /// Smith-neutral run policy selected for this charter. The root owns the
    /// policy; its typed adapter supplies Smith's vocabulary at the boundary.
    pub run: RunPolicy,
    /// Maximum committed conversation bytes across a task's attempts that may be resumed whole.
    /// A longer transcript starts the next run fresh with its bounded tail in the brief.
    pub resume_bytes: u32,
    /// Finite period number used to address project/person funding ledgers; restoring an existing
    /// ledger never resets it.
    pub period: u64,
    /// Initial project period budget, checked against current project policy before its first
    /// opening; restored funding wins.
    pub period_budget: u64,
    /// Initial person pool budget, checked against the authenticated role's period ceiling before
    /// carving; restored funding wins.
    pub person_budget: u64,
    /// Exact task authority checked for each authenticated chat.
    pub chat_authority: authority::Authority,
    /// Account required by the chat charter, configured through the accounts child.
    pub account: u32,
    /// Existing secret-free generation at startup.
    pub account_generation: u64,
    /// Startup credential lifetime; a refresh is required when absent.
    pub account_valid: Option<skein_lib::Duration>,
    /// Forge writer identities and this deployment's branch namespace.
    pub forge: forge_client::Config,
}

#[derive(Debug)]
struct RootConfig {
    landing: LandingPolicy,
    forge_connector: u16,
}

fn split_config(config: Config, projects: List<u32>) -> (jig_core::Config, RootConfig) {
    let Config {
        deployment,
        seed,
        owners,
        deployment_provider,
        authority,
        landing,
        permission_roles,
        forge_connector,
        recurring_connector,
        charter,
        run,
        resume_bytes,
        period,
        period_budget,
        person_budget,
        chat_authority,
        account,
        account_generation,
        account_valid,
        forge: _,
    } = config;
    (
        jig_core::Config {
            deployment,
            seed,
            owners,
            authority,
            projects,
            permission_roles,
            connectors: Box::new([forge_connector]),
            settings: jig_core::Settings {
                deployment_provider,
                recurring_connector,
                charter,
                run,
                resume_bytes,
                period,
                period_budget,
                person_budget,
                chat_authority,
                account,
                account_generation,
                account_valid,
            },
        },
        RootConfig { landing, forge_connector },
    )
}

/// Typed forge landing policy retained by the application for policy snapshots.
#[derive(Debug)]
pub struct LandingPolicy {
    pub deployment: Box<[LandingRule]>,
    pub projects: Map<u32, Box<[LandingRule]>>,
}

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
    /// The engine records an unavailable answer for a route not yet installed.
    Unavailable,
}

/// Action a worker can submit for an authority holder's decision.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ProposedAction {
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
#[derive(Debug)]
pub enum Event {
    /// One bounded provider call terminal routed to its connector.
    ForgeAnswered { call: Token, cost: u32, result: Result<forge_client::api::Answer, forge_client::api::Error> },
    /// An untrusted provider hint; the connector reads current facts.
    ForgeHint { hint: forge_client::api::Hint },
    /// An authenticated person opens one live run, task-tree or project-goal watch.
    Watch { watcher: Token, sign_in: u64, key: [u8; 16], subject: views::Subject },
    /// A person or stream stops one watch.
    Unwatch { watcher: Token },
    /// One watcher delivery was consumed or dropped by its stream.
    ViewDelivered { watcher: Token, done: bool },
    /// Deployment configuration starts one durable core recurring procedure.
    StartRecurring { project: u32, authority: tasks::Authority, template: tasks::RecurringTemplate },
    /// Deployment configuration opens a newer period and wakes its recurring procedures.
    Period { project: u32, period: u64, budget: u64 },
    /// Connector to root: one fenced procedure step, after a committed step request.
    ProcedureStep { task: u64, step: u64, connector: u16, code: u32, action: ProcedureAction },
    /// Worker host call, validated by fleet and decided once by the root.
    Call { channel: Token, task: u64, attempt: u64, call: Token, body: Call },
    /// Authenticated named held-chat read; one bounded view or refusal terminal,
    /// without persistent inbox/history restore.
    ReadEscalation {
        /// One web-issued reply right.
        reply_to: ReplyTo,
        /// Root-issued session, checked against both clocks.
        sign_in: u64,
        /// Positive chat task known from Started.
        task: u64,
    },
    /// Shell to root: begin cold paged restoration and account setup. Repeated starts are inert;
    /// readiness follows all restored/adopted claims, or a terminal startup/storage failure emits
    /// `Stop`.
    Start,
    /// Store's cumulative successful terminal for issued commits.
    Committed {
        /// Store-echoed positive commit number, at most the latest issued commit; older cumulative
        /// answers are inert.
        number: u64,
    },
    /// Store's failed terminal; root stops without releasing held work.
    Uncommitted {
        /// Store-echoed issued commit that failed; failures at/before durable progress or after
        /// stop are inert.
        number: u64,
    },
    /// Store's one page terminal for an issued load; decoded bounds are rechecked.
    Loaded {
        /// Echoed generational load token, fenced after its one terminal and reclaim.
        owner: Token,
        /// Owned decoded rows, at most `loads::Limits::rows` and `loads::Limits::reply_bytes`
        /// before restoration.
        rows: Box<[Record]>,
        /// Exclusive last-row continuation in the requested range, or terminal page.
        next: Option<Key>,
    },
    /// Store's failed terminal for a page.
    Unloaded {
        /// Echoed generational load token, fenced after its one terminal and reclaim.
        owner: Token,
    },
    /// Web supplies authenticated identity; root allocates person and sign-in candidates.
    SignedIn {
        /// Web-issued right to this one sign-in reply, returned busy immediately or moved through
        /// people to a durable terminal.
        reply_to: ReplyTo,
        /// Protocol-authenticated forge/user key and display bytes, bounded by people
        /// `identity_bytes`.
        identity: people::Identity,
    },
    /// Web's keyed chat request; the people child authenticates session and role.
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
    /// Worker first reports its bounded slots and hosted attempts.
    Hello {
        /// Protocol-issued opaque channel identity; at most fleet `workers` are retained cold, and
        /// fleet admits the live channel.
        channel: Token,
        /// Worker slot/host/workstream report, bounded by fleet limits before cold retention.
        hello: fleet::Hello,
    },
    /// Protocol to root: channel closed. Known cold losses coalesce; unknown losses are inert. Live
    /// loss changes fleet topology/deadlines immediately without a commit or outward terminal.
    Lost {
        /// Protocol-issued identity of the channel that closed; duplicates/unknown
        /// channels consume no queued handoff room.
        channel: Token,
    },
    /// Worker to root: numbered body for its current claim. A valid admitted turn ends in a durable
    /// ACK; pressure is a retry notice and stale bodies may be dropped.
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
    /// Worker to root: attempt terminal retained until ACK. Accepted charge and task state commit
    /// first; pressure asks for retry. A refused charged terminal becomes an uncharged invalid
    /// activation before its durable ACK.
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
    },
    /// Web to root: read a named historical result. People validates the session; the loaded ended
    /// task must name its person as requester. Ends once with `ResultReply` or a refused
    /// `WebReply`.
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
    /// Authenticated bounded page of unread results, consumed in committed result order.
    ReadInbox {
        reply_to: ReplyTo,
        sign_in: u64,
        /// Positive maximum entries, capped by the configured per-person inbox limit.
        most: u32,
    },
    /// Authenticated newest-first page of live waiting work and unread results.
    ViewInbox { reply_to: ReplyTo, sign_in: u64, most: u32, before: Option<crate::InboxCursor> },
    /// Account protocol completes a refresh with secret-free lifetime.
    Refreshed {
        /// Configured secret-free account number; bounded by accounts account room.
        account: u32,
        /// Echoed account refresh generation; stale completions change nothing.
        generation: u64,
        /// Remaining credential lifetime, represented by a u64 duration; no token bytes cross the
        /// domain.
        valid: skein_lib::Duration,
    },
    /// Account protocol ends its refresh unsuccessfully.
    RefreshFailed {
        /// Configured secret-free account number; bounded by accounts account room.
        account: u32,
        /// Echoed account refresh generation; stale completions change nothing.
        generation: u64,
        /// Terminal account failure class, including retry delay where required.
        failure: accounts::Failure,
    },
}

/// Root to shell/worker/web. Commit and load requests each have one store
/// terminal; deliveries are notices or consume a `ReplyTo` (domain/engine.md, 5).
#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    /// One bounded forge API call after the decision it follows.
    Forge { call: Token, repository: forge_client::api::Repository, op: forge_client::api::Op },
    /// Live view/watch request routed to the shell.
    View(views::Request),
    /// A watch failed authentication or project standing before views admission.
    WatchRefused { watcher: Token, refusal: people::Refusal },
    /// Root had no decision or payload room; worker retries the same name.
    CallBusy { channel: Token, task: u64, attempt: u64, call: Token },
    /// Ordered atomic transaction; store ends with Committed or Uncommitted.
    Commit {
        /// Positive ordered commit number allocated by the journal, echoed by the store terminal;
        /// checked `u64` exhaustion stops admission.
        number: u64,
        /// Owned unique-key transaction, at most journal writes including its deployment header;
        /// applied atomically.
        writes: Box<[Write]>,
    },
    /// Paged read, issued only after its prerequisite commits are durable.
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
    /// Held durable outward delivery; internal fleet callbacks never reach the shell.
    Deliver(
        /// Owned bounded effect, released once after the commit it follows.
        Delivery,
    ),
    /// Root to account protocol: refresh/keep actions or secret-free notices. Refresh/keep ends
    /// through `Refreshed` or `RefreshFailed`; grant and availability notices require no root
    /// terminal.
    Account(
        /// Fixed-size accounts request/notice; token values are filled only by the protocol and
        /// never retained here.
        accounts::Request,
    ),
    /// Retry notice for a turn that could not enter a decision; worker retains body.
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
    /// Root to worker: answer could not enter a decision; retain and retry it after backoff,
    /// without charging.
    AnswerBusy {
        /// Worker protocol destination echoed from the refused terminal; no new worker is retained.
        channel: Token,
        /// Task number echoed from the refused answer; no task or charge is admitted.
        task: u64,
        /// Attempt number echoed for the worker to fence its retained terminal and retry.
        attempt: u64,
    },
    /// Root to shell: failed storage/startup requires stopping this process; no terminal is owed to
    /// this notice and held effects remain unreleased.
    Stop,
}

#[derive(Debug)]
enum Work {
    Core(jig_core::Event),
    Tasks(tasks::Event),
    People(people::Event),
    Fleet(fleet::Event),
    Brief(brief::GatherEvent),
    StartBrief { task: u64 },
    Forge(forge::Event),
    ProjectGoal(Box<tasks::TaskRecord>),
    GoalSubscribe(forge::Subscriber),
    Activate(Box<tasks::RunContext>),
    EscalationLoaded { waiter: Token, rows: Box<[Record]> },
    EscalationFailed { waiter: Token },
    ProposalLoaded { waiter: Token, rows: Box<[Record]> },
    ProposalFailed { waiter: Token },
}

#[derive(Debug)]
struct BriefConnector {
    task: u64,
    kind: ForgeBriefKind,
    cutting: bool,
}

#[derive(Debug)]
struct PreparedWorkspace {
    workspace: forge_route::RunWorkspace,
    claim_names: Box<[forge::Name]>,
}

#[derive(Debug)]
#[expect(clippy::large_enum_variant, reason = "the bounded whole call remains with its durable decision payload")]
enum Payload {
    Call { key: CallKey, body: Call },
    CallAnswer(CallAnswer),
    Turn { task: u64, attempt: u64, body: Turn },
    Answer { task: u64, attempt: u64, cumulative: u64, end: tasks::End, saved: Option<Box<[u32]>> },
}

#[derive(Debug)]
enum Read {
    Result(results::Read),
    Inbox(inbox::Read),
    Escalation(escalation::Query),
    Proposal(proposals::Query),
    Transcript { task: u64 },
    Dependency(DependencyRead),
    InputCheck(InputCheck),
}

#[derive(Debug)]
struct InputCheck {
    to: Token,
    key: CallKey,
    batch: Box<[Delegate]>,
    ids: Box<[u64]>,
    at: u32,
    project: u32,
    stubs: List<tasks::Stub>,
}

#[derive(Debug)]
struct DependencyRead {
    task: u64,
    ids: Box<[u64]>,
    at: u32,
    results: List<HistoricalResult>,
}

#[derive(Debug)]
struct ResultPage {
    waiter: Token,
    rows: Box<[Record]>,
    next: Option<Key>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Startup {
    Cold,
    Loading(Range),
    Adopting(jig_core::connector::RestartStage),
    Running,
    Failed,
}

/// Root owns every participating child and all unfinished handoffs; no child
/// effect bypasses its decision journal. Bounded activation contexts replace
/// raw task peeks; current proofs are root-owned and authentic financial state
/// stays in tasks (domain/engine.md, sections 3–5).
#[derive(Debug)]
pub struct Domain {
    limits: Limits,
    config: RootConfig,
    core: Core,
    journal: Journal,
    stop_pending: bool,
    door_pass: bool,
    door_count: u32,
    startup: Startup,
    brief_connectors: Slab<BriefConnector>,
    brief_sections: Map<u64, Box<[BriefSection]>>,
    run_workspaces: Map<u64, PreparedWorkspace>,
    forge: forge::Domain,
    forge_keys: Map<forge::Key, u64>,
    adoption_restore: Map<Token, Option<forge::Repository>>,
    forge_subscribing: Map<Token, forge::Subscriber>,
    forge_unsubscribing: Map<Token, (u64, forge::Topic)>,
    forge_reading: Map<Token, (ReplyTo, CallKey)>,
    forge_effecting: Map<u64, (ReplyTo, CallKey)>,
    forge_projection_due: Map<u64, skein_lib::Wall>,
    forge_change_due: Map<u64, skein_lib::Wall>,
    forge_delegating: Map<(u64, u64), forge_change::Delegate>,
    loads: loads::Loads,
    assignments: Map<u64, Assignment>,
    payloads: Slab<Option<Payload>>,
    result_reads: Slab<Option<Read>>,
    result_pages: Queue<ResultPage>,
    connector_calls: Map<CallKey, CallAnswer>,
    work: Queue<Work>,
    before_header: Queue<Work>,
    cold_channels: Map<Token, bool>,
}

impl Domain {
    /// Allocate the root and fixed child room from shell-supplied configuration and validated
    /// cross-child limits. Checks bounded authority/bootstrap configuration; issues no request.
    /// Startup pages/account setup begin only on `Event::Start`.
    #[must_use]
    #[expect(clippy::too_many_lines, reason = "the root allocates every child and bounded handoff table together")]
    pub fn new(mut config: Config, limits: &Limits) -> Domain {
        assert!(worst_case(limits).is_some(), "root limits are valid");
        assert!(config.deployment_provider != 0, "deployment service provider differs from forge sign-in provider 0");
        assert!(
            match run_policy_bound(&config.run, limits) {
                Some(bytes) => bytes <= u64::from(limits.journal.run_bytes),
                None => false,
            },
            "run policy fits assignment bound"
        );
        assert!(
            config.resume_bytes > 0 && config.resume_bytes <= limits.journal.transcript_bytes,
            "configured task transcript bound fits the root's owned-byte limit"
        );
        assert!(*config.authority.limits() == limits.authority, "root prices its exact authority limits");
        assert!(
            limits.forge.judge_projects >= limits.authority.projects
                && limits.forge.judge_criteria >= limits.authority.requirements,
            "forge judge table covers authority policy bounds"
        );
        assert!(
            config.landing.projects.capacity() <= limits.authority.projects,
            "typed landing project table fits the policy bound"
        );
        let landing_bytes = |rules: &[LandingRule]| match landing::landing_rules_bytes(rules) {
            Some(bytes) => bytes <= u64::from(limits.journal.transcript_bytes),
            None => false,
        };
        assert!(landing_bytes(&config.landing.deployment), "deployment landing policy fits owned-byte bound");
        for (_, rules) in &config.landing.projects {
            assert!(landing_bytes(rules), "project landing policy fits owned-byte bound");
        }
        assert!(config.permission_roles.capacity() == limits.authority.projects, "permission policy project bound");
        for (_, mappings) in &config.permission_roles {
            let count = u64::try_from(mappings.len()).expect("usize fits u64");
            let unit = u64::try_from(size_of::<people::PermissionRole>()).expect("type size fits u64");
            assert!(
                match count.checked_mul(unit) {
                    Some(bytes) => bytes <= u64::from(limits.journal.transcript_bytes),
                    None => false,
                },
                "permission policy bound"
            );
        }
        assert!(authority_within(&config.chat_authority, limits), "chat authority shape bounded before copying");
        let mut projects = List::with_capacity(limits.people.projects);
        for owner in &config.owners {
            assert!(config.authority.policy(owner.project).is_some(), "bootstrap owner names configured policy");
            assert!(
                match config.authority.policy(owner.project).expect("known policy").escalation_role {
                    Some(role) => role <= 3,
                    None => false,
                },
                "current root requires final escalation role"
            );
            let mut found = false;
            for project in &projects {
                if *project == owner.project {
                    found = true;
                }
            }
            if !found {
                projects.push(owner.project).expect("configured projects bounded");
            }
        }
        let mut forge_config = core::mem::replace(
            &mut config.forge,
            forge_client::Config { namespace: Box::new([]), writers: Box::new([]) },
        );
        forge_config.namespace = Box::from(config.deployment);
        let mut forge =
            forge::Domain::new(&limits.forge, config.seed, forge_config).expect("valid forge configuration");
        let built = policy_translate::build_landing(
            &config.landing.deployment,
            false,
            limits.authority.requirements,
            config.forge_connector,
        )
        .expect("deployment landing requirements bounded");
        assert!(
            config.authority.add_configured_requirements(&built.requirements),
            "deployment landing requirements fit authority configuration"
        );
        assert!(forge.deployment_judges(built.criteria), "deployment judge table bounded");
        for (&project, rules) in &config.landing.projects {
            let built =
                policy_translate::build_landing(rules, true, limits.authority.requirements, config.forge_connector)
                    .expect("project landing requirements bounded");
            let mut policy = config.authority.policy(project).expect("landing project has a policy").clone();
            assert!(
                policy_translate::landing_roles(&policy, &config.landing.deployment),
                "deployment landing roles configured"
            );
            assert!(policy_translate::landing_roles(&policy, rules), "project landing roles configured");
            let combined = policy_translate::combine_requirements(
                &policy.requirements,
                &built.requirements,
                limits.authority.requirements,
            )
            .expect("project landing requirements fit authority policy");
            policy.requirements = combined;
            let mut facts = Queue::with_capacity(authority::POLICY_MAX_OUT);
            authority::step(&mut config.authority, authority::Event::Policy { project, policy }, &mut facts);
            assert_eq!(facts.pop(), Some(authority::PolicyFact::Changed { project }), "landing policy configured");
            assert!(forge.project_judges(project, built.criteria), "project judge table bounded");
        }
        let (core_config, root_config) = split_config(config, projects);
        let mut core = Core::new(core_config, &core_limits(limits));
        let core_env = Env { now: skein_lib::Time::ZERO, wall: skein_lib::Wall::EPOCH, limits: core_limits(limits) };
        let configured = jig_core::step(
            &mut core,
            &core_env,
            jig_core::Event::Tasks(tasks::Event::Kinds {
                connector: root_config.forge_connector,
                kinds: Box::new([tasks::Kind {
                    connector: root_config.forge_connector,
                    kind: 1,
                    hold: tasks::HoldKind::Exclusive { taken: tasks::Taken::Waits },
                }]),
            }),
        );
        match configured {
            jig_core::Requests::Out(mut output) => {
                match output.pop() {
                    Some(jig_core::Request::Decided) => {}
                    Some(
                        jig_core::Request::Write(_)
                        | jig_core::Request::Ask { .. }
                        | jig_core::Request::Held(_)
                        | jig_core::Request::Now(_),
                    )
                    | None => unreachable!("configuration decides nothing"),
                }
                assert!(output.is_empty(), "configuration emits no request");
            }
        }
        Domain {
            journal: Journal::new(&root_journal_limits(limits)),
            stop_pending: false,
            door_pass: false,
            door_count: 0,
            startup: Startup::Cold,
            core,
            brief_connectors: Slab::with_capacity(
                limits
                    .brief
                    .briefs
                    .checked_mul(limits.brief.sections)
                    .expect("validated brief count")
                    .checked_mul(2)
                    .expect("validated brief connector room"),
            ),
            brief_sections: Map::with_capacity(limits.tasks.tasks),
            run_workspaces: Map::with_capacity(limits.tasks.tasks),
            forge,
            forge_keys: Map::with_capacity(forge_route::rows(limits).expect("forge row capacity")),
            adoption_restore: Map::with_capacity(limits.forge.adoptions),
            forge_subscribing: Map::with_capacity(limits.fleet.calls),
            forge_unsubscribing: Map::with_capacity(limits.fleet.calls),
            forge_reading: Map::with_capacity(limits.fleet.calls),
            forge_effecting: Map::with_capacity(limits.fleet.calls),
            forge_projection_due: Map::with_capacity(limits.forge.issues),
            forge_change_due: Map::with_capacity(limits.forge.changes),
            forge_delegating: Map::with_capacity(limits.forge.changes),
            loads: loads::Loads::new(&limits.loads),
            assignments: Map::with_capacity(limits.tasks.tasks),
            payloads: Slab::with_capacity(payload_slots(limits).expect("valid payload room")),
            result_reads: Slab::with_capacity(limits.loads.loads),
            result_pages: Queue::with_capacity(limits.loads.loads),
            connector_calls: Map::with_capacity(limits.call_records),
            work: Queue::with_capacity(route_bound(limits).expect("valid routes")),
            before_header: Queue::with_capacity(limits.fleet.workers),
            cold_channels: Map::with_capacity(limits.fleet.workers),
            config: root_config,
            limits: *limits,
        }
    }

    /// Pure readiness query: true only after all child/current proof pages validate, tasks
    /// restoration consequences finish routing, and every restored claim reaches fleet before
    /// `Loaded`. New work enters only then.
    #[must_use]
    pub fn ready(&self) -> bool {
        self.startup == Startup::Running && !self.journal.stopped()
    }

    /// Pure shell idle query: no immediate internal handoff or issued store operation
    /// remains unfinished. Call after the iteration's reclaim; sessions, idle
    /// workers, assigned workers awaiting external answers and future task/account
    /// timers are permitted. Story completion also requires the shell's independent
    /// final result condition. This query never cancels work.
    #[must_use]
    pub fn quiescent(&self) -> bool {
        self.ready()
            && self.core.quiescent(self.journal.idle())
            && self.loads.quiescent()
            && self.work.is_empty()
            && self.before_header.is_empty()
            && self.cold_channels.is_empty()
            && self.assignments.is_empty()
            && self.payloads.is_empty()
            && self.result_reads.is_empty()
            && self.result_pages.is_empty()
            && self.adoption_restore.is_empty()
            && self.forge_subscribing.is_empty()
            && self.forge_unsubscribing.is_empty()
            && self.forge_reading.is_empty()
            && self.forge.briefs_idle()
            && self.forge_effecting.is_empty()
            && self.brief_connectors.is_empty()
            && !self.forge.is_ready()
    }

    /// Pure snapshot query for the latest allocated deployment counters; these may run ahead of
    /// store durability. Exposes neither children nor owned handoff bodies and emits no effect.
    #[must_use]
    pub fn deployment(&self) -> crate::Deployment {
        self.core.counters.deployment()
    }

    /// Retired IO/body/child slots are reclaimed at iteration end, after every event and ready
    /// pass. Bounded child/slab bookkeeping releases only retired entries; issued loads still
    /// awaiting a terminal remain owned. Emits no request or terminal.
    pub fn reclaim(&mut self) {
        self.core.reclaim();
        self.brief_connectors.reclaim();
        self.forge.reclaim();
        self.payloads.reclaim();
        self.result_reads.reclaim();
        loads::reclaim(&mut self.loads);
    }

    /// Shell/root caller discards every currently queued child observation, scanning at most each
    /// child's configured fact capacity. Emits no effect or terminal and changes no decision state.
    pub fn drain_facts(&mut self) {
        self.core.drain_facts(&core_limits(&self.limits));
    }
}

/// One outward commit, or one ready delivery, or a bounded account step; reserve before every
/// `step`, `resume` and `fire` call. Pure constant query, four output slots; emits no effect or
/// terminal.
#[must_use]
pub const fn max_out(limits: &Limits) -> u32 {
    let viewed = views::max_out(&limits.views).saturating_add(4);
    if viewed > 4 { viewed } else { 4 }
}

fn root_journal_limits(limits: &Limits) -> skein_lib::JournalLimits {
    let mut journal = crate::journal_limits(&limits.journal);
    journal.now = max_out(limits);
    journal
}

fn environment_core(env: &Env<Limits>) -> Env<jig_core::Limits> {
    Env { now: env.now, wall: env.wall, limits: core_limits(&env.limits) }
}

fn core_limits(limits: &Limits) -> jig_core::Limits {
    jig_core::Limits {
        connectors: 1,
        resume_bytes: limits.journal.transcript_bytes,
        run_bytes: limits.journal.run_bytes,
        policy_bytes: u64::from(limits.journal.transcript_bytes).min(row_bound(limits).expect("validated row bound")),
        escalation_reason_bytes: limits
            .tasks
            .result_bytes
            .min(limits.journal.result_bytes)
            .min(limits.journal.transcript_bytes),
        tasks: limits.tasks,
        authority: limits.authority,
        load_slots: limits.loads.loads,
        call_records: limits.call_records,
        people: limits.people,
        fleet: limits.fleet,
        brief: brief_limits(&limits.brief),
        brief_parts: limits.brief.parts,
        brief_core_budgets: jig_core::CoreBriefBudgets {
            task: limits.brief.budgets.task,
            dependencies: limits.brief.budgets.dependencies,
            attempts: limits.brief.budgets.attempts,
            plan: limits.brief.budgets.plan,
        },
        accounts: limits.accounts,
        notes: limits.notes,
        views: limits.views,
    }
}

fn view_outputs(domain: &mut Domain, env: &Env<Limits>, output: &mut Queue<views::Request>, out: &mut Queue<Request>) {
    for _ in 0..output.len() {
        let request = output.pop().expect("view output count");
        match request {
            views::Request::Ended { watcher, .. } | views::Request::Refused { watcher, .. } => {
                domain.core.watch_closed(&environment_core(env), watcher);
            }
            views::Request::Watching { .. } | views::Request::Deliver { .. } => {}
        }
        out.push(Request::View(request));
    }
}

fn view_requests(routed: jig_core::Requests, room: u32) -> Queue<views::Request> {
    let mut child = Queue::with_capacity(room);
    let jig_core::Requests::Out(mut marked) = routed;
    for _ in 0..marked.len() {
        match marked.pop().expect("view mark count") {
            jig_core::Request::Now(value) => match *value {
                jig_core::Now::View(request) => child.push(request),
                jig_core::Now::Account(_) => unreachable!("view route does not own account output"),
                jig_core::Now::SignInRefused { .. }
                | jig_core::Now::WatchRefused { .. }
                | jig_core::Now::NotesIndexed { .. }
                | jig_core::Now::NotesRecalled { .. }
                | jig_core::Now::NotesRefused { .. }
                | jig_core::Now::DropPayload { .. }
                | jig_core::Now::DropAssignment { .. }
                | jig_core::Now::TurnPayload { .. }
                | jig_core::Now::AnswerPayload { .. }
                | jig_core::Now::AcceptedTurn { .. }
                | jig_core::Now::RefusedPayload { .. }
                | jig_core::Now::Activate { .. }
                | jig_core::Now::PrepareAgent { .. }
                | jig_core::Now::StartPreparation { .. }
                | jig_core::Now::HistoricalProposal { .. }
                | jig_core::Now::BriefCorePlanned { .. }
                | jig_core::Now::WorkspaceRequest { .. }
                | jig_core::Now::RunPreparationFailed { .. }
                | jig_core::Now::RunPrepared { .. }
                | jig_core::Now::CompleteBrief { .. }
                | jig_core::Now::HistoricalEscalation { .. }
                | jig_core::Now::EscalationInspection { .. }
                | jig_core::Now::EscalationReply { .. }
                | jig_core::Now::EscalationRefused { .. }
                | jig_core::Now::CallPayload { .. }
                | jig_core::Now::ProcedureDelegateOutcome { .. }
                | jig_core::Now::RestoreRefused => unreachable!("view route owns its now output"),
            },
            jig_core::Request::Decided => {}
            jig_core::Request::Write(_) | jig_core::Request::Ask { .. } | jig_core::Request::Held(_) => {
                unreachable!("view route changes no decision")
            }
        }
    }
    child
}

fn view_step(domain: &mut Domain, env: &Env<Limits>, event: views::Event, out: &mut Queue<Request>) {
    let routed = jig_core::step(&mut domain.core, &environment_core(env), jig_core::Event::View(event));
    let mut child = view_requests(routed, views::max_out(&env.limits.views));
    view_outputs(domain, env, &mut child, out);
}

fn watch_subject(subject: views::Subject) -> people::WatchSubject {
    match subject {
        views::Subject::Run { task, attempt } => people::WatchSubject::Run { task: task.raw(), attempt: attempt.raw() },
        views::Subject::Tree { task } => people::WatchSubject::Tree { task: task.raw() },
        views::Subject::Goals { .. } => people::WatchSubject::Goals,
        views::Subject::Inbox { party } => people::WatchSubject::Inbox { party },
    }
}

/// A volatile watch has no durable root decision; the core admits its party and opens its view.
#[expect(clippy::too_many_arguments, reason = "watch admission carries the signed-in caller, key and subject")]
fn open_watch(
    domain: &mut Domain,
    env: &Env<Limits>,
    watcher: Token,
    sign_in: u64,
    key: [u8; 16],
    project: u32,
    subject: people::WatchSubject,
    out: &mut Queue<Request>,
) {
    let ready = domain.ready() && domain.core.counters.quiescent(domain.journal.idle()) && domain.work.is_empty();
    let routed = jig_core::step(
        &mut domain.core,
        &environment_core(env),
        jig_core::Event::Watch { watcher, sign_in, key, project, subject, ready },
    );
    let jig_core::Requests::Out(mut marked) = routed;
    for _ in 0..marked.len() {
        match marked.pop().expect("watch output count") {
            jig_core::Request::Now(value) => match *value {
                jig_core::Now::View(request) => out.push(Request::View(request)),
                jig_core::Now::WatchRefused { watcher, refusal } => {
                    out.push(Request::WatchRefused { watcher, refusal });
                }
                jig_core::Now::SignInRefused { .. }
                | jig_core::Now::Account(_)
                | jig_core::Now::NotesIndexed { .. }
                | jig_core::Now::NotesRecalled { .. }
                | jig_core::Now::NotesRefused { .. }
                | jig_core::Now::DropPayload { .. }
                | jig_core::Now::DropAssignment { .. }
                | jig_core::Now::TurnPayload { .. }
                | jig_core::Now::AnswerPayload { .. }
                | jig_core::Now::AcceptedTurn { .. }
                | jig_core::Now::RefusedPayload { .. }
                | jig_core::Now::Activate { .. }
                | jig_core::Now::PrepareAgent { .. }
                | jig_core::Now::StartPreparation { .. }
                | jig_core::Now::HistoricalProposal { .. }
                | jig_core::Now::BriefCorePlanned { .. }
                | jig_core::Now::WorkspaceRequest { .. }
                | jig_core::Now::RunPreparationFailed { .. }
                | jig_core::Now::RunPrepared { .. }
                | jig_core::Now::CompleteBrief { .. }
                | jig_core::Now::HistoricalEscalation { .. }
                | jig_core::Now::EscalationInspection { .. }
                | jig_core::Now::EscalationReply { .. }
                | jig_core::Now::EscalationRefused { .. }
                | jig_core::Now::CallPayload { .. }
                | jig_core::Now::ProcedureDelegateOutcome { .. }
                | jig_core::Now::RestoreRefused => unreachable!("watch route has only volatile view outputs"),
            },
            jig_core::Request::Decided => {}
            jig_core::Request::Write(_) | jig_core::Request::Ask { .. } | jig_core::Request::Held(_) => {
                unreachable!("watch route changes no store decision")
            }
        }
    }
}

fn brief_limits(limits: &BriefLimits) -> brief::Limits {
    brief::Limits {
        briefs: limits.briefs,
        sections: limits.sections,
        read_bytes: limits.read_bytes,
        brief_bytes: limits.brief_bytes,
    }
}

fn environment_forge(env: &Env<Limits>) -> Env<forge::Limits> {
    Env { now: env.now, wall: env.wall, limits: env.limits.forge }
}

fn internal(number: u64) -> ReplyTo {
    ReplyTo::new(Token::new(number))
}

fn emit(decision: &mut Decision, limits: &Limits, delivery: Delivery) {
    decision.deliver(&limits.journal, delivery).expect("whole root decision delivery reserved");
}

fn save(decision: &mut Decision, limits: &Limits, write: Write) {
    decision.write(&limits.journal, write).expect("validated bounded root decision record");
}

/// Route one admitted input and all synchronous child callbacks in one decision; store terminals
/// are accepted even under pressure. The shell supplies
/// unchanged configured limits with injected time. Refused web/worker calls get terminal/busy
/// notices before child mutation; admitted effects may wait in the journal until store durability.
/// Account operations keep their own secret-free terminal contract.
pub fn step(domain: &mut Domain, env: &Env<Limits>, event: Event) {
    let mut out = Queue::with_capacity(max_out(&env.limits));
    step_routed(domain, env, event, &mut out);
    stage_now(domain, &mut out);
}

#[expect(
    clippy::too_many_lines,
    reason = "one exhaustive admission match keeps every input before a single decision close"
)]
fn step_routed(domain: &mut Domain, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    assert!(domain.limits == env.limits, "root uses configured limits");
    assert!(out.room() >= max_out(&env.limits), "root output room reserved");
    if domain.journal.stopped() || domain.startup == Startup::Failed {
        discard_after_stop(domain, event);
        return;
    }
    match event {
        Event::ForgeAnswered { call, cost, result } => {
            domain.work.push(Work::Forge(forge::Event::Client(forge_client::Event::Answered { call, cost, result })));
        }
        Event::ForgeHint { hint } => {
            if domain.ready() {
                let changes = domain.forge.changes_for(hint.repository, env.limits.forge.changes);
                domain.work.push(Work::Forge(forge::Event::Hint { hint }));
                for task in changes {
                    domain.work.push(Work::Tasks(tasks::Event::WakeProcedure { task }));
                }
            }
        }
        Event::Watch { watcher, sign_in, key, subject } => {
            let project = jig_core::watch_project(&domain.core, subject);
            open_watch(domain, env, watcher, sign_in, key, project, watch_subject(subject), out);
            return;
        }
        Event::Unwatch { watcher } => {
            if domain.core.watching.contains_key(&watcher) {
                view_step(domain, env, views::Event::Unwatch { watcher }, out);
            }
            return;
        }
        Event::ViewDelivered { watcher, done } => {
            view_step(domain, env, views::Event::Delivered { watcher, done }, out);
            return;
        }
        Event::StartRecurring { project, authority, template } => {
            if domain.ready() && admits(domain, &env.limits) {
                domain.work.push(Work::Core(jig_core::Event::StartRecurring { project, authority, template }));
            }
        }
        Event::Period { project, period, budget } => {
            if domain.ready() && admits(domain, &env.limits) {
                domain.work.push(Work::Core(jig_core::Event::Period { project, period, budget }));
            }
        }
        Event::ProcedureStep { task, step, connector, code, action } => {
            if !domain.ready() || !admits(domain, &env.limits) {
                return;
            }
            domain.work.push(Work::Core(jig_core::Event::ProcedureStep { task, step, connector, code, action }));
        }
        Event::Committed { number } => {
            crate::committed(&mut domain.journal, number);
            return;
        }
        Event::Uncommitted { number } => {
            let mut journal_out = Queue::with_capacity(1);
            crate::uncommitted(&mut domain.journal, number, &mut journal_out);
            journal_outputs(&mut journal_out, out);
            return;
        }
        Event::Loaded { owner, rows, next } => {
            if domain.journal.stopped() {
                loads::abandon(&mut domain.loads, owner);
            }
            let mut load_out = Queue::with_capacity(1);
            loads::loaded(&mut domain.loads, owner, rows, next, &mut load_out);
            load_outputs(domain, env, &mut load_out, out);
            return;
        }
        Event::Unloaded { owner } => {
            if domain.journal.stopped() {
                loads::abandon(&mut domain.loads, owner);
            }
            let mut load_out = Queue::with_capacity(1);
            loads::unloaded(&mut domain.loads, owner, &mut load_out);
            load_outputs(domain, env, &mut load_out, out);
            return;
        }
        Event::Start => {
            if domain.startup != Startup::Cold {
                return;
            }
            domain.startup = Startup::Loading(Range::Deployment);
            request_load(domain, Token::new(u64::MAX), Range::Deployment, None, out);
            account_event(
                domain,
                env,
                accounts::Event::Add {
                    account: domain.core.settings.account,
                    generation: domain.core.settings.account_generation,
                    valid: domain.core.settings.account_valid,
                },
                out,
            );
            return;
        }
        Event::Refreshed { account, generation, valid } => {
            if domain.journal.stopped() {
                return;
            }
            account_event(domain, env, accounts::Event::Refreshed { account, generation, valid }, out);
            return;
        }
        Event::RefreshFailed { account, generation, failure } => {
            if domain.journal.stopped() {
                return;
            }
            account_event(domain, env, accounts::Event::Failed { account, generation, failure }, out);
            return;
        }
        Event::Hello { channel, hello } => {
            if !hello_within(&hello, &env.limits.fleet) {
                out.push(Request::Deliver(Delivery::Refuse { channel }));
                return;
            }
            if !header_loaded(domain.startup) {
                if domain.cold_channels.contains_key(&channel) {
                    out.push(Request::Deliver(Delivery::Refuse { channel }));
                    return;
                }
                let event = Work::Fleet(fleet::Event::Hello { channel, hello });
                if domain.before_header.try_push(event).is_err() {
                    out.push(Request::Deliver(Delivery::Refuse { channel }));
                } else {
                    let _old =
                        domain.cold_channels.insert(channel, false).expect("cold names bounded by admitted hellos");
                }
                return;
            }
            if !domain.ready() || !admits(domain, &env.limits) {
                out.push(Request::Deliver(Delivery::Refuse { channel }));
                return;
            }
            domain.work.push(Work::Fleet(fleet::Event::Hello { channel, hello }));
        }
        Event::Lost { channel } => {
            if !header_loaded(domain.startup) {
                if let Some(lost) = domain.cold_channels.get_mut(&channel) {
                    *lost = true;
                }
                return;
            }
            lose_channel(domain, env, channel);
            return;
        }
        Event::SignedIn { reply_to, identity } => {
            if !domain.ready() || !admits(domain, &env.limits) {
                out.push(Request::Deliver(Delivery::WebReply {
                    to: reply_to,
                    sign_in: None,
                    reply: people::Reply::Refused(people::Refusal::Busy),
                }));
                return;
            }
            domain.work.push(Work::Core(jig_core::Event::SignIn { reply_to, identity }));
        }
        Event::Ask { reply_to, sign_in, key, ask } => {
            if let people::Ask::Watch { project, subject } = &ask {
                open_watch(domain, env, reply_to.into_token(), sign_in, key, *project, *subject, out);
                return;
            }
            if !domain.ready() || !admits(domain, &env.limits) {
                out.push(Request::Deliver(Delivery::WebReply {
                    to: reply_to,
                    sign_in: None,
                    reply: people::Reply::Refused(people::Refusal::Busy),
                }));
                return;
            }
            domain.work.push(Work::People(people::Event::Ask { reply_to, sign_in, key, ask }));
        }
        Event::Call { channel, task, attempt, call, mut body } => {
            let key = CallKey { task, attempt, completion: body.completion, position: body.position };
            if !domain.ready()
                || !admits(domain, &env.limits)
                || task == 0
                || attempt == 0
                || body.completion == 0
                || domain.core.fleet.calls() >= env.limits.fleet.calls
                || domain.core.pending_calls.contains_key(&key)
                || (call_needs_input(&body.tool) && domain.result_reads.len() >= domain.result_reads.capacity())
                || (!domain.core.call_parts.contains_key(&key)
                    && domain.core.call_parts.len().saturating_add(domain.core.pending_calls.len())
                        >= env.limits.call_records)
            {
                out.push(Request::CallBusy { channel, task, attempt, call });
                return;
            }
            if let Some(why) = call_shape(&body.tool, &env.limits.tasks) {
                body.tool = match body.tool {
                    Tool::Message { .. } => Tool::RejectedMessage(why),
                    Tool::Amend { .. } | Tool::Cancel { .. } | Tool::Release { .. } | Tool::DecideEscalation { .. } => {
                        Tool::RejectedControl(why)
                    }
                    Tool::Propose { .. } | Tool::Decide { .. } | Tool::Withdraw { .. } => Tool::RejectedProposal(why),
                    Tool::Delegate { .. }
                    | Tool::Introduce { .. }
                    | Tool::Subscribe { .. }
                    | Tool::SubscribeForge { .. }
                    | Tool::ReadForge { .. }
                    | Tool::EffectForge { .. }
                    | Tool::Unsubscribe { .. }
                    | Tool::Rejected(_)
                    | Tool::RejectedMessage(_)
                    | Tool::RejectedControl(_)
                    | Tool::RejectedProposal(_)
                    | Tool::Unavailable => Tool::Rejected(why),
                };
            }
            let Ok(id) = domain.payloads.insert(Some(Payload::Call { key, body })) else {
                out.push(Request::CallBusy { channel, task, attempt, call });
                return;
            };
            domain.work.push(Work::Fleet(fleet::Event::Relay {
                channel,
                run: Token::new(task),
                attempt: Token::new(attempt),
                call,
                body: id.token(),
            }));
        }
        Event::Turn { channel, task, attempt, turn } => {
            if !domain.ready()
                || !admits(domain, &env.limits)
                || turn.transcript.len() > usize::try_from(env.limits.journal.transcript_bytes).expect("u32 fits")
            {
                out.push(Request::TurnBusy { channel, task, attempt, turn: turn.number });
                return;
            }
            let number = turn.number;
            let Ok(id) = domain.payloads.insert(Some(Payload::Turn { task, attempt, body: turn })) else {
                out.push(Request::TurnBusy { channel, task, attempt, turn: number });
                return;
            };
            domain.work.push(Work::Fleet(fleet::Event::Turn {
                channel,
                run: Token::new(task),
                attempt: Token::new(attempt),
                turn: number,
                body: id.token(),
            }));
        }
        Event::Answer { channel, task, attempt, cumulative, end, saved } => {
            if !domain.ready() || !admits(domain, &env.limits) {
                out.push(Request::AnswerBusy { channel, task, attempt });
                return;
            }
            if end_bytes(&end) > u64::from(env.limits.tasks.result_bytes).checked_mul(2).expect("bounded result bytes")
                || !tasks_saved_within(saved.as_deref(), env.limits.tasks.saved_resources)
            {
                out.push(Request::AnswerBusy { channel, task, attempt });
                return;
            }
            let Ok(id) = domain.payloads.insert(Some(Payload::Answer { task, attempt, cumulative, end, saved })) else {
                out.push(Request::AnswerBusy { channel, task, attempt });
                return;
            };
            domain.work.push(Work::Fleet(fleet::Event::Answer {
                channel,
                run: Token::new(task),
                attempt: Token::new(attempt),
                answer: fleet::Answer::Ended,
                payload: id.token(),
            }));
        }
        Event::ReadEscalation { reply_to, sign_in, task } => {
            escalation::read(domain, env, reply_to, sign_in, task, out);
            return;
        }
        Event::ReadResult { reply_to, sign_in, task } => {
            results::begin(domain, env, reply_to, sign_in, results::Query::Named { task }, out);
            return;
        }
        Event::ReadInbox { reply_to, sign_in, most } => {
            results::begin(domain, env, reply_to, sign_in, results::Query::Inbox { most }, out);
            return;
        }
        Event::ViewInbox { reply_to, sign_in, most, before } => {
            inbox::begin(domain, env, reply_to, sign_in, most, before, out);
            return;
        }
    }
    // Connector terminals already belong to the root's bounded work queue.
    // Keep them there until the journal can admit the entire child route.
    if !route_takes(domain, &env.limits) {
        return;
    }
    let decision = route(domain, env);
    domain.core.signing_in = None;
    close(domain, env, decision, out);
}

fn admits(domain: &Domain, limits: &Limits) -> bool {
    route_takes(domain, limits)
        && domain.core.counters.deployment().messages
            <= u64::MAX.checked_sub(u64::from(limits.tasks.tasks)).expect("task count fits u64")
        && domain.work.is_empty()
        && domain.journal.takes(&skein_lib::JournalRoom {
            writes: 0,
            held: limits.journal.deliveries.checked_mul(3).expect("root held reserve bounded"),
        })
}

fn lose_channel(domain: &mut Domain, env: &Env<Limits>, channel: Token) {
    domain.core.lost_channel(&environment_core(env), channel);
}

fn close(domain: &mut Domain, env: &Env<Limits>, decision: Decision, out: &mut Queue<Request>) {
    crate::accept_pending(&mut domain.journal, &mut domain.core.counters, &env.limits.journal, decision)
        .expect("root pressure reserved before child mutation");
    if domain.journal.stopped() {
        out.push(Request::Stop);
    }
}

fn journal_outputs(journal_out: &mut Queue<Output>, out: &mut Queue<Request>) {
    for _ in 0..journal_out.len() {
        match journal_out.pop().expect("journal output count") {
            Output::Now(request) => out.push(request),
            Output::Commit { number, writes } => out.push(Request::Commit { number, writes }),
            Output::Stop => out.push(Request::Stop),
            Output::Deliver(delivery) => out.push(Request::Deliver(delivery)),
        }
    }
}

/// The only admission call for outputs that have no durability dependency.
fn now(domain: &mut Domain, request: Request) -> bool {
    match domain.journal.now(Output::Now(request)) {
        Ok(()) => true,
        Err(Output::Now(_)) => false,
        Err(Output::Commit { .. } | Output::Deliver(_) | Output::Stop) => unreachable!("the door returns its input"),
    }
}

/// Pass every immediate root request through the journal's door before it
/// reaches the caller. The stopped notice translates the journal's terminal
/// state, since a stopped journal admits no new output.
fn publish_now(domain: &mut Domain, out: &mut Queue<Request>) {
    let count = out.len();
    let mut released = Queue::with_capacity(1);
    for _ in 0..count {
        let request = out.pop().expect("original output count");
        match request {
            Request::Stop => out.push(Request::Stop),
            Request::Commit { number, writes } => out.push(Request::Commit { number, writes }),
            request @ (Request::Forge { .. }
            | Request::View(_)
            | Request::WatchRefused { .. }
            | Request::CallBusy { .. }
            | Request::Load { .. }
            | Request::Deliver(_)
            | Request::Account(_)
            | Request::TurnBusy { .. }
            | Request::AnswerBusy { .. }) => {
                if !now(domain, request) {
                    out.push(Request::Stop);
                    return;
                }
                let _released: skein_lib::Released = domain.journal.release(&mut released);
                match released.pop().expect("journal releases an admitted door output") {
                    Output::Now(request) => out.push(request),
                    Output::Commit { .. } | Output::Deliver(_) | Output::Stop => {
                        unreachable!("door output leaves before held output")
                    }
                }
            }
        }
    }
}

/// Admit immediate requests through the journal's door. The caller takes
/// them on a later release pass; no step returns an outbound queue.
fn stage_now(domain: &mut Domain, out: &mut Queue<Request>) {
    domain.door_pass = true;
    while let Some(request) = out.pop() {
        match request {
            Request::Stop => domain.stop_pending = true,
            request @ (Request::Forge { .. }
            | Request::View(_)
            | Request::WatchRefused { .. }
            | Request::CallBusy { .. }
            | Request::Load { .. }
            | Request::Deliver(_)
            | Request::Account(_)
            | Request::TurnBusy { .. }
            | Request::AnswerBusy { .. }) => {
                assert!(now(domain, request), "root door reserves one route's immediate outputs");
                domain.door_count = domain.door_count.checked_add(1).expect("root door count bounded by max_out");
            }
            Request::Commit { .. } => unreachable!("release takes commits from the journal"),
        }
    }
}

/// Release one held effect after durability, preserving internal callbacks while journal pressure
/// is full. The shell reserves `max_out` output slots. Deferred callbacks run before another held
/// callback is consumed; other ready work may produce one commit. This pass never waits for IO or
/// drops a retained callback on pressure
fn resume(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    resume_routed(domain, env, out);
    publish_now(domain, out);
}

/// Release one bounded journal output or route one ready child continuation.
/// The engine's iteration calls this after accepting store terminals.
pub fn release(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    if domain.stop_pending {
        domain.stop_pending = false;
        domain.door_pass = false;
        domain.door_count = 0;
        out.push(Request::Stop);
        return;
    }
    if domain.door_pass {
        domain.door_pass = false;
        send_commit(domain, out);
        let mut released = Queue::with_capacity(1);
        while domain.door_count > 0 {
            let _status: skein_lib::Released = domain.journal.release(&mut released);
            match released.pop().expect("one staged door output") {
                Output::Now(request) => out.push(request),
                Output::Commit { .. } | Output::Deliver(_) | Output::Stop => {
                    unreachable!("door output precedes held outputs")
                }
            }
            domain.door_count = domain.door_count.checked_sub(1).expect("staged output released");
        }
        return;
    }
    resume(domain, env, out);
    send_commit(domain, out);
}

fn send_commit(domain: &mut Domain, out: &mut Queue<Request>) {
    match crate::commit(&mut domain.journal, &domain.limits.journal) {
        Some(Output::Commit { number, writes }) => out.push(Request::Commit { number, writes }),
        Some(Output::Now(_) | Output::Deliver(_) | Output::Stop) => unreachable!("journal commit has writes only"),
        None => {}
    }
}

#[expect(clippy::too_many_lines, reason = "root release path routes each held delivery exhaustively")]
fn resume_routed(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    assert!(out.room() >= max_out(&env.limits), "root ready output room");
    if domain.journal.stopped() || domain.startup == Startup::Failed {
        return;
    }
    if !domain.result_pages.is_empty() && route_takes(domain, &env.limits) {
        let page = domain.result_pages.pop().expect("pending result page");
        match domain.result_reads.get(Id::from_token(page.waiter)) {
            Some(Some(Read::Inbox(_))) => inbox::page(domain, env, page.waiter, page.rows, page.next, out),
            Some(Some(Read::Result(_))) => results::page(domain, env, page.waiter, page.rows, page.next, out),
            Some(
                Some(
                    Read::Escalation(_)
                    | Read::Proposal(_)
                    | Read::Transcript { .. }
                    | Read::Dependency(_)
                    | Read::InputCheck(_),
                )
                | None,
            )
            | None => unreachable!("queued person page has its live read"),
        }
        return;
    }
    if !domain.work.is_empty() {
        if route_takes(domain, &env.limits) {
            let decision = route(domain, env);
            close(domain, env, decision, out);
        }
        return;
    }
    let mut journal_out = Queue::with_capacity(1);
    crate::resume(&mut domain.journal, &mut journal_out);
    if let Some(output) = journal_out.pop() {
        match output {
            Output::Now(request) => {
                out.push(request);
                return;
            }
            Output::Deliver(Delivery::ForgeCommitted { entry }) => {
                domain.work.push(Work::Forge(forge::Event::Committed { entry }));
            }
            Output::Deliver(Delivery::ForgeCall { call, repository, op }) => {
                out.push(Request::Forge { call, repository, op });
                return;
            }
            Output::Deliver(Delivery::Fleet(event)) => domain.work.push(Work::Fleet(event)),
            Output::Deliver(Delivery::View(event)) => {
                view_step(domain, env, *event, out);
                return;
            }
            Output::Deliver(Delivery::Relay { task, attempt, previous, word }) => {
                let event = Token::new(word.number);
                assert!(domain.core.relaying.replace(PendingRelay { previous, word }).is_none(), "one relay at a time");
                domain.work.push(Work::Fleet(fleet::Event::Inbound {
                    run: Token::new(task),
                    attempt: Token::new(attempt),
                    event,
                }));
            }
            Output::Deliver(Delivery::Load { waiter, range, after }) => {
                request_load(domain, waiter, range, after, out);
                return;
            }
            Output::Deliver(Delivery::ReadEscalationDecision { waiter }) => {
                let Some(Some(read)) = domain.result_reads.get(Id::from_token(waiter)) else { return };
                let (task, revision) = match read {
                    Read::Escalation(escalation::Query::Historical { task, revision, .. }) => (*task, *revision),
                    Read::Result(_)
                    | Read::Inbox(_)
                    | Read::Transcript { .. }
                    | Read::Dependency(_)
                    | Read::InputCheck(_)
                    | Read::Escalation(escalation::Query::Read { .. })
                    | Read::Proposal(_) => {
                        unreachable!("decision archive waiter")
                    }
                };
                request_load(domain, waiter, Range::EscalationDecision { task, revision }, None, out);
                return;
            }
            Output::Deliver(Delivery::ReadResult { waiter }) => {
                let Some(read) = domain.result_reads.get(Id::from_token(waiter)) else {
                    return;
                };
                let Some(read) = read else {
                    return;
                };
                match read {
                    Read::Result(_) => request_load(domain, waiter, Range::EndedResults, None, out),
                    Read::Inbox(_)
                    | Read::Escalation(_)
                    | Read::Proposal(_)
                    | Read::Transcript { .. }
                    | Read::Dependency(_)
                    | Read::InputCheck(_) => {
                        unreachable!("result waiter")
                    }
                }
                return;
            }
            Output::Deliver(Delivery::BeginInboxView { waiter }) => {
                match domain.result_reads.get(Id::from_token(waiter)) {
                    Some(Some(Read::Inbox(_))) => {}
                    Some(
                        Some(
                            Read::Result(_)
                            | Read::Escalation(_)
                            | Read::Proposal(_)
                            | Read::Transcript { .. }
                            | Read::Dependency(_)
                            | Read::InputCheck(_),
                        )
                        | None,
                    )
                    | None => unreachable!("inbox start has its live waiter"),
                }
                request_load(domain, waiter, Range::Tasks, None, out);
                return;
            }
            Output::Deliver(delivery) => {
                out.push(Request::Deliver(delivery));
                return;
            }
            Output::Commit { number, writes } => {
                out.push(Request::Commit { number, writes });
                return;
            }
            Output::Stop => {
                out.push(Request::Stop);
                return;
            }
        }
    }
    if !route_takes(domain, &env.limits) {
        return;
    }
    if !domain.work.is_empty() {
        let decision = route(domain, env);
        close(domain, env, decision, out);
        return;
    }
    if domain.ready() || startup_adopting(domain.startup) {
        if domain.forge.is_ready() {
            let mut decision =
                route_decision(domain, &env.limits).expect("journal room checked before connector continuation");
            let mut child = Queue::with_capacity(forge::max_out(&env.limits.forge));
            forge::resume(&mut domain.forge, &environment_forge(env), &mut child);
            forge_route::outputs(domain, env, &mut decision, &mut child);
            route_into(domain, env, &mut decision);
            close(domain, env, decision, out);
            return;
        }
        if !domain.ready() {
            return;
        }
        if domain.core.accounts.usable(domain.core.settings.account) && !domain.core.due.is_empty() {
            for _ in 0..domain.core.due.len() {
                domain.work.push(Work::Activate(domain.core.due.pop().expect("waiting activation")));
            }
            let decision = route(domain, env);
            close(domain, env, decision, out);
            return;
        }
        let mut decision = route_decision(domain, &env.limits).expect("journal room checked before fleet continuation");
        let routed = jig_core::resume_fleet(&mut domain.core, &environment_core(env));
        route_core_requests(domain, env, &mut decision, routed);
        route_into(domain, env, &mut decision);
        close(domain, env, decision, out);
    }
}

/// Fire participating child timers through the same barrier; store durability and inputs run before
/// this pass. The shell supplies injected monotonic/wall time. Account
/// timers may emit bounded protocol actions independently; root routes that mutate tasks/fleet wait
/// for whole-decision admission. Their store and account outcomes enter later through `step`.
pub fn fire(domain: &mut Domain, env: &Env<Limits>) {
    let mut out = Queue::with_capacity(max_out(&env.limits));
    fire_routed(domain, env, &mut out);
    stage_now(domain, &mut out);
}

fn fire_routed(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    if domain.journal.stopped() || domain.startup == Startup::Failed {
        return;
    }
    account_fire(domain, env, out);
    if !domain.core.watching.is_empty() {
        let routed = jig_core::fire(&mut domain.core, &environment_core(env), jig_core::Timer::View);
        let mut view_out = view_requests(routed, views::max_out(&env.limits.views));
        view_outputs(domain, env, &mut view_out, out);
    }
    if !admits(domain, &env.limits) {
        return;
    }
    if startup_adopting(domain.startup) {
        let mut decision =
            route_decision(domain, &env.limits).expect("journal room checked before forge restart timer");
        let mut forge_out = Queue::with_capacity(forge::max_out(&env.limits.forge));
        forge::fire(&mut domain.forge, &environment_forge(env), &mut forge_out);
        forge_route::outputs(domain, env, &mut decision, &mut forge_out);
        route_into(domain, env, &mut decision);
        close(domain, env, decision, out);
        return;
    }
    if !domain.ready() {
        return;
    }
    let mut decision = route_decision(domain, &env.limits).expect("journal room checked before firing children");
    let mut due = List::with_capacity(env.limits.forge.issues);
    for (goal, when) in &domain.forge_projection_due {
        if *when <= env.wall {
            due.push(*goal).expect("projection due room");
        }
    }
    for goal in &due {
        domain.forge_projection_due.remove(goal);
        if domain.forge.issue(*goal).is_some()
            && let Some(entry) = crate::fresh(&mut domain.core.counters, Family::ConnectorRow)
        {
            domain.work.push(Work::Forge(forge::Event::ProjectDesired { entry, goal: *goal }));
        }
    }
    let mut due_changes = List::with_capacity(env.limits.forge.changes);
    for (task, when) in &domain.forge_change_due {
        if *when <= env.wall {
            due_changes.push(*task).expect("change due room");
        }
    }
    for task in &due_changes {
        domain.forge_change_due.remove(task);
        domain.work.push(Work::Tasks(tasks::Event::WakeProcedure { task: *task }));
    }
    let mut forge_out = Queue::with_capacity(forge::max_out(&env.limits.forge));
    forge::fire(&mut domain.forge, &environment_forge(env), &mut forge_out);
    forge_route::outputs(domain, env, &mut decision, &mut forge_out);
    let routed = jig_core::fire(&mut domain.core, &environment_core(env), jig_core::Timer::Tasks);
    route_core_requests(domain, env, &mut decision, routed);
    let routed = jig_core::fire(&mut domain.core, &environment_core(env), jig_core::Timer::Fleet);
    route_core_requests(domain, env, &mut decision, routed);
    let routed = jig_core::fire(&mut domain.core, &environment_core(env), jig_core::Timer::Brief);
    route_core_requests(domain, env, &mut decision, routed);
    route_into(domain, env, &mut decision);
    close(domain, env, decision, out);
}

fn route(domain: &mut Domain, env: &Env<Limits>) -> Decision {
    let mut decision = route_decision(domain, &env.limits).expect("journal room checked before routing children");
    route_into(domain, env, &mut decision);
    decision
}

fn forge_release_ending(ending: tasks::Ending) -> forge::ReleaseEnding {
    match ending {
        tasks::Ending::Done(_) => forge::ReleaseEnding::Done,
        tasks::Ending::Failed { .. } => forge::ReleaseEnding::Failed,
        tasks::Ending::Cancelled { .. } => forge::ReleaseEnding::Cancelled,
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "the root translates each closed core request variant for its connector and journal"
)]
fn route_core_requests(domain: &mut Domain, env: &Env<Limits>, decision: &mut Decision, requests: jig_core::Requests) {
    match requests {
        jig_core::Requests::Out(mut output) => {
            for _ in 0..output.len() {
                match output.pop().expect("core output count") {
                    jig_core::Request::Write(write) => match write {
                        jig_core::Write::Save(record) => match record {
                            jig_core::Record::Core(jig_core::CoreRecord::Call(row)) => {
                                let answer =
                                    CallAnswer::from_core(&row.part).expect("core call owns its complete answer");
                                save(
                                    decision,
                                    &env.limits,
                                    Write::Save(Record::Call(crate::CallRecord { key: row.key, answer })),
                                );
                            }
                            jig_core::Record::Core(jig_core::CoreRecord::RunProof(proof)) => {
                                save(decision, &env.limits, Write::Save(Record::RunProof(proof)));
                            }
                            jig_core::Record::Core(jig_core::CoreRecord::Terminal(terminal)) => {
                                save(decision, &env.limits, Write::Save(Record::Terminal(terminal)));
                            }
                            jig_core::Record::Core(jig_core::CoreRecord::Turn(turn)) => {
                                save(decision, &env.limits, Write::Save(Record::Turn(turn)));
                            }
                            jig_core::Record::Core(jig_core::CoreRecord::ProposalDecision(archive)) => {
                                save(decision, &env.limits, Write::Save(Record::ProposalDecision(archive)));
                            }
                            jig_core::Record::Core(jig_core::CoreRecord::EscalationDecision(archive)) => {
                                save(decision, &env.limits, Write::Save(Record::EscalationDecision(archive)));
                            }
                            jig_core::Record::People(row) => {
                                save(decision, &env.limits, Write::Save(Record::People(row)));
                            }
                            jig_core::Record::Tasks(row) => {
                                save(decision, &env.limits, Write::Save(Record::Tasks(row)));
                            }
                            jig_core::Record::Core(_) | jig_core::Record::Notes(_) => {
                                unreachable!("the core route owns its write family")
                            }
                        },
                        jig_core::Write::Erase(key) => match key {
                            jig_core::Key::People(key) => save(decision, &env.limits, Write::Erase(Key::People(key))),
                            jig_core::Key::Tasks(key) => save(decision, &env.limits, Write::Erase(Key::Tasks(key))),
                            jig_core::Key::Core(jig_core::CoreKey::Call(key)) => {
                                drop(domain.connector_calls.remove(&key));
                                save(decision, &env.limits, Write::Erase(Key::Call(key)));
                            }
                            jig_core::Key::Core(jig_core::CoreKey::RunProof(task)) => {
                                save(decision, &env.limits, Write::Erase(Key::RunProof { task }));
                            }
                            jig_core::Key::Core(
                                jig_core::CoreKey::Deployment
                                | jig_core::CoreKey::Turn { .. }
                                | jig_core::CoreKey::Terminal { .. }
                                | jig_core::CoreKey::ProposalDecision(_)
                                | jig_core::CoreKey::EscalationDecision { .. },
                            )
                            | jig_core::Key::Notes(_) => {
                                unreachable!("the core route owns its erase family")
                            }
                        },
                    },
                    jig_core::Request::Ask { connector, ask } => {
                        let request = match ask {
                            jig_core::Ask::Gather { section, budget } => {
                                Some(brief::GatherRequest::Gather { connector, section, budget })
                            }
                            jig_core::Ask::CutTo { section, size } => {
                                Some(brief::GatherRequest::CutTo { connector, section, size })
                            }
                            jig_core::Ask::Drop { section } => Some(brief::GatherRequest::Drop { connector, section }),
                            jig_core::Ask::Hold { task, resource } => {
                                if connector == domain.config.forge_connector
                                    && let Some(name) = forge_route::forge_name(
                                        domain.config.forge_connector,
                                        &resource,
                                        env.limits.forge.name_bytes,
                                    )
                                {
                                    let from = match domain.forge.hold(&name) {
                                        Some(row) if row.task != task => Some(row.task),
                                        Some(_) | None => None,
                                    };
                                    domain.work.push(Work::Forge(forge::Event::Hold { task, resource: name, from }));
                                }
                                None
                            }
                            jig_core::Ask::EndTopic { task, subscription } => {
                                if connector == domain.config.forge_connector
                                    && let Some(topic) = domain.forge.subscription(task, subscription)
                                {
                                    domain.work.push(Work::Forge(forge::Event::Unsubscribe { task, topic }));
                                }
                                None
                            }
                            jig_core::Ask::Adopt { request, project, adoption } => {
                                let parsed = if connector == domain.config.forge_connector {
                                    forge_route::parse_adoption(project, adoption, connector)
                                } else {
                                    None
                                };
                                match parsed {
                                    Some(adoption) if domain.adoption_restore.len() < env.limits.forge.adoptions => {
                                        let previous = domain.forge.repository(adoption.provider).cloned();
                                        assert!(
                                            domain.adoption_restore.insert(request, previous) == Ok(None),
                                            "one keyed adoption flight"
                                        );
                                        domain
                                            .work
                                            .push(Work::Forge(forge::Event::Adopt { reply_to: request, adoption }));
                                    }
                                    Some(_) => domain.work.push(Work::People(people::Event::Decided {
                                        request,
                                        outcome: people::Outcome::Refused(people::Refusal::Busy),
                                    })),
                                    None => domain.work.push(Work::People(people::Event::Decided {
                                        request,
                                        outcome: people::Outcome::Refused(people::Refusal::Unknown),
                                    })),
                                }
                                None
                            }
                            jig_core::Ask::Close { task, root, ending } => {
                                if connector == domain.config.forge_connector {
                                    domain.work.push(Work::Forge(forge::Event::SettleEffects {
                                        task,
                                        root,
                                        ending: forge_release_ending(ending),
                                    }));
                                }
                                None
                            }
                            jig_core::Ask::Release { task, root, ending, entry } => {
                                if connector == domain.config.forge_connector {
                                    domain.work.push(Work::Forge(forge::Event::Release {
                                        task,
                                        root,
                                        ending: forge_release_ending(ending),
                                        entry,
                                    }));
                                }
                                None
                            }
                            jig_core::Ask::Lost { task, attempt } => {
                                if connector == domain.config.forge_connector {
                                    domain.work.push(Work::Forge(forge::Event::Lost { task, attempt }));
                                }
                                None
                            }
                            jig_core::Ask::TaskHoldings { request, project, root, number, executor, spec } => {
                                let holdings = if connector == domain.config.forge_connector {
                                    forge_route::task_holdings(
                                        domain, env, project, root, number, executor, &spec, None,
                                    )
                                } else {
                                    None
                                };
                                domain.work.push(Work::Core(jig_core::Event::Holdings {
                                    request,
                                    connector,
                                    holdings,
                                }));
                                None
                            }
                            jig_core::Ask::DelegateHoldings { request, from, members } => {
                                let holdings = if connector == domain.config.forge_connector {
                                    let mut collected = List::with_capacity(
                                        u32::try_from(members.len()).expect("bounded delegate batch"),
                                    );
                                    let mut failed = false;
                                    for member in members {
                                        match forge_route::task_holdings(
                                            domain,
                                            env,
                                            member.project,
                                            member.root,
                                            member.number,
                                            member.executor,
                                            &member.spec,
                                            Some(from),
                                        ) {
                                            Some(holdings) => collected.push(holdings).expect("one per member"),
                                            None => failed = true,
                                        }
                                    }
                                    if failed { None } else { Some(collected.into_boxed()) }
                                } else {
                                    None
                                };
                                domain.work.push(Work::Core(jig_core::Event::DelegateHoldings {
                                    request,
                                    connector,
                                    holdings,
                                }));
                                None
                            }
                            jig_core::Ask::ProcedureHoldings { task, step, members } => {
                                let holdings = if connector == domain.config.forge_connector {
                                    let mut collected = List::with_capacity(
                                        u32::try_from(members.len()).expect("bounded procedure batch"),
                                    );
                                    let mut failed = false;
                                    for member in members {
                                        match forge_route::task_holdings(
                                            domain,
                                            env,
                                            member.project,
                                            member.root,
                                            member.number,
                                            member.executor,
                                            &member.spec,
                                            Some(task),
                                        ) {
                                            Some(holdings) => collected.push(holdings).expect("one per member"),
                                            None => failed = true,
                                        }
                                    }
                                    if failed { None } else { Some(collected.into_boxed()) }
                                } else {
                                    None
                                };
                                domain.work.push(Work::Core(jig_core::Event::ProcedureHoldings {
                                    task,
                                    step,
                                    connector,
                                    holdings,
                                }));
                                None
                            }
                            jig_core::Ask::ProjectGoal { goal } => {
                                if connector == domain.config.forge_connector
                                    && domain.forge.home(goal.project).is_some()
                                {
                                    domain.work.push(Work::ProjectGoal(goal));
                                }
                                None
                            }
                            jig_core::Ask::SubscriptionDone { request, key, subscription } => {
                                if connector == domain.config.forge_connector {
                                    if let Some(pending) = domain.forge_subscribing.remove(&request) {
                                        let names = forge_route::watch_names(domain, &env.limits, &pending)
                                            .expect("connector names preflighted at subscription");
                                        let owner = pending.task;
                                        domain
                                            .work
                                            .push(Work::Forge(forge::Event::Subscribe { subscription: pending }));
                                        domain
                                            .work
                                            .push(Work::Forge(forge::Event::Names { task: owner, resources: names }));
                                    }
                                    decide_call(
                                        domain,
                                        &env.limits,
                                        decision,
                                        ReplyTo::new(request),
                                        key,
                                        CallAnswer::Subscribed { subscription },
                                    );
                                }
                                None
                            }
                            jig_core::Ask::UnsubscriptionDone { request, key } => {
                                if connector == domain.config.forge_connector {
                                    if let Some((owner, topic)) = domain.forge_unsubscribing.remove(&request) {
                                        domain.work.push(Work::Forge(forge::Event::Unsubscribe { task: owner, topic }));
                                    }
                                    decide_call(
                                        domain,
                                        &env.limits,
                                        decision,
                                        ReplyTo::new(request),
                                        key,
                                        CallAnswer::Unsubscribed,
                                    );
                                }
                                None
                            }
                            jig_core::Ask::ClaimDone { task, attempt } => {
                                if connector == domain.config.forge_connector {
                                    let (writes, holders) = forge_route::claimed_writes(domain, env, task)
                                        .expect("the admitted assignment retains its bounded held forge writes");
                                    if !writes.is_empty() {
                                        domain.work.push(Work::Forge(forge::Event::Claim {
                                            task,
                                            attempt,
                                            writes,
                                            holders,
                                        }));
                                    }
                                    let key = &domain.assignments.get(&task).expect("claimed assignment").workspace.key;
                                    let workstream =
                                        u64::from_be_bytes(key.as_ref().try_into().expect("task-number workstream"));
                                    emit(
                                        decision,
                                        &env.limits,
                                        Delivery::Fleet(fleet::Event::Start {
                                            kinds: fleet::Kinds::Workers,
                                            reply_to: internal(task),
                                            run: Token::new(task),
                                            attempt: Token::new(attempt),
                                            workstream,
                                        }),
                                    );
                                }
                                None
                            }
                            jig_core::Ask::DropSubscription { request } => {
                                if connector == domain.config.forge_connector {
                                    drop(domain.forge_subscribing.remove(&request));
                                    drop(domain.forge_unsubscribing.remove(&request));
                                }
                                None
                            }
                            jig_core::Ask::RepairRefused { repair } => {
                                if connector == domain.config.forge_connector
                                    && let Some(repair) = repair
                                    && let Some(owner) = domain.forge.queue_repair_owner(repair)
                                {
                                    domain.work.push(Work::Tasks(tasks::Event::Hold {
                                        task: owner,
                                        why: tasks::Hold::Effects,
                                    }));
                                }
                                None
                            }
                            jig_core::Ask::DelegateRefused { task } => {
                                if connector == domain.config.forge_connector
                                    && let Some(task) = task
                                    && let Some(row) = domain.forge.change(task)
                                    && let Some((child, _)) = row.delegate
                                {
                                    domain.work.push(Work::Forge(forge::Event::DelegateRefused { task, child }));
                                    domain
                                        .work
                                        .push(Work::Tasks(tasks::Event::Hold { task, why: tasks::Hold::Procedure }));
                                }
                                None
                            }
                            jig_core::Ask::StartProcedure { context, step } => {
                                let task = context.task;
                                let code = match context.executor {
                                    tasks::Executor::Procedure { code, .. } => code,
                                    tasks::Executor::Agent { .. } | tasks::Executor::Person(_) => {
                                        unreachable!("procedure route owns a procedure context")
                                    }
                                };
                                if connector == domain.config.forge_connector && code == 2 {
                                    if !forge_route::start_change(domain, env, &context) {
                                        domain
                                            .work
                                            .push(Work::Tasks(tasks::Event::Hold { task, why: tasks::Hold::Effects }));
                                    }
                                } else {
                                    emit(decision, &env.limits, Delivery::Procedure { task, step, connector, code });
                                }
                                None
                            }
                        };
                        if let Some(request) = request {
                            let mut child = Queue::with_capacity(1);
                            child.push(request);
                            brief_outputs(domain, env, decision, &mut child);
                        }
                    }
                    jig_core::Request::Held(held) => match *held {
                        jig_core::Held::Relay { task, attempt, previous, word } => {
                            emit(decision, &env.limits, Delivery::Relay { task, attempt, previous, word });
                        }
                        jig_core::Held::PeopleReply { to, sign_in, reply } => {
                            emit(decision, &env.limits, Delivery::WebReply { to, sign_in, reply });
                        }
                        jig_core::Held::CallAnswer { to, key: _, part } => {
                            let answer = CallAnswer::from_core(&part).expect("core call owns its complete answer");
                            relay_call(domain, &env.limits, decision, to, answer);
                        }
                        jig_core::Held::ViewStart { run, attempt } => {
                            emit(
                                decision,
                                &env.limits,
                                Delivery::View(Box::new(views::Event::Started { task: run, attempt })),
                            );
                        }
                        jig_core::Held::ViewFinished { task } => {
                            emit(
                                decision,
                                &env.limits,
                                Delivery::View(Box::new(views::Event::Finished { task: Token::new(task) })),
                            );
                        }
                        jig_core::Held::ViewTaskPhase { task, trees, project, phase, priority } => {
                            emit(
                                decision,
                                &env.limits,
                                Delivery::View(Box::new(views::Event::TaskPhase {
                                    task,
                                    trees,
                                    project,
                                    phase,
                                    priority,
                                })),
                            );
                        }
                        jig_core::Held::Result { person, task, words } => {
                            emit(decision, &env.limits, Delivery::Result { person, task, words });
                        }
                        jig_core::Held::Assign { channel, run, attempt } => {
                            let assignment =
                                domain.assignments.remove(&run.raw()).expect("durable claim has prepared assignment");
                            assert!(assignment.attempt == attempt.raw(), "assignment names current attempt");
                            emit(decision, &env.limits, Delivery::Assigned { channel, assignment });
                        }
                        jig_core::Held::Acknowledge { channel, run, attempt } => {
                            emit(
                                decision,
                                &env.limits,
                                Delivery::Acknowledge { channel, task: run.raw(), attempt: attempt.raw() },
                            );
                        }
                        jig_core::Held::TaskTerminalAcknowledged { task, attempt } => {
                            emit(
                                decision,
                                &env.limits,
                                Delivery::Fleet(fleet::Event::Acknowledge {
                                    run: Token::new(task),
                                    attempt: Token::new(attempt),
                                }),
                            );
                        }
                        jig_core::Held::TaskTurnKept { task, attempt, turn } => {
                            emit(
                                decision,
                                &env.limits,
                                Delivery::Fleet(fleet::Event::TurnKept {
                                    run: Token::new(task),
                                    attempt: Token::new(attempt),
                                    turn,
                                }),
                            );
                        }
                        jig_core::Held::ViewTurn { task, attempt, turn } => {
                            emit(
                                decision,
                                &env.limits,
                                Delivery::View(Box::new(views::Event::Turn {
                                    task: Token::new(task),
                                    attempt: Token::new(attempt),
                                    number: turn,
                                })),
                            );
                        }
                        jig_core::Held::AcknowledgeTurn { channel, run, attempt, turn } => {
                            emit(
                                decision,
                                &env.limits,
                                Delivery::AcknowledgeTurn { channel, task: run.raw(), attempt: attempt.raw(), turn },
                            );
                        }
                        jig_core::Held::Cancel { channel, run, attempt } => {
                            emit(
                                decision,
                                &env.limits,
                                Delivery::Cancel { channel, task: run.raw(), attempt: attempt.raw() },
                            );
                        }
                        jig_core::Held::Refuse { channel } => emit(decision, &env.limits, Delivery::Refuse { channel }),
                        jig_core::Held::TurnBusy { channel, run, attempt, turn } => {
                            emit(
                                decision,
                                &env.limits,
                                Delivery::TurnBusy { channel, task: run.raw(), attempt: attempt.raw(), turn },
                            );
                        }
                        jig_core::Held::Relayed { channel, run, attempt, call, answer } => {
                            let Some(Payload::CallAnswer(answer)) = take_payload(domain, answer) else {
                                unreachable!("fleet relays an owned call answer")
                            };
                            emit(
                                decision,
                                &env.limits,
                                Delivery::CallAnswer { channel, task: run.raw(), attempt: attempt.raw(), call, answer },
                            );
                        }
                        jig_core::Held::Inbound { channel, run, attempt, word } => {
                            emit(
                                decision,
                                &env.limits,
                                Delivery::Inbound { channel, task: run.raw(), attempt: attempt.raw(), word },
                            );
                        }
                        jig_core::Held::StopRun { task, attempt } => {
                            emit(decision, &env.limits, Delivery::Fleet(Core::stop_run(task, attempt)));
                        }
                        jig_core::Held::NotesLoad { .. }
                        | jig_core::Held::NotesWritten { .. }
                        | jig_core::Held::NotesDeleted { .. } => {
                            unreachable!("the current application has no note caller")
                        }
                    },
                    jig_core::Request::Now(now) => match *now {
                        jig_core::Now::SignInRefused { to } => {
                            emit(
                                decision,
                                &env.limits,
                                Delivery::WebReply {
                                    to,
                                    sign_in: None,
                                    reply: people::Reply::Refused(people::Refusal::Limit),
                                },
                            );
                        }
                        jig_core::Now::WatchRefused { .. } => {
                            unreachable!("volatile watch route handles its own refusal")
                        }
                        jig_core::Now::DropPayload { payload } => drop(take_payload(domain, payload)),
                        jig_core::Now::TurnPayload { run, attempt, turn, body } => {
                            let payload = domain.payloads.get(Id::from_token(body)).expect("fleet returns owned token");
                            let payload = match payload.as_ref().expect("fleet returns owned payload") {
                                Payload::Turn { body: payload, .. } => payload,
                                Payload::Answer { .. } | Payload::Call { .. } | Payload::CallAnswer(_) => {
                                    unreachable!("fleet returns turn family")
                                }
                            };
                            domain.work.push(Work::Core(jig_core::Event::TurnPayload {
                                run,
                                attempt,
                                turn,
                                body,
                                read: payload.read,
                                cumulative: payload.cumulative,
                            }));
                        }
                        jig_core::Now::AnswerPayload { run, attempt, payload } => {
                            let body = domain.payloads.get(Id::from_token(payload)).expect("fleet returns owned token");
                            let (cumulative, end, saved) = match body.as_ref().expect("fleet returns owned payload") {
                                Payload::Answer { cumulative, end, saved, .. } => {
                                    (*cumulative, end.clone(), saved.clone())
                                }
                                Payload::Turn { .. } | Payload::Call { .. } | Payload::CallAnswer(_) => {
                                    unreachable!("fleet returns answer family")
                                }
                            };
                            let (saved, invalid_saved) = match saved {
                                Some(tags) => {
                                    match forge_route::saved_resources(domain.config.forge_connector, &tags) {
                                        Some(resources) => (Some(resources), false),
                                        None => (None, true),
                                    }
                                }
                                None => (None, false),
                            };
                            domain.work.push(Work::Core(jig_core::Event::AnswerPayload {
                                run,
                                attempt,
                                payload,
                                cumulative,
                                end,
                                saved,
                                invalid_saved,
                            }));
                        }
                        jig_core::Now::AcceptedTurn { payload, task, attempt, turn, accepted } => {
                            let payload = take_payload(domain, payload).expect("charged turn owns payload");
                            let body = match payload {
                                Payload::Turn { body, .. } => body,
                                Payload::Answer { .. } | Payload::Call { .. } | Payload::CallAnswer(_) => {
                                    unreachable!("turn family")
                                }
                            };
                            domain.work.push(Work::Core(jig_core::Event::AcceptedTurn {
                                task,
                                attempt,
                                turn,
                                accepted,
                                cumulative: body.cumulative,
                                read: body.read,
                                transcript: body.transcript,
                            }));
                        }
                        jig_core::Now::RefusedPayload { request, problem } => {
                            let payload = match take_payload(domain, request) {
                                Some(Payload::Turn { task, attempt, body }) => {
                                    Some(jig_core::PayloadRefusal::Turn { task, attempt, turn: body.number })
                                }
                                Some(Payload::Answer { task, attempt, .. }) => {
                                    Some(jig_core::PayloadRefusal::Answer { task, attempt })
                                }
                                Some(Payload::Call { .. } | Payload::CallAnswer(_)) => {
                                    unreachable!("task refusal owns a task payload")
                                }
                                None => None,
                            };
                            domain.work.push(Work::Core(jig_core::Event::RefusedPayload { request, problem, payload }));
                        }
                        jig_core::Now::Activate { context } => {
                            let ready = domain.ready();
                            domain.work.push(Work::Core(jig_core::Event::Activate { context, ready }));
                        }
                        jig_core::Now::PrepareAgent { context } => {
                            let (transcript_waiter, busy) = if context.ever_turned {
                                match domain.result_reads.insert(Some(Read::Transcript { task: context.task })) {
                                    Ok(waiter) => (Some(waiter.token()), false),
                                    Err(_) => (None, true),
                                }
                            } else {
                                (None, false)
                            };
                            domain.work.push(Work::Core(jig_core::Event::PreparedAgent {
                                context,
                                transcript_waiter,
                                busy,
                            }));
                        }
                        jig_core::Now::StartPreparation { task, transcript_waiter } => {
                            domain.work.push(Work::Tasks(tasks::Event::Prepare { reply_to: internal(task), task }));
                            match transcript_waiter {
                                Some(waiter) => emit(
                                    decision,
                                    &env.limits,
                                    Delivery::Load { waiter, range: Range::TaskTranscript { task }, after: None },
                                ),
                                None => {
                                    if let Some((waiter, first)) = begin_dependency_read(domain, task) {
                                        emit(
                                            decision,
                                            &env.limits,
                                            Delivery::Load {
                                                waiter,
                                                range: Range::TaskResult { task: first },
                                                after: None,
                                            },
                                        );
                                    }
                                }
                            }
                        }
                        jig_core::Now::HistoricalProposal { request, person, project, proposer, proposal } => {
                            proposals::historical_begin(
                                domain, env, decision, request, person, project, proposer, proposal,
                            );
                        }
                        jig_core::Now::BriefCorePlanned { task, parent, sections } => {
                            finish_brief_plan(domain, env, task, parent, sections);
                        }
                        jig_core::Now::WorkspaceRequest { task, attempt, context } => {
                            let workspace = forge_route::run_workspace(domain, env, &context, attempt);
                            let writes = match workspace {
                                Some(workspace) => {
                                    match forge_route::claim_names(domain, &env.limits, task, &workspace.names) {
                                        Some(claim_names) => {
                                            let writes = workspace.writes.clone();
                                            let inserted = match domain
                                                .run_workspaces
                                                .insert(task, PreparedWorkspace { workspace, claim_names })
                                            {
                                                Ok(None) => true,
                                                Ok(Some(_)) | Err(_) => false,
                                            };
                                            assert!(inserted, "one workspace flight per task");
                                            Some(writes)
                                        }
                                        None => None,
                                    }
                                }
                                None => None,
                            };
                            domain.work.push(Work::Core(jig_core::Event::WorkspacePrepared { task, attempt, writes }));
                        }
                        jig_core::Now::RunPreparationFailed { task } => {
                            drop(domain.brief_sections.remove(&task));
                            drop(domain.run_workspaces.remove(&task));
                        }
                        jig_core::Now::RunPrepared {
                            task,
                            attempt,
                            charter,
                            run,
                            inbox,
                            saved,
                            transcript,
                            answered,
                            grant,
                        } => {
                            let sections = domain.brief_sections.remove(&task).expect("prepared brief sections");
                            let PreparedWorkspace { workspace, claim_names } =
                                domain.run_workspaces.remove(&task).expect("prepared connector workspace");
                            let mut records = List::with_capacity(domain.limits.call_records);
                            for key in answered {
                                let answer = call_answer(domain, key).expect("live call part has its connector answer");
                                records.push(crate::CallRecord { key, answer }).expect("retained call bound");
                            }
                            let assignment = Assignment {
                                task,
                                attempt,
                                charter,
                                run,
                                sections,
                                inbox,
                                saved: forge_route::saved_tags(&saved, domain.config.forge_connector)
                                    .expect("task saved names were admitted by the root"),
                                workspace: workspace.workspace.clone(),
                                transcript,
                                answered: records.into_boxed(),
                                grant,
                            };
                            let budget = assignment.run.budget;
                            assert!(
                                domain.assignments.insert(task, assignment).is_ok(),
                                "assignment fits live task room"
                            );
                            if workspace.names.is_empty() {
                                domain.work.push(Work::Core(jig_core::Event::ClaimPrepared {
                                    task,
                                    attempt,
                                    budget,
                                    writes: Box::new([]),
                                }));
                            } else {
                                let mut hub_writes = List::with_capacity(env.limits.tasks.holdings);
                                for name in &workspace.names {
                                    let resource =
                                        forge_route::hub_name(domain.config.forge_connector, name, &env.limits.tasks)
                                            .expect("admitted forge branch name fits the hub's bound");
                                    hub_writes.push(resource).expect("workspace write bound fits hub");
                                }
                                domain.work.push(Work::Forge(forge::Event::Names { task, resources: claim_names }));
                                for name in workspace.own_holds {
                                    domain.work.push(Work::Forge(forge::Event::Hold {
                                        task,
                                        resource: name,
                                        from: None,
                                    }));
                                }
                                domain.work.push(Work::Core(jig_core::Event::ClaimPrepared {
                                    task,
                                    attempt,
                                    budget,
                                    writes: hub_writes.into_boxed(),
                                }));
                            }
                        }
                        jig_core::Now::CompleteBrief { brief, order } => {
                            let mut child = Queue::with_capacity(1);
                            child.push(brief::GatherRequest::Complete { brief, order });
                            brief_outputs(domain, env, decision, &mut child);
                        }
                        jig_core::Now::ProcedureDelegateOutcome { task, step, child } => {
                            if let Some(kind) = domain.forge_delegating.remove(&(task, step))
                                && let Some(child) = child
                            {
                                domain.work.push(Work::Forge(forge::Event::Delegated { task, child, kind }));
                            }
                        }
                        jig_core::Now::HistoricalEscalation { request, person, project, task, revision } => {
                            escalation::historical_begin(
                                domain, env, decision, request, person, project, task, revision,
                            );
                        }
                        jig_core::Now::EscalationInspection { waiter, context } => {
                            escalation::inspected(domain, waiter, context);
                        }
                        jig_core::Now::EscalationReply { to, person, context } => {
                            emit(decision, &env.limits, Delivery::EscalationReply { to, person, context });
                        }
                        jig_core::Now::EscalationRefused { to, why } => {
                            emit(
                                decision,
                                &env.limits,
                                Delivery::WebReply { to, sign_in: None, reply: people::Reply::Refused(why) },
                            );
                        }
                        jig_core::Now::CallPayload { to, run, attempt, body } => {
                            relay_payload(domain, env, decision, to, run, attempt, body);
                        }
                        jig_core::Now::DropAssignment { task } => {
                            drop(domain.assignments.remove(&task));
                        }
                        jig_core::Now::RestoreRefused => domain.startup = Startup::Failed,
                        jig_core::Now::Account(_)
                        | jig_core::Now::View(_)
                        | jig_core::Now::NotesIndexed { .. }
                        | jig_core::Now::NotesRecalled { .. }
                        | jig_core::Now::NotesRefused { .. } => {
                            unreachable!("the current application has no immediate core route")
                        }
                    },
                    jig_core::Request::Decided => {}
                }
            }
        }
    }
}

fn route_into(domain: &mut Domain, env: &Env<Limits>, decision: &mut Decision) {
    for _ in 0..route_bound(&env.limits).expect("valid route bound") {
        let Some(work) = domain.work.pop() else {
            break;
        };
        match work {
            Work::Core(event) => {
                let routed = jig_core::step(&mut domain.core, &environment_core(env), event);
                route_core_requests(domain, env, decision, routed);
            }
            Work::Tasks(event) => {
                let routed = jig_core::step(&mut domain.core, &environment_core(env), jig_core::Event::Tasks(event));
                route_core_requests(domain, env, decision, routed);
            }
            Work::People(event) => {
                let routed = jig_core::step(&mut domain.core, &environment_core(env), jig_core::Event::People(event));
                route_core_requests(domain, env, decision, routed);
            }
            Work::Fleet(event) => {
                let routed = jig_core::step(&mut domain.core, &environment_core(env), jig_core::Event::Fleet(event));
                route_core_requests(domain, env, decision, routed);
            }
            Work::Brief(event) => {
                let routed = jig_core::step(&mut domain.core, &environment_core(env), jig_core::Event::Brief(event));
                route_core_requests(domain, env, decision, routed);
            }
            Work::StartBrief { task } => domain.work.push(Work::Core(jig_core::Event::StartBrief { task })),
            Work::Forge(event) => {
                let mut out = Queue::with_capacity(forge::max_out(&env.limits.forge));
                forge::step(&mut domain.forge, &environment_forge(env), event, &mut out);
                forge_route::outputs(domain, env, decision, &mut out);
            }
            Work::ProjectGoal(goal) => forge_route::project_goal(domain, env, &goal),
            Work::GoalSubscribe(subscriber) => forge_route::goal_subscribed(domain, env, subscriber),
            Work::Activate(context) => {
                let ready = domain.ready();
                let routed = jig_core::step(
                    &mut domain.core,
                    &environment_core(env),
                    jig_core::Event::Activate { context, ready },
                );
                route_core_requests(domain, env, decision, routed);
            }
            Work::EscalationLoaded { waiter, rows } => escalation::loaded(domain, env, waiter, rows),
            Work::EscalationFailed { waiter } => escalation::failed(domain, waiter),
            Work::ProposalLoaded { waiter, rows } => proposals::historical_loaded(domain, waiter, rows),
            Work::ProposalFailed { waiter } => proposals::historical_failed(domain, waiter),
        }
    }
    assert!(domain.work.is_empty(), "finite synchronous root handoffs finish within the configured route bound");
}

fn call_answer(domain: &Domain, key: CallKey) -> Option<CallAnswer> {
    match domain.core.replay_call(key) {
        Some(jig_core::CallReplay::Core(part)) => CallAnswer::from_core(&part),
        Some(jig_core::CallReplay::Connector { .. }) => domain.connector_calls.get(&key).cloned(),
        None => None,
    }
}

fn relay_call(domain: &mut Domain, limits: &Limits, decision: &mut Decision, to: ReplyTo, answer: CallAnswer) {
    let id = domain.payloads.insert(Some(Payload::CallAnswer(answer))).expect("answer payload room reserved");
    emit(decision, limits, Delivery::Fleet(fleet::Event::Relayed { to, answer: id.token() }));
}

fn call_needs_input(tool: &Tool) -> bool {
    match tool {
        Tool::Unavailable
        | Tool::Rejected(_)
        | Tool::RejectedMessage(_)
        | Tool::RejectedControl(_)
        | Tool::RejectedProposal(_)
        | Tool::Message { .. }
        | Tool::Introduce { .. }
        | Tool::Subscribe { .. }
        | Tool::SubscribeForge { .. }
        | Tool::ReadForge { .. }
        | Tool::EffectForge { .. }
        | Tool::Unsubscribe { .. }
        | Tool::Cancel { .. }
        | Tool::Release { .. }
        | Tool::Amend { .. }
        | Tool::Propose { .. }
        | Tool::Decide { .. }
        | Tool::Withdraw { .. }
        | Tool::DecideEscalation { .. } => false,
        Tool::Delegate { batch } => {
            for member in batch {
                if !member.spec.inputs.is_empty() {
                    return true;
                }
            }
            false
        }
    }
}

#[expect(clippy::too_many_lines, reason = "one tool shape entrance preflights every bounded call")]
fn call_shape(tool: &Tool, limits: &tasks::Limits) -> Option<tasks::Refusal> {
    match tool {
        Tool::Propose { action, reason, .. } => {
            if reason.len() > usize::try_from(limits.message_bytes).expect("u32 fits usize") {
                return Some(tasks::Refusal::Read);
            }
            match action {
                ProposedAction::Batch(batch) => {
                    if batch.is_empty() || batch.len() > usize::try_from(limits.batch).expect("u32 fits usize") {
                        return Some(tasks::Refusal::Batch);
                    }
                    for member in batch {
                        if !tasks::valid_spec(limits, &member.spec) || !member.spec.inputs.is_empty() {
                            return Some(tasks::Refusal::Spec);
                        }
                        if !tasks::valid_contract(limits, &member.contract)
                            || !tasks::valid_authority(limits, &member.authority)
                            || member.dependencies.len() > usize::try_from(limits.dependencies).expect("u32 fits usize")
                        {
                            return Some(tasks::Refusal::AuthorityShape);
                        }
                    }
                    None
                }
                ProposedAction::Amend { amendment, .. } => {
                    if amendment.reason.len() > usize::try_from(limits.message_bytes).expect("u32 fits usize") {
                        Some(tasks::Refusal::Read)
                    } else if match &amendment.authority {
                        Some(authority) => !tasks::valid_authority(limits, authority),
                        None => true,
                    } {
                        Some(tasks::Refusal::AuthorityShape)
                    } else {
                        None
                    }
                }
                ProposedAction::Widen { authority, .. } => {
                    if tasks::valid_authority(limits, authority) {
                        None
                    } else {
                        Some(tasks::Refusal::AuthorityShape)
                    }
                }
                ProposedAction::Release { .. } => None,
            }
        }
        Tool::Decide { decision, .. } => match decision {
            ProposalChoice::Reject { reason }
                if reason.len() > usize::try_from(limits.message_bytes).expect("u32 fits usize") =>
            {
                Some(tasks::Refusal::Read)
            }
            ProposalChoice::Accept | ProposalChoice::Reject { .. } | ProposalChoice::Pass => None,
        },
        Tool::DecideEscalation { decision, .. } => match decision {
            EscalationChoice::Reject { reason }
                if reason.len() > usize::try_from(limits.result_bytes).expect("u32 fits usize") =>
            {
                Some(tasks::Refusal::Read)
            }
            EscalationChoice::Release | EscalationChoice::Reject { .. } | EscalationChoice::Pass => None,
        },
        Tool::Withdraw { .. }
        | Tool::Unavailable
        | Tool::Rejected(_)
        | Tool::RejectedMessage(_)
        | Tool::RejectedControl(_)
        | Tool::RejectedProposal(_)
        | Tool::Introduce { .. }
        | Tool::Subscribe { .. }
        | Tool::SubscribeForge { .. }
        | Tool::ReadForge { .. }
        | Tool::EffectForge { .. }
        | Tool::Unsubscribe { .. }
        | Tool::Release { .. } => None,
        Tool::Cancel { reason, .. } => {
            if reason.len() > usize::try_from(limits.result_bytes).expect("u32 fits usize") {
                Some(tasks::Refusal::Read)
            } else {
                None
            }
        }
        Tool::Amend { amendment, .. } => {
            if amendment.reason.len() > usize::try_from(limits.message_bytes).expect("u32 fits usize") {
                return Some(tasks::Refusal::Read);
            }
            if let Some(spec) = &amendment.spec
                && (!tasks::valid_spec(limits, spec) || !spec.inputs.is_empty())
            {
                return Some(tasks::Refusal::Spec);
            }
            if let Some(authority) = &amendment.authority
                && !tasks::valid_authority(limits, authority)
            {
                return Some(tasks::Refusal::AuthorityShape);
            }
            if let Some(dependencies) = &amendment.dependencies
                && dependencies.len() > usize::try_from(limits.dependencies).expect("u32 fits usize")
            {
                return Some(tasks::Refusal::Dependencies);
            }
            None
        }
        Tool::Message { form, words, .. } => {
            if words.is_empty() || words.len() > usize::try_from(limits.message_bytes).expect("u32 fits usize") {
                return Some(tasks::Refusal::Read);
            }
            match form {
                MessageForm::Answer { question } if *question == 0 => return Some(tasks::Refusal::Read),
                MessageForm::Words | MessageForm::Question | MessageForm::Answer { .. } => {}
            }
            None
        }
        Tool::Delegate { batch } => {
            if batch.len() > usize::try_from(limits.batch).expect("u32 fits usize") {
                return Some(tasks::Refusal::Batch);
            }
            for member in batch {
                if !tasks::valid_spec(limits, &member.spec) {
                    return Some(tasks::Refusal::Spec);
                }
                if !tasks::valid_contract(limits, &member.contract) {
                    return Some(tasks::Refusal::Contract);
                }
                if !tasks::valid_authority(limits, &member.authority) {
                    return Some(tasks::Refusal::AuthorityShape);
                }
                if member.dependencies.len() > usize::try_from(limits.dependencies).expect("u32 fits usize") {
                    return Some(tasks::Refusal::Dependencies);
                }
            }
            None
        }
    }
}

fn decide_call(
    domain: &mut Domain,
    limits: &Limits,
    decision: &mut Decision,
    to: ReplyTo,
    key: CallKey,
    answer: CallAnswer,
) {
    let part = answer.core_part(domain.config.forge_connector);
    let connector_owned = match &part {
        jig_core::CallPart::Connector { .. } => true,
        jig_core::CallPart::EffectDenied { .. }
        | jig_core::CallPart::EscalationDecided { .. }
        | jig_core::CallPart::EscalationRefused(_)
        | jig_core::CallPart::Proposed { .. }
        | jig_core::CallPart::ProposalDecided { .. }
        | jig_core::CallPart::ProposalRefused(_)
        | jig_core::CallPart::Controlled
        | jig_core::CallPart::ControlRefused(_)
        | jig_core::CallPart::ControlDenied { .. }
        | jig_core::CallPart::Sent { .. }
        | jig_core::CallPart::Introduced
        | jig_core::CallPart::MessageRefused(_)
        | jig_core::CallPart::Subscribed { .. }
        | jig_core::CallPart::Unsubscribed
        | jig_core::CallPart::SubscriptionRefused(_)
        | jig_core::CallPart::Delegated(_)
        | jig_core::CallPart::DelegationDenied { .. }
        | jig_core::CallPart::DelegationRefused(_)
        | jig_core::CallPart::Unavailable => false,
    };
    if !domain.core.decide_named_call(key, part) {
        relay_call(domain, limits, decision, to, answer);
        return;
    }
    if connector_owned {
        assert!(domain.connector_calls.insert(key, answer.clone()) == Ok(None), "connector answer room reserved");
    }
    save(decision, limits, Write::Save(Record::Call(crate::CallRecord { key, answer: answer.clone() })));
    relay_call(domain, limits, decision, to, answer);
}

#[expect(clippy::too_many_arguments, reason = "validated historical input pointers travel with the delegated call")]
fn delegate_call(
    domain: &mut Domain,
    env: &Env<Limits>,
    decision: &mut Decision,
    to: ReplyTo,
    key: CallKey,
    batch: Box<[Delegate]>,
    validated: bool,
    stubs: Box<[tasks::Stub]>,
) {
    if let Err(part) = domain.core.delegate_preflight(&env.limits.tasks, key, &batch) {
        domain.work.push(Work::Core(jig_core::Event::NamedAnswer { to, key, part }));
        return;
    }
    if !validated {
        let (project, ids) = match domain.core.delegate_inputs(&env.limits.tasks, key, &batch) {
            Ok(value) => value,
            Err(part) => {
                domain.work.push(Work::Core(jig_core::Event::NamedAnswer { to, key, part }));
                return;
            }
        };
        if let Some(&first) = ids.first() {
            let capacity = batch
                .len()
                .checked_mul(usize::try_from(env.limits.tasks.inputs).expect("u32 fits usize"))
                .expect("bounded batch input count");
            let to = to.into_token();
            let read = InputCheck {
                to,
                key,
                batch,
                ids,
                at: 0,
                project,
                stubs: List::with_capacity(u32::try_from(capacity).expect("bounded input IDs")),
            };
            let waiter =
                domain.result_reads.insert(Some(Read::InputCheck(read))).expect("preflighted input read slot").token();
            assert!(domain.core.reserve_connector_call(key), "reserved call record room");
            emit(
                decision,
                &env.limits,
                Delivery::Load { waiter, range: Range::TaskResult { task: first }, after: None },
            );
            return;
        }
    }
    domain.work.push(Work::Core(jig_core::Event::DelegateValidated { to, key, batch, stubs }));
}

fn brief_outputs(
    domain: &mut Domain,
    env: &Env<Limits>,
    _decision: &mut Decision,
    out: &mut Queue<brief::GatherRequest>,
) {
    for _ in 0..out.len() {
        match out.pop().expect("brief output count") {
            brief::GatherRequest::Gather { connector: 0, section, budget } => {
                if domain.brief_connectors.get(Id::from_token(section)).is_none() {
                    continue;
                }
                domain.work.push(Work::Forge(forge::Event::GatherPlanned {
                    section,
                    parts: env.limits.brief.parts,
                    bytes: budget,
                    ci_budget: env.limits.brief.budgets.ci,
                }));
            }
            brief::GatherRequest::CutTo { connector: 0, section, size } => {
                if let Some(row) = domain.brief_connectors.get_mut(Id::from_token(section)) {
                    row.cutting = true;
                    domain.work.push(Work::Forge(forge::Event::CutBrief { section, bytes: size }));
                }
            }
            brief::GatherRequest::Drop { connector: 0, section } => {
                let id = Id::from_token(section);
                if domain.brief_connectors.get(id).is_some() {
                    domain.brief_connectors.retire(id);
                    domain.work.push(Work::Forge(forge::Event::DropBrief { section }));
                }
            }
            brief::GatherRequest::Gather { .. }
            | brief::GatherRequest::CutTo { .. }
            | brief::GatherRequest::Drop { .. } => unreachable!("temper's sole connector is numbered zero"),
            brief::GatherRequest::Complete { brief, order } => {
                let task = brief.raw();
                let mut sections = List::with_capacity(u32::try_from(order.len()).expect("bounded section count"));
                let mut missing_owner = false;
                for placed in order {
                    let section = match placed {
                        brief::GatherPlaced::Core { kind, text } => {
                            BriefSection { kind: BriefKind::Core(kind), body: BriefBody::Text(text) }
                        }
                        brief::GatherPlaced::CoreMissing { kind, why } => {
                            BriefSection { kind: BriefKind::Core(kind), body: BriefBody::Missing(why) }
                        }
                        brief::GatherPlaced::Connector { token, size, .. } => {
                            let id = Id::from_token(token);
                            let Some(row) = domain.brief_connectors.get(id) else {
                                missing_owner = true;
                                continue;
                            };
                            let kind = row.kind;
                            domain.brief_connectors.retire(id);
                            let bytes = domain.forge.take_brief(token);
                            match bytes {
                                Some(bytes) if bytes.len() == usize::try_from(size).expect("bounded section") => {
                                    BriefSection { kind: BriefKind::Forge(kind), body: BriefBody::Text(bytes) }
                                }
                                Some(_) | None => {
                                    missing_owner = true;
                                    continue;
                                }
                            }
                        }
                        brief::GatherPlaced::Missing { kind, why, .. } => BriefSection {
                            kind: BriefKind::Forge(numbered_brief_kind(kind)),
                            body: BriefBody::Missing(why),
                        },
                    };
                    sections.push(section).expect("bounded brief sections");
                }
                if missing_owner {
                    domain.work.push(Work::Core(jig_core::Event::BriefAssembled { task, ready: false }));
                    continue;
                }
                assert!(domain.brief_sections.insert(task, sections.into_boxed()) == Ok(None), "one brief assembly");
                domain.work.push(Work::Core(jig_core::Event::BriefAssembled { task, ready: true }));
            }
            brief::GatherRequest::Failed { .. } | brief::GatherRequest::Refused { .. } => {
                unreachable!("the core abandons refused brief preparation")
            }
        }
    }
}

fn numbered_brief_kind(kind: u16) -> ForgeBriefKind {
    match kind {
        1 => ForgeBriefKind::Ci,
        2 => ForgeBriefKind::Reviews,
        3 => ForgeBriefKind::Pull,
        _ => unreachable!("temper's forge connector uses three section kinds"),
    }
}

fn take_payload(domain: &mut Domain, token: Token) -> Option<Payload> {
    let id = Id::from_token(token);
    let payload = domain.payloads.get_mut(id)?.take()?;
    domain.payloads.retire(id);
    Some(payload)
}

#[expect(
    clippy::too_many_lines,
    reason = "the root translates every temper host tool into core or connector vocabulary"
)]
fn relay_payload(
    domain: &mut Domain,
    env: &Env<Limits>,
    decision: &mut Decision,
    reply_to: ReplyTo,
    run: Token,
    attempt: Token,
    body: Token,
) {
    let Some(Payload::Call { key, body }) = take_payload(domain, body) else {
        unreachable!("fleet returns admitted call payload")
    };
    assert!(key.task == run.raw() && key.attempt == attempt.raw(), "fleet call envelope is unchanged");
    assert!(current_proof(domain, key.task, key.attempt), "fleet only relays a current claim");
    match call_answer(domain, key) {
        Some(answer) => relay_call(domain, &env.limits, decision, reply_to, answer),
        None => match body.tool {
            Tool::Unavailable => {
                domain.work.push(Work::Core(jig_core::Event::NamedAnswer {
                    to: reply_to,
                    key,
                    part: jig_core::CallPart::Unavailable,
                }));
            }
            Tool::Rejected(why) => domain.work.push(Work::Core(jig_core::Event::NamedAnswer {
                to: reply_to,
                key,
                part: jig_core::CallPart::DelegationRefused(tasks::Problem { task: None, why, blocked_by: None }),
            })),
            Tool::RejectedMessage(why) => domain.work.push(Work::Core(jig_core::Event::NamedAnswer {
                to: reply_to,
                key,
                part: jig_core::CallPart::MessageRefused(tasks::Problem { task: None, why, blocked_by: None }),
            })),
            Tool::RejectedControl(why) => domain.work.push(Work::Core(jig_core::Event::NamedAnswer {
                to: reply_to,
                key,
                part: jig_core::CallPart::ControlRefused(tasks::Problem { task: None, why, blocked_by: None }),
            })),
            Tool::RejectedProposal(why) => domain.work.push(Work::Core(jig_core::Event::NamedAnswer {
                to: reply_to,
                key,
                part: jig_core::CallPart::ProposalRefused(tasks::Problem { task: None, why, blocked_by: None }),
            })),
            Tool::Delegate { batch } => {
                delegate_call(domain, env, decision, reply_to, key, batch, false, Box::new([]));
            }
            Tool::Propose { action, reason, as_holder } => {
                proposals::propose_call(domain, reply_to, key, action, reason, as_holder);
            }
            Tool::Decide { proposer, proposal, decision: choice } => {
                domain.work.push(Work::Core(jig_core::Event::NamedAction {
                    to: reply_to,
                    key,
                    action: jig_core::NamedAction::DecideProposal {
                        proposer,
                        proposal,
                        choice: match choice {
                            ProposalChoice::Accept => jig_core::ProposalChoice::Accept,
                            ProposalChoice::Reject { reason } => jig_core::ProposalChoice::Reject { reason },
                            ProposalChoice::Pass => jig_core::ProposalChoice::Pass,
                        },
                    },
                }));
            }
            Tool::Withdraw { proposal } => {
                domain.work.push(Work::Core(jig_core::Event::NamedAction {
                    to: reply_to,
                    key,
                    action: jig_core::NamedAction::WithdrawProposal { proposal },
                }));
            }
            Tool::DecideEscalation { task, revision, decision: choice } => {
                domain.work.push(Work::Core(jig_core::Event::NamedAction {
                    to: reply_to,
                    key,
                    action: jig_core::NamedAction::DecideEscalation {
                        task,
                        revision,
                        choice: match choice {
                            EscalationChoice::Release => jig_core::EscalationChoice::Release,
                            EscalationChoice::Reject { reason } => jig_core::EscalationChoice::Reject { reason },
                            EscalationChoice::Pass => jig_core::EscalationChoice::Pass,
                        },
                    },
                }));
            }
            Tool::Amend { target, amendment } => {
                domain.work.push(Work::Core(jig_core::Event::NamedAction {
                    to: reply_to,
                    key,
                    action: jig_core::NamedAction::Amend { target, amendment },
                }));
            }
            Tool::Cancel { target, reason } => {
                domain.work.push(Work::Core(jig_core::Event::NamedAction {
                    to: reply_to,
                    key,
                    action: jig_core::NamedAction::Control { target, control: tasks::Control::Cancel { reason } },
                }));
            }
            Tool::Release { target } => {
                domain.work.push(Work::Core(jig_core::Event::NamedAction {
                    to: reply_to,
                    key,
                    action: jig_core::NamedAction::Control { target, control: tasks::Control::Release },
                }));
            }
            Tool::Message { target, form, words } => {
                let kind = match form {
                    MessageForm::Words => tasks::MessageKind::Words,
                    MessageForm::Question => tasks::MessageKind::Question,
                    MessageForm::Answer { question } => tasks::MessageKind::Answer { question },
                };
                domain.work.push(Work::Core(jig_core::Event::NamedAction {
                    to: reply_to,
                    key,
                    action: jig_core::NamedAction::Message { target, kind, words },
                }));
            }
            Tool::Introduce { left, right } => {
                domain.work.push(Work::Core(jig_core::Event::NamedAction {
                    to: reply_to,
                    key,
                    action: jig_core::NamedAction::Introduce { left, right },
                }));
            }
            Tool::Subscribe { kind } => domain.work.push(Work::Core(jig_core::Event::NamedAction {
                to: reply_to,
                key,
                action: jig_core::NamedAction::Subscribe { kind },
            })),
            Tool::SubscribeForge { topic, own_change, paths } => {
                forge_route::subscribe_call(domain, env, decision, reply_to, key, topic, own_change, paths);
            }
            Tool::ReadForge { repository, read } => {
                forge_route::read_call(domain, env, decision, reply_to, key, repository, read);
            }
            Tool::EffectForge { repository, resource, write } => {
                forge_route::effect_call(domain, env, decision, reply_to, key, repository, resource, *write);
            }
            Tool::Unsubscribe { subscription } => {
                let token = reply_to.into_token();
                if let Some(topic) = domain.forge.subscription(key.task, subscription) {
                    assert!(
                        domain.forge_unsubscribing.insert(token, (key.task, topic)) == Ok(None),
                        "one connector unsubscribe"
                    );
                }
                domain.work.push(Work::Core(jig_core::Event::NamedAction {
                    to: ReplyTo::new(token),
                    key,
                    action: jig_core::NamedAction::Unsubscribe { subscription },
                }));
            }
        },
    }
}

fn request_load(domain: &mut Domain, waiter: Token, range: Range, after: Option<Key>, out: &mut Queue<Request>) {
    let mut load_out = Queue::with_capacity(1);
    let most = match range {
        Range::Deployment
        | Range::TaskResult { .. }
        | Range::EscalationDecision { .. }
        | Range::ProposalDecision { .. } => 1,
        Range::Calls
        | Range::Tasks
        | Range::EndedResults
        | Range::People
        | Range::Forge
        | Range::RunProofs
        | Range::Turns { .. }
        | Range::TaskTranscript { .. } => domain.limits.loads.rows,
    };
    if loads::begin(&mut domain.loads, waiter, range, after, most, &mut load_out).is_none() {
        match range {
            Range::TaskTranscript { .. } => {
                transcript_failed(domain, waiter);
                return;
            }
            Range::ProposalDecision { .. }
            | Range::EscalationDecision { .. }
            | Range::Calls
            | Range::Deployment
            | Range::Tasks
            | Range::EndedResults
            | Range::RunProofs
            | Range::People
            | Range::Forge
            | Range::TaskResult { .. }
            | Range::Turns { .. } => {}
        }
        match range {
            Range::ProposalDecision { .. } => {
                domain.work.push(Work::ProposalFailed { waiter });
                return;
            }
            Range::EscalationDecision { .. } => {
                domain.work.push(Work::EscalationFailed { waiter });
                return;
            }
            Range::Calls
            | Range::Deployment
            | Range::Tasks
            | Range::EndedResults
            | Range::People
            | Range::Forge
            | Range::RunProofs
            | Range::Turns { .. }
            | Range::TaskTranscript { .. }
            | Range::TaskResult { .. } => {
                unreachable!("startup/result load room reserved")
            }
        }
    }
    match load_out.pop().expect("load issued") {
        loads::Request::Load { owner, range, after, most, bytes } => {
            out.push(Request::Load { owner, range, after, most, bytes });
        }
        loads::Request::Loaded { .. } | loads::Request::Unloaded { .. } => unreachable!("begin only issues IO"),
    }
}

fn transcript_waiter(domain: &Domain, waiter: Token) -> bool {
    match domain.result_reads.get(Id::from_token(waiter)) {
        Some(Some(Read::Transcript { .. })) => true,
        Some(
            Some(
                Read::Result(_)
                | Read::Inbox(_)
                | Read::Escalation(_)
                | Read::Proposal(_)
                | Read::Dependency(_)
                | Read::InputCheck(_),
            )
            | None,
        )
        | None => false,
    }
}

fn proposal_waiter(domain: &Domain, waiter: Token) -> bool {
    match domain.result_reads.get(Id::from_token(waiter)) {
        Some(Some(Read::Proposal(_))) => true,
        Some(
            Some(
                Read::Result(_)
                | Read::Inbox(_)
                | Read::Escalation(_)
                | Read::Transcript { .. }
                | Read::Dependency(_)
                | Read::InputCheck(_),
            )
            | None,
        )
        | None => false,
    }
}

fn inbox_waiter(domain: &Domain, waiter: Token) -> bool {
    match domain.result_reads.get(Id::from_token(waiter)) {
        Some(Some(Read::Inbox(_))) => true,
        Some(
            Some(
                Read::Result(_)
                | Read::Escalation(_)
                | Read::Proposal(_)
                | Read::Transcript { .. }
                | Read::Dependency(_)
                | Read::InputCheck(_),
            )
            | None,
        )
        | None => false,
    }
}

fn dependency_waiter(domain: &Domain, waiter: Token) -> bool {
    match domain.result_reads.get(Id::from_token(waiter)) {
        Some(Some(Read::Dependency(_))) => true,
        Some(
            Some(
                Read::Result(_)
                | Read::Inbox(_)
                | Read::Escalation(_)
                | Read::Proposal(_)
                | Read::Transcript { .. }
                | Read::InputCheck(_),
            )
            | None,
        )
        | None => false,
    }
}

fn input_waiter(domain: &Domain, waiter: Token) -> bool {
    match domain.result_reads.get(Id::from_token(waiter)) {
        Some(Some(Read::InputCheck(_))) => true,
        Some(
            Some(
                Read::Result(_)
                | Read::Inbox(_)
                | Read::Escalation(_)
                | Read::Proposal(_)
                | Read::Transcript { .. }
                | Read::Dependency(_),
            )
            | None,
        )
        | None => false,
    }
}

fn input_failed(domain: &mut Domain, waiter: Token) {
    let Some(Read::InputCheck(read)) = take_read(domain, waiter) else { return };
    domain.result_reads.retire(Id::from_token(waiter));
    domain.work.push(Work::Core(jig_core::Event::DelegateInputFailed { to: ReplyTo::new(read.to), key: read.key }));
}

fn input_loaded(domain: &mut Domain, waiter: Token, rows: Box<[Record]>, next: Option<Key>, out: &mut Queue<Request>) {
    let Some(Some(Read::InputCheck(read))) = domain.result_reads.get(Id::from_token(waiter)) else { return };
    let wanted = *read.ids.get(usize::try_from(read.at).expect("bounded input index")).expect("requested input");
    let project = read.project;
    let creator = read.key.task;
    if next.is_some() || rows.len() != 1 {
        input_failed(domain, waiter);
        return;
    }
    let row = match rows.into_iter().next().expect("one input result row") {
        Record::Tasks(tasks::Stored::Ended(row)) => row,
        Record::Call(_)
        | Record::Deployment(_)
        | Record::EscalationDecision(_)
        | Record::ProposalDecision(_)
        | Record::Turn(_)
        | Record::RunProof(_)
        | Record::Terminal(_)
        | Record::Tasks(_)
        | Record::People(_)
        | Record::Forge { .. } => {
            input_failed(domain, waiter);
            return;
        }
    };
    let Some(stub) = domain.core.input_stub(creator, project, wanted, row) else {
        input_failed(domain, waiter);
        return;
    };
    let Some(Some(Read::InputCheck(read))) = domain.result_reads.get_mut(Id::from_token(waiter)) else {
        unreachable!("input read survives validation")
    };
    read.stubs.push(stub).expect("bounded historical input count");
    read.at = read.at.checked_add(1).expect("bounded input index");
    if let Some(&next) = read.ids.get(usize::try_from(read.at).expect("bounded input index")) {
        request_load(domain, waiter, Range::TaskResult { task: next }, None, out);
        return;
    }
    let Some(Read::InputCheck(read)) = take_read(domain, waiter) else { unreachable!("completed input read") };
    domain.result_reads.retire(Id::from_token(waiter));
    domain.work.push(Work::Core(jig_core::Event::DelegateValidated {
        to: ReplyTo::new(read.to),
        key: read.key,
        batch: read.batch,
        stubs: read.stubs.into_boxed(),
    }));
}

#[expect(clippy::too_many_lines, reason = "one store terminal dispatcher covers every live read owner")]
fn load_outputs(
    domain: &mut Domain,
    env: &Env<Limits>,
    load_out: &mut Queue<loads::Request>,
    out: &mut Queue<Request>,
) {
    for _ in 0..load_out.len() {
        match load_out.pop().expect("load terminal count") {
            loads::Request::Loaded { waiter, rows, next, cut } => {
                if input_waiter(domain, waiter) {
                    if cut.is_some() {
                        input_failed(domain, waiter);
                    } else {
                        input_loaded(domain, waiter, rows, next, out);
                    }
                    return;
                }
                if dependency_waiter(domain, waiter) {
                    if cut.is_some() {
                        dependency_failed(domain, waiter);
                    } else {
                        dependency_loaded(domain, waiter, rows, next, out);
                    }
                    return;
                }
                if transcript_waiter(domain, waiter) {
                    transcript_loaded(domain, waiter, rows, next, cut, out);
                    return;
                }
                if inbox_waiter(domain, waiter) {
                    if cut.is_some() {
                        inbox::failed(domain, waiter, people::Refusal::Limit, out);
                    } else {
                        domain.result_pages.push(ResultPage { waiter, rows, next });
                    }
                    return;
                }
                if proposal_waiter(domain, waiter) {
                    if cut.is_some() || next.is_some() {
                        domain.work.push(Work::ProposalFailed { waiter });
                    } else {
                        domain.work.push(Work::ProposalLoaded { waiter, rows });
                    }
                    return;
                }
                if cut.is_some() {
                    let archive = match domain.result_reads.get(Id::from_token(waiter)) {
                        Some(Some(Read::Escalation(_))) => true,
                        Some(Some(Read::Proposal(_))) => unreachable!("proposal handled above"),
                        Some(
                            Some(
                                Read::Result(_)
                                | Read::Inbox(_)
                                | Read::Transcript { .. }
                                | Read::Dependency(_)
                                | Read::InputCheck(_),
                            )
                            | None,
                        )
                        | None => false,
                    };
                    if archive {
                        domain.work.push(Work::EscalationFailed { waiter });
                    } else if waiter != Token::new(u64::MAX) {
                        results::failed(domain, waiter, people::Refusal::Limit, out);
                    } else {
                        domain.startup = Startup::Failed;
                        out.push(Request::Stop);
                    }
                    return;
                }
                if waiter == Token::new(u64::MAX) {
                    startup_page(domain, env, rows, next, out);
                } else {
                    let archive = match domain.result_reads.get(Id::from_token(waiter)) {
                        Some(Some(Read::Escalation(_))) => true,
                        Some(Some(Read::Proposal(_))) => unreachable!("proposal handled above"),
                        Some(
                            Some(
                                Read::Result(_)
                                | Read::Inbox(_)
                                | Read::Transcript { .. }
                                | Read::Dependency(_)
                                | Read::InputCheck(_),
                            )
                            | None,
                        )
                        | None => false,
                    };
                    if archive {
                        domain.work.push(Work::EscalationLoaded { waiter, rows });
                    } else {
                        domain.result_pages.push(ResultPage { waiter, rows, next });
                    }
                }
            }
            loads::Request::Unloaded { waiter, .. } => {
                if input_waiter(domain, waiter) {
                    input_failed(domain, waiter);
                    return;
                }
                if dependency_waiter(domain, waiter) {
                    dependency_failed(domain, waiter);
                    return;
                }
                if transcript_waiter(domain, waiter) {
                    transcript_failed(domain, waiter);
                    return;
                }
                if inbox_waiter(domain, waiter) {
                    inbox::failed(domain, waiter, people::Refusal::Busy, out);
                    return;
                }
                if proposal_waiter(domain, waiter) {
                    domain.work.push(Work::ProposalFailed { waiter });
                    return;
                }
                if waiter == Token::new(u64::MAX) {
                    domain.startup = Startup::Failed;
                    out.push(Request::Stop);
                } else {
                    let archive = match domain.result_reads.get(Id::from_token(waiter)) {
                        Some(Some(Read::Escalation(_))) => true,
                        Some(Some(Read::Proposal(_))) => unreachable!("proposal handled above"),
                        Some(
                            Some(
                                Read::Result(_)
                                | Read::Inbox(_)
                                | Read::Transcript { .. }
                                | Read::Dependency(_)
                                | Read::InputCheck(_),
                            )
                            | None,
                        )
                        | None => false,
                    };
                    if archive {
                        domain.work.push(Work::EscalationFailed { waiter });
                    } else {
                        results::failed(domain, waiter, people::Refusal::Busy, out);
                    }
                }
            }
            loads::Request::Load { .. } => unreachable!("terminal methods never issue IO"),
        }
    }
}

fn transcript_failed(domain: &mut Domain, waiter: Token) {
    let Some(Read::Transcript { task }) = take_read(domain, waiter) else { return };
    domain.result_reads.retire(Id::from_token(waiter));
    domain.work.push(Work::Core(jig_core::Event::PreparationFailed { task }));
}

fn begin_dependency_read(domain: &mut Domain, task: u64) -> Option<(Token, u64)> {
    let ids = domain.core.preparation_ids(task).expect("preparing task context");
    if ids.is_empty() {
        domain.work.push(Work::StartBrief { task });
        return None;
    }
    let first = *ids.first().expect("nonempty preparation IDs");
    let count = ids.len();
    let read = DependencyRead {
        task,
        ids,
        at: 0,
        results: List::with_capacity(u32::try_from(count).expect("bounded result count")),
    };
    let Ok(waiter) = domain.result_reads.insert(Some(Read::Dependency(read))) else {
        domain.work.push(Work::Core(jig_core::Event::PreparationFailed { task }));
        return None;
    };
    Some((waiter.token(), first))
}

fn dependency_failed(domain: &mut Domain, waiter: Token) {
    let Some(Read::Dependency(read)) = take_read(domain, waiter) else { return };
    domain.result_reads.retire(Id::from_token(waiter));
    domain.work.push(Work::Core(jig_core::Event::PreparationFailed { task: read.task }));
}

fn dependency_loaded(
    domain: &mut Domain,
    waiter: Token,
    rows: Box<[Record]>,
    next: Option<Key>,
    out: &mut Queue<Request>,
) {
    let Some(Some(Read::Dependency(read))) = domain.result_reads.get(Id::from_token(waiter)) else { return };
    let wanted = *read.ids.get(usize::try_from(read.at).expect("bounded index")).expect("one requested result");
    if next.is_some() || rows.len() != 1 {
        dependency_failed(domain, waiter);
        return;
    }
    let row = match rows.into_iter().next().expect("one dependency result row") {
        Record::Tasks(tasks::Stored::Ended(row)) => row,
        Record::Call(_)
        | Record::Deployment(_)
        | Record::EscalationDecision(_)
        | Record::ProposalDecision(_)
        | Record::Turn(_)
        | Record::RunProof(_)
        | Record::Terminal(_)
        | Record::Tasks(_)
        | Record::People(_)
        | Record::Forge { .. } => {
            dependency_failed(domain, waiter);
            return;
        }
    };
    let Some(result) = domain.core.dependency_result(read.task, wanted, row) else {
        dependency_failed(domain, waiter);
        return;
    };
    let Some(Some(Read::Dependency(read))) = domain.result_reads.get_mut(Id::from_token(waiter)) else {
        unreachable!("read survives validation")
    };
    read.results.push(result).expect("one result per bounded ID");
    read.at = read.at.checked_add(1).expect("bounded result index");
    if let Some(&next) = read.ids.get(usize::try_from(read.at).expect("bounded index")) {
        request_load(domain, waiter, Range::TaskResult { task: next }, None, out);
        return;
    }
    let Some(Read::Dependency(read)) = take_read(domain, waiter) else { unreachable!("complete dependency read") };
    domain.result_reads.retire(Id::from_token(waiter));
    assert!(
        domain.core.dependency_results.insert(read.task, read.results.into_boxed()).is_ok(),
        "one preparation result set"
    );
    domain.work.push(Work::StartBrief { task: read.task });
}

fn transcript_loaded(
    domain: &mut Domain,
    waiter: Token,
    rows: Box<[Record]>,
    next: Option<Key>,
    cut: Option<loads::Cut>,
    out: &mut Queue<Request>,
) {
    if cut.is_some() {
        transcript_failed(domain, waiter);
        return;
    }
    let Some(Some(Read::Transcript { task })) = domain.result_reads.get(Id::from_token(waiter)) else { return };
    let task = *task;
    for row in rows {
        let turn = match row {
            Record::Turn(turn) => turn,
            Record::ProposalDecision(_)
            | Record::Call(_)
            | Record::EscalationDecision(_)
            | Record::Deployment(_)
            | Record::RunProof(_)
            | Record::Terminal(_)
            | Record::Tasks(_)
            | Record::People(_)
            | Record::Forge { .. } => {
                transcript_failed(domain, waiter);
                return;
            }
        };
        if !domain.core.append_transcript(task, turn) {
            transcript_failed(domain, waiter);
            return;
        }
    }
    if let Some(after) = next {
        request_load(domain, waiter, Range::TaskTranscript { task }, Some(after), out);
        return;
    }
    let Some(Read::Transcript { .. }) = take_read(domain, waiter) else { unreachable!("finished transcript read") };
    domain.result_reads.retire(Id::from_token(waiter));
    if let Some((waiter, first)) = begin_dependency_read(domain, task) {
        request_load(domain, waiter, Range::TaskResult { task: first }, None, out);
    }
}

fn finish_brief_plan(
    domain: &mut Domain,
    env: &Env<Limits>,
    task: u64,
    parent: Option<u64>,
    mut wanted: List<brief::Planned>,
) {
    if let Some(parent) = parent
        && let Some(row) = domain.forge.change(parent)
        && let Some((child, _)) = row.delegate
        && child == task
        && let Some(number) = row.pull
        && let Some(head) = row.change.last_head
        && wanted.room() > 0
    {
        let item = forge::BriefItem { repository: row.repository.repository, number };
        let source = match row.delegate.expect("matched delegate").1 {
            forge_change::Delegate::Repair(forge_change::Repair::Gate(_)) => {
                forge::BriefSource::Reviews { item, head: forge::BriefCommit(head) }
            }
            forge_change::Delegate::Repair(forge_change::Repair::Ci | forge_change::Repair::Semantic) => {
                forge::BriefSource::Ci { item, head: forge::BriefCommit(head) }
            }
            forge_change::Delegate::Resolve { .. } => forge::BriefSource::Pull { item, head: forge::BriefCommit(head) },
            forge_change::Delegate::Produce | forge_change::Delegate::Gate { .. } => {
                forge::BriefSource::Pull { item, head: forge::BriefCommit(head) }
            }
        };
        let kind = forge_brief_kind(source);
        let token = domain
            .brief_connectors
            .insert(BriefConnector { task, kind, cutting: false })
            .expect("reserved connector section room")
            .token();
        domain.work.push(Work::Forge(forge::Event::PlanBrief { section: token, source }));
        wanted
            .push(brief::Planned::Connector {
                connector: 0,
                kind: forge_kind_number(kind),
                token,
                size: 0,
                limit: forge_brief_budget(kind, &domain.limits.brief.budgets),
                priority: 5,
                required: true,
            })
            .expect("connector section room");
        let semantic = match row.delegate {
            Some((_, forge_change::Delegate::Repair(forge_change::Repair::Semantic))) => true,
            Some((
                _,
                forge_change::Delegate::Repair(forge_change::Repair::Ci | forge_change::Repair::Gate(_))
                | forge_change::Delegate::Resolve { .. }
                | forge_change::Delegate::Gate { .. }
                | forge_change::Delegate::Produce,
            ))
            | None => false,
        };
        if semantic && wanted.room() > 0 {
            let source = forge::BriefSource::Pull { item, head: forge::BriefCommit(head) };
            let kind = forge_brief_kind(source);
            let token = domain
                .brief_connectors
                .insert(BriefConnector { task, kind, cutting: false })
                .expect("reserved connector section room")
                .token();
            domain.work.push(Work::Forge(forge::Event::PlanBrief { section: token, source }));
            wanted
                .push(brief::Planned::Connector {
                    connector: 0,
                    kind: forge_kind_number(kind),
                    token,
                    size: 0,
                    limit: forge_brief_budget(kind, &domain.limits.brief.budgets),
                    priority: 6,
                    required: true,
                })
                .expect("semantic update section room");
        }
    }
    domain.work.push(Work::Brief(brief::GatherEvent::Plan {
        brief: Token::new(task),
        budget: domain.limits.brief.brief_bytes,
        deadline: env.now.saturating_add(domain.limits.brief.gather),
        sections: wanted.into_boxed(),
    }));
}

fn forge_brief_kind(source: forge::BriefSource) -> ForgeBriefKind {
    match source {
        forge::BriefSource::Ci { .. } => ForgeBriefKind::Ci,
        forge::BriefSource::Reviews { .. } => ForgeBriefKind::Reviews,
        forge::BriefSource::Pull { .. } => ForgeBriefKind::Pull,
    }
}

fn forge_kind_number(kind: ForgeBriefKind) -> u16 {
    match kind {
        ForgeBriefKind::Ci => 1,
        ForgeBriefKind::Reviews => 2,
        ForgeBriefKind::Pull => 3,
    }
}

fn forge_brief_budget(kind: ForgeBriefKind, budgets: &BriefBudgets) -> u32 {
    match kind {
        ForgeBriefKind::Ci => budgets.ci,
        ForgeBriefKind::Reviews => budgets.reviews,
        ForgeBriefKind::Pull => budgets.pull,
    }
}

fn startup_page(
    domain: &mut Domain,
    env: &Env<Limits>,
    rows: Box<[Record]>,
    next: Option<Key>,
    out: &mut Queue<Request>,
) {
    let range = match domain.startup {
        Startup::Loading(range) => range,
        Startup::Cold | Startup::Adopting(_) | Startup::Running | Startup::Failed => return,
    };
    for row in rows {
        restore_page_row(domain, env, row);
    }
    if range == Range::Deployment {
        for _ in 0..domain.before_header.len() {
            domain.work.push(domain.before_header.pop().expect("cold worker event"));
        }
    }
    let mut decision = route(domain, env);
    if domain.startup == Startup::Failed {
        out.push(Request::Stop);
        return;
    }
    if range == Range::Deployment {
        let mut lost = List::with_capacity(domain.limits.fleet.workers);
        for (&channel, &was_lost) in &domain.cold_channels {
            lost.push((channel, was_lost)).expect("cold names room");
        }
        for &(channel, was_lost) in &lost {
            let _old = domain.cold_channels.remove(&channel);
            if was_lost {
                lose_channel(domain, env, channel);
            }
        }
    }
    if let Some(after) = next {
        emit(&mut decision, &env.limits, Delivery::Load { waiter: Token::new(u64::MAX), range, after: Some(after) });
        close(domain, env, decision, out);
        return;
    }
    let following = match range {
        Range::Deployment => Some(Range::People),
        Range::People => Some(Range::Tasks),
        Range::Tasks => Some(Range::Forge),
        Range::Forge => Some(Range::RunProofs),
        Range::RunProofs => Some(Range::Calls),
        Range::Calls => None,
        Range::EscalationDecision { .. }
        | Range::ProposalDecision { .. }
        | Range::Turns { .. }
        | Range::TaskTranscript { .. }
        | Range::TaskResult { .. }
        | Range::EndedResults => {
            unreachable!("startup range")
        }
    };
    if range == Range::Tasks {
        for project in &domain.core.projects {
            if !domain.core.people.has_project(*project) {
                domain.work.push(Work::People(people::Event::Roles { project: *project, holdings: Box::new([]) }));
            }
        }
        domain.work.push(Work::People(people::Event::Restored));
        route_into(domain, env, &mut decision);
        if domain.startup == Startup::Failed {
            out.push(Request::Stop);
            return;
        }
    }
    if let Some(range) = following {
        domain.startup = Startup::Loading(range);
        emit(&mut decision, &env.limits, Delivery::Load { waiter: Token::new(u64::MAX), range, after: None });
        close(domain, env, decision, out);
        return;
    }
    if !domain.core.restoring_proofs.is_empty() {
        domain.startup = Startup::Failed;
        out.push(Request::Stop);
        return;
    }
    domain.startup = Startup::Adopting(jig_core::connector::RestartStage::Restored);
    domain.work.push(Work::Tasks(tasks::Event::Restored));
    route_into(domain, env, &mut decision);
    if domain.startup == Startup::Failed {
        out.push(Request::Stop);
        return;
    }
    for _ in 0..domain.core.adopted.len() {
        domain.work.push(Work::Fleet(domain.core.adopted.pop().expect("all restored claims")));
    }
    domain.work.push(Work::Fleet(fleet::Event::Loaded));
    route_into(domain, env, &mut decision);
    if domain.startup == Startup::Failed {
        out.push(Request::Stop);
        return;
    }
    domain.work.push(Work::Forge(forge::Event::Restored { clock: forge_client::RecoveryClock::Wall }));
    route_into(domain, env, &mut decision);
    close(domain, env, decision, out);
}

/// Until jig-core owns the script, the root advances only on the connector's
/// explicit completion in jig's restart vocabulary.
fn connector_restart_done(domain: &mut Domain, stage: jig_core::connector::RestartStage) {
    if domain.startup != Startup::Adopting(stage) {
        domain.startup = Startup::Failed;
        return;
    }
    match stage {
        jig_core::connector::RestartStage::Restored => {
            domain.startup = Startup::Adopting(jig_core::connector::RestartStage::ReadAfresh);
            domain.work.push(Work::Forge(forge::Event::ReadAfresh));
        }
        jig_core::connector::RestartStage::ReadAfresh => {
            domain.startup = Startup::Adopting(jig_core::connector::RestartStage::Settled);
            domain.work.push(Work::Forge(forge::Event::SettleOutbox));
        }
        jig_core::connector::RestartStage::Settled => {
            domain.startup = Startup::Running;
            for _ in 0..domain.core.due.len() {
                domain.work.push(Work::Activate(domain.core.due.pop().expect("restored due tasks")));
            }
        }
    }
}

fn startup_adopting(startup: Startup) -> bool {
    match startup {
        Startup::Adopting(_) => true,
        Startup::Cold | Startup::Loading(_) | Startup::Running | Startup::Failed => false,
    }
}

fn account_event(domain: &mut Domain, env: &Env<Limits>, event: accounts::Event, out: &mut Queue<Request>) {
    let routed = jig_core::step(&mut domain.core, &environment_core(env), jig_core::Event::Account(event));
    account_outputs(routed, out);
}

fn account_outputs(routed: jig_core::Requests, out: &mut Queue<Request>) {
    let jig_core::Requests::Out(mut marked) = routed;
    for _ in 0..marked.len() {
        match marked.pop().expect("account mark count") {
            jig_core::Request::Now(value) => match *value {
                jig_core::Now::Account(request) => out.push(Request::Account(request)),
                jig_core::Now::View(_)
                | jig_core::Now::SignInRefused { .. }
                | jig_core::Now::WatchRefused { .. }
                | jig_core::Now::NotesIndexed { .. }
                | jig_core::Now::NotesRecalled { .. }
                | jig_core::Now::NotesRefused { .. }
                | jig_core::Now::DropPayload { .. }
                | jig_core::Now::DropAssignment { .. }
                | jig_core::Now::TurnPayload { .. }
                | jig_core::Now::AnswerPayload { .. }
                | jig_core::Now::AcceptedTurn { .. }
                | jig_core::Now::RefusedPayload { .. }
                | jig_core::Now::Activate { .. }
                | jig_core::Now::PrepareAgent { .. }
                | jig_core::Now::StartPreparation { .. }
                | jig_core::Now::HistoricalProposal { .. }
                | jig_core::Now::BriefCorePlanned { .. }
                | jig_core::Now::WorkspaceRequest { .. }
                | jig_core::Now::RunPreparationFailed { .. }
                | jig_core::Now::RunPrepared { .. }
                | jig_core::Now::CompleteBrief { .. }
                | jig_core::Now::HistoricalEscalation { .. }
                | jig_core::Now::EscalationInspection { .. }
                | jig_core::Now::EscalationReply { .. }
                | jig_core::Now::EscalationRefused { .. }
                | jig_core::Now::CallPayload { .. }
                | jig_core::Now::ProcedureDelegateOutcome { .. }
                | jig_core::Now::RestoreRefused => unreachable!("account route owns its now output"),
            },
            jig_core::Request::Decided => {}
            jig_core::Request::Write(_) | jig_core::Request::Ask { .. } | jig_core::Request::Held(_) => {
                unreachable!("account route changes no store decision")
            }
        }
    }
}

fn account_fire(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    let routed = jig_core::fire(&mut domain.core, &environment_core(env), jig_core::Timer::Account);
    account_outputs(routed, out);
}

fn result_words(result: tasks::TaskResult) -> Box<[u8]> {
    match result {
        tasks::TaskResult::Report { words }
        | tasks::TaskResult::Verdict { words, .. }
        | tasks::TaskResult::Change { words, .. } => words,
        tasks::TaskResult::Failure { reason } => reason,
    }
}

fn ending_words(ending: tasks::Ending) -> Box<[u8]> {
    match ending {
        tasks::Ending::Done(result) => result_words(result),
        tasks::Ending::Failed { reason } => reason,
        tasks::Ending::Cancelled { reason, result } => match result {
            Some(result) => result_words(result),
            None => reason,
        },
    }
}

fn tasks_saved_within(saved: Option<&[u32]>, most: u32) -> bool {
    let Some(tags) = saved else { return true };
    if tags.len() > usize::try_from(most).expect("u32 fits usize") {
        return false;
    }
    let mut previous = 0;
    for tag in tags {
        if *tag <= previous {
            return false;
        }
        previous = *tag;
    }
    true
}

fn end_bytes(end: &tasks::End) -> u64 {
    tasks::terminal_bytes(end).expect("bounded terminal bytes")
}

fn payload_slots(limits: &Limits) -> Option<u32> {
    limits
        .fleet
        .turns
        .checked_add(limits.fleet.attempts)?
        .checked_mul(2)?
        .checked_add(limits.fleet.calls.checked_mul(2)?)
}

fn route_bound(limits: &Limits) -> Option<u32> {
    tasks::max_out(&limits.tasks)
        .checked_mul(8)?
        .checked_add(people::max_out(&limits.people).checked_mul(4)?)?
        // Every in-flight historical decision may complete under journal pressure:
        // one retained IO terminal and one people Decided callback per flight.
        .checked_add(limits.people.pending.checked_mul(2)?)?
        .checked_add(fleet::max_out(&limits.fleet).checked_mul(4)?)?
        .checked_add(forge::max_out(&limits.forge))?
        // One serialized role cohort revisits each Waiting task, then answers.
        .checked_add(limits.tasks.tasks.checked_add(4)?)
}

// The walking root drains its pending callbacks in the same decision as a newly
// admitted event. Thus any event may reach the full child route. The bound sums
// each child's maximum output cohort, the retained callbacks, call records and
// the deployment header; the configured journal may have more per-commit room.
fn route_room(limits: &Limits) -> skein_lib::JournalRoom {
    skein_lib::JournalRoom {
        writes: route_bound(limits)
            .expect("validated route bound")
            .checked_add(limits.call_records)
            .expect("validated call record bound"),
        held: limits.journal.deliveries,
    }
}

fn route_takes(domain: &Domain, limits: &Limits) -> bool {
    domain.journal.takes(&route_room(limits))
}

fn route_decision(domain: &mut Domain, limits: &Limits) -> Option<Decision> {
    Decision::reserve_room(&mut domain.journal, &limits.journal, route_room(limits))
}

/// Count participating child state, fixed handoffs, decoded input and all simultaneously retained
/// owned payloads, including current proofs, terminal scratch copies, transient restore
/// correlations and preparation contexts. Child limits validate before route capacity arithmetic;
/// malformed or unrepresentable bounds return `None`. Pure startup heap calculation, excluding
/// allocator overhead. Child declared output bounds must already be representable; checked root
/// arithmetic or refused child/cross-route bounds return `None`. It allocates no state and emits no
/// request or terminal.
#[must_use]
#[expect(clippy::too_many_lines, reason = "one checked sum of the root's bounded participating state")]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    if limits.brief.parts == 0 || limits.brief.gather == skein_lib::Duration::ZERO {
        return None;
    }
    let task_bytes = tasks::worst_case(&limits.tasks)?;
    let fleet_bytes = fleet::worst_case(&limits.fleet)?;
    let people_bytes = people::worst_case(&limits.people)?;
    let authority_bytes = authority::worst_case(&limits.authority)?;
    let brief_bytes = brief::gather_worst_case(&brief_limits(&limits.brief))?.checked_add(
        Slab::<BriefConnector>::worst_case(limits.brief.briefs.checked_mul(limits.brief.sections)?.checked_mul(2)?)?,
    )?;
    let brief_fetches = limits.brief.briefs.checked_mul(limits.brief.sections)?.checked_mul(2)?;
    if limits.forge.brief_sections < brief_fetches || limits.forge.brief_bytes < limits.brief.read_bytes {
        return None;
    }
    let account_bytes = accounts::worst_case(&limits.accounts)?;
    let view_bytes = views::worst_case(&limits.views)?;
    let note_bytes = notes::worst_case(&limits.notes)?;
    let forge_bytes = forge::worst_case(&limits.forge)?;
    let landing_bytes = Map::<u32, Box<[LandingRule]>>::worst_case(limits.authority.projects)?.checked_add(
        u64::from(limits.authority.projects).checked_add(1)?.checked_mul(u64::from(limits.journal.transcript_bytes))?,
    )?;
    let permission_bytes = Map::<u32, Box<[people::PermissionRole]>>::worst_case(limits.authority.projects)?
        .checked_add(u64::from(limits.authority.projects).checked_mul(u64::from(limits.journal.transcript_bytes))?)?;
    let load_bytes = loads::worst_case(&limits.loads)?;
    let routes = route_bound(limits)?;
    let tool_bytes = u64::from(limits.tasks.batch).checked_mul(row_bound(limits)?)?;
    let inbox_entry = size_of::<crate::ResultEntry>().max(size_of::<crate::InboxViewEntry>());
    let inbox_bytes = u64::from(limits.people.inbox_entries)
        .checked_mul(u64::try_from(inbox_entry).ok()?.checked_add(u64::from(limits.journal.result_bytes))?)?;
    if limits.journal.writes < routes
        || limits.views.report_bytes < 8
        || limits.call_records == 0
        || limits.call_records > limits.journal.writes.checked_sub(routes)?
        || limits.journal.deliveries
            < limits
                .tasks
                .tasks
                .checked_mul(4)?
                .checked_add(8)?
                .checked_add(limits.people.pending.checked_mul(limits.people.waiters)?)?
        || limits.loads.loads < 2
        || limits.journal.held < limits.journal.deliveries.checked_mul(3)?
        || limits.journal.deliveries
            < limits.fleet.workers.checked_mul(fleet::max_out(&limits.fleet))?.checked_add(1)?
        || limits.loads.rows.checked_add(limits.fleet.workers)? > routes
        || limits.journal.result_bytes < limits.brief.brief_bytes
        || limits.journal.deliveries < limits.brief.sections
        || limits.journal.deliveries < limits.tasks.saved_resources
        || limits.journal.result_bytes < limits.tasks.result_bytes
        || limits.journal.result_bytes < limits.people.words
        || limits.people.inbox_entries == 0
        || inbox_bytes > u64::from(limits.journal.transcript_bytes)
        || u64::from(limits.journal.transcript_bytes) < row_bound(limits)?
    {
        return None;
    }
    let mut bytes = crate::worst_case(&limits.journal)?;
    if max_out(limits) > limits.journal.deliveries {
        bytes = bytes.checked_add(
            Queue::<Output>::worst_case(max_out(limits))?
                .checked_sub(Queue::<Output>::worst_case(limits.journal.deliveries)?)?,
        )?;
    }
    bytes = bytes.checked_add(Queue::<Request>::worst_case(max_out(limits))?)?;
    bytes = bytes.checked_add(role_scratch_bytes(limits)?)?;
    let cold = limits.fleet.workers;
    let hello = u64::from(limits.fleet.slots)
        .checked_mul(u64::try_from(size_of::<fleet::Hosted>()).ok()?)?
        .checked_add(u64::from(limits.fleet.workstreams).checked_mul(u64::try_from(size_of::<u64>()).ok()?)?)?;
    bytes = bytes
        .checked_add(Queue::<Work>::worst_case(cold)?)?
        .checked_add(Map::<Token, bool>::worst_case(cold)?)?
        .checked_add(List::<(Token, bool)>::worst_case(cold)?)?
        .checked_add(u64::from(cold).checked_mul(hello)?)?
        .checked_add(List::<u32>::worst_case(limits.people.projects)?)?;
    for child in [
        task_bytes,
        authority_bytes.checked_mul(3)?,
        people_bytes,
        fleet_bytes,
        brief_bytes,
        account_bytes,
        load_bytes,
        view_bytes,
        note_bytes,
        forge_bytes,
        landing_bytes,
        permission_bytes,
    ] {
        bytes = bytes.checked_add(child)?;
    }
    bytes = bytes
        .checked_add(Map::<forge::Key, u64>::worst_case(forge_route::rows(limits)?)?)?
        .checked_add(Map::<Token, Option<forge::Repository>>::worst_case(limits.forge.adoptions)?)?
        .checked_add(
            u64::from(limits.forge.adoptions).checked_mul(u64::from(limits.forge.name_bytes).checked_mul(4)?)?,
        )?
        .checked_add(Map::<Token, forge::Subscriber>::worst_case(limits.fleet.calls)?)?
        .checked_add(Map::<Token, (u64, forge::Topic)>::worst_case(limits.fleet.calls)?)?
        .checked_add(Map::<Token, (ReplyTo, CallKey)>::worst_case(limits.fleet.calls)?)?
        .checked_add(Map::<u64, (ReplyTo, CallKey)>::worst_case(limits.fleet.calls)?)?
        .checked_add(Map::<u64, skein_lib::Wall>::worst_case(limits.forge.issues)?)?
        .checked_add(Map::<u64, skein_lib::Wall>::worst_case(limits.forge.changes)?)?
        .checked_add(
            u64::from(limits.fleet.calls)
                .checked_mul(u64::from(limits.forge.paths_per_subscription))?
                .checked_mul(u64::from(limits.forge.name_bytes))?,
        )?
        .checked_add(Map::<Token, u64>::worst_case(limits.views.watchers)?)?
        .checked_add(Map::<u64, (u32, Option<u32>)>::worst_case(limits.tasks.tasks)?)?
        .checked_add(u64::from(limits.views.snapshot_bytes))?
        .checked_add(
            u64::from(limits.tasks.depth.saturating_add(1)).checked_mul(u64::try_from(size_of::<Token>()).ok()?)?,
        )?
        .checked_add(Queue::<Work>::worst_case(routes)?)?
        .checked_add(u64::from(routes).checked_mul(row_bound(limits)?)?)?
        .checked_add(Queue::<fleet::Event>::worst_case(limits.tasks.tasks.checked_mul(2)?)?)?
        .checked_add(Queue::<Box<tasks::RunContext>>::worst_case(limits.tasks.tasks)?)?;
    bytes = bytes.checked_add(
        u64::from(limits.tasks.tasks).checked_mul(u64::from(limits.tasks.saved_resources).checked_mul(4)?)?,
    )?;
    bytes = bytes.checked_add(Slab::<Option<Payload>>::worst_case(payload_slots(limits)?)?)?.checked_add(
        u64::from(payload_slots(limits)?).checked_mul(
            u64::from(limits.journal.transcript_bytes)
                .max(u64::from(limits.tasks.result_bytes).checked_mul(2)?)
                .max(tool_bytes),
        )?,
    )?;
    bytes = bytes.checked_add(
        u64::from(payload_slots(limits)?).checked_mul(u64::from(limits.tasks.saved_resources).checked_mul(4)?)?,
    )?;
    bytes = bytes
        .checked_add(Slab::<Option<Read>>::worst_case(limits.loads.loads)?)?
        .checked_add(Map::<u64, Token>::worst_case(limits.loads.loads)?)?
        .checked_add(Queue::<ResultPage>::worst_case(limits.loads.loads)?)?
        .checked_add(u64::from(limits.loads.loads).checked_mul(u64::from(limits.loads.reply_bytes))?)?
        .checked_add(u64::from(limits.loads.loads).checked_mul(inbox_bytes)?)?
        // Each reserved semantic/history query can retain one independent
        // rejection offer while child/people own their copies.
        .checked_add(u64::from(limits.loads.loads).checked_mul(u64::from(limits.people.words))?)?
        // At most one historical terminal per reserved read slot can wait in
        // work under journal pressure; include decoded row slot and owned bytes.
        .checked_add(
            u64::from(limits.loads.loads)
                .checked_mul(u64::try_from(size_of::<Record>()).ok()?.checked_add(u64::from(limits.people.words))?)?,
        )?
        .checked_add(Map::<Token, u64>::worst_case(limits.people.pending)?)?;
    bytes = bytes
        .checked_add(Map::<Token, u64>::worst_case(limits.people.pending)?)?
        .checked_add(Map::<Token, u64>::worst_case(limits.people.pending)?)?
        .checked_add(Map::<Token, PersonTaskRoute>::worst_case(limits.people.pending)?)?
        .checked_add(Map::<Token, (CallKey, Box<[tasks::Stub]>)>::worst_case(limits.fleet.calls)?)?
        .checked_add(
            u64::from(limits.fleet.calls)
                .checked_add(u64::from(limits.loads.loads))?
                .checked_mul(u64::from(limits.tasks.batch))?
                .checked_mul(u64::from(limits.tasks.inputs))?
                .checked_mul(u64::try_from(size_of::<tasks::Stub>()).ok()?)?,
        )?
        .checked_add(Map::<Token, RoutedCall>::worst_case(limits.fleet.calls)?)?
        .checked_add(Map::<Token, PersonProposalRoute>::worst_case(limits.people.pending)?)?
        .checked_add(Map::<u64, u64>::worst_case(limits.tasks.tasks)?)?
        .checked_add(Map::<CallKey, bool>::worst_case(limits.call_records)?)?
        .checked_add(Map::<u64, Box<[HistoricalResult]>>::worst_case(limits.tasks.tasks)?)?
        .checked_add(u64::from(limits.tasks.message_bytes))?;
    let result_count = u64::from(limits.tasks.dependencies).checked_add(u64::from(limits.tasks.inputs))?;
    let result_view =
        u64::try_from(size_of::<HistoricalResult>()).ok()?.checked_add(u64::from(limits.tasks.result_bytes))?;
    bytes = bytes
        .checked_add(u64::from(limits.tasks.tasks).checked_mul(result_count)?.checked_mul(result_view)?)?
        .checked_add(u64::from(limits.loads.loads).checked_mul(result_count)?.checked_mul(result_view)?)?
        .checked_add(
            u64::from(limits.loads.loads)
                .checked_mul(u64::from(limits.tasks.batch))?
                .checked_mul(row_bound(limits)?)?,
        )?
        .checked_add(u64::from(limits.call_records).checked_mul(u64::from(limits.tasks.batch))?.checked_mul(8)?)?
        .checked_add(
            u64::from(limits.call_records)
                .checked_mul(u64::from(authority::max_out(&limits.authority)?))?
                .checked_mul(u64::try_from(size_of::<authority::Finding>()).ok()?)?,
        )?
        .checked_add(
            u64::from(limits.tasks.tasks).checked_mul(u64::from(limits.tasks.delegates))?.checked_mul(
                u64::try_from(size_of::<tasks::DelegateState>())
                    .ok()?
                    .checked_add(u64::from(limits.tasks.result_bytes).checked_mul(2)?)?,
            )?,
        )?;
    bytes = bytes
        .checked_add(Map::<u64, u64>::worst_case(limits.tasks.tasks)?)?
        .checked_add(Map::<u64, RunProof>::worst_case(limits.tasks.tasks)?)?
        .checked_add(Map::<CallKey, CallAnswer>::worst_case(limits.call_records)?)?
        .checked_add(
            u64::from(limits.tasks.tasks)
                .checked_mul(u64::from(limits.call_records))?
                .checked_mul(u64::try_from(size_of::<crate::CallRecord>()).ok()?)?,
        )?
        .checked_add(Map::<u64, RestoringProof>::worst_case(limits.tasks.tasks)?)?
        .checked_add(Map::<u64, u64>::worst_case(limits.tasks.tasks)?)?
        .checked_add(Map::<u64, Box<tasks::RunContext>>::worst_case(limits.tasks.tasks)?)?
        .checked_add(
            u64::from(limits.tasks.tasks).checked_mul(u64::from(limits.tasks.saved_resources).checked_mul(4)?)?,
        )?
        .checked_add(u64::from(limits.tasks.tasks).checked_mul(u64::from(limits.tasks.result_bytes).checked_mul(2)?)?)?
        .checked_add(Map::<u64, Transcript>::worst_case(limits.tasks.tasks)?)?
        .checked_add(
            u64::from(limits.tasks.tasks).checked_mul(
                Queue::<Box<[u8]>>::worst_case(limits.journal.transcript_bytes)?
                    .checked_add(u64::from(limits.journal.transcript_bytes))?,
            )?,
        )?
        .checked_add(u64::from(limits.tasks.result_bytes).checked_mul(6)?)?
        // One routing/view context per live task plus an incoming clone; the
        // rejected reason is additional to the context's boxed inline slot.
        .checked_add(
            u64::from(limits.tasks.tasks).checked_add(1)?.checked_mul(
                u64::try_from(size_of::<tasks::EscalationContext>())
                    .ok()?
                    .checked_add(u64::from(limits.tasks.result_bytes))?,
            )?,
        )?
        .checked_add(u64::from(limits.tasks.tasks).checked_mul(row_bound(limits)?.checked_mul(2)?)?)?;
    bytes = bytes.checked_add(Map::<u64, Assignment>::worst_case(limits.tasks.tasks)?)?.checked_add(
        u64::from(limits.tasks.tasks).checked_add(u64::from(limits.journal.held))?.checked_mul(
            u64::from(limits.brief.brief_bytes)
                .checked_add(List::<BriefSection>::worst_case(limits.brief.sections)?)?
                .checked_add(u64::from(limits.tasks.inbox_bytes))?
                .checked_add(
                    u64::from(limits.tasks.tasks)
                        .checked_mul(u64::from(limits.tasks.message_bytes).checked_add(32)?)?,
                )?
                .checked_add(u64::from(limits.tasks.saved_resources).checked_mul(4)?)?
                .checked_add(u64::from(limits.journal.run_bytes))?
                .checked_add(u64::from(limits.journal.transcript_bytes))?
                .checked_add(List::<Box<[u8]>>::worst_case(limits.journal.transcript_bytes)?)?
                .checked_add(
                    u64::from(limits.tasks.inbox_messages.checked_add(limits.tasks.tasks.checked_mul(2)?)?)
                        .checked_mul(u64::try_from(size_of::<tasks::Word>()).ok()?)?,
                )?,
        )?,
    )?;
    bytes
        .checked_add(Queue::<authority::Finding>::worst_case(authority::max_out(&limits.authority)?)?)?
        .checked_add(Queue::<tasks::Request>::worst_case(tasks::max_out(&limits.tasks))?)?
        .checked_add(u64::from(limits.tasks.message_bytes).checked_mul(3)?)?
        // The outer Ask output queue stays allocated while the serialized
        // application owns its separate bounded people terminal/save queue.
        .checked_add(Queue::<people::Request>::worst_case(people::max_out(&limits.people))?.checked_mul(2)?)?
        .checked_add(Queue::<fleet::Request>::worst_case(fleet::max_out(&limits.fleet))?)?
        .checked_add(Queue::<brief::GatherRequest>::worst_case(brief::gather_max_out(&brief_limits(&limits.brief)))?)?
        .checked_add(Queue::<Request>::worst_case(max_out(limits))?)?
        .checked_add(Queue::<Output>::worst_case(1)?)?
        .checked_add(Queue::<loads::Request>::worst_case(1)?)?
        .checked_add(Queue::<accounts::Request>::worst_case(accounts::MAX_OUT)?)
}

fn role_scratch_bytes(limits: &Limits) -> Option<u64> {
    // Role administration owns one incoming/routed candidate and application
    // scratch copies independently of people's pending/completed asks. Semantic
    // inspection/recheck arrays contain Waiting contexts only (no reason bytes).
    let roster_bytes =
        u64::from(limits.people.holdings).checked_mul(u64::try_from(size_of::<people::Holding>()).ok()?)?;
    roster_bytes
        .checked_mul(4)?
        .checked_add(List::<tasks::EscalationContext>::worst_case(limits.tasks.tasks)?.checked_mul(2)?)?
        .checked_add(u64::try_from(size_of::<tasks::EscalationContext>()).ok()?)
}

fn take_read(domain: &mut Domain, waiter: Token) -> Option<Read> {
    let entry = domain.result_reads.get_mut(Id::from_token(waiter))?;
    entry.take()
}

fn authority_within(value: &authority::Authority, limits: &Limits) -> bool {
    if value.grants.len()
        > usize::try_from(limits.authority.grants.min(limits.tasks.authority_grants)).expect("u32 fits usize")
        || value.note_resources.len()
            > usize::try_from(limits.authority.grants.min(limits.tasks.authority_grants)).expect("u32 fits usize")
        || value.delegation.kinds.len()
            > usize::try_from(limits.authority.executors.min(limits.tasks.executor_kinds)).expect("u32 fits usize")
        || value.notes.0 & !7 != 0
    {
        return false;
    }
    let mut bytes = 0_usize;
    for grant in &value.grants {
        if grant.pattern.segments.len()
            > usize::try_from(limits.authority.segments.min(limits.tasks.authority_segments)).expect("u32 fits usize")
        {
            return false;
        }
        for segment in &grant.pattern.segments {
            if segment.len() > usize::try_from(limits.authority.segment_bytes).expect("u32 fits usize") {
                return false;
            }
            let Some(total) = bytes.checked_add(segment.len()) else {
                return false;
            };
            bytes = total;
        }
        let terminal = match &grant.pattern.last {
            authority::Last::Exact(bytes) | authority::Last::Open(bytes) => bytes.len(),
        };
        if terminal > usize::try_from(limits.authority.segment_bytes).expect("u32 fits usize") {
            return false;
        }
        let Some(total) = bytes.checked_add(terminal) else {
            return false;
        };
        bytes = total;
    }
    for scope in &value.note_resources {
        if scope.pattern.segments.len()
            > usize::try_from(limits.authority.segments.min(limits.tasks.authority_segments)).expect("u32 fits usize")
        {
            return false;
        }
        for segment in &scope.pattern.segments {
            if segment.len() > usize::try_from(limits.authority.segment_bytes).expect("u32 fits usize") {
                return false;
            }
            let Some(total) = bytes.checked_add(segment.len()) else { return false };
            bytes = total;
        }
        let terminal = match &scope.pattern.last {
            authority::Last::Exact(bytes) | authority::Last::Open(bytes) => bytes.len(),
        };
        if terminal > usize::try_from(limits.authority.segment_bytes).expect("u32 fits usize") {
            return false;
        }
        let Some(total) = bytes.checked_add(terminal) else { return false };
        bytes = total;
    }
    bytes <= usize::try_from(limits.tasks.authority_bytes).expect("u32 fits usize")
}

fn run_policy_bytes(policy: &RunPolicy) -> Option<u64> {
    if policy.turns == 0
        || policy.waiting == skein_lib::Duration::ZERO
        || policy.time == skein_lib::Duration::ZERO
        || policy.call_timeout == skein_lib::Duration::ZERO
        || policy.model.name.is_empty()
        || policy.model.max_tokens == 0
        || policy.model.price_unit == 0
    {
        return None;
    }
    let mut bytes = u64::try_from(size_of::<RunCharter>())
        .ok()?
        .checked_add(u64::try_from(policy.instructions.len()).ok()?)?
        .checked_add(u64::try_from(policy.model.name.len()).ok()?)?
        .checked_add(
            u64::try_from(policy.alternatives.len()).ok()?.checked_mul(u64::try_from(size_of::<Model>()).ok()?)?,
        )?;
    for model in &policy.alternatives {
        if model.name.is_empty() || model.max_tokens == 0 || model.price_unit == 0 {
            return None;
        }
        bytes = bytes.checked_add(u64::try_from(model.name.len()).ok()?)?;
    }
    Some(bytes)
}

fn run_carriers_bound(limits: &Limits) -> Option<u64> {
    u64::from(limits.tasks.contract_choices)
        .checked_mul(u64::try_from(size_of::<tasks::Verdict>()).ok()?)?
        .checked_add(
            u64::from(limits.tasks.authority_grants).checked_mul(u64::try_from(size_of::<tasks::Grant>()).ok()?)?,
        )?
        .checked_add(
            u64::from(limits.tasks.authority_grants)
                .checked_mul(u64::try_from(size_of::<tasks::ResourceScope>()).ok()?)?,
        )?
        .checked_add(
            u64::from(limits.tasks.authority_grants)
                .checked_mul(2)?
                .checked_mul(u64::from(limits.tasks.authority_segments))?
                .checked_mul(u64::try_from(size_of::<Box<[u8]>>()).ok()?)?,
        )?
        .checked_add(u64::from(limits.tasks.authority_bytes))?
        .checked_add(
            u64::from(limits.tasks.executor_kinds)
                .checked_mul(u64::try_from(size_of::<tasks::AuthorityExecutor>()).ok()?)?,
        )
}

fn run_policy_bound(policy: &RunPolicy, limits: &Limits) -> Option<u64> {
    run_policy_bytes(policy)?.checked_add(run_carriers_bound(limits)?)
}

pub(crate) fn run_charter_bytes(charter: &RunCharter) -> Option<u64> {
    let mut bytes = run_policy_bytes(&charter.policy)?;
    match &charter.contract {
        tasks::Contract::Report { .. } | tasks::Contract::Change { .. } => {}
        tasks::Contract::Verdict { choices } => {
            bytes = bytes.checked_add(
                u64::try_from(choices.len()).ok()?.checked_mul(u64::try_from(size_of::<tasks::Verdict>()).ok()?)?,
            )?;
        }
    }
    bytes = bytes
        .checked_add(
            u64::try_from(charter.authority.grants.len())
                .ok()?
                .checked_mul(u64::try_from(size_of::<tasks::Grant>()).ok()?)?,
        )?
        .checked_add(
            u64::try_from(charter.authority.note_resources.len())
                .ok()?
                .checked_mul(u64::try_from(size_of::<tasks::ResourceScope>()).ok()?)?,
        )?
        .checked_add(
            u64::try_from(charter.authority.delegation.kinds.len())
                .ok()?
                .checked_mul(u64::try_from(size_of::<tasks::AuthorityExecutor>()).ok()?)?,
        )?;
    for grant in &charter.authority.grants {
        bytes = bytes.checked_add(
            u64::try_from(grant.pattern.segments.len())
                .ok()?
                .checked_mul(u64::try_from(size_of::<Box<[u8]>>()).ok()?)?,
        )?;
        for segment in &grant.pattern.segments {
            bytes = bytes.checked_add(u64::try_from(segment.len()).ok()?)?;
        }
        let terminal = match &grant.pattern.last {
            tasks::Last::Exact(bytes) | tasks::Last::Open(bytes) => bytes.len(),
        };
        bytes = bytes.checked_add(u64::try_from(terminal).ok()?)?;
    }
    for scope in &charter.authority.note_resources {
        bytes = bytes.checked_add(
            u64::try_from(scope.pattern.segments.len())
                .ok()?
                .checked_mul(u64::try_from(size_of::<Box<[u8]>>()).ok()?)?,
        )?;
        for segment in &scope.pattern.segments {
            bytes = bytes.checked_add(u64::try_from(segment.len()).ok()?)?;
        }
        let terminal = match &scope.pattern.last {
            tasks::Last::Exact(bytes) | tasks::Last::Open(bytes) => bytes.len(),
        };
        bytes = bytes.checked_add(u64::try_from(terminal).ok()?)?;
    }
    Some(bytes)
}

fn header_loaded(startup: Startup) -> bool {
    match startup {
        Startup::Cold | Startup::Loading(Range::Deployment) | Startup::Failed => false,
        Startup::Loading(
            Range::Calls
            | Range::Tasks
            | Range::EndedResults
            | Range::People
            | Range::Forge
            | Range::RunProofs
            | Range::EscalationDecision { .. }
            | Range::ProposalDecision { .. }
            | Range::Turns { .. }
            | Range::TaskTranscript { .. }
            | Range::TaskResult { .. },
        )
        | Startup::Adopting(_)
        | Startup::Running => true,
    }
}

fn hello_within(hello: &fleet::Hello, limits: &fleet::Limits) -> bool {
    let stop_before_grace = hello.stop_bound < limits.grace;
    if hello.hosting.len() > usize::try_from(limits.slots).expect("u32 fits usize")
        || hello.workstreams.len() > usize::try_from(limits.workstreams).expect("u32 fits usize")
        || !stop_before_grace
    {
        return false;
    }
    true
}

fn row_bound(limits: &Limits) -> Option<u64> {
    let tasks = limits.tasks;
    let mut bytes = u64::try_from(size_of::<tasks::TaskRecord>()).ok()?;
    for retained in [
        u64::from(tasks.spec_bytes),
        u64::from(tasks.result_bytes).checked_mul(3)?,
        u64::from(tasks.inbox_bytes),
        u64::from(tasks.inbox_messages).checked_mul(u64::try_from(size_of::<tasks::Word>()).ok()?)?,
        u64::from(tasks.saved_resources).checked_mul(4)?,
        u64::from(tasks.parameters).checked_mul(u64::try_from(size_of::<tasks::Parameter>()).ok()?)?,
        u64::from(tasks.inputs)
            .checked_add(u64::from(tasks.dependencies).checked_mul(2)?)?
            .checked_add(u64::from(tasks.delegates))?
            .checked_mul(8)?,
        u64::from(tasks.contract_choices).checked_mul(u64::try_from(size_of::<tasks::Verdict>()).ok()?)?,
        u64::from(tasks.authority_grants).checked_mul(u64::try_from(size_of::<tasks::Grant>()).ok()?.checked_add(
            u64::from(tasks.authority_segments).checked_mul(u64::try_from(size_of::<Box<[u8]>>()).ok()?)?,
        )?)?,
        u64::from(tasks.authority_grants).checked_mul(
            u64::try_from(size_of::<tasks::ResourceScope>()).ok()?.checked_add(
                u64::from(tasks.authority_segments).checked_mul(u64::try_from(size_of::<Box<[u8]>>()).ok()?)?,
            )?,
        )?,
        u64::from(tasks.authority_bytes),
        u64::from(tasks.executor_kinds).checked_mul(u64::try_from(size_of::<tasks::AuthorityExecutor>()).ok()?)?,
    ] {
        bytes = bytes.checked_add(retained)?;
    }
    let people = limits.people;
    Some(
        bytes
            .max(u64::from(people.identity_bytes))
            .max(u64::from(people.words))
            .max(u64::from(limits.forge.client.answer_bytes))
            .max(u64::from(people.holdings).checked_mul(u64::try_from(size_of::<people::Holding>()).ok()?)?),
    )
}

fn discard_after_stop(domain: &mut Domain, event: Event) {
    match event {
        Event::Loaded { owner, rows, next } => {
            loads::abandon(&mut domain.loads, owner);
            let mut out = Queue::with_capacity(1);
            loads::loaded(&mut domain.loads, owner, rows, next, &mut out);
            assert!(out.is_empty(), "halted waiter emits no delivery");
        }
        Event::Unloaded { owner } => {
            loads::abandon(&mut domain.loads, owner);
            let mut out = Queue::with_capacity(1);
            loads::unloaded(&mut domain.loads, owner, &mut out);
            assert!(out.is_empty(), "halted waiter emits no delivery");
        }
        Event::StartRecurring { .. }
        | Event::ForgeAnswered { .. }
        | Event::ForgeHint { .. }
        | Event::Period { .. }
        | Event::Start
        | Event::ProcedureStep { .. }
        | Event::Committed { .. }
        | Event::Uncommitted { .. }
        | Event::SignedIn { .. }
        | Event::Ask { .. }
        | Event::Hello { .. }
        | Event::Lost { .. }
        | Event::Turn { .. }
        | Event::Call { .. }
        | Event::Answer { .. }
        | Event::ReadEscalation { .. }
        | Event::ReadResult { .. }
        | Event::ReadInbox { .. }
        | Event::ViewInbox { .. }
        | Event::Watch { .. }
        | Event::Unwatch { .. }
        | Event::ViewDelivered { .. }
        | Event::Refreshed { .. }
        | Event::RefreshFailed { .. } => {}
    }
}

fn current_proof(domain: &Domain, task: u64, attempt: u64) -> bool {
    domain.core.current_proof(task, attempt)
}

fn valid_connector_answer(answer: &CallAnswer, deployment: &crate::Deployment, limits: &Limits) -> bool {
    match answer {
        CallAnswer::ForgeEffect { entry, .. } => *entry != 0 && *entry <= deployment.connector_rows,
        CallAnswer::ForgeRead(result) => match result.as_ref() {
            Ok(answer) => match forge_client::answer_bytes(answer, &limits.forge.client) {
                Some(bytes) => {
                    bytes <= u64::from(limits.forge.client.answer_bytes)
                        && bytes <= u64::from(limits.journal.transcript_bytes)
                }
                None => false,
            },
            Err(_) => true,
        },
        CallAnswer::ForgeEffectRefused(_)
        | CallAnswer::ForgeEffectDenied { .. }
        | CallAnswer::Unavailable
        | CallAnswer::Introduced
        | CallAnswer::Unsubscribed
        | CallAnswer::Controlled
        | CallAnswer::ControlDenied { .. }
        | CallAnswer::Proposed { .. }
        | CallAnswer::ProposalDecided { .. }
        | CallAnswer::EscalationDecided { .. }
        | CallAnswer::Sent { .. }
        | CallAnswer::Subscribed { .. }
        | CallAnswer::Delegated(_)
        | CallAnswer::DelegationDenied { .. }
        | CallAnswer::MessageRefused(_)
        | CallAnswer::ProposalRefused(_)
        | CallAnswer::EscalationRefused(_)
        | CallAnswer::SubscriptionRefused(_)
        | CallAnswer::DelegationRefused(_)
        | CallAnswer::ControlRefused(_) => true,
    }
}

/// Reject unsupported root shapes and identities above durable high-water marks before child
/// restoration. Proof rows consume exact transient live-row correlations and never load archive
/// history into the live map.
#[expect(clippy::too_many_lines, reason = "each durable record family has one exhaustive restoration route")]
fn restore_page_row(domain: &mut Domain, env: &Env<Limits>, row: Record) {
    match row {
        Record::Call(record) => {
            let key = record.key;
            if !valid_connector_answer(&record.answer, &domain.core.counters.deployment(), &env.limits) {
                domain.startup = Startup::Failed;
                return;
            }
            let part = record.answer.core_part(domain.config.forge_connector);
            if domain.core.restore_core(
                jig_core::CoreRecord::Call(jig_core::CallRecord { key, part: part.clone() }),
                &core_limits(&env.limits),
            ) != jig_core::Restored::Live
            {
                domain.startup = Startup::Failed;
                return;
            }
            if let jig_core::CallPart::Connector { .. } = part
                && domain.connector_calls.insert(key, record.answer).is_err()
            {
                domain.startup = Startup::Failed;
            }
        }
        Record::Deployment(deployment) => {
            if !domain.forge.bind_deployment(deployment.id, &env.limits.forge) {
                domain.startup = Startup::Failed;
                return;
            }
            match domain.core.restore_core(jig_core::CoreRecord::Deployment(deployment), &core_limits(&env.limits)) {
                jig_core::Restored::Deployment { commits } => {
                    domain.journal = Journal::from_durable(&root_journal_limits(&env.limits), commits);
                }
                jig_core::Restored::Live | jig_core::Restored::Archive | jig_core::Restored::Rejected => {
                    unreachable!("deployment restore result")
                }
            }
        }
        Record::People(people::Stored::Policy { project, value }) => {
            if !domain.core.restore_policy(&core_limits(&domain.limits), project, value) {
                domain.startup = Startup::Failed;
            }
        }
        Record::People(record) => domain.work.push(Work::People(people::Event::Restore { record })),
        Record::Forge { id, row } => {
            let key = forge::stored_key(&row);
            if id == 0
                || id > domain.core.counters.deployment().connector_rows
                || domain.forge_keys.insert(key, id) != Ok(None)
            {
                domain.startup = Startup::Failed;
                return;
            }
            domain.work.push(Work::Forge(forge::Event::Restore { record: *row }));
        }
        Record::Tasks(record) => match record {
            tasks::Stored::PersonProposal(ref row) => {
                if !domain.core.restore_task_row(&record) {
                    domain.startup = Startup::Failed;
                    return;
                }
                domain.work.push(Work::People(people::Event::Waiting {
                    task: row.goal.number,
                    entries: inbox::person_proposal_entries(domain, row),
                }));
                domain.work.push(Work::Tasks(tasks::Event::Restore { record }));
            }
            tasks::Stored::Ended(_) => {
                unreachable!("historical child rows excluded from startup")
            }
            tasks::Stored::Stub(_) => {
                if !domain.core.restore_task_row(&record) {
                    domain.startup = Startup::Failed;
                    return;
                }
                domain.work.push(Work::Tasks(tasks::Event::Restore { record }));
            }
            tasks::Stored::History(_) => unreachable!("history rows excluded from startup"),
            tasks::Stored::Live(ref task) => {
                if !escalation::supported(domain, task) || !domain.core.restore_task_row(&record) {
                    domain.startup = Startup::Failed;
                    return;
                }
                domain.work.push(Work::People(people::Event::Waiting {
                    task: task.number,
                    entries: inbox::entries(domain, task),
                }));
                domain.work.push(Work::Tasks(tasks::Event::Restore { record }));
            }
            tasks::Stored::Ledger(_) | tasks::Stored::Writer(_) | tasks::Stored::Pool(_) => {
                domain.work.push(Work::Tasks(tasks::Event::Restore { record }));
            }
        },
        Record::RunProof(proof) => {
            if domain.core.restore_core(jig_core::CoreRecord::RunProof(proof), &core_limits(&env.limits))
                != jig_core::Restored::Live
            {
                domain.startup = Startup::Failed;
            }
        }
        Record::Turn(_) | Record::Terminal(_) | Record::EscalationDecision(_) | Record::ProposalDecision(_) => {
            unreachable!("startup excludes archive families")
        }
    }
}
