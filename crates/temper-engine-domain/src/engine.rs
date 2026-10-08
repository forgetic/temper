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
mod amendments;
mod escalation;
mod forge_route;
mod goals;
mod inbox;
mod landing;
mod policy;
mod policy_translate;
mod proposals;
mod results;

mod roles;

use crate::{
    CallAnswer, CallKey, Decision, Delivery, Family, Journal, JournalLimits, Key, Output, Range, Record, RunProof,
    TerminalRecord, TurnProof, TurnRecord, Write, loads,
};
use alloc::boxed::Box;
use jig_core::{
    Core, GoalRoute, HistoricalResult, PendingRelay, PersonProposalRoute, PersonTaskRoute, RestoringProof, RoutedCall,
    Transcript,
};
pub use jig_core::{Model, RunCharter, RunPolicy};
use jig_core_accounts as accounts;
use jig_core_authority as authority;
use jig_core_brief as brief;
use jig_core_fleet as fleet;
use jig_core_notes as notes;
use jig_core_people as people;
use jig_core_tasks as tasks;
use jig_core_views as views;
use skein_lib::{Decimal, Env, Id, List, Map, Queue, ReplyTo, Slab, Token, Writer};
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
    permission_roles: Map<u32, Box<[people::PermissionRole]>>,
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
        RootConfig { landing, permission_roles, forge_connector },
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

/// A root-owned part of the task context rendered for a brief.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TaskBriefPart {
    Spec,
    Dependencies,
    Delegates,
    Attempts,
    TranscriptTail,
}

/// A bounded fragment and the amount its root renderer omitted.
#[derive(Debug)]
struct TaskBriefFragment {
    bytes: Box<[u8]>,
    left: u64,
}

/// The result of reading a root-owned part of a task.
enum TaskBriefRead {
    Got(Box<[TaskBriefFragment]>),
    Failed,
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

/// A dependency named by a delegate call before the root allocates task IDs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dependency {
    /// One member of the same batch, by zero-based position.
    Batch(u32),
    /// A live task already referenced by the creator.
    Existing(u64),
}

/// One proposed direct child; the root supplies its ID, requester and funder.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Delegate {
    pub executor: tasks::Executor,
    pub spec: tasks::Spec,
    pub contract: tasks::Contract,
    pub authority: tasks::Authority,
    /// Connector grants whose open terminal is narrowed with the new task number.
    pub symbolic_grants: Box<[tasks::Grant]>,
    pub dependencies: Box<[Dependency]>,
    pub wake: tasks::WakePolicy,
}

/// One connector-owned procedure's chosen task action. The root supplies delegate numbers and
/// checks batch authority before the tasks child admits it.
#[derive(PartialEq, Eq, Debug)]
pub enum ProcedureAction {
    Delegate(Box<[Delegate]>),
    Result(tasks::TaskResult),
    Hold(tasks::Hold),
    Wait,
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
    Tasks(tasks::Event),
    PersonProposal(tasks::Event),
    TaskEscalation(tasks::Event),
    People(people::Event),
    Fleet(fleet::Event),
    Brief(brief::GatherEvent),
    StartBrief { task: u64 },
    Forge(forge::Event),
    TasksClaim { task: u64, attempt: u64, writes: Box<[tasks::Name]> },
    ProjectGoal(Box<tasks::TaskRecord>),
    GoalSubscribe(forge::Subscriber),
    Activate(Box<tasks::RunContext>),
    EscalationLoaded { waiter: Token, rows: Box<[Record]> },
    EscalationFailed { waiter: Token },
    ProposalLoaded { waiter: Token, rows: Box<[Record]> },
    ProposalFailed { waiter: Token },
    DelegateValidated { to: Token, key: CallKey, batch: Box<[Delegate]>, stubs: Box<[tasks::Stub]> },
    DelegateInputRefused { to: Token, key: CallKey },
}

#[derive(Debug)]
struct BriefConnector {
    task: u64,
    source: forge::BriefSource,
    kind: ForgeBriefKind,
    cutting: bool,
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
    Adopting,
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
    forge: forge::Domain,
    forge_keys: Map<forge::Key, u64>,
    adoption_restore: Map<Token, Option<forge::Repository>>,
    forge_subscribing: Map<Token, forge::Subscriber>,
    forge_unsubscribing: Map<Token, (u64, forge::Topic)>,
    forge_reading: Map<Token, (ReplyTo, CallKey)>,
    forge_effecting: Map<u64, (ReplyTo, CallKey)>,
    forge_projection_due: Map<u64, skein_lib::Wall>,
    forge_change_due: Map<u64, skein_lib::Wall>,
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
        let mut forge = forge::Domain::new(
            &limits.forge,
            config.seed,
            core::mem::replace(
                &mut config.forge,
                forge_client::Config { namespace: Box::new([]), writers: Box::new([]) },
            ),
        )
        .expect("valid forge configuration");
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
                        | jig_core::Request::Now(_)
                        | jig_core::Request::Route(_),
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
            forge,
            forge_keys: Map::with_capacity(forge_route::rows(limits).expect("forge row capacity")),
            adoption_restore: Map::with_capacity(limits.forge.adoptions),
            forge_subscribing: Map::with_capacity(limits.fleet.calls),
            forge_unsubscribing: Map::with_capacity(limits.fleet.calls),
            forge_reading: Map::with_capacity(limits.fleet.calls),
            forge_effecting: Map::with_capacity(limits.fleet.calls),
            forge_projection_due: Map::with_capacity(limits.forge.issues),
            forge_change_due: Map::with_capacity(limits.forge.changes),
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

fn environment_views(env: &Env<Limits>) -> Env<views::Limits> {
    Env { now: env.now, wall: env.wall, limits: env.limits.views }
}

fn environment_core(env: &Env<Limits>) -> Env<jig_core::Limits> {
    Env { now: env.now, wall: env.wall, limits: core_limits(&env.limits) }
}

fn core_limits(limits: &Limits) -> jig_core::Limits {
    jig_core::Limits {
        tasks: limits.tasks,
        load_slots: limits.loads.loads,
        call_records: limits.call_records,
        people: limits.people,
        fleet: limits.fleet,
        brief: brief_limits(&limits.brief),
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
                jig_core::Now::NotesIndexed { .. }
                | jig_core::Now::NotesRecalled { .. }
                | jig_core::Now::NotesRefused { .. } => unreachable!("view route owns its now output"),
            },
            jig_core::Request::Decided => {}
            jig_core::Request::Write(_)
            | jig_core::Request::Ask { .. }
            | jig_core::Request::Held(_)
            | jig_core::Request::Route(_) => unreachable!("view route changes no decision"),
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

fn watched_project(domain: &Domain, subject: views::Subject) -> u32 {
    match subject {
        views::Subject::Run { task, .. } | views::Subject::Tree { task } => {
            match domain.core.tasks.delegation(task.raw()) {
                Some(context) => context.project,
                None => 0,
            }
        }
        views::Subject::Goals { project } => project,
        views::Subject::Inbox { .. } => 0,
    }
}

fn watch_views(subject: people::WatchSubject, project: u32) -> views::Subject {
    match subject {
        people::WatchSubject::Run { task, attempt } => {
            views::Subject::Run { task: Token::new(task), attempt: Token::new(attempt) }
        }
        people::WatchSubject::Tree { task } => views::Subject::Tree { task: Token::new(task) },
        people::WatchSubject::Goals => views::Subject::Goals { project },
        people::WatchSubject::Inbox { party } => views::Subject::Inbox { party },
    }
}

fn watch_authorized(domain: &Domain, project: u32, role: Option<people::Role>) -> bool {
    domain.core.watch_authorized(project, role)
}

fn note_authorized(domain: &Domain, project: u32, role: Option<people::Role>, scope: &people::NoteScope) -> bool {
    domain.core.note_authorized(project, role, scope)
}

/// A watch uses people's keyed admission, then opens a volatile view after the last commit is
/// durable. Its key is retained only while the view is open (domain/people.md, 5.1; domain/root.md, 4).
#[expect(clippy::too_many_arguments, reason = "watch admission carries the signed-in caller, key and subject")]
#[expect(clippy::too_many_lines, reason = "one keyed watch admission owns the view opening and terminal answer")]
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
    if !domain.ready() || !domain.core.counters.quiescent(domain.journal.idle()) || !domain.work.is_empty() {
        out.push(Request::WatchRefused { watcher, refusal: people::Refusal::Busy });
        return;
    }
    let ask = people::Ask::Watch { project, subject };
    let mut admitted = Queue::with_capacity(people::max_out(&env.limits.people));
    people::step(
        &mut domain.core.people,
        &environment_people(env),
        people::Event::Ask { reply_to: ReplyTo::new(watcher), sign_in, key, ask },
        &mut admitted,
    );
    let mut opened = false;
    for _ in 0..admitted.len() {
        match admitted.pop().expect("watch admission output count") {
            people::Request::Route { request, person, project, role, ask } => {
                let people::Ask::Watch { subject, .. } = *ask else {
                    unreachable!("watch admission routes only its watch ask")
                };
                let subject = watch_views(subject, project);
                let actual = watched_project(domain, subject);
                let inbox = match subject {
                    views::Subject::Inbox { .. } => true,
                    views::Subject::Run { .. } | views::Subject::Tree { .. } | views::Subject::Goals { .. } => false,
                };
                let outcome = if actual != project || (project == 0 && !inbox) {
                    people::Outcome::Refused(people::Refusal::Unknown)
                } else if project != 0 && !watch_authorized(domain, project, role) {
                    people::Outcome::Refused(people::Refusal::Authority)
                } else if domain.core.watching.contains_key(&watcher)
                    || domain.core.watching.len() >= env.limits.views.watchers
                {
                    people::Outcome::Refused(people::Refusal::Busy)
                } else {
                    match view_snapshot(domain, subject, env.limits.views.snapshot_bytes) {
                        Some(snapshot) => {
                            let mut views_out = Queue::with_capacity(views::max_out(&env.limits.views));
                            views::step(
                                &mut domain.core.views,
                                &environment_views(env),
                                views::Event::Watch { watcher, subject, snapshot },
                                &mut views_out,
                            );
                            let mut result = people::Outcome::Refused(people::Refusal::Unknown);
                            for _ in 0..views_out.len() {
                                match views_out.pop().expect("watch output count") {
                                    views::Request::Watching { watcher: opened_watcher } => {
                                        assert!(opened_watcher == watcher, "watch name is echoed");
                                        out.push(Request::View(views::Request::Watching { watcher }));
                                        result = people::Outcome::Watching { watcher };
                                        opened = true;
                                    }
                                    request @ views::Request::Deliver { .. } => out.push(Request::View(request)),
                                    views::Request::Refused { refusal, .. } => {
                                        result = people::Outcome::Refused(match refusal {
                                            views::Refusal::Busy => people::Refusal::Busy,
                                            views::Refusal::Oversized => people::Refusal::Limit,
                                            views::Refusal::Unknown | views::Refusal::Unfollowed => {
                                                people::Refusal::Unknown
                                            }
                                        });
                                    }
                                    views::Request::Ended { .. } => {
                                        unreachable!("a new watch cannot end before opening")
                                    }
                                }
                            }
                            result
                        }
                        None => people::Outcome::Refused(people::Refusal::Limit),
                    }
                };
                if opened {
                    let inserted = domain.core.watching.insert(watcher, person);
                    assert!(inserted == Ok(None), "watch slot checked before opening");
                }
                let mut decided = Queue::with_capacity(people::max_out(&env.limits.people));
                people::step(
                    &mut domain.core.people,
                    &environment_people(env),
                    people::Event::Decided { request, outcome },
                    &mut decided,
                );
                while let Some(reply) = decided.pop() {
                    match reply {
                        people::Request::Reply { to, reply } => match reply {
                            people::Reply::Outcome(people::Outcome::Watching { watcher: first }) => {
                                if !opened {
                                    out.push(Request::View(views::Request::Watching { watcher: first }));
                                }
                            }
                            people::Reply::Outcome(people::Outcome::Refused(refusal))
                            | people::Reply::Refused(refusal) => {
                                out.push(Request::WatchRefused { watcher: to.into_token(), refusal });
                            }
                            people::Reply::Outcome(_) | people::Reply::SignedIn { .. } | people::Reply::SignedOut => {
                                unreachable!("watch replies only with open or refusal")
                            }
                        },
                        people::Request::Save { .. }
                        | people::Request::Erase { .. }
                        | people::Request::Route { .. }
                        | people::Request::RolesApplied { .. }
                        | people::Request::RolesRefused { .. }
                        | people::Request::ServiceMade { .. }
                        | people::Request::RestoreRefused { .. } => unreachable!("watch writes nothing"),
                    }
                }
            }
            people::Request::Reply { to, reply } => match reply {
                people::Reply::Outcome(people::Outcome::Watching { watcher: first }) => {
                    out.push(Request::View(views::Request::Watching { watcher: first }));
                }
                people::Reply::Outcome(people::Outcome::Refused(refusal)) | people::Reply::Refused(refusal) => {
                    out.push(Request::WatchRefused { watcher: to.into_token(), refusal });
                }
                people::Reply::Outcome(_) | people::Reply::SignedIn { .. } | people::Reply::SignedOut => {
                    unreachable!("watch admission reply shape")
                }
            },
            people::Request::Save { .. }
            | people::Request::Erase { .. }
            | people::Request::RolesApplied { .. }
            | people::Request::RolesRefused { .. }
            | people::Request::ServiceMade { .. }
            | people::Request::RestoreRefused { .. } => unreachable!("watch admission writes nothing"),
        }
    }
}

fn view_byte(bytes: &mut List<u8>, value: &[u8]) -> Option<()> {
    for byte in value {
        bytes.push(*byte).ok()?;
    }
    Some(())
}

fn in_tree(rows: &[tasks::ViewTask], number: u64, ancestor: u64, depth: u32) -> bool {
    let mut current = number;
    for _ in 0..=depth {
        if current == ancestor {
            return true;
        }
        let mut next = None;
        for row in rows {
            if row.number == current {
                next = match row.requester {
                    tasks::Party::Task(parent) => Some(parent),
                    tasks::Party::Person(_) | tasks::Party::Deployment { .. } => None,
                };
                break;
            }
        }
        let Some(parent) = next else { return false };
        current = parent;
    }
    false
}

/// The snapshot's fixed rows carry task number, phase and priority in that order.
/// A run snapshot carries its attempt and last committed turn.
fn view_snapshot(domain: &Domain, subject: views::Subject, bound: u32) -> Option<Box<[u8]>> {
    let mut bytes = List::with_capacity(bound);
    match subject {
        views::Subject::Run { task, attempt } => {
            let proof = domain.core.proofs.get(&task.raw())?;
            if proof.attempt != attempt.raw() {
                return None;
            }
            view_byte(&mut bytes, &proof.attempt.to_be_bytes())?;
            let turn = match proof.turn {
                Some(turn) => turn.turn,
                None => 0,
            };
            view_byte(&mut bytes, &turn.to_be_bytes())?;
        }
        views::Subject::Tree { task: ancestor } => {
            let rows = domain.core.tasks.view_tasks();
            for row in &rows {
                if in_tree(&rows, row.number, ancestor.raw(), domain.limits.tasks.depth) {
                    view_byte(&mut bytes, &row.number.to_be_bytes())?;
                    view_byte(&mut bytes, &row.phase.to_be_bytes())?;
                    view_byte(&mut bytes, &row.tracked.unwrap_or(0).to_be_bytes())?;
                }
            }
        }
        views::Subject::Goals { project } => {
            let rows = domain.core.tasks.view_tasks();
            for row in &rows {
                if row.project == project && row.tracked.is_some() {
                    view_byte(&mut bytes, &row.number.to_be_bytes())?;
                    view_byte(&mut bytes, &row.phase.to_be_bytes())?;
                    view_byte(&mut bytes, &row.tracked.unwrap_or(0).to_be_bytes())?;
                }
            }
        }
        views::Subject::Inbox { .. } => {}
    }
    Some(bytes.into_boxed())
}

fn view_task_saved(domain: &mut Domain, limits: &Limits, decision: &mut Decision, task: &tasks::TaskRecord) {
    let phase = tasks::view_phase(&task.phase);
    let current = (phase, task.tracked);
    let changed = match domain.core.view_phases.get(&task.number) {
        Some(previous) => *previous != current,
        None => true,
    };
    if phase == 4 {
        domain.core.view_phases.remove(&task.number);
    } else {
        domain.core.view_phases.insert(task.number, current).expect("one live phase per task");
    }
    if !changed || domain.core.watching.is_empty() {
        return;
    }
    let mut trees = List::with_capacity(limits.tasks.depth.saturating_add(1));
    trees.push(Token::new(task.number)).expect("self is in its own bounded tree");
    let mut requester = task.requester;
    for _ in 0..limits.tasks.depth {
        requester = match requester {
            tasks::Party::Task(parent) => {
                trees.push(Token::new(parent)).expect("bounded ancestor depth");
                match domain.core.tasks.delegation(parent) {
                    Some(context) => context.requester,
                    None => break,
                }
            }
            tasks::Party::Person(_) | tasks::Party::Deployment { .. } => break,
        };
    }
    emit(
        decision,
        limits,
        Delivery::View(Box::new(views::Event::TaskPhase {
            task: Token::new(task.number),
            trees: trees.into_boxed(),
            project: task.project,
            phase,
            priority: task.tracked,
        })),
    );
}

fn environment_tasks(env: &Env<Limits>) -> Env<tasks::Limits> {
    Env { now: env.now, wall: env.wall, limits: env.limits.tasks }
}

fn environment_people(env: &Env<Limits>) -> Env<people::Limits> {
    Env { now: env.now, wall: env.wall, limits: env.limits.people }
}

fn environment_fleet(env: &Env<Limits>) -> Env<fleet::Limits> {
    Env { now: env.now, wall: env.wall, limits: env.limits.fleet }
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
            let project = watched_project(domain, subject);
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
                start_recurring(domain, env, project, authority, template);
            }
        }
        Event::Period { project, period, budget } => {
            let allowed = match domain.core.authority.policy(project) {
                Some(policy) => budget <= policy.period_spend,
                None => false,
            };
            if domain.ready()
                && admits(domain, &env.limits)
                && period > 0
                && period >= domain.core.settings.period
                && allowed
            {
                domain.core.settings.period = period;
                domain.core.settings.period_budget = budget;
                if domain.core.tasks.funding(tasks::Funder::Period { project, period }).is_none() {
                    domain.work.push(Work::Tasks(tasks::Event::OpenPeriod {
                        reply_to: internal(u64::MAX - 2),
                        project,
                        period,
                        budget,
                    }));
                }
                for task in domain.core.tasks.recurring_tasks(project) {
                    domain.work.push(Work::Tasks(tasks::Event::TickRecurring { task, period }));
                }
                for task in domain.core.tasks.standing_tasks(project) {
                    domain.work.push(Work::Tasks(tasks::Event::RenewStanding { task, period }));
                }
            }
        }
        Event::ProcedureStep { task, step, connector, code, action } => {
            if !domain.ready() || !admits(domain, &env.limits) {
                return;
            }
            drop(procedure_step(domain, env, task, step, connector, code, action));
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
            if domain.core.counters.deployment().people == u64::MAX
                || domain.core.counters.deployment().sign_ins == u64::MAX
            {
                out.push(Request::Deliver(Delivery::WebReply {
                    to: reply_to,
                    sign_in: None,
                    reply: people::Reply::Refused(people::Refusal::Limit),
                }));
                return;
            }
            let person = crate::fresh(&mut domain.core.counters, Family::Person).expect("person counter available");
            let sign_in = crate::fresh(&mut domain.core.counters, Family::SignIn).expect("sign-in counter available");
            domain.core.signing_in = Some(sign_in);
            let kind = if identity.key.provider == domain.core.settings.deployment_provider {
                people::Kind::Service
            } else {
                people::Kind::Person
            };
            domain.work.push(Work::People(people::Event::SignedIn { reply_to, person, sign_in, identity, kind }));
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

fn remember_due(domain: &mut Domain, context: Box<tasks::RunContext>) {
    for task in &domain.core.due {
        if task.task == context.task {
            return;
        }
    }
    domain.core.due.push(context);
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
                    Read::Escalation(escalation::Query::Decide { task, revision, .. }) => (*task, *revision),
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
    if domain.ready() {
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
        if domain.core.accounts.usable(domain.core.settings.account) && !domain.core.due.is_empty() {
            for _ in 0..domain.core.due.len() {
                domain.work.push(Work::Activate(domain.core.due.pop().expect("waiting activation")));
            }
            let decision = route(domain, env);
            close(domain, env, decision, out);
            return;
        }
        let mut decision = route_decision(domain, &env.limits).expect("journal room checked before fleet continuation");
        let mut fleet_out = Queue::with_capacity(fleet::max_out(&env.limits.fleet));
        fleet::resume(&mut domain.core.fleet, &environment_fleet(env), &mut fleet_out);
        fleet_outputs(domain, env, &mut decision, &mut fleet_out);
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
    if !domain.ready() || !admits(domain, &env.limits) {
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
    route_core_requests(domain, env, &mut decision, routed, false, false);
    let routed = jig_core::fire(&mut domain.core, &environment_core(env), jig_core::Timer::Fleet);
    route_core_requests(domain, env, &mut decision, routed, false, false);
    let routed = jig_core::fire(&mut domain.core, &environment_core(env), jig_core::Timer::Brief);
    route_core_requests(domain, env, &mut decision, routed, false, false);
    route_into(domain, env, &mut decision);
    close(domain, env, decision, out);
}

fn route(domain: &mut Domain, env: &Env<Limits>) -> Decision {
    let mut decision = route_decision(domain, &env.limits).expect("journal room checked before routing children");
    route_into(domain, env, &mut decision);
    decision
}

fn route_core_requests(
    domain: &mut Domain,
    env: &Env<Limits>,
    decision: &mut Decision,
    requests: jig_core::Requests,
    person_proposal: bool,
    task_escalation: bool,
) {
    match requests {
        jig_core::Requests::Out(mut output) => {
            for _ in 0..output.len() {
                match output.pop().expect("core output count") {
                    jig_core::Request::Write(write) => match write {
                        jig_core::Write::Save(record) => match record {
                            jig_core::Record::People(row) => {
                                save(decision, &env.limits, Write::Save(Record::People(row)));
                            }
                            jig_core::Record::Core(_) | jig_core::Record::Tasks(_) | jig_core::Record::Notes(_) => {
                                unreachable!("the core route owns its write family")
                            }
                        },
                        jig_core::Write::Erase(key) => match key {
                            jig_core::Key::People(key) => save(decision, &env.limits, Write::Erase(Key::People(key))),
                            jig_core::Key::Tasks(key) => save(decision, &env.limits, Write::Erase(Key::Tasks(key))),
                            jig_core::Key::Core(_) | jig_core::Key::Notes(_) => {
                                unreachable!("the core route owns its erase family")
                            }
                        },
                    },
                    jig_core::Request::Ask { connector, ask } => {
                        let request = match ask {
                            jig_core::Ask::Gather { section, budget } => {
                                brief::GatherRequest::Gather { connector, section, budget }
                            }
                            jig_core::Ask::CutTo { section, size } => {
                                brief::GatherRequest::CutTo { connector, section, size }
                            }
                            jig_core::Ask::Drop { section } => brief::GatherRequest::Drop { connector, section },
                        };
                        let mut child = Queue::with_capacity(1);
                        child.push(request);
                        brief_outputs(domain, env, decision, &mut child);
                    }
                    jig_core::Request::Route(route) => match *route {
                        jig_core::Route::Tasks(request) => {
                            let mut child = Queue::with_capacity(1);
                            child.push(*request);
                            tasks_outputs(domain, env, decision, &mut child, person_proposal, task_escalation);
                        }
                        jig_core::Route::People(request) => {
                            let mut child = Queue::with_capacity(1);
                            child.push(request);
                            people_outputs(domain, env, decision, &mut child);
                        }
                        jig_core::Route::Brief(request) => {
                            let mut child = Queue::with_capacity(1);
                            child.push(*request);
                            brief_outputs(domain, env, decision, &mut child);
                        }
                        jig_core::Route::Fleet(request) => {
                            let mut child = Queue::with_capacity(1);
                            child.push(*request);
                            fleet_outputs(domain, env, decision, &mut child);
                        }
                    },
                    jig_core::Request::Held(held) => match *held {
                        jig_core::Held::Relay { task, attempt, previous, word } => {
                            emit(decision, &env.limits, Delivery::Relay { task, attempt, previous, word });
                        }
                        jig_core::Held::PeopleReply { to, sign_in, reply } => {
                            emit(decision, &env.limits, Delivery::WebReply { to, sign_in, reply });
                        }
                        jig_core::Held::NotesLoad { .. }
                        | jig_core::Held::NotesWritten { .. }
                        | jig_core::Held::NotesDeleted { .. } => {
                            unreachable!("the current application has no note caller")
                        }
                    },
                    jig_core::Request::Now(_) => unreachable!("the current application has no immediate core route"),
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
            Work::Tasks(event) => {
                let routed = jig_core::step(&mut domain.core, &environment_core(env), jig_core::Event::Tasks(event));
                route_core_requests(domain, env, decision, routed, false, false);
            }
            Work::PersonProposal(event) => {
                let routed = jig_core::step(&mut domain.core, &environment_core(env), jig_core::Event::Tasks(event));
                route_core_requests(domain, env, decision, routed, true, false);
            }
            Work::TaskEscalation(event) => {
                let routed = jig_core::step(&mut domain.core, &environment_core(env), jig_core::Event::Tasks(event));
                route_core_requests(domain, env, decision, routed, false, true);
            }
            Work::People(event) => {
                let routed = jig_core::step(&mut domain.core, &environment_core(env), jig_core::Event::People(event));
                route_core_requests(domain, env, decision, routed, false, false);
            }
            Work::Fleet(event) => {
                let routed = jig_core::step(&mut domain.core, &environment_core(env), jig_core::Event::Fleet(event));
                route_core_requests(domain, env, decision, routed, false, false);
            }
            Work::Brief(event) => {
                let routed = jig_core::step(&mut domain.core, &environment_core(env), jig_core::Event::Brief(event));
                route_core_requests(domain, env, decision, routed, false, false);
            }
            Work::StartBrief { task } => start_brief(domain, env, task),
            Work::Forge(event) => {
                let mut out = Queue::with_capacity(forge::max_out(&env.limits.forge));
                forge::step(&mut domain.forge, &environment_forge(env), event, &mut out);
                forge_route::outputs(domain, env, decision, &mut out);
            }
            Work::TasksClaim { task, attempt, writes } => {
                if domain.core.claiming.get(&task) == Some(&attempt) {
                    let budget = domain.assignments.get(&task).expect("claim has assignment").run.budget;
                    domain.work.push(Work::Tasks(tasks::Event::Claim {
                        reply_to: internal(task),
                        task,
                        attempt,
                        budget,
                        writes,
                    }));
                }
            }
            Work::ProjectGoal(goal) => forge_route::project_goal(domain, env, &goal),
            Work::GoalSubscribe(subscriber) => forge_route::goal_subscribed(domain, env, subscriber),
            Work::Activate(task) => activate(domain, env, decision, task),
            Work::EscalationLoaded { waiter, rows } => escalation::loaded(domain, env, waiter, rows),
            Work::EscalationFailed { waiter } => escalation::failed(domain, waiter),
            Work::ProposalLoaded { waiter, rows } => proposals::historical_loaded(domain, waiter, rows),
            Work::ProposalFailed { waiter } => proposals::historical_failed(domain, waiter),
            Work::DelegateValidated { to, key, batch, stubs } => {
                delegate_call(domain, env, decision, ReplyTo::new(to), key, batch, true, stubs);
            }
            Work::DelegateInputRefused { to, key } => decide_call(
                domain,
                &env.limits,
                decision,
                ReplyTo::new(to),
                key,
                CallAnswer::DelegationRefused(tasks::Problem {
                    task: Some(key.task),
                    why: tasks::Refusal::Inputs,
                    blocked_by: None,
                }),
            ),
        }
    }
    assert!(domain.work.is_empty(), "finite synchronous root handoffs finish within the configured route bound");
}

#[expect(clippy::too_many_arguments, reason = "one authenticated message route with optional question identity")]
fn route_person_message(
    domain: &mut Domain,
    env: &Env<Limits>,
    request: Token,
    person: u64,
    project: u32,
    task: u64,
    question: Option<u64>,
    words: Box<[u8]>,
) {
    match domain.core.person_message(jig_core::PersonMessage {
        request,
        person,
        project,
        task,
        question,
        words,
        at: env.wall,
    }) {
        Ok(event) => domain.work.push(Work::Tasks(event)),
        Err(why) => {
            domain.work.push(Work::People(people::Event::Decided { request, outcome: people::Outcome::Refused(why) }));
        }
    }
}

fn route_person_priorities(
    domain: &mut Domain,
    request: Token,
    person: u64,
    project: u32,
    role: Option<people::Role>,
    goals: Box<[(u64, u32)]>,
) {
    let event = match domain.core.prioritise(request, person, project, role, goals, domain.limits.tasks.tasks) {
        Ok(event) => event,
        Err(why) => return person_control_refused(domain, request, why),
    };
    domain.work.push(Work::Tasks(event));
}

#[expect(clippy::too_many_lines, reason = "the keyed people routes each retain an exhaustive typed branch")]
fn people_outputs(domain: &mut Domain, env: &Env<Limits>, decision: &mut Decision, out: &mut Queue<people::Request>) {
    for _ in 0..out.len() {
        match out.pop().expect("people output count") {
            people::Request::ServiceMade { request, outcome } => {
                domain.work.push(Work::People(people::Event::Decided { request, outcome }));
            }
            people::Request::Save { .. } | people::Request::Erase { .. } | people::Request::Reply { .. } => {
                unreachable!("the core marks party writes and held replies")
            }
            people::Request::Route { request, person, project, role, ask } => match *ask {
                people::Ask::Watch { .. } => unreachable!("watch routes before the decision loop"),
                people::Ask::EditNote { scope, .. } => {
                    let allowed = note_authorized(domain, project, role, &scope);
                    domain.work.push(Work::People(people::Event::Decided {
                        request,
                        outcome: people::Outcome::Refused(if allowed {
                            people::Refusal::NotOffered
                        } else {
                            people::Refusal::Authority
                        }),
                    }));
                }
                people::Ask::MakeService { role: service_role, .. } => {
                    let outcome = match roles::allowed(domain, person, project) {
                        Ok(())
                            if domain.core.authority.role(project, escalation::role_number(service_role)).is_some() =>
                        {
                            match crate::fresh(&mut domain.core.counters, Family::Person) {
                                Some(candidate) => {
                                    domain
                                        .work
                                        .push(Work::People(people::Event::MakeService { request, person: candidate }));
                                    None
                                }
                                None => Some(people::Outcome::Refused(people::Refusal::Limit)),
                            }
                        }
                        Ok(()) => Some(people::Outcome::Refused(people::Refusal::Unknown)),
                        Err(refusal) => Some(people::Outcome::Refused(refusal)),
                    };
                    if let Some(outcome) = outcome {
                        domain.work.push(Work::People(people::Event::Decided { request, outcome }));
                    }
                }
                people::Ask::Adopt { adoption, .. } => {
                    let parsed = match roles::allowed(domain, person, project) {
                        Ok(()) => Ok(forge_route::parse_adoption(project, adoption, domain.config.forge_connector)),
                        Err(refusal) => Err(refusal),
                    };
                    match parsed {
                        Ok(Some(adoption)) if domain.adoption_restore.len() < env.limits.forge.adoptions => {
                            let previous = domain.forge.repository(adoption.provider).cloned();
                            assert!(
                                domain.adoption_restore.insert(request, previous) == Ok(None),
                                "one keyed adoption flight"
                            );
                            domain.work.push(Work::Forge(forge::Event::Adopt { reply_to: request, adoption }));
                        }
                        Ok(Some(_)) => domain.work.push(Work::People(people::Event::Decided {
                            request,
                            outcome: people::Outcome::Refused(people::Refusal::Busy),
                        })),
                        Ok(None) => domain.work.push(Work::People(people::Event::Decided {
                            request,
                            outcome: people::Outcome::Refused(people::Refusal::Unknown),
                        })),
                        Err(refusal) => domain.work.push(Work::People(people::Event::Decided {
                            request,
                            outcome: people::Outcome::Refused(refusal),
                        })),
                    }
                }
                people::Ask::SetGoal { spec, charter, budget, priority, .. } => {
                    goals::start(
                        domain,
                        env,
                        request,
                        person,
                        project,
                        role.expect("goal membership admitted"),
                        spec,
                        charter,
                        budget,
                        priority,
                    );
                }
                control @ (people::Ask::Stop { .. } | people::Ask::Cancel { .. } | people::Ask::Release { .. }) => {
                    route_person_control(domain, request, person, project, role, control);
                }
                task_ask @ (people::Ask::TakePerson { .. }
                | people::Ask::HandBackPerson { .. }
                | people::Ask::AnswerPerson { .. }) => {
                    route_person_task(domain, request, person, project, role, task_ask);
                }
                people::Ask::Say { task, words, .. } => {
                    route_person_message(domain, env, request, person, project, task, None, words);
                }
                people::Ask::AnswerQuestion { task, question, words, .. } => {
                    route_person_message(domain, env, request, person, project, task, Some(question), words);
                }
                people::Ask::Prioritise { goals, .. } => {
                    route_person_priorities(domain, request, person, project, role, goals);
                }
                people::Ask::Amend { task, amendment, .. } => {
                    amendments::begin(domain, env, request, person, role, project, task, amendment);
                }
                chat_ask @ people::Ask::StartChat { .. } => {
                    let Some(role) = role else { unreachable!("chat membership admitted") };
                    make_chat(domain, env, request, person, project, role, chat_ask);
                }
                people::Ask::SetRoles { holdings, .. } => {
                    roles::begin(domain, env, decision, request, person, project, holdings);
                }
                people::Ask::ChangePolicy { change, .. } => {
                    policy::change(domain, env, decision, request, person, project, change);
                }
                people::Ask::SetPool { person: beneficiary, budget, .. } => {
                    policy::pool(domain, env, decision, request, person, project, beneficiary, budget);
                }
                people::Ask::Move { task, reason, .. } => {
                    move_for_person(domain, env, request, person, role, project, task, reason);
                }
                people::Ask::DecideEscalation { task, revision, decision, .. } => {
                    escalation::begin(domain, request, person, role, project, task, revision, decision);
                }
                people::Ask::DecideProposal { proposer, proposal, decision: choice, .. } => {
                    if domain.core.tasks.person_proposal(proposer, proposal).is_some() {
                        goals::decide(domain, env, request, person, role, project, proposer, proposal, choice);
                    } else if domain.core.tasks.proposal(proposer, proposal).is_some() {
                        proposals::person_decide(
                            domain, env, decision, request, person, role, project, proposer, proposal, choice,
                        );
                    } else {
                        proposals::historical_begin(
                            domain, env, decision, request, person, project, proposer, proposal,
                        );
                    }
                }
            },
            people::Request::RolesApplied { .. } => {
                unreachable!("serialized roles route consumes application terminal")
            }
            people::Request::RolesRefused { .. } | people::Request::RestoreRefused { .. } => {
                domain.startup = Startup::Failed;
            }
        }
    }
}

fn route_person_task(
    domain: &mut Domain,
    request: Token,
    person: u64,
    project: u32,
    role: Option<people::Role>,
    ask: people::Ask,
) {
    match domain.core.person_task(request, person, project, role, ask) {
        Ok(event) => domain.work.push(Work::Tasks(event)),
        Err(why) => {
            domain.work.push(Work::People(people::Event::Decided { request, outcome: people::Outcome::Refused(why) }));
        }
    }
}

fn person_control_refused(domain: &mut Domain, request: Token, why: people::Refusal) {
    domain.work.push(Work::People(people::Event::Decided { request, outcome: people::Outcome::Refused(why) }));
}

fn route_person_control(
    domain: &mut Domain,
    request: Token,
    person: u64,
    project: u32,
    role: Option<people::Role>,
    ask: people::Ask,
) {
    let task = match domain.core.control_admit(
        &ask,
        person,
        project,
        role,
        domain.limits.tasks.depth,
        domain.limits.tasks.result_bytes,
    ) {
        Ok(task) => task,
        Err(why) => return person_control_refused(domain, request, why),
    };
    match ask {
        people::Ask::Stop { .. } => {
            domain.work.push(Work::Tasks(tasks::Event::Hold { task, why: tasks::Hold::StoppedBy { party: person } }));
            domain
                .work
                .push(Work::People(people::Event::Decided { request, outcome: people::Outcome::Stopped { task } }));
        }
        people::Ask::Cancel { reason, .. } => {
            assert!(
                domain.core.person_tasks.insert(request, PersonTaskRoute::Cancel(task)) == Ok(None),
                "one cancel route"
            );
            domain.work.push(Work::Tasks(tasks::Event::Control {
                reply_to: ReplyTo::new(request),
                by: tasks::Party::Person(person),
                task,
                control: tasks::Control::Cancel { reason },
            }));
        }
        people::Ask::Release { .. } => {
            assert!(
                domain.core.person_tasks.insert(request, PersonTaskRoute::Release(task)) == Ok(None),
                "one release route"
            );
            domain.work.push(Work::Tasks(tasks::Event::Control {
                reply_to: ReplyTo::new(request),
                by: tasks::Party::Person(person),
                task,
                control: tasks::Control::Release,
            }));
        }
        people::Ask::TakePerson { .. }
        | people::Ask::HandBackPerson { .. }
        | people::Ask::AnswerPerson { .. }
        | people::Ask::Move { .. }
        | people::Ask::DecideProposal { .. }
        | people::Ask::Say { .. }
        | people::Ask::AnswerQuestion { .. }
        | people::Ask::Prioritise { .. }
        | people::Ask::Amend { .. }
        | people::Ask::Watch { .. }
        | people::Ask::EditNote { .. }
        | people::Ask::MakeService { .. }
        | people::Ask::SetRoles { .. }
        | people::Ask::Adopt { .. }
        | people::Ask::ChangePolicy { .. }
        | people::Ask::SetPool { .. }
        | people::Ask::DecideEscalation { .. }
        | people::Ask::StartChat { .. }
        | people::Ask::SetGoal { .. } => {
            unreachable!("control route owns its ask")
        }
    }
}

#[expect(clippy::too_many_arguments, reason = "one authenticated keyed move carries its person, project and target")]
fn move_for_person(
    domain: &mut Domain,
    env: &Env<Limits>,
    request: Token,
    person: u64,
    role: Option<people::Role>,
    project: u32,
    task: u64,
    reason: Box<[u8]>,
) {
    if let Err(why) =
        domain.core.move_admit(person, role, project, task, env.limits.tasks.tree_tasks, env.limits.tasks.depth)
    {
        domain.work.push(Work::People(people::Event::Decided { request, outcome: people::Outcome::Refused(why) }));
        return;
    }
    assert!(domain.core.moving.insert(request, task) == Ok(None), "one move flight per keyed request");
    domain.work.push(Work::Tasks(tasks::Event::Move {
        reply_to: ReplyTo::new(request),
        task,
        person,
        period: domain.core.settings.period,
        pool_budget: domain.core.settings.person_budget,
        period_budget: domain.core.settings.period_budget,
        reason,
    }));
}

fn make_chat(
    domain: &mut Domain,
    env: &Env<Limits>,
    request: Token,
    person: u64,
    project: u32,
    role: people::Role,
    ask: people::Ask,
) {
    if let Err(why) = domain.core.chat_admit(project, person, role, env.limits.tasks.tree_tasks) {
        domain.work.push(Work::People(people::Event::Decided { request, outcome: people::Outcome::Refused(why) }));
        return;
    }
    let pool = tasks::Funder::Pool { project, person, period: domain.core.settings.period };
    let period = tasks::Funder::Period { project, period: domain.core.settings.period };
    if domain.core.tasks.funding(period).is_none() {
        domain.work.push(Work::Tasks(tasks::Event::OpenPeriod {
            reply_to: internal(0),
            project,
            period: domain.core.settings.period,
            budget: domain.core.settings.period_budget,
        }));
    }
    if domain.core.tasks.funding(pool).is_none() {
        domain.work.push(Work::Tasks(tasks::Event::CarvePool {
            reply_to: internal(0),
            project,
            person,
            period: domain.core.settings.period,
            budget: domain.core.settings.person_budget,
        }));
    }
    let Some(number) = crate::fresh(&mut domain.core.counters, Family::Task) else {
        domain.work.push(Work::People(people::Event::Decided {
            request,
            outcome: people::Outcome::Refused(people::Refusal::Limit),
        }));
        return;
    };
    assert!(domain.core.made.insert(request, (number, false)) == Ok(None), "people route has unique pending key");
    let words = match ask {
        people::Ask::StartChat { words, .. } => words,
        people::Ask::DecideEscalation { .. }
        | people::Ask::DecideProposal { .. }
        | people::Ask::Watch { .. }
        | people::Ask::EditNote { .. }
        | people::Ask::MakeService { .. }
        | people::Ask::SetRoles { .. }
        | people::Ask::Adopt { .. }
        | people::Ask::ChangePolicy { .. }
        | people::Ask::SetPool { .. }
        | people::Ask::Say { .. }
        | people::Ask::AnswerQuestion { .. }
        | people::Ask::Prioritise { .. }
        | people::Ask::Amend { .. }
        | people::Ask::Move { .. }
        | people::Ask::TakePerson { .. }
        | people::Ask::HandBackPerson { .. }
        | people::Ask::AnswerPerson { .. }
        | people::Ask::Stop { .. }
        | people::Ask::Cancel { .. }
        | people::Ask::Release { .. }
        | people::Ask::SetGoal { .. } => {
            unreachable!("other asks routed separately")
        }
    };
    let spec = tasks::Spec { words, parameters: Box::new([]), inputs: Box::new([]) };
    let Some(holdings) = forge_route::task_holdings(
        domain,
        env,
        project,
        number,
        number,
        tasks::Executor::Agent { charter: domain.core.settings.charter },
        &spec,
        None,
    ) else {
        let _: Option<(u64, bool)> = domain.core.made.remove(&request);
        domain.work.push(Work::People(people::Event::Decided {
            request,
            outcome: people::Outcome::Refused(people::Refusal::Limit),
        }));
        return;
    };
    domain.work.push(Work::Tasks(tasks::Event::Make {
        reply_to: ReplyTo::new(request),
        creator: tasks::Party::Person(person),
        batch: Box::new([tasks::New {
            number,
            project,
            executor: tasks::Executor::Agent { charter: domain.core.settings.charter },
            spec,
            contract: tasks::Contract::Report { words: env.limits.tasks.result_bytes },
            authority: task_authority(&domain.core.settings.chat_authority),
            numbers: tasks::Numbers {
                budget: domain.core.settings.chat_authority.budget.spend,
                spent: 0,
                spent_below: 0,
                reserved: 0,
            },
            funder: pool,
            dependencies: Box::new([]),
            holdings,
            wake: tasks::WakePolicy::DEFAULT,
            recurring: None,
            tracked: None,
        }]),
    }));
}

/// Consume the actual child activation context for authority/account readiness and the person-chat
/// brief; retain only credential waits and drop the context on claim/failure.
fn activate(domain: &mut Domain, env: &Env<Limits>, decision: &mut Decision, task: Box<tasks::RunContext>) {
    let number = task.task;
    if !domain.ready() {
        remember_due(domain, task);
        return;
    }
    if domain.core.tasks.recurring_template(number).is_some() {
        return;
    }
    match task.executor {
        tasks::Executor::Person(_) => return,
        tasks::Executor::Procedure { connector, code: 2 } if connector == domain.config.forge_connector => {
            if !forge_route::start_change(domain, env, &task) {
                domain.work.push(Work::Tasks(tasks::Event::Hold { task: number, why: tasks::Hold::Effects }));
            }
            return;
        }
        tasks::Executor::Procedure { connector, code } => {
            let Some(step) = task.previous_attempt.checked_add(1) else {
                domain.work.push(Work::Tasks(tasks::Event::Hold { task: number, why: tasks::Hold::Effects }));
                return;
            };
            emit(decision, &env.limits, Delivery::Procedure { task: number, step, connector, code });
            return;
        }
        tasks::Executor::Agent { .. } => {}
    }
    match domain.core.run_admission(&task, env.wall, Box::new([])) {
        jig_core::RunAdmission::Allow => {}
        jig_core::RunAdmission::Account => {
            remember_due(domain, task);
            return;
        }
        jig_core::RunAdmission::Hold(why) => {
            domain.work.push(Work::Tasks(tasks::Event::Hold { task: number, why }));
            return;
        }
    }
    let previous_attempt = task.previous_attempt;
    let has_transcript = task.ever_turned;
    let waiter = if has_transcript {
        match domain.result_reads.insert(Some(Read::Transcript { task: number })) {
            Ok(waiter) => Some(waiter),
            Err(_) => {
                remember_due(domain, task);
                return;
            }
        }
    } else {
        None
    };
    assert!(domain.core.contexts.insert(number, task).is_ok(), "bounded activation context");
    domain.core.begin_transcript(number, previous_attempt);
    domain.work.push(Work::Tasks(tasks::Event::Prepare { reply_to: internal(number), task: number }));
    match waiter {
        Some(waiter) => emit(
            decision,
            &env.limits,
            Delivery::Load { waiter: waiter.token(), range: Range::TaskTranscript { task: number }, after: None },
        ),
        None => {
            if let Some((waiter, first)) = begin_dependency_read(domain, number) {
                emit(
                    decision,
                    &env.limits,
                    Delivery::Load { waiter, range: Range::TaskResult { task: first }, after: None },
                );
            }
        }
    }
}

fn retire_calls(domain: &mut Domain, limits: &Limits, decision: &mut Decision, task: u64, attempt: u64, turn: u32) {
    for key in domain.core.retire_calls(task, attempt, turn, limits.call_records) {
        let _connector = domain.connector_calls.remove(&key);
        save(decision, limits, Write::Erase(Key::Call(key)));
    }
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

#[expect(clippy::too_many_arguments, reason = "the named message route carries root, call and whole words")]
fn message_call(
    domain: &mut Domain,
    env: &Env<Limits>,
    decision: &mut Decision,
    to: ReplyTo,
    key: CallKey,
    target: u64,
    form: MessageForm,
    words: Box<[u8]>,
) {
    let Some(context) = domain.core.tasks.delegation(key.task) else {
        decide_call(
            domain,
            &env.limits,
            decision,
            to,
            key,
            CallAnswer::MessageRefused(tasks::Problem {
                task: Some(key.task),
                why: tasks::Refusal::Unknown,
                blocked_by: None,
            }),
        );
        return;
    };
    let Some(number) = crate::fresh(&mut domain.core.counters, Family::Message) else {
        decide_call(
            domain,
            &env.limits,
            decision,
            to,
            key,
            CallAnswer::MessageRefused(tasks::Problem {
                task: Some(target),
                why: tasks::Refusal::Busy,
                blocked_by: None,
            }),
        );
        return;
    };
    let token = to.into_token();
    assert!(domain.core.pending_calls.insert(key, true).is_ok(), "call record room reserved");
    assert!(domain.core.routing_calls.insert(token, RoutedCall::Message(key)) == Ok(None), "one live routed call");
    domain.work.push(Work::Tasks(tasks::Event::Message {
        reply_to: ReplyTo::new(token),
        project: context.project,
        task: target,
        word: tasks::Word {
            number,
            from: tasks::Party::Task(key.task),
            kind: match form {
                MessageForm::Words => tasks::MessageKind::Words,
                MessageForm::Question => tasks::MessageKind::Question,
                MessageForm::Answer { question } => tasks::MessageKind::Answer { question },
            },
            words,
            at: env.wall,
            hits: 1,
            eligible: false,
        },
    }));
}

fn introduce_call(domain: &mut Domain, to: ReplyTo, key: CallKey, left: u64, right: u64) {
    let token = to.into_token();
    assert!(domain.core.pending_calls.insert(key, true).is_ok(), "call record room reserved");
    assert!(domain.core.routing_calls.insert(token, RoutedCall::Introduce(key)) == Ok(None), "one live routed call");
    domain.work.push(Work::Tasks(tasks::Event::Introduce { reply_to: ReplyTo::new(token), by: key.task, left, right }));
}

fn subscribe_call(
    domain: &mut Domain,
    env: &Env<Limits>,
    decision: &mut Decision,
    to: ReplyTo,
    key: CallKey,
    kind: tasks::SubscriptionKind,
) {
    let Some(subscription) = crate::fresh(&mut domain.core.counters, Family::Message) else {
        decide_call(
            domain,
            &env.limits,
            decision,
            to,
            key,
            CallAnswer::SubscriptionRefused(tasks::Problem {
                task: Some(key.task),
                why: tasks::Refusal::Busy,
                blocked_by: None,
            }),
        );
        return;
    };
    let token = to.into_token();
    assert!(domain.core.pending_calls.insert(key, true).is_ok(), "call record room reserved");
    assert!(
        domain.core.routing_calls.insert(token, RoutedCall::Subscribe { key, subscription }) == Ok(None),
        "one live routed call"
    );
    domain.work.push(Work::Tasks(tasks::Event::Subscribe {
        reply_to: ReplyTo::new(token),
        task: key.task,
        subscription: tasks::Subscription { number: subscription, kind },
    }));
}

fn unsubscribe_call(domain: &mut Domain, to: ReplyTo, key: CallKey, subscription: u64) {
    let token = to.into_token();
    assert!(domain.core.pending_calls.insert(key, true).is_ok(), "call record room reserved");
    assert!(domain.core.routing_calls.insert(token, RoutedCall::Unsubscribe(key)) == Ok(None), "one live routed call");
    if let Some(topic) = domain.forge.subscription(key.task, subscription) {
        assert!(domain.forge_unsubscribing.insert(token, (key.task, topic)) == Ok(None), "one connector unsubscribe");
    }
    domain.work.push(Work::Tasks(tasks::Event::Unsubscribe {
        reply_to: ReplyTo::new(token),
        task: key.task,
        subscription,
    }));
}

fn control_call(domain: &mut Domain, to: ReplyTo, key: CallKey, target: u64, control: tasks::Control) {
    let token = to.into_token();
    assert!(domain.core.pending_calls.insert(key, true).is_ok(), "call record room reserved");
    assert!(domain.core.routing_calls.insert(token, RoutedCall::Control(key)) == Ok(None), "one live routed call");
    domain.work.push(Work::Tasks(tasks::Event::Control {
        reply_to: ReplyTo::new(token),
        by: tasks::Party::Task(key.task),
        task: target,
        control,
    }));
}

fn amend_call(
    domain: &mut Domain,
    env: &Env<Limits>,
    decision: &mut Decision,
    to: ReplyTo,
    key: CallKey,
    target: u64,
    amendment: tasks::Amendment,
) {
    let stop_run = match domain.core.task_amend_admit(key.task, target, &amendment, env.limits.tasks.depth) {
        Ok(stop_run) => stop_run,
        Err(jig_core::TaskAmendDenied::Refused { task, why }) => {
            return decide_call(
                domain,
                &env.limits,
                decision,
                to,
                key,
                CallAnswer::ControlRefused(tasks::Problem { task: Some(task), why, blocked_by: None }),
            );
        }
        Err(jig_core::TaskAmendDenied::Denied(answer)) => {
            return decide_call(domain, &env.limits, decision, to, key, CallAnswer::ControlDenied { answer });
        }
    };
    let Some(message) = crate::fresh(&mut domain.core.counters, Family::Message) else {
        return decide_call(
            domain,
            &env.limits,
            decision,
            to,
            key,
            CallAnswer::ControlRefused(tasks::Problem {
                task: Some(target),
                why: tasks::Refusal::Busy,
                blocked_by: None,
            }),
        );
    };
    let token = to.into_token();
    assert!(domain.core.pending_calls.insert(key, true).is_ok(), "call record room reserved");
    assert!(domain.core.routing_calls.insert(token, RoutedCall::Control(key)) == Ok(None), "one amendment route");
    domain.work.push(Work::Tasks(tasks::Event::Amend {
        reply_to: ReplyTo::new(token),
        by: tasks::Party::Task(key.task),
        task: target,
        message,
        stop_run,
        amendment,
    }));
}

fn delegation_executor(domain: &Domain, project: u32, executor: tasks::Executor) -> Option<authority::Executor> {
    match executor {
        tasks::Executor::Agent { charter } => Some(authority::Executor::Charter(charter)),
        tasks::Executor::Procedure { code, .. } => Some(authority::Executor::Procedure(code)),
        tasks::Executor::Person(tasks::PersonAddress::Role(role)) => {
            if role <= 3 {
                Some(authority::Executor::Role(role))
            } else {
                None
            }
        }
        tasks::Executor::Person(tasks::PersonAddress::Person(person)) => {
            let holding = domain.core.people.role(person, project)?;
            Some(authority::Executor::Role(escalation::role_number(holding)))
        }
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "one delegated call checks authority, inputs and the atomic batch before routing"
)]
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
    if !current_proof(domain, key.task, key.attempt) {
        decide_call(
            domain,
            &env.limits,
            decision,
            to,
            key,
            CallAnswer::DelegationRefused(tasks::Problem {
                task: Some(key.task),
                why: tasks::Refusal::State,
                blocked_by: None,
            }),
        );
        return;
    }
    let Some(context) = domain.core.tasks.delegation(key.task) else {
        decide_call(
            domain,
            &env.limits,
            decision,
            to,
            key,
            CallAnswer::DelegationRefused(tasks::Problem {
                task: Some(key.task),
                why: tasks::Refusal::Unknown,
                blocked_by: None,
            }),
        );
        return;
    };
    if batch.is_empty() || batch.len() > usize::try_from(env.limits.tasks.batch).expect("u32 fits usize") {
        decide_call(
            domain,
            &env.limits,
            decision,
            to,
            key,
            CallAnswer::DelegationRefused(tasks::Problem { task: None, why: tasks::Refusal::Batch, blocked_by: None }),
        );
        return;
    }
    let mut asked = List::with_capacity(env.limits.tasks.batch);
    for member in &batch {
        let Some(executor) = delegation_executor(domain, context.project, member.executor) else {
            decide_call(
                domain,
                &env.limits,
                decision,
                to,
                key,
                CallAnswer::DelegationRefused(tasks::Problem {
                    task: None,
                    why: tasks::Refusal::Executor,
                    blocked_by: None,
                }),
            );
            return;
        };
        let Some(symbolic) = symbolic_grants(&member.symbolic_grants, env.limits.authority.grants) else {
            decide_call(
                domain,
                &env.limits,
                decision,
                to,
                key,
                CallAnswer::DelegationRefused(tasks::Problem {
                    task: None,
                    why: tasks::Refusal::AuthorityShape,
                    blocked_by: None,
                }),
            );
            return;
        };
        asked
            .push(authority::Delegate { executor, authority: authority_value(&member.authority), symbolic })
            .expect("bounded delegation request");
    }
    let mut findings =
        Queue::with_capacity(authority::max_out(domain.core.authority.limits()).expect("findings bound"));
    let checked = authority::check_batch(
        &domain.core.authority,
        &authority::BatchAsk {
            project: context.project,
            creator: authority_value(&context.authority),
            numbers: authority_numbers(context.numbers),
            tasks_left: context.tasks_left,
            tasks: asked.into_boxed(),
        },
        &mut findings,
    );
    if checked.answer != authority::Answer::Allow {
        let mut found =
            List::with_capacity(authority::max_out(domain.core.authority.limits()).expect("findings bound"));
        for _ in 0..findings.len() {
            found.push(findings.pop().expect("finding count")).expect("finding bound");
        }
        decide_call(
            domain,
            &env.limits,
            decision,
            to,
            key,
            CallAnswer::DelegationDenied { answer: checked.answer, findings: found.into_boxed() },
        );
        return;
    }
    if !validated {
        let capacity = batch
            .len()
            .checked_mul(usize::try_from(env.limits.tasks.inputs).expect("u32 fits usize"))
            .expect("bounded batch input count");
        let mut ids = List::with_capacity(u32::try_from(capacity).expect("bounded input IDs"));
        for member in &batch {
            if member.spec.inputs.len() > usize::try_from(env.limits.tasks.inputs).expect("u32 fits usize") {
                decide_call(
                    domain,
                    &env.limits,
                    decision,
                    to,
                    key,
                    CallAnswer::DelegationRefused(tasks::Problem {
                        task: None,
                        why: tasks::Refusal::Inputs,
                        blocked_by: None,
                    }),
                );
                return;
            }
            for &input in &member.spec.inputs {
                let mut known = false;
                for &id in &ids {
                    if id == input {
                        known = true;
                    }
                }
                if !known {
                    ids.push(input).expect("bounded input ID count");
                }
            }
        }
        if !ids.is_empty() {
            let first = *ids.get(0).expect("nonempty input IDs");
            let to = to.into_token();
            let read = InputCheck {
                to,
                key,
                batch,
                ids: ids.into_boxed(),
                at: 0,
                project: context.project,
                stubs: List::with_capacity(u32::try_from(capacity).expect("bounded input IDs")),
            };
            let waiter =
                domain.result_reads.insert(Some(Read::InputCheck(read))).expect("preflighted input read slot").token();
            assert!(domain.core.pending_calls.insert(key, true).is_ok(), "reserved call record room");
            emit(
                decision,
                &env.limits,
                Delivery::Load { waiter, range: Range::TaskResult { task: first }, after: None },
            );
            return;
        }
    }
    let mut numbers = List::with_capacity(env.limits.tasks.batch);
    for _ in &batch {
        let Some(number) = crate::fresh(&mut domain.core.counters, Family::Task) else {
            decide_call(
                domain,
                &env.limits,
                decision,
                to,
                key,
                CallAnswer::DelegationRefused(tasks::Problem {
                    task: None,
                    why: tasks::Refusal::Live,
                    blocked_by: None,
                }),
            );
            return;
        };
        numbers.push(number).expect("bounded delegation IDs");
    }
    let mut created = List::with_capacity(env.limits.tasks.batch);
    for (index, member) in batch.into_iter().enumerate() {
        let index = u32::try_from(index).expect("batch length fits u32");
        let mut dependencies = List::with_capacity(env.limits.tasks.dependencies);
        for dependency in member.dependencies {
            let number = match dependency {
                Dependency::Batch(at) => match numbers.get(at) {
                    Some(number) => *number,
                    None => {
                        decide_call(
                            domain,
                            &env.limits,
                            decision,
                            to,
                            key,
                            CallAnswer::DelegationRefused(tasks::Problem {
                                task: numbers.get(index).copied(),
                                why: tasks::Refusal::Dependencies,
                                blocked_by: None,
                            }),
                        );
                        return;
                    }
                },
                Dependency::Existing(number) => number,
            };
            if dependencies.push(number).is_err() {
                decide_call(
                    domain,
                    &env.limits,
                    decision,
                    to,
                    key,
                    CallAnswer::DelegationRefused(tasks::Problem {
                        task: numbers.get(index).copied(),
                        why: tasks::Refusal::Dependencies,
                        blocked_by: None,
                    }),
                );
                return;
            }
        }
        let number = *numbers.get(index).expect("one ID per member");
        let Some(authority) =
            resolved_delegate_authority(&member.authority, &member.symbolic_grants, number, &env.limits)
        else {
            decide_call(
                domain,
                &env.limits,
                decision,
                to,
                key,
                CallAnswer::DelegationRefused(tasks::Problem {
                    task: Some(number),
                    why: tasks::Refusal::AuthorityShape,
                    blocked_by: None,
                }),
            );
            return;
        };
        let root = domain.core.tasks.root(key.task).expect("delegator live");
        let Some(holdings) = forge_route::task_holdings(
            domain,
            env,
            context.project,
            root,
            number,
            member.executor,
            &member.spec,
            Some(key.task),
        ) else {
            decide_call(
                domain,
                &env.limits,
                decision,
                to,
                key,
                CallAnswer::DelegationRefused(tasks::Problem {
                    task: Some(number),
                    why: tasks::Refusal::Holds,
                    blocked_by: None,
                }),
            );
            return;
        };
        created
            .push(tasks::New {
                number,
                project: context.project,
                executor: member.executor,
                spec: member.spec,
                contract: member.contract,
                authority,
                numbers: tasks::Numbers {
                    budget: member.authority.budget.spend,
                    spent: 0,
                    spent_below: 0,
                    reserved: 0,
                },
                funder: tasks::Funder::Task(key.task),
                dependencies: dependencies.into_boxed(),
                holdings,
                wake: member.wake,
                recurring: None,
                tracked: None,
            })
            .expect("bounded delegation batch");
    }
    let token = to.into_token();
    if !validated {
        assert!(domain.core.pending_calls.insert(key, true).is_ok(), "reserved call record room");
    }
    assert!(domain.core.delegating.insert(token, (key, stubs)) == Ok(None), "one pending delegated call");
    domain.work.push(Work::Tasks(tasks::Event::Make {
        reply_to: ReplyTo::new(token),
        creator: tasks::Party::Task(key.task),
        batch: created.into_boxed(),
    }));
}

/// Route a connector-owned step through current task authority and the tasks hub in one root
/// decision. Invalid or stale owner inputs make no change; the owner retries from current facts.
#[expect(clippy::too_many_lines, reason = "procedure admission checks and routing form one bounded decision")]
fn procedure_step(
    domain: &mut Domain,
    env: &Env<Limits>,
    task: u64,
    step: u64,
    connector: u16,
    code: u32,
    action: ProcedureAction,
) -> Option<Box<[u64]>> {
    if domain.core.tasks.procedure_due(task) != Some((connector, code, step)) {
        return None;
    }
    let mut delegated = None;
    let decision = match action {
        ProcedureAction::Delegate(batch) => {
            let context = domain.core.tasks.delegation(task)?;
            if batch.is_empty() || batch.len() > usize::try_from(env.limits.tasks.batch).expect("bounded batch") {
                return None;
            }
            let mut asked = List::with_capacity(env.limits.tasks.batch);
            for member in &batch {
                if !member.spec.inputs.is_empty() {
                    return None;
                }
                let executor = delegation_executor(domain, context.project, member.executor)?;
                let symbolic = symbolic_grants(&member.symbolic_grants, env.limits.authority.grants)?;
                asked
                    .push(authority::Delegate { executor, authority: authority_value(&member.authority), symbolic })
                    .expect("bounded procedure batch");
            }
            let mut findings = Queue::with_capacity(
                authority::max_out(domain.core.authority.limits()).expect("authority finding bound"),
            );
            let checked = authority::check_batch(
                &domain.core.authority,
                &authority::BatchAsk {
                    project: context.project,
                    creator: authority_value(&context.authority),
                    numbers: authority_numbers(context.numbers),
                    tasks_left: context.tasks_left,
                    tasks: asked.into_boxed(),
                },
                &mut findings,
            );
            if checked.answer != authority::Answer::Allow {
                return None;
            }
            let mut numbers = List::with_capacity(env.limits.tasks.batch);
            for _ in &batch {
                let number = crate::fresh(&mut domain.core.counters, Family::Task)?;
                numbers.push(number).expect("bounded procedure task IDs");
            }
            let mut created = List::with_capacity(env.limits.tasks.batch);
            for (index, member) in batch.into_iter().enumerate() {
                let at = u32::try_from(index).expect("bounded batch index");
                let mut dependencies = List::with_capacity(env.limits.tasks.dependencies);
                for dependency in member.dependencies {
                    let number = match dependency {
                        Dependency::Batch(index) => match numbers.get(index) {
                            Some(number) => *number,
                            None => return None,
                        },
                        Dependency::Existing(number) => number,
                    };
                    if dependencies.push(number).is_err() {
                        return None;
                    }
                }
                let budget = member.authority.budget.spend;
                let number = *numbers.get(at).expect("one ID per member");
                let authority =
                    resolved_delegate_authority(&member.authority, &member.symbolic_grants, number, &env.limits)?;
                let root = domain.core.tasks.root(task)?;
                let holdings = forge_route::task_holdings(
                    domain,
                    env,
                    context.project,
                    root,
                    number,
                    member.executor,
                    &member.spec,
                    Some(task),
                )?;
                created
                    .push(tasks::New {
                        number,
                        project: context.project,
                        executor: member.executor,
                        spec: member.spec,
                        contract: member.contract,
                        authority,
                        numbers: tasks::Numbers { budget, spent: 0, spent_below: 0, reserved: 0 },
                        funder: tasks::Funder::Task(task),
                        dependencies: dependencies.into_boxed(),
                        holdings,
                        wake: member.wake,
                        recurring: None,
                        tracked: None,
                    })
                    .expect("bounded procedure batch");
            }
            delegated = Some(numbers.into_boxed());
            tasks::ProcedureDecision::Delegate(created.into_boxed())
        }
        ProcedureAction::Result(result) => tasks::ProcedureDecision::Result(result),
        ProcedureAction::Hold(why) => tasks::ProcedureDecision::Hold(why),
        ProcedureAction::Wait => tasks::ProcedureDecision::Wait,
    };
    domain.work.push(Work::Tasks(tasks::Event::Procedure { reply_to: internal(u64::MAX), task, step, decision }));
    delegated
}

/// Admit a configured core procedure through the same durable task creation route as other roots.
fn start_recurring(
    domain: &mut Domain,
    _env: &Env<Limits>,
    project: u32,
    authority: tasks::Authority,
    template: tasks::RecurringTemplate,
) {
    if !domain.core.recurring_admit(project, &authority, &template) {
        return;
    }
    let period = domain.core.settings.period;
    if domain.core.tasks.funding(tasks::Funder::Period { project, period }).is_none() {
        domain.work.push(Work::Tasks(tasks::Event::OpenPeriod {
            reply_to: internal(u64::MAX - 2),
            project,
            period,
            budget: domain.core.settings.period_budget,
        }));
    }
    let Some(number) = crate::fresh(&mut domain.core.counters, Family::Task) else { return };
    domain.work.push(Work::Tasks(tasks::Event::Make {
        reply_to: internal(u64::MAX - 2),
        creator: tasks::Party::Deployment { project },
        batch: Box::new([tasks::New {
            number,
            project,
            executor: tasks::Executor::Procedure { connector: domain.core.settings.recurring_connector, code: 1 },
            spec: tasks::Spec { words: b"recurring".as_slice().into(), parameters: Box::new([]), inputs: Box::new([]) },
            contract: tasks::Contract::Report { words: 0 },
            numbers: tasks::Numbers { budget: 0, spent: 0, spent_below: 0, reserved: 0 },
            authority,
            funder: tasks::Funder::Period { project, period },
            dependencies: Box::new([]),
            holdings: Box::new([]),
            wake: tasks::WakePolicy::DEFAULT,
            recurring: Some(Box::new(template)),
            tracked: None,
        }]),
    }));
    domain.work.push(Work::Tasks(tasks::Event::TickRecurring { task: number, period }));
}

#[expect(clippy::too_many_lines, reason = "the closed child vocabulary is routed exhaustively inside one decision")]
fn tasks_outputs(
    domain: &mut Domain,
    env: &Env<Limits>,
    decision: &mut Decision,
    out: &mut Queue<tasks::Request>,
    person_proposal: bool,
    task_escalation: bool,
) {
    for _ in 0..out.len() {
        match out.pop().expect("tasks output count") {
            tasks::Request::PersonProposed { reply_to, proposal } => {
                let request = reply_to.into_token();
                domain.work.push(Work::People(domain.core.person_proposed(request, proposal)));
            }
            tasks::Request::PersonProposalDecided { reply_to, proposer, number, outcome } => {
                let request = reply_to.into_token();
                domain.work.push(Work::People(domain.core.person_proposal_decided(request, proposer, number, outcome)));
            }
            tasks::Request::RecurringDue { task, period, members } => {
                let Some(context) = domain.core.tasks.delegation(task) else { continue };
                let Some(template) = domain.core.tasks.recurring_template(task) else { continue };
                if members != u32::try_from(template.batch.len()).expect("bounded template") {
                    continue;
                }
                let mut asked = List::with_capacity(env.limits.tasks.batch);
                for member in &template.batch {
                    let executor = match member.executor {
                        tasks::Executor::Agent { charter } => authority::Executor::Charter(charter),
                        tasks::Executor::Procedure { code, .. } => authority::Executor::Procedure(code),
                        tasks::Executor::Person(_) => unreachable!("person template refused at root admission"),
                    };
                    asked
                        .push(authority::Delegate {
                            executor,
                            authority: authority_value(&member.authority),
                            symbolic: Box::new([]),
                        })
                        .expect("bounded template");
                }
                let checked = authority::check_batch(
                    &domain.core.authority,
                    &authority::BatchAsk {
                        project: context.project,
                        creator: authority_value(&context.authority),
                        numbers: authority::Numbers {
                            budget: context.authority.budget.spend,
                            spent: 0,
                            spent_below: 0,
                            reserved: 0,
                        },
                        tasks_left: context.tasks_left,
                        tasks: asked.into_boxed(),
                    },
                    &mut Queue::with_capacity(
                        authority::max_out(domain.core.authority.limits()).expect("bounded authority findings"),
                    ),
                );
                if checked.answer != authority::Answer::Allow {
                    continue;
                }
                let mut numbers = List::with_capacity(env.limits.tasks.batch);
                for _ in 0..members {
                    let Some(number) = crate::fresh(&mut domain.core.counters, Family::Task) else { break };
                    numbers.push(number).expect("bounded recurring batch");
                }
                if numbers.len() == members {
                    domain.work.push(Work::Tasks(tasks::Event::RecurringBatch {
                        task,
                        period,
                        numbers: numbers.into_boxed(),
                    }));
                }
            }
            tasks::Request::EscalationStalled { task, revision, holder } => {
                escalation::stalled(domain, task, revision, holder);
            }
            tasks::Request::Notify { task, subscription, target, state, words } => {
                domain.work.push(Work::Tasks(domain.core.notice(task, subscription, target, state, words, env.wall)));
            }
            tasks::Request::Timer { task, subscription } => {
                domain.work.push(Work::Tasks(domain.core.notice_timer(task, subscription, env.wall)));
            }
            tasks::Request::EndTopic { task, subscription, connector } => {
                if connector == domain.config.forge_connector
                    && let Some(topic) = domain.forge.subscription(task, subscription)
                {
                    domain.work.push(Work::Forge(forge::Event::Unsubscribe { task, topic }));
                }
            }
            tasks::Request::Sent { reply_to, task, word } => {
                let request = reply_to.into_token();
                match domain.core.sent(request, task, &word, env.limits.tasks.inbox_messages, env.limits.tasks.tasks) {
                    jig_core::SentRoute::Call { key, message } => decide_call(
                        domain,
                        &env.limits,
                        decision,
                        ReplyTo::new(request),
                        key,
                        CallAnswer::Sent { message },
                    ),
                    jig_core::SentRoute::Person(event) => domain.work.push(Work::People(*event)),
                }
            }
            tasks::Request::Relay { .. } => unreachable!("the core marks committed relays held"),
            tasks::Request::EscalationsInspected { .. } | tasks::Request::EscalationsRechecked { .. } => {
                unreachable!("serialized roles route consumes project terminals")
            }
            tasks::Request::EscalationNeeded { context } => escalation::needed(domain, context),
            tasks::Request::EscalationInspected { reply_to, context } => {
                escalation::inspected(domain, env, decision, reply_to.into_token(), context);
            }
            tasks::Request::EscalationDecided { reply_to, task, revision, outcome } => {
                if task_escalation {
                    let token = reply_to.into_token();
                    let route = domain.core.routing_calls.remove(&token).expect("task escalation decision route");
                    let (key, named, current) = match route {
                        RoutedCall::Escalation { key, task, revision } => (key, task, revision),
                        RoutedCall::Propose { .. }
                        | RoutedCall::Decide { .. }
                        | RoutedCall::Withdraw { .. }
                        | RoutedCall::Accepting { .. }
                        | RoutedCall::Message(_)
                        | RoutedCall::Introduce(_)
                        | RoutedCall::Subscribe { .. }
                        | RoutedCall::Unsubscribe(_)
                        | RoutedCall::Control(_) => unreachable!("task escalation route kind"),
                    };
                    assert!(task == named && revision == current, "exact task escalation terminal");
                    decide_call(
                        domain,
                        &env.limits,
                        decision,
                        ReplyTo::new(token),
                        key,
                        CallAnswer::EscalationDecided { task, revision, outcome },
                    );
                } else {
                    escalation::completed(domain, env, decision, reply_to.into_token(), task, revision, outcome);
                }
            }
            tasks::Request::ProposalDecided { reply_to, proposer, number, outcome } => {
                let token = reply_to.into_token();
                if let Some(event) =
                    domain.core.task_proposal_decided_for_person(token, proposer, number, outcome, person_proposal)
                {
                    domain.work.push(Work::People(event));
                    continue;
                }
                let route = domain.core.routing_calls.remove(&token).expect("pending proposal decision route");
                let key = match route {
                    RoutedCall::Decide { key, proposal } | RoutedCall::Withdraw { key, proposal }
                        if proposal == number =>
                    {
                        key
                    }
                    RoutedCall::Propose { .. }
                    | RoutedCall::Accepting { .. }
                    | RoutedCall::Decide { .. }
                    | RoutedCall::Escalation { .. }
                    | RoutedCall::Withdraw { .. }
                    | RoutedCall::Message(_)
                    | RoutedCall::Introduce(_)
                    | RoutedCall::Subscribe { .. }
                    | RoutedCall::Unsubscribe(_)
                    | RoutedCall::Control(_) => unreachable!("matching proposal decision"),
                };
                assert!(key.task == proposer || outcome != tasks::ProposalOutcome::Withdrawn, "withdrawal by proposer");
                decide_call(
                    domain,
                    &env.limits,
                    decision,
                    ReplyTo::new(token),
                    key,
                    CallAnswer::ProposalDecided { proposal: number, outcome },
                );
            }
            tasks::Request::ProposalStalled { proposer, proposal, holder } => {
                if let Some(pending) = domain.core.tasks.proposal(proposer, proposal) {
                    match pending.state {
                        tasks::ProposalState::Pending { holder: current, .. } if current == holder => {
                            if let Some(next) = proposals::holder(domain, proposer, &pending.action, Some(holder)) {
                                let revision =
                                    domain.core.tasks.task(proposer).expect("pending proposer remains live").revision;
                                domain.work.push(Work::Tasks(tasks::Event::StalledProposal {
                                    proposer,
                                    proposal,
                                    from: holder,
                                    revision,
                                    holder: next,
                                }));
                            }
                        }
                        tasks::ProposalState::Pending { .. }
                        | tasks::ProposalState::Accepted { .. }
                        | tasks::ProposalState::Rejected { .. }
                        | tasks::ProposalState::Withdrawn => {}
                    }
                }
            }
            tasks::Request::ProposalRerouteNeeded { proposer, proposal } => {
                if let Some(pending) = domain.core.tasks.proposal(proposer, proposal) {
                    match pending.state {
                        tasks::ProposalState::Pending { holder: current, .. } => {
                            if let Some(next) = proposals::holder(domain, proposer, &pending.action, None)
                                && current != next
                            {
                                let revision =
                                    domain.core.tasks.task(proposer).expect("pending proposer remains live").revision;
                                domain.work.push(Work::Tasks(tasks::Event::StalledProposal {
                                    proposer,
                                    proposal,
                                    from: current,
                                    revision,
                                    holder: next,
                                }));
                            }
                        }
                        tasks::ProposalState::Accepted { .. }
                        | tasks::ProposalState::Rejected { .. }
                        | tasks::ProposalState::Withdrawn => {}
                    }
                }
            }
            tasks::Request::Save { record } => {
                if let Some(archive) = proposals::decision_record(&record) {
                    save(decision, &env.limits, Write::Save(Record::ProposalDecision(archive)));
                }
                match &record {
                    tasks::Stored::Live(task) | tasks::Stored::Ended(task) => {
                        let stale = match domain.core.contexts.get(&task.number) {
                            Some(context) => {
                                task.phase != tasks::Phase::Active(tasks::Active::Preparing)
                                    || task.last_message != context.last_message
                            }
                            None => false,
                        };
                        if stale {
                            drop(domain.core.contexts.remove(&task.number));
                            drop(domain.core.dependency_results.remove(&task.number));
                            drop(domain.core.transcripts.remove(&task.number));
                            domain
                                .work
                                .push(Work::Brief(brief::GatherEvent::Abandon { brief: Token::new(task.number) }));
                            if task.phase == tasks::Phase::Active(tasks::Active::Preparing) {
                                domain.work.push(Work::Tasks(tasks::Event::PreparationFailed { task: task.number }));
                            }
                        }
                        view_task_saved(domain, &env.limits, decision, task);
                        if task.tracked.is_some() && domain.forge.home(task.project).is_some() {
                            domain.work.push(Work::ProjectGoal(task.clone()));
                        }
                        domain.work.push(Work::People(people::Event::Waiting {
                            task: task.number,
                            entries: inbox::entries(domain, task),
                        }));
                    }
                    tasks::Stored::PersonProposal(row) => {
                        domain.work.push(Work::People(people::Event::Waiting {
                            task: row.goal.number,
                            entries: inbox::person_proposal_entries(domain, row),
                        }));
                    }
                    tasks::Stored::Ledger(_)
                    | tasks::Stored::Writer(_)
                    | tasks::Stored::Pool(_)
                    | tasks::Stored::History(_)
                    | tasks::Stored::Stub(_) => {}
                }
                let record = match record {
                    tasks::Stored::Ended(mut task) => {
                        let position = crate::fresh(&mut domain.core.counters, Family::Message)
                            .expect("ending position preflighted before mutation");
                        task.result_position = position;
                        assert!(
                            domain.core.ending_positions.insert(task.number, position) == Ok(None),
                            "one ending position per ended task"
                        );
                        match task.requester {
                            tasks::Party::Person(person) => {
                                domain.core.people.remember_result(
                                    &env.limits.people,
                                    person,
                                    people::ResultRef { task: task.number, position },
                                );
                            }
                            tasks::Party::Task(_) | tasks::Party::Deployment { .. } => {}
                        }
                        tasks::Stored::Ended(task)
                    }
                    tasks::Stored::Live(_)
                    | tasks::Stored::Writer(_)
                    | tasks::Stored::Pool(_)
                    | tasks::Stored::Ledger(_)
                    | tasks::Stored::History(_)
                    | tasks::Stored::Stub(_)
                    | tasks::Stored::PersonProposal(_) => record,
                };
                save(decision, &env.limits, Write::Save(Record::Tasks(record)));
            }
            tasks::Request::Erase { key } => save(decision, &env.limits, Write::Erase(Key::Tasks(key))),
            tasks::Request::Made { reply_to, tasks } => {
                let request = reply_to.into_token();
                match domain.core.made(request, tasks, person_proposal) {
                    jig_core::MadeRoute::Internal => {}
                    jig_core::MadeRoute::Tasks(event) => domain.work.push(Work::Tasks(event)),
                    jig_core::MadeRoute::PersonProposal(event) => domain.work.push(Work::PersonProposal(event)),
                    jig_core::MadeRoute::Person(event) => domain.work.push(Work::People(*event)),
                    jig_core::MadeRoute::Delegated { key, tasks, stubs } => {
                        for stub in stubs {
                            domain.work.push(Work::Tasks(tasks::Event::RememberStub { stub }));
                        }
                        decide_call(
                            domain,
                            &env.limits,
                            decision,
                            ReplyTo::new(request),
                            key,
                            CallAnswer::Delegated(tasks),
                        );
                    }
                }
            }
            tasks::Request::Refused { reply_to, problem } => {
                let token = reply_to.into_token();
                if token.raw() == u64::MAX - 3 {
                    if let Some(repair) = problem.task
                        && let Some(owner) = domain.forge.queue_repair_owner(repair)
                    {
                        domain.work.push(Work::Tasks(tasks::Event::Hold { task: owner, why: tasks::Hold::Effects }));
                    }
                    continue;
                }
                if token.raw() == u64::MAX
                    && let Some(task) = problem.task
                    && let Some(row) = domain.forge.change(task)
                    && let Some((child, _)) = row.delegate
                {
                    domain.work.push(Work::Forge(forge::Event::DelegateRefused { task, child }));
                    domain.work.push(Work::Tasks(tasks::Event::Hold { task, why: tasks::Hold::Procedure }));
                    continue;
                }
                if let Some(event) = domain.core.goal_refused(token, problem.why) {
                    domain.work.push(Work::People(event));
                    continue;
                }
                if domain.core.person_tasks.remove(&token).is_some() {
                    let why = match problem.why {
                        tasks::Refusal::Busy | tasks::Refusal::NotReady => people::Refusal::Busy,
                        tasks::Refusal::Unknown => people::Refusal::Ended,
                        tasks::Refusal::State | tasks::Refusal::Executor | tasks::Refusal::Reference => {
                            people::Refusal::Standing
                        }
                        tasks::Refusal::Funding | tasks::Refusal::AuthorityShape => people::Refusal::Authority,
                        tasks::Refusal::Duplicate
                        | tasks::Refusal::Empty
                        | tasks::Refusal::Batch
                        | tasks::Refusal::Live
                        | tasks::Refusal::Project
                        | tasks::Refusal::Tree
                        | tasks::Refusal::Depth
                        | tasks::Refusal::Delegates
                        | tasks::Refusal::Subscription
                        | tasks::Refusal::Dependencies
                        | tasks::Refusal::Cycle
                        | tasks::Refusal::Spec
                        | tasks::Refusal::Contract
                        | tasks::Refusal::Inputs
                        | tasks::Refusal::Attempt
                        | tasks::Refusal::LiveDelegates
                        | tasks::Refusal::Restore
                        | tasks::Refusal::Read
                        | tasks::Refusal::Turn
                        | tasks::Refusal::HoldKind
                        | tasks::Refusal::HoldTaken
                        | tasks::Refusal::Holds => people::Refusal::Limit,
                    };
                    domain.work.push(Work::People(people::Event::Decided {
                        request: token,
                        outcome: people::Outcome::Refused(why),
                    }));
                    continue;
                }
                if domain.core.moving.remove(&token).is_some() {
                    domain.work.push(Work::People(people::Event::Decided {
                        request: token,
                        outcome: people::Outcome::Refused(match problem.why {
                            tasks::Refusal::Busy | tasks::Refusal::NotReady => people::Refusal::Busy,
                            tasks::Refusal::Unknown => people::Refusal::Ended,
                            tasks::Refusal::Reference | tasks::Refusal::State => people::Refusal::Standing,
                            tasks::Refusal::Funding => people::Refusal::Authority,
                            tasks::Refusal::Duplicate
                            | tasks::Refusal::Empty
                            | tasks::Refusal::Batch
                            | tasks::Refusal::Live
                            | tasks::Refusal::Project
                            | tasks::Refusal::Tree
                            | tasks::Refusal::Depth
                            | tasks::Refusal::Delegates
                            | tasks::Refusal::Subscription
                            | tasks::Refusal::Dependencies
                            | tasks::Refusal::Cycle
                            | tasks::Refusal::Executor
                            | tasks::Refusal::Spec
                            | tasks::Refusal::Contract
                            | tasks::Refusal::AuthorityShape
                            | tasks::Refusal::Inputs
                            | tasks::Refusal::Attempt
                            | tasks::Refusal::LiveDelegates
                            | tasks::Refusal::Restore
                            | tasks::Refusal::Read
                            | tasks::Refusal::Turn
                            | tasks::Refusal::HoldKind
                            | tasks::Refusal::HoldTaken
                            | tasks::Refusal::Holds => people::Refusal::Limit,
                        }),
                    }));
                    continue;
                }
                let person_route =
                    if person_proposal { domain.core.routing_people_proposals.remove(&token) } else { None };
                if let Some(route) = person_route {
                    let request = match route {
                        PersonProposalRoute::Deciding { request, .. }
                        | PersonProposalRoute::Accepting { request, .. } => request,
                    };
                    domain.work.push(Work::People(people::Event::Decided {
                        request,
                        outcome: people::Outcome::Refused(match problem.why {
                            tasks::Refusal::Busy | tasks::Refusal::NotReady => people::Refusal::Busy,
                            tasks::Refusal::Reference | tasks::Refusal::State => people::Refusal::Standing,
                            tasks::Refusal::Unknown => people::Refusal::Ended,
                            tasks::Refusal::Funding => people::Refusal::Authority,
                            tasks::Refusal::Duplicate
                            | tasks::Refusal::Empty
                            | tasks::Refusal::Batch
                            | tasks::Refusal::Live
                            | tasks::Refusal::Project
                            | tasks::Refusal::Tree
                            | tasks::Refusal::Depth
                            | tasks::Refusal::Delegates
                            | tasks::Refusal::Subscription
                            | tasks::Refusal::Dependencies
                            | tasks::Refusal::Cycle
                            | tasks::Refusal::Executor
                            | tasks::Refusal::Spec
                            | tasks::Refusal::Contract
                            | tasks::Refusal::AuthorityShape
                            | tasks::Refusal::Inputs
                            | tasks::Refusal::Attempt
                            | tasks::Refusal::LiveDelegates
                            | tasks::Refusal::Restore
                            | tasks::Refusal::Read
                            | tasks::Refusal::Turn
                            | tasks::Refusal::HoldKind
                            | tasks::Refusal::HoldTaken
                            | tasks::Refusal::Holds => people::Refusal::Limit,
                        }),
                    }));
                    continue;
                }
                if let Some(route) = domain.core.routing_calls.remove(&token) {
                    drop(domain.forge_subscribing.remove(&token));
                    drop(domain.forge_unsubscribing.remove(&token));
                    let (key, answer) = match route {
                        RoutedCall::Message(key) | RoutedCall::Introduce(key) => {
                            (key, CallAnswer::MessageRefused(problem))
                        }
                        RoutedCall::Subscribe { key, .. } | RoutedCall::Unsubscribe(key) => {
                            (key, CallAnswer::SubscriptionRefused(problem))
                        }
                        RoutedCall::Control(key) => (key, CallAnswer::ControlRefused(problem)),
                        RoutedCall::Escalation { key, .. } => (key, CallAnswer::EscalationRefused(problem)),
                        RoutedCall::Propose { key, .. }
                        | RoutedCall::Decide { key, .. }
                        | RoutedCall::Withdraw { key, .. }
                        | RoutedCall::Accepting { key, .. } => (key, CallAnswer::ProposalRefused(problem)),
                    };
                    decide_call(domain, &env.limits, decision, ReplyTo::new(token), key, answer);
                } else if let Some((key, _)) = domain.core.delegating.remove(&token) {
                    decide_call(
                        domain,
                        &env.limits,
                        decision,
                        ReplyTo::new(token),
                        key,
                        CallAnswer::DelegationRefused(problem),
                    );
                } else if domain.core.made.remove(&token).is_some() {
                    domain.work.push(Work::People(people::Event::Decided {
                        request: token,
                        outcome: people::Outcome::Refused(people::Refusal::Limit),
                    }));
                } else if domain.core.saying.remove(&token).is_some() {
                    domain.work.push(Work::People(people::Event::Decided {
                        request: token,
                        outcome: people::Outcome::Refused(match problem.why {
                            tasks::Refusal::Busy | tasks::Refusal::NotReady => people::Refusal::Busy,
                            tasks::Refusal::Unknown => people::Refusal::Unknown,
                            tasks::Refusal::State => people::Refusal::Standing,
                            tasks::Refusal::Duplicate
                            | tasks::Refusal::Empty
                            | tasks::Refusal::Batch
                            | tasks::Refusal::Live
                            | tasks::Refusal::Project
                            | tasks::Refusal::Tree
                            | tasks::Refusal::Depth
                            | tasks::Refusal::Delegates
                            | tasks::Refusal::Reference
                            | tasks::Refusal::Subscription
                            | tasks::Refusal::Dependencies
                            | tasks::Refusal::Cycle
                            | tasks::Refusal::Executor
                            | tasks::Refusal::Spec
                            | tasks::Refusal::Contract
                            | tasks::Refusal::AuthorityShape
                            | tasks::Refusal::Inputs
                            | tasks::Refusal::Attempt
                            | tasks::Refusal::LiveDelegates
                            | tasks::Refusal::Restore
                            | tasks::Refusal::Read
                            | tasks::Refusal::Turn
                            | tasks::Refusal::Funding
                            | tasks::Refusal::HoldKind
                            | tasks::Refusal::HoldTaken
                            | tasks::Refusal::Holds => people::Refusal::Limit,
                        }),
                    }));
                } else if domain.core.claiming.remove(&token.raw()).is_some() {
                    drop(domain.assignments.remove(&token.raw()));
                    drop(domain.core.proofs.remove(&token.raw()));
                    domain.work.push(Work::Tasks(tasks::Event::PreparationFailed { task: token.raw() }));
                } else if let Some(payload) = take_payload(domain, token) {
                    match payload {
                        Payload::Turn { task, attempt, body: turn } => {
                            if problem.task == Some(task) {
                                domain.work.push(Work::Fleet(fleet::Event::TurnBusy {
                                    run: Token::new(task),
                                    attempt: Token::new(attempt),
                                    turn: turn.number,
                                }));
                            }
                        }
                        Payload::Answer { task, attempt, .. } => {
                            if problem.task == Some(task) {
                                let proof = domain.core.proofs.get_mut(&task).expect("answer proof reserved");
                                proof.terminal = Some(TerminalRecord {
                                    task,
                                    attempt,
                                    cumulative: match proof.turn {
                                        Some(turn) => turn.cumulative,
                                        None => 0,
                                    },
                                    end: tasks::End::Failed(tasks::Class::Invalid),
                                });
                                domain.work.push(Work::Tasks(tasks::Event::Activation {
                                    reply_to: ReplyTo::new(token),
                                    task,
                                    attempt,
                                    end: tasks::End::Failed(tasks::Class::Invalid),
                                    saved: None,
                                    cause: tasks::Cause::Unpriced,
                                }));
                            }
                        }
                        Payload::Call { .. } | Payload::CallAnswer(_) => {
                            unreachable!("task refusal owns a task payload")
                        }
                    }
                }
            }
            tasks::Request::TurnAcknowledged { reply_to, task, attempt, turn, accepted } => {
                let token = reply_to.into_token();
                let payload = take_payload(domain, token).expect("charged turn owns payload");
                let body = match payload {
                    Payload::Turn { body, .. } => body,
                    Payload::Answer { .. } | Payload::Call { .. } | Payload::CallAnswer(_) => {
                        unreachable!("turn family")
                    }
                };
                let proof = domain.core.proofs.get_mut(&task).expect("turn proof reserved before child mutation");
                assert!(proof.attempt == attempt, "turn callback retains its actual claim");
                proof.turn = Some(TurnProof { turn, cumulative: body.cumulative, read: body.read });
                save(decision, &env.limits, Write::Save(Record::RunProof(proof.clone())));
                match accepted {
                    tasks::Accepted::New => save(
                        decision,
                        &env.limits,
                        Write::Save(Record::Turn(TurnRecord {
                            task,
                            attempt,
                            turn,
                            spent: body.cumulative,
                            read: body.read,
                            at: env.wall,
                            transcript: body.transcript,
                        })),
                    ),
                    tasks::Accepted::Already => {}
                }
                if accepted == tasks::Accepted::New {
                    retire_calls(domain, &env.limits, decision, task, attempt, turn);
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
            tasks::Request::Acknowledged { reply_to, task, attempt, .. } => {
                let token = reply_to.into_token();
                if token.raw() != u64::MAX {
                    drop(take_payload(domain, token));
                }
                if let Some(proof) = domain.core.proofs.get(&task) {
                    if let Some(terminal) = &proof.terminal {
                        save(decision, &env.limits, Write::Save(Record::Terminal(terminal.clone())));
                    }
                    save(decision, &env.limits, Write::Save(Record::RunProof(proof.clone())));
                }
                emit(
                    decision,
                    &env.limits,
                    Delivery::Fleet(fleet::Event::Acknowledge { run: Token::new(task), attempt: Token::new(attempt) }),
                );
                domain.work.push(Work::Forge(forge::Event::Lost { task, attempt }));
            }
            tasks::Request::Activate { context } => domain.work.push(Work::Activate(context)),
            tasks::Request::Adopt { task, attempt, kept } => {
                domain.core.adopt(&environment_core(env), task, attempt, kept);
            }
            tasks::Request::Stop { task, attempt } => {
                emit(decision, &env.limits, Delivery::Fleet(Core::stop_run(task, attempt)));
            }
            tasks::Request::Close { task, ending } => {
                let root = domain.core.tasks.root(task).unwrap_or(task);
                let ending = match ending {
                    tasks::Ending::Done(_) => forge::ReleaseEnding::Done,
                    tasks::Ending::Failed { .. } => forge::ReleaseEnding::Failed,
                    tasks::Ending::Cancelled { .. } => forge::ReleaseEnding::Cancelled,
                };
                domain.work.push(Work::Forge(forge::Event::SettleEffects { task, root, ending }));
            }
            tasks::Request::Release { task, ending } => {
                let root = domain.core.tasks.root(task).unwrap_or(task);
                let ending = match ending {
                    tasks::Ending::Done(_) => forge::ReleaseEnding::Done,
                    tasks::Ending::Failed { .. } => forge::ReleaseEnding::Failed,
                    tasks::Ending::Cancelled { .. } => forge::ReleaseEnding::Cancelled,
                };
                if let Some(entry) = crate::fresh(&mut domain.core.counters, Family::ConnectorRow) {
                    domain.work.push(Work::Forge(forge::Event::Release { task, root, ending, entry }));
                } else {
                    domain.work.push(Work::Tasks(tasks::Event::Hold { task, why: tasks::Hold::Budget }));
                }
            }
            tasks::Request::Ended { task, requester, ending } => {
                let position =
                    domain.core.ending_positions.remove(&task).expect("ended row assigned its result position");
                emit(
                    decision,
                    &env.limits,
                    Delivery::View(Box::new(views::Event::Finished { task: Token::new(task) })),
                );
                retire_calls(domain, &env.limits, decision, task, u64::MAX, u32::MAX);
                drop(domain.core.proofs.remove(&task));
                save(decision, &env.limits, Write::Erase(Key::RunProof { task }));
                match requester {
                    tasks::Party::Person(person) => {
                        emit(decision, &env.limits, Delivery::Result { person, task, words: ending_words(ending) });
                    }
                    tasks::Party::Task(parent) => {
                        let (kind, words) = result_notice(ending);
                        let word = tasks::Word {
                            number: position,
                            from: tasks::Party::Task(task),
                            kind: tasks::MessageKind::Result(kind),
                            words,
                            at: env.wall,
                            hits: 1,
                            eligible: false,
                        };
                        domain.work.push(Work::Tasks(tasks::Event::DelegateResult { task: parent, word }));
                    }
                    tasks::Party::Deployment { .. } => {}
                }
            }
            tasks::Request::Done { reply_to } => {
                let task = reply_to.into_token().raw();
                if let Some(route) = domain.core.person_tasks.remove(&Token::new(task)) {
                    let outcome = match route {
                        PersonTaskRoute::Take(task) => people::Outcome::PersonTaken { task },
                        PersonTaskRoute::PoolSet { project, person } => people::Outcome::PoolSet { project, person },
                        PersonTaskRoute::HandBack(task) => people::Outcome::PersonHandedBack { task },
                        PersonTaskRoute::Answer(task) => people::Outcome::PersonAnswered { task },
                        PersonTaskRoute::Cancel(task) => people::Outcome::Cancelled { task },
                        PersonTaskRoute::Release(task) => people::Outcome::Released { task },
                        PersonTaskRoute::Prioritised(project) => people::Outcome::Prioritised { project },
                        PersonTaskRoute::Amended(task) => people::Outcome::Amended { task },
                        PersonTaskRoute::AmendProposed { task, proposal } => {
                            people::Outcome::AmendProposed { task, proposal }
                        }
                    };
                    domain.work.push(Work::People(people::Event::Decided { request: Token::new(task), outcome }));
                    continue;
                }
                if let Some(moved) = domain.core.moving.remove(&Token::new(task)) {
                    domain.work.push(Work::People(people::Event::Decided {
                        request: Token::new(task),
                        outcome: people::Outcome::Moved { task: moved },
                    }));
                    continue;
                }
                let person_route =
                    if person_proposal { domain.core.routing_people_proposals.remove(&Token::new(task)) } else { None };
                if let Some(PersonProposalRoute::Accepting { request, person, proposer, proposal, message }) =
                    person_route
                {
                    domain
                        .core
                        .routing_people_proposals
                        .insert(request, PersonProposalRoute::Deciding { request, proposer, proposal, by: person })
                        .expect("person acceptance route room");
                    domain.work.push(Work::PersonProposal(tasks::Event::DecideProposal {
                        reply_to: ReplyTo::new(request),
                        proposer,
                        proposal,
                        message: Some(message),
                        by: tasks::Party::Person(person),
                        decision: tasks::ProposalDecision::Accept,
                    }));
                    continue;
                }
                if let Some(route) = domain.core.routing_calls.remove(&Token::new(task)) {
                    if let Some(subscription) = domain.forge_subscribing.remove(&Token::new(task)) {
                        let names = forge_route::watch_names(domain, &env.limits, &subscription)
                            .expect("connector names preflighted at subscription");
                        let owner = subscription.task;
                        domain.work.push(Work::Forge(forge::Event::Subscribe { subscription }));
                        domain.work.push(Work::Forge(forge::Event::Names { task: owner, resources: names }));
                    }
                    if let Some((owner, topic)) = domain.forge_unsubscribing.remove(&Token::new(task)) {
                        domain.work.push(Work::Forge(forge::Event::Unsubscribe { task: owner, topic }));
                    }
                    let (key, answer) = match route {
                        RoutedCall::Propose { key, proposal } => (key, CallAnswer::Proposed { proposal }),
                        RoutedCall::Accepting { key, proposer, proposal, message } => {
                            domain
                                .core
                                .routing_calls
                                .insert(Token::new(task), RoutedCall::Decide { key, proposal })
                                .expect("acceptance route room");
                            domain.work.push(Work::Tasks(tasks::Event::DecideProposal {
                                reply_to: ReplyTo::new(Token::new(task)),
                                proposer,
                                proposal,
                                message: Some(message),
                                by: tasks::Party::Task(key.task),
                                decision: tasks::ProposalDecision::Accept,
                            }));
                            continue;
                        }
                        RoutedCall::Introduce(key) => (key, CallAnswer::Introduced),
                        RoutedCall::Subscribe { key, subscription } => (key, CallAnswer::Subscribed { subscription }),
                        RoutedCall::Unsubscribe(key) => (key, CallAnswer::Unsubscribed),
                        RoutedCall::Control(key) => (key, CallAnswer::Controlled),
                        RoutedCall::Message(_) => unreachable!("message calls produce Sent"),
                        RoutedCall::Decide { .. } | RoutedCall::Withdraw { .. } | RoutedCall::Escalation { .. } => {
                            unreachable!("proposal decisions produce their own terminal")
                        }
                    };
                    decide_call(domain, &env.limits, decision, ReplyTo::new(Token::new(task)), key, answer);
                } else if let Some(attempt) = domain.core.claiming.remove(&task) {
                    let (writes, holders) = forge_route::claimed_writes(domain, env, task)
                        .expect("the admitted assignment retains its bounded held forge writes");
                    if !writes.is_empty() {
                        domain.work.push(Work::Forge(forge::Event::Claim { task, attempt, writes, holders }));
                    }
                    let proof = domain.core.proofs.get(&task).expect("claim proof pre-reserved");
                    save(decision, &env.limits, Write::Save(Record::RunProof(proof.clone())));
                    let key = &domain.assignments.get(&task).expect("claimed assignment").workspace.key;
                    let workstream = u64::from_be_bytes(key.as_ref().try_into().expect("task-number workstream"));
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
            }
            tasks::Request::WriterWaiting { task: Some(task), .. } => {
                if domain.core.claiming.remove(&task).is_some() {
                    drop(domain.assignments.remove(&task));
                    drop(domain.core.proofs.remove(&task));
                    domain.work.push(Work::Tasks(tasks::Event::PreparationFailed { task }));
                }
            }
            tasks::Request::WriterWaiting { task: None, .. } | tasks::Request::Waiting { .. } => {}
            tasks::Request::Taken { task, holdings } => {
                for holding in holdings {
                    if let tasks::Holding::Write { resource, .. } = holding
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
                }
            }
            tasks::Request::RestoreRefused { .. } => domain.startup = Startup::Failed,
        }
    }
}

#[expect(clippy::too_many_lines, reason = "the brief route consumes every child request and prepares the claimed run")]
fn brief_outputs(
    domain: &mut Domain,
    env: &Env<Limits>,
    _decision: &mut Decision,
    out: &mut Queue<brief::GatherRequest>,
) {
    for _ in 0..out.len() {
        match out.pop().expect("brief output count") {
            brief::GatherRequest::Gather { connector: 0, section, budget } => {
                let Some(row) = domain.brief_connectors.get(Id::from_token(section)) else { continue };
                let max_job_bytes = match row.source {
                    forge::BriefSource::Ci { .. } => {
                        env.limits.forge.client.answer_bytes.min(env.limits.brief.budgets.ci / 5).max(1)
                    }
                    forge::BriefSource::Reviews { .. } | forge::BriefSource::Pull { .. } => 0,
                };
                domain.work.push(Work::Forge(forge::Event::GatherBriefHeld {
                    section,
                    source: row.source,
                    parts: env.limits.brief.parts,
                    bytes: budget,
                    max_job_bytes,
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
                let current = match domain.core.contexts.get(&task) {
                    Some(context) => match domain.core.tasks.task(task) {
                        Some(row) => {
                            row.phase == tasks::Phase::Active(tasks::Active::Preparing)
                                && row.last_message == context.last_message
                        }
                        None => false,
                    },
                    None => false,
                };
                if !current {
                    for placed in order {
                        if let brief::GatherPlaced::Connector { token, .. } = placed {
                            let id = Id::from_token(token);
                            if domain.brief_connectors.get(id).is_some() {
                                domain.brief_connectors.retire(id);
                                domain.work.push(Work::Forge(forge::Event::DropBrief { section: token }));
                            }
                        }
                    }
                    continue;
                }
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
                    drop(domain.core.contexts.remove(&task));
                    drop(domain.core.dependency_results.remove(&task));
                    drop(domain.core.transcripts.remove(&task));
                    domain.work.push(Work::Tasks(tasks::Event::PreparationFailed { task }));
                    continue;
                }
                let sections = sections.into_boxed();
                let context = domain.core.contexts.remove(&task).expect("rendered task owns context");
                drop(domain.core.dependency_results.remove(&task));
                let transcript = domain.core.transcripts.remove(&task).expect("rendered task owns loaded transcript");
                let Some(attempt) = crate::fresh(&mut domain.core.counters, Family::Run) else {
                    domain.work.push(Work::Tasks(tasks::Event::PreparationFailed { task }));
                    continue;
                };
                let Some(workspace) = forge_route::run_workspace(domain, env, &context, attempt) else {
                    domain.work.push(Work::Tasks(tasks::Event::Hold { task, why: tasks::Hold::Effects }));
                    continue;
                };
                let Some(claim_names) = forge_route::claim_names(domain, &env.limits, task, &workspace.names) else {
                    domain.work.push(Work::Tasks(tasks::Event::Hold { task, why: tasks::Hold::Effects }));
                    continue;
                };
                if domain.core.run_admission(&context, env.wall, workspace.writes.clone())
                    != jig_core::RunAdmission::Allow
                {
                    domain.work.push(Work::Tasks(tasks::Event::Hold { task, why: tasks::Hold::Effects }));
                    continue;
                }
                let Some(grant) = domain.core.accounts.grant(domain.core.settings.account, env.now) else {
                    domain.work.push(Work::Tasks(tasks::Event::PreparationFailed { task }));
                    continue;
                };
                if !domain.core.proofs.contains_key(&task) && domain.core.proofs.len() == domain.core.proofs.capacity()
                {
                    domain.work.push(Work::Tasks(tasks::Event::PreparationFailed { task }));
                    continue;
                }
                let offered = if context.last_message == 0 { None } else { Some(context.last_message) };
                assert!(
                    domain
                        .core
                        .proofs
                        .insert(task, RunProof { task, attempt, offered, turn: None, terminal: None })
                        .is_ok(),
                    "claim proof reserved before child mutation"
                );
                let turns = domain.core.resumed_turns(transcript);
                let assignment = Assignment {
                    task,
                    attempt,
                    charter: domain.core.settings.charter,
                    run: Box::new(RunCharter {
                        policy: domain.core.settings.run.clone(),
                        contract: context.contract,
                        authority: context.authority,
                        budget: authority::left(authority_numbers(context.numbers))
                            .min(domain.core.authority.rules().maximum_run_spend),
                    }),
                    sections,
                    inbox: context.inbox,
                    saved: forge_route::saved_tags(&context.saved, domain.config.forge_connector)
                        .expect("task saved names were admitted by the root"),
                    workspace: workspace.workspace.clone(),
                    transcript: turns,
                    answered: {
                        let mut answered = List::with_capacity(domain.limits.call_records);
                        for (&key, _) in &domain.core.call_parts {
                            if key.task == task && key.attempt < attempt {
                                let answer = call_answer(domain, key).expect("live call part has its connector answer");
                                answered.push(crate::CallRecord { key, answer }).expect("retained call bound");
                            }
                        }
                        answered.into_boxed()
                    },
                    grant,
                };
                assert!(domain.assignments.insert(task, assignment).is_ok(), "assignment fits live task room");
                assert!(domain.core.claiming.insert(task, attempt) == Ok(None), "one pending claim per task");
                if workspace.names.is_empty() {
                    domain.work.push(Work::TasksClaim { task, attempt, writes: Box::new([]) });
                } else {
                    let mut hub_writes = List::with_capacity(env.limits.tasks.holdings);
                    for name in &workspace.names {
                        let resource = forge_route::hub_name(domain.config.forge_connector, name, &env.limits.tasks)
                            .expect("admitted forge branch name fits the hub's bound");
                        hub_writes.push(resource).expect("workspace write bound fits hub");
                    }
                    domain.work.push(Work::Forge(forge::Event::Names { task, resources: claim_names }));
                    for name in workspace.own_holds {
                        domain.work.push(Work::Forge(forge::Event::Hold { task, resource: name, from: None }));
                    }
                    domain.work.push(Work::TasksClaim { task, attempt, writes: hub_writes.into_boxed() });
                }
            }
            brief::GatherRequest::Failed { brief, .. } | brief::GatherRequest::Refused { brief } => {
                let task = brief.raw();
                drop(domain.core.contexts.remove(&task));
                drop(domain.core.dependency_results.remove(&task));
                drop(domain.core.transcripts.remove(&task));
                domain.work.push(Work::Tasks(tasks::Event::PreparationFailed { task }));
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
    reason = "the closed fleet output vocabulary remains exhaustive within one root decision"
)]
fn fleet_outputs(domain: &mut Domain, env: &Env<Limits>, decision: &mut Decision, out: &mut Queue<fleet::Request>) {
    for _ in 0..out.len() {
        match out.pop().expect("fleet output count") {
            fleet::Request::Assign { channel, kind: _, run, attempt } => {
                let assignment = domain.assignments.remove(&run.raw()).expect("durable claim has prepared assignment");
                assert!(assignment.attempt == attempt.raw(), "assignment names current attempt");
                emit(decision, &env.limits, Delivery::View(Box::new(views::Event::Started { task: run, attempt })));
                emit(decision, &env.limits, Delivery::Assigned { channel, assignment });
            }
            fleet::Request::Placed { run, attempt } => {
                if domain.core.unreported_restored.get(&run.raw()) == Some(&attempt.raw()) {
                    let _: Option<u64> = domain.core.unreported_restored.remove(&run.raw());
                }
                domain.work.push(Work::Tasks(tasks::Event::Started { task: run.raw(), attempt: attempt.raw() }));
            }
            fleet::Request::Turned { run, attempt, turn, body } => {
                assert!(
                    current_proof(domain, run.raw(), attempt.raw()),
                    "actual current turn has reserved root proof before child mutation"
                );
                let payload = domain.payloads.get(Id::from_token(body)).expect("fleet returns owned token");
                let payload = match payload.as_ref().expect("fleet returns owned payload") {
                    Payload::Turn { body: payload, .. } => payload,
                    Payload::Answer { .. } | Payload::Call { .. } | Payload::CallAnswer(_) => {
                        unreachable!("fleet returns turn family")
                    }
                };
                domain.work.push(Work::Tasks(tasks::Event::Turn {
                    reply_to: ReplyTo::new(body),
                    task: run.raw(),
                    attempt: attempt.raw(),
                    turn,
                    read: payload.read,
                    offered: domain.core.proofs.get(&run.raw()).expect("current proof").offered,
                    cumulative: payload.cumulative,
                }));
            }
            fleet::Request::Answered { run, attempt, payload, to, .. } => {
                let _: Option<u64> = domain.core.unreported_restored.remove(&run.raw());
                assert!(
                    current_proof(domain, run.raw(), attempt.raw()),
                    "actual current answer has reserved root proof before child mutation"
                );
                let _answered = to.into_token();
                let body = domain.payloads.get_mut(Id::from_token(payload)).expect("fleet returns owned token");
                let (cumulative, end, saved) = match body.as_mut().expect("fleet returns owned payload") {
                    Payload::Answer { cumulative, end, saved, .. } => (*cumulative, end.clone(), saved.clone()),
                    Payload::Turn { .. } | Payload::Call { .. } | Payload::CallAnswer(_) => {
                        unreachable!("fleet returns answer family")
                    }
                };
                let (end, saved) = match saved {
                    Some(tags) => match forge_route::saved_resources(domain.config.forge_connector, &tags) {
                        Some(resources) => (end, Some(resources)),
                        None => (tasks::End::Failed(tasks::Class::Invalid), None),
                    },
                    None => (end, None),
                };
                let proof = domain.core.proofs.get_mut(&run.raw()).expect("terminal proof pre-reserved");
                assert!(proof.attempt == attempt.raw(), "terminal callback belongs to current proof");
                proof.terminal =
                    Some(TerminalRecord { task: run.raw(), attempt: attempt.raw(), cumulative, end: end.clone() });
                domain.work.push(Work::Tasks(tasks::Event::Activation {
                    reply_to: ReplyTo::new(payload),
                    task: run.raw(),
                    attempt: attempt.raw(),
                    cause: tasks::Cause::Priced { cumulative },
                    end,
                    saved,
                }));
            }
            fleet::Request::Acknowledge { channel, run, attempt } => {
                emit(decision, &env.limits, Delivery::Acknowledge { channel, task: run.raw(), attempt: attempt.raw() });
            }
            fleet::Request::AcknowledgeTurn { channel, run, attempt, turn } => emit(
                decision,
                &env.limits,
                Delivery::AcknowledgeTurn { channel, task: run.raw(), attempt: attempt.raw(), turn },
            ),
            fleet::Request::Cancel { channel, run, attempt } => {
                emit(decision, &env.limits, Delivery::Cancel { channel, task: run.raw(), attempt: attempt.raw() });
            }
            fleet::Request::Refuse { channel } => emit(decision, &env.limits, Delivery::Refuse { channel }),
            fleet::Request::Drop { payload } => drop(take_payload(domain, payload)),
            fleet::Request::Listed { .. } => {}
            fleet::Request::TurnBusy { channel, run, attempt, turn } => emit(
                decision,
                &env.limits,
                Delivery::TurnBusy { channel, task: run.raw(), attempt: attempt.raw(), turn },
            ),
            fleet::Request::Lost { to, run, attempt } => {
                let _answered = to.into_token();
                let proof = domain.core.proofs.get(&run.raw()).expect("lost claim has reserved durable evidence");
                assert!(proof.attempt == attempt.raw(), "lost callback belongs to current proof");
                let never_reported = domain.core.unreported_restored.remove(&run.raw()) == Some(attempt.raw());
                let mut committed_call = false;
                for (key, _) in &domain.core.call_parts {
                    if key.task == run.raw() && key.attempt == attempt.raw() {
                        committed_call = true;
                        break;
                    }
                }
                let end = if never_reported && proof.turn.is_none() && !committed_call {
                    tasks::End::Refused
                } else {
                    tasks::End::Failed(tasks::Class::Lost)
                };
                remember_unpriced_terminal(domain, run, attempt, end.clone());
                domain.work.push(Work::Tasks(tasks::Event::Activation {
                    reply_to: internal(u64::MAX),
                    task: run.raw(),
                    attempt: attempt.raw(),
                    end,
                    saved: None,
                    cause: tasks::Cause::Unpriced,
                }));
                domain.work.push(Work::Forge(forge::Event::Lost { task: run.raw(), attempt: attempt.raw() }));
            }
            fleet::Request::NotStarted { to, run, attempt }
            | fleet::Request::Withdrawn { to, run, attempt, .. }
            | fleet::Request::Refused { to, run, attempt, .. } => {
                let _answered = to.into_token();
                let _: Option<u64> = domain.core.unreported_restored.remove(&run.raw());
                drop(domain.assignments.remove(&run.raw()));
                remember_unpriced_terminal(domain, run, attempt, tasks::End::Refused);
                domain.work.push(Work::Tasks(tasks::Event::Activation {
                    reply_to: internal(u64::MAX),
                    task: run.raw(),
                    attempt: attempt.raw(),
                    end: tasks::End::Refused,
                    saved: None,
                    cause: tasks::Cause::Unpriced,
                }));
            }
            fleet::Request::Inbound { channel, run, attempt, event } => {
                let relay = domain.core.relaying.take().expect("fleet inbound follows committed word");
                assert!(event.raw() == relay.word.number, "relay event identifies word");
                if let Some(proof) = domain.core.proofs.get_mut(&run.raw())
                    && proof.attempt == attempt.raw()
                    && proof.offered == relay.previous
                {
                    proof.offered = Some(match proof.offered {
                        Some(previous) => previous.max(relay.word.number),
                        None => relay.word.number,
                    });
                    save(decision, &env.limits, Write::Save(Record::RunProof(proof.clone())));
                    emit(
                        decision,
                        &env.limits,
                        Delivery::Inbound { channel, task: run.raw(), attempt: attempt.raw(), word: relay.word },
                    );
                }
            }
            fleet::Request::Undelivered { .. } => {
                drop(domain.core.relaying.take());
            }
            fleet::Request::Relay { reply_to, run, attempt, body } => {
                let Some(Payload::Call { key, body }) = take_payload(domain, body) else {
                    unreachable!("fleet returns admitted call payload")
                };
                assert!(key.task == run.raw() && key.attempt == attempt.raw(), "fleet call envelope is unchanged");
                assert!(current_proof(domain, key.task, key.attempt), "fleet only relays a current claim");
                match call_answer(domain, key) {
                    Some(answer) => relay_call(domain, &env.limits, decision, reply_to, answer),
                    None => match body.tool {
                        Tool::Unavailable => {
                            decide_call(domain, &env.limits, decision, reply_to, key, CallAnswer::Unavailable);
                        }
                        Tool::Rejected(why) => decide_call(
                            domain,
                            &env.limits,
                            decision,
                            reply_to,
                            key,
                            CallAnswer::DelegationRefused(tasks::Problem { task: None, why, blocked_by: None }),
                        ),
                        Tool::RejectedMessage(why) => decide_call(
                            domain,
                            &env.limits,
                            decision,
                            reply_to,
                            key,
                            CallAnswer::MessageRefused(tasks::Problem { task: None, why, blocked_by: None }),
                        ),
                        Tool::RejectedControl(why) => decide_call(
                            domain,
                            &env.limits,
                            decision,
                            reply_to,
                            key,
                            CallAnswer::ControlRefused(tasks::Problem { task: None, why, blocked_by: None }),
                        ),
                        Tool::RejectedProposal(why) => decide_call(
                            domain,
                            &env.limits,
                            decision,
                            reply_to,
                            key,
                            CallAnswer::ProposalRefused(tasks::Problem { task: None, why, blocked_by: None }),
                        ),
                        Tool::Delegate { batch } => {
                            delegate_call(domain, env, decision, reply_to, key, batch, false, Box::new([]));
                        }
                        Tool::Propose { action, reason, as_holder } => {
                            proposals::propose_call(domain, env, decision, reply_to, key, action, reason, as_holder);
                        }
                        Tool::Decide { proposer, proposal, decision: choice } => {
                            proposals::decide_call(domain, env, decision, reply_to, key, proposer, proposal, choice);
                        }
                        Tool::Withdraw { proposal } => {
                            proposals::withdraw_call(domain, env, decision, reply_to, key, proposal);
                        }
                        Tool::DecideEscalation { task, revision, decision: choice } => {
                            escalation::task_decide(domain, env, decision, reply_to, key, task, revision, choice);
                        }
                        Tool::Amend { target, amendment } => {
                            amend_call(domain, env, decision, reply_to, key, target, amendment);
                        }
                        Tool::Cancel { target, reason } => {
                            control_call(domain, reply_to, key, target, tasks::Control::Cancel { reason });
                        }
                        Tool::Release { target } => {
                            control_call(domain, reply_to, key, target, tasks::Control::Release);
                        }
                        Tool::Message { target, form, words } => {
                            message_call(domain, env, decision, reply_to, key, target, form, words);
                        }
                        Tool::Introduce { left, right } => {
                            introduce_call(domain, reply_to, key, left, right);
                        }
                        Tool::Subscribe { kind } => subscribe_call(domain, env, decision, reply_to, key, kind),
                        Tool::SubscribeForge { topic, own_change, paths } => {
                            forge_route::subscribe_call(domain, env, decision, reply_to, key, topic, own_change, paths);
                        }
                        Tool::ReadForge { repository, read } => {
                            forge_route::read_call(domain, env, decision, reply_to, key, repository, read);
                        }
                        Tool::EffectForge { repository, resource, write } => {
                            forge_route::effect_call(
                                domain, env, decision, reply_to, key, repository, resource, *write,
                            );
                        }
                        Tool::Unsubscribe { subscription } => unsubscribe_call(domain, reply_to, key, subscription),
                    },
                }
            }
            fleet::Request::Relayed { channel, run, attempt, call, answer } => {
                let Some(Payload::CallAnswer(answer)) = take_payload(domain, answer) else {
                    unreachable!("fleet relays an owned call answer")
                };
                emit(
                    decision,
                    &env.limits,
                    Delivery::CallAnswer { channel, task: run.raw(), attempt: attempt.raw(), call, answer },
                );
            }
            fleet::Request::AssignTyped { .. }
            | fleet::Request::InboundTyped { .. }
            | fleet::Request::RelayTyped { .. }
            | fleet::Request::RelayedTyped { .. }
            | fleet::Request::DropTyped { .. }
            | fleet::Request::UndeliveredTyped { .. }
            | fleet::Request::Grant { .. }
            | fleet::Request::Rejected { .. }
            | fleet::Request::Exhausted { .. }
            | fleet::Request::Bounced { .. }
            | fleet::Request::Told { .. } => unreachable!("06a does not route tool/credential worker messages"),
        }
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
    domain.work.push(Work::DelegateInputRefused { to: read.to, key: read.key });
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
    if row.number != wanted
        || row.project != project
        || row.requester != tasks::Party::Task(creator)
        || !match &row.phase {
            tasks::Phase::Ended(_) => true,
            tasks::Phase::Waiting | tasks::Phase::Active(_) | tasks::Phase::Closing(_) | tasks::Phase::Held { .. } => {
                false
            }
        }
    {
        input_failed(domain, waiter);
        return;
    }
    let Some(Some(Read::InputCheck(read))) = domain.result_reads.get_mut(Id::from_token(waiter)) else {
        unreachable!("input read survives validation")
    };
    let phase = match &row.phase {
        tasks::Phase::Ended(tasks::Ending::Done(_)) => tasks::Status::Done,
        tasks::Phase::Ended(tasks::Ending::Failed { .. }) => tasks::Status::Failed,
        tasks::Phase::Ended(tasks::Ending::Cancelled { .. }) => tasks::Status::Cancelled,
        tasks::Phase::Waiting | tasks::Phase::Active(_) | tasks::Phase::Closing(_) | tasks::Phase::Held { .. } => {
            unreachable!("validated historical input")
        }
    };
    read.stubs
        .push(tasks::Stub { task: wanted, phase, result: Token::new(wanted) })
        .expect("bounded historical input count");
    read.at = read.at.checked_add(1).expect("bounded input index");
    if let Some(&next) = read.ids.get(usize::try_from(read.at).expect("bounded input index")) {
        request_load(domain, waiter, Range::TaskResult { task: next }, None, out);
        return;
    }
    let Some(Read::InputCheck(read)) = take_read(domain, waiter) else { unreachable!("completed input read") };
    domain.result_reads.retire(Id::from_token(waiter));
    domain.work.push(Work::DelegateValidated {
        to: read.to,
        key: read.key,
        batch: read.batch,
        stubs: read.stubs.into_boxed(),
    });
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
    drop(domain.core.contexts.remove(&task));
    drop(domain.core.transcripts.remove(&task));
    domain.work.push(Work::Tasks(tasks::Event::PreparationFailed { task }));
}

fn begin_dependency_read(domain: &mut Domain, task: u64) -> Option<(Token, u64)> {
    let context = domain.core.contexts.get(&task).expect("preparing task context");
    let count = context.dependencies.len().checked_add(context.spec.inputs.len()).expect("bounded input count");
    if count == 0 {
        domain.work.push(Work::StartBrief { task });
        return None;
    }
    let mut ids = List::with_capacity(u32::try_from(count).expect("bounded dependency count"));
    for &dependency in &context.dependencies {
        ids.push(dependency).expect("dependency identity room");
    }
    for &input in &context.spec.inputs {
        ids.push(input).expect("input identity room");
    }
    let first = *ids.get(0).expect("nonempty result identities");
    let read = DependencyRead {
        task,
        ids: ids.into_boxed(),
        at: 0,
        results: List::with_capacity(u32::try_from(count).expect("bounded result count")),
    };
    let Ok(waiter) = domain.result_reads.insert(Some(Read::Dependency(read))) else {
        drop(domain.core.contexts.remove(&task));
        drop(domain.core.transcripts.remove(&task));
        domain.work.push(Work::Tasks(tasks::Event::PreparationFailed { task }));
        return None;
    };
    Some((waiter.token(), first))
}

fn dependency_failed(domain: &mut Domain, waiter: Token) {
    let Some(Read::Dependency(read)) = take_read(domain, waiter) else { return };
    domain.result_reads.retire(Id::from_token(waiter));
    drop(domain.core.contexts.remove(&read.task));
    drop(domain.core.transcripts.remove(&read.task));
    domain.work.push(Work::Tasks(tasks::Event::PreparationFailed { task: read.task }));
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
    let project = domain.core.contexts.get(&read.task).expect("preparing task context").project;
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
    if row.number != wanted || row.project != project {
        dependency_failed(domain, waiter);
        return;
    }
    let ending = match row.phase {
        tasks::Phase::Ended(ending) => ending,
        tasks::Phase::Waiting | tasks::Phase::Active(_) | tasks::Phase::Closing(_) | tasks::Phase::Held { .. } => {
            dependency_failed(domain, waiter);
            return;
        }
    };
    let (kind, words) = result_notice(ending);
    let Some(Some(Read::Dependency(read))) = domain.result_reads.get_mut(Id::from_token(waiter)) else {
        unreachable!("read survives validation")
    };
    read.results.push(HistoricalResult { task: wanted, kind, words }).expect("one result per bounded ID");
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

#[expect(clippy::too_many_lines, reason = "one preparation gathers typed core sections and pinned forge sources")]
fn start_brief(domain: &mut Domain, env: &Env<Limits>, task: u64) {
    let Some(context) = domain.core.contexts.get(&task) else { return };
    let Some(current) = domain.core.tasks.task(task) else { return };
    if current.phase != tasks::Phase::Active(tasks::Active::Preparing) || current.last_message != context.last_message {
        return;
    }
    let oversized = domain.core.transcript_oversized(task);
    let mut wanted = List::with_capacity(domain.limits.brief.sections);
    let Some(task_text) = read_core(domain, task, TaskBriefPart::Spec) else {
        domain.work.push(Work::Tasks(tasks::Event::PreparationFailed { task }));
        return;
    };
    wanted
        .push(brief::Planned::Core {
            kind: brief::Core::Task,
            text: task_text,
            limit: domain.limits.brief.budgets.task,
            priority: 0,
            required: true,
        })
        .expect("task brief room");
    if !context.dependencies.is_empty() || !context.spec.inputs.is_empty() {
        let Some(text) = read_core(domain, task, TaskBriefPart::Dependencies) else {
            domain.work.push(Work::Tasks(tasks::Event::PreparationFailed { task }));
            return;
        };
        wanted
            .push(brief::Planned::Core {
                kind: brief::Core::Results,
                text,
                limit: domain.limits.brief.budgets.dependencies,
                priority: 1,
                required: true,
            })
            .expect("dependency result section room");
    }
    if oversized {
        let Some(text) = read_core(domain, task, TaskBriefPart::TranscriptTail) else {
            domain.work.push(Work::Tasks(tasks::Event::PreparationFailed { task }));
            return;
        };
        wanted
            .push(brief::Planned::Core {
                kind: brief::Core::TranscriptTail,
                text,
                limit: domain.limits.brief.budgets.task,
                priority: 2,
                required: true,
            })
            .expect("tail brief room");
    }
    if !context.delegates.is_empty()
        && wanted.room() > 0
        && let Some(text) = read_core(domain, task, TaskBriefPart::Delegates)
    {
        wanted
            .push(brief::Planned::Core {
                kind: brief::Core::Plan,
                text,
                limit: domain.limits.brief.budgets.plan,
                priority: 3,
                required: false,
            })
            .expect("delegate section room");
    }
    if context.tries != tasks::Tries::NONE
        && wanted.room() > 0
        && let Some(text) = read_core(domain, task, TaskBriefPart::Attempts)
    {
        wanted
            .push(brief::Planned::Core {
                kind: brief::Core::Attempts,
                text,
                limit: domain.limits.brief.budgets.attempts,
                priority: 4,
                required: false,
            })
            .expect("attempt brief room");
    }
    let parent = match context.requester {
        tasks::Party::Task(parent) => Some(parent),
        tasks::Party::Person(_) | tasks::Party::Deployment { .. } => None,
    };
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
            .insert(BriefConnector { task, source, kind, cutting: false })
            .expect("reserved connector section room")
            .token();
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
                .insert(BriefConnector { task, source, kind, cutting: false })
                .expect("reserved connector section room")
                .token();
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

fn read_core(domain: &Domain, task: u64, part: TaskBriefPart) -> Option<Box<[u8]>> {
    let tail = match part {
        TaskBriefPart::TranscriptTail => true,
        TaskBriefPart::Spec | TaskBriefPart::Dependencies | TaskBriefPart::Delegates | TaskBriefPart::Attempts => false,
    };
    let read = task_section(domain, task, part, domain.limits.brief.parts, domain.limits.brief.read_bytes);
    let parts = match read {
        TaskBriefRead::Got(parts) => parts,
        TaskBriefRead::Failed => return None,
    };
    let mut length = 0_usize;
    let mut ended_line = true;
    for part in &parts {
        length = length.checked_add(part.bytes.len())?;
        if let Some(last) = part.bytes.last() {
            ended_line = *last == b'\n';
        }
        if part.left > 0 {
            length = length
                .checked_add(usize::from(!tail && !ended_line))?
                .checked_add(1)?
                .checked_add(Decimal::of(part.left).as_bytes().len())?
                .checked_add(b" bytes cut]\n".len())?;
            ended_line = true;
        }
    }
    if length > usize::try_from(domain.limits.brief.brief_bytes).ok()? {
        return None;
    }
    let mut writer = Writer::new(length);
    let mut ended_line = true;
    for part in parts {
        if tail && part.left > 0 {
            writer.put(b"[").expect("measured cut marker");
            writer.put(Decimal::of(part.left).as_bytes()).expect("measured lost count");
            writer.put(b" bytes cut]\n").expect("measured cut marker");
        }
        writer.put(&part.bytes).expect("measured part");
        if let Some(last) = part.bytes.last() {
            ended_line = *last == b'\n';
        }
        if !tail && part.left > 0 {
            if !ended_line {
                writer.put(b"\n").expect("measured break");
            }
            writer.put(b"[").expect("measured cut marker");
            writer.put(Decimal::of(part.left).as_bytes()).expect("measured lost count");
            writer.put(b" bytes cut]\n").expect("measured cut marker");
            ended_line = true;
        }
    }
    Some(writer.finish())
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
        Startup::Cold | Startup::Adopting | Startup::Running | Startup::Failed => return,
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
    domain.startup = Startup::Adopting;
    domain.work.push(Work::Forge(forge::Event::Restored { clock: forge_client::RecoveryClock::Wall }));
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
    domain.startup = Startup::Running;
    for _ in 0..domain.core.due.len() {
        domain.work.push(Work::Activate(domain.core.due.pop().expect("restored due tasks")));
    }
    route_into(domain, env, &mut decision);
    close(domain, env, decision, out);
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
                | jig_core::Now::NotesIndexed { .. }
                | jig_core::Now::NotesRecalled { .. }
                | jig_core::Now::NotesRefused { .. } => unreachable!("account route owns its now output"),
            },
            jig_core::Request::Decided => {}
            jig_core::Request::Write(_)
            | jig_core::Request::Ask { .. }
            | jig_core::Request::Held(_)
            | jig_core::Request::Route(_) => unreachable!("account route changes no store decision"),
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

fn result_notice(ending: tasks::Ending) -> (tasks::ResultKind, Box<[u8]>) {
    match ending {
        tasks::Ending::Done(result) => match result {
            tasks::TaskResult::Report { words } => (tasks::ResultKind::Report, words),
            tasks::TaskResult::Verdict { code, words } => (tasks::ResultKind::Verdict { code }, words),
            tasks::TaskResult::Change { connector, kind, resource, words } => {
                (tasks::ResultKind::Change { connector, kind, resource }, words)
            }
            tasks::TaskResult::Failure { reason } => (tasks::ResultKind::Failed, reason),
        },
        tasks::Ending::Failed { reason } => (tasks::ResultKind::Failed, reason),
        tasks::Ending::Cancelled { reason, .. } => (tasks::ResultKind::Cancelled, reason),
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

fn authority_numbers(numbers: tasks::Numbers) -> authority::Numbers {
    authority::Numbers {
        budget: numbers.budget,
        spent: numbers.spent,
        spent_below: numbers.spent_below,
        reserved: numbers.reserved,
    }
}

fn symbolic_grants(grants: &[tasks::Grant], limit: u32) -> Option<Box<[authority::Grant]>> {
    let mut result = List::with_capacity(limit);
    for grant in grants {
        let last = match &grant.pattern.last {
            tasks::Last::Open(prefix) => authority::Last::Open(prefix.clone()),
            tasks::Last::Exact(_) => return None,
        };
        result
            .push(authority::Grant {
                connector: grant.connector,
                kind: grant.kind,
                pattern: authority::Pattern { segments: grant.pattern.segments.clone(), last },
            })
            .ok()?;
    }
    Some(result.into_boxed())
}

fn resolved_delegate_authority(
    base: &tasks::Authority,
    symbolic: &[tasks::Grant],
    task: u64,
    limits: &Limits,
) -> Option<tasks::Authority> {
    let symbols = symbolic_grants(symbolic, limits.authority.grants)?;
    let resolved = authority::resolve_task_grants(&symbols, task, &limits.authority)?;
    let mut authority = authority_value(base);
    let mut grants = List::with_capacity(limits.authority.grants);
    for grant in &authority.grants {
        grants.push(grant.clone()).ok()?;
    }
    for grant in resolved {
        grants.push(grant).ok()?;
    }
    authority.grants = grants.into_boxed();
    Some(task_authority(&authority))
}

fn task_authority(value: &authority::Authority) -> tasks::Authority {
    let mut grants = List::with_capacity(u32::try_from(value.grants.len()).expect("validated authority grants"));
    for grant in &value.grants {
        let last = match &grant.pattern.last {
            authority::Last::Exact(bytes) => tasks::Last::Exact(bytes.clone()),
            authority::Last::Open(bytes) => tasks::Last::Open(bytes.clone()),
        };
        grants
            .push(tasks::Grant {
                connector: grant.connector,
                kind: grant.kind,
                pattern: tasks::Pattern { segments: grant.pattern.segments.clone(), last },
            })
            .expect("grant capacity");
    }
    let mut note_resources =
        List::with_capacity(u32::try_from(value.note_resources.len()).expect("validated note scopes"));
    for scope in &value.note_resources {
        note_resources
            .push(tasks::ResourceScope {
                connector: scope.connector,
                pattern: tasks::Pattern {
                    segments: scope.pattern.segments.clone(),
                    last: match &scope.pattern.last {
                        authority::Last::Exact(bytes) => tasks::Last::Exact(bytes.clone()),
                        authority::Last::Open(bytes) => tasks::Last::Open(bytes.clone()),
                    },
                },
            })
            .expect("note scope capacity");
    }
    let mut kinds = List::with_capacity(u32::try_from(value.delegation.kinds.len()).expect("validated executors"));
    for kind in &value.delegation.kinds {
        kinds
            .push(match kind {
                authority::Executor::Charter(number) => tasks::AuthorityExecutor::Charter(*number),
                authority::Executor::Procedure(number) => tasks::AuthorityExecutor::Procedure(*number),
                authority::Executor::Role(number) => tasks::AuthorityExecutor::Role(*number),
            })
            .expect("executor capacity");
    }
    tasks::Authority {
        tools: tasks::Tools(value.tools.0),
        grants: grants.into_boxed(),
        delegation: tasks::Delegation {
            kinds: kinds.into_boxed(),
            tasks: value.delegation.tasks,
            depth: value.delegation.depth,
        },
        budget: tasks::Budget { spend: value.budget.spend, deadline: value.budget.deadline },
        notes: tasks::Scopes(value.notes.0),
        note_resources: note_resources.into_boxed(),
    }
}

fn authority_value(value: &tasks::Authority) -> authority::Authority {
    let mut grants = List::with_capacity(u32::try_from(value.grants.len()).expect("validated task grants"));
    for grant in &value.grants {
        let last = match &grant.pattern.last {
            tasks::Last::Exact(bytes) => authority::Last::Exact(bytes.clone()),
            tasks::Last::Open(bytes) => authority::Last::Open(bytes.clone()),
        };
        grants
            .push(authority::Grant {
                connector: grant.connector,
                kind: grant.kind,
                pattern: authority::Pattern { segments: grant.pattern.segments.clone(), last },
            })
            .expect("grant capacity");
    }
    let mut note_resources =
        List::with_capacity(u32::try_from(value.note_resources.len()).expect("validated task note scopes"));
    for scope in &value.note_resources {
        note_resources
            .push(authority::ResourceScope {
                connector: scope.connector,
                pattern: authority::Pattern {
                    segments: scope.pattern.segments.clone(),
                    last: match &scope.pattern.last {
                        tasks::Last::Exact(bytes) => authority::Last::Exact(bytes.clone()),
                        tasks::Last::Open(bytes) => authority::Last::Open(bytes.clone()),
                    },
                },
            })
            .expect("note scope capacity");
    }
    let mut kinds = List::with_capacity(u32::try_from(value.delegation.kinds.len()).expect("validated task executors"));
    for kind in &value.delegation.kinds {
        kinds
            .push(match kind {
                tasks::AuthorityExecutor::Charter(number) => authority::Executor::Charter(*number),
                tasks::AuthorityExecutor::Procedure(number) => authority::Executor::Procedure(*number),
                tasks::AuthorityExecutor::Role(number) => authority::Executor::Role(*number),
            })
            .expect("executor capacity");
    }
    authority::Authority {
        tools: authority::Tools(value.tools.0),
        grants: grants.into_boxed(),
        delegation: authority::Delegation {
            kinds: kinds.into_boxed(),
            tasks: value.delegation.tasks,
            depth: value.delegation.depth,
        },
        budget: authority::Budget { spend: value.budget.spend, deadline: value.budget.deadline },
        notes: authority::Scopes(value.notes.0),
        note_resources: note_resources.into_boxed(),
    }
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

fn text_part(first: &[u8], second: &[u8], third: &[u8], fourth: &[u8], available: u32) -> TaskBriefFragment {
    let total = first
        .len()
        .checked_add(second.len())
        .expect("bounded part bytes")
        .checked_add(third.len())
        .expect("bounded part bytes")
        .checked_add(fourth.len())
        .expect("bounded part bytes");
    let wanted = total.min(usize::try_from(available).expect("u32 fits usize"));
    let mut keep = 0_usize;
    for source in [first, second, third, fourth] {
        let demand = wanted.checked_sub(keep).expect("prefix within demand");
        let length = prefix(source, demand).len();
        keep = keep.checked_add(length).expect("bounded part prefix");
        if length < source.len() {
            break;
        }
    }
    let mut text = Writer::new(keep);
    for source in [first, second, third, fourth] {
        let bytes = prefix(source, text.room());
        text.put(bytes).expect("measured exact text part");
        if bytes.len() < source.len() {
            break;
        }
    }
    TaskBriefFragment {
        bytes: text.finish(),
        left: u64::try_from(total.checked_sub(keep).expect("prefix in part")).expect("usize fits u64"),
    }
}

fn prefix(bytes: &[u8], most: usize) -> &[u8] {
    let mut end = most.min(bytes.len());
    for _ in 0_u32..3 {
        if let Some(byte) = bytes.get(end)
            && byte & 0b1100_0000 == 0b1000_0000
        {
            if end == 0 {
                break;
            }
            end = end.checked_sub(1).expect("positive prefix end");
        }
    }
    bytes.get(..end).expect("UTF-8 prefix within source")
}

fn task_section(domain: &Domain, task: u64, part: TaskBriefPart, parts: u32, bytes: u32) -> TaskBriefRead {
    if parts == 0 {
        return TaskBriefRead::Failed;
    }
    match part {
        TaskBriefPart::Spec => match domain.core.contexts.get(&task) {
            Some(context) => task_read(context, parts, bytes),
            None => TaskBriefRead::Failed,
        },
        TaskBriefPart::Delegates => match domain.core.contexts.get(&task) {
            Some(context) => delegates_read(&context.delegates, bytes),
            None => TaskBriefRead::Failed,
        },
        TaskBriefPart::Dependencies => match domain.core.dependency_results.get(&task) {
            Some(results) => dependency_read(results, bytes),
            None => TaskBriefRead::Failed,
        },
        TaskBriefPart::Attempts => match domain.core.contexts.get(&task) {
            Some(context) => attempt_read(context.tries, context.invalid_result, bytes),
            None => TaskBriefRead::Failed,
        },
        TaskBriefPart::TranscriptTail => match domain.core.transcripts.get(&task) {
            Some(transcript) => tail_read(transcript, bytes),
            None => TaskBriefRead::Failed,
        },
    }
}

fn attempt_read(tries: tasks::Tries, invalid: Option<tasks::InvalidResult>, bytes: u32) -> TaskBriefRead {
    let classes: [(&[u8], u32); 6] = [
        (b"transient: ", tries.transient),
        (b"permanent: ", tries.permanent),
        (b"run: ", tries.run),
        (b"agent: ", tries.agent),
        (b"lost: ", tries.lost),
        (b"invalid: ", tries.invalid),
    ];
    let mut total = 0_usize;
    for (name, count) in classes {
        if count > 0 {
            total = total
                .checked_add(name.len())
                .expect("bounded attempt label")
                .checked_add(Decimal::of(u64::from(count)).as_bytes().len())
                .expect("bounded count")
                .checked_add(1)
                .expect("newline");
        }
    }
    let reason = match invalid {
        Some(tasks::InvalidResult::Form) => b"last invalid result: contract form\n".as_slice(),
        Some(tasks::InvalidResult::Verdict) => b"last invalid result: verdict code\n",
        Some(tasks::InvalidResult::Words) => b"last invalid result: word limit\n",
        Some(tasks::InvalidResult::Change) => b"last invalid result: change identity\n",
        Some(tasks::InvalidResult::Followups) => b"last invalid result: follow-up limit\n",
        None => b"",
    };
    total = total.checked_add(reason.len()).expect("bounded invalid reason");
    let mut writer = Writer::new(total.min(usize::try_from(bytes).expect("u32 fits usize")));
    for (name, count) in classes {
        if count > 0 {
            for fragment in [name, Decimal::of(u64::from(count)).as_bytes(), b"\n"] {
                let kept = prefix(fragment, writer.room());
                writer.put(kept).expect("attempt prefix fits");
            }
        }
    }
    let kept = prefix(reason, writer.room());
    writer.put(kept).expect("invalid reason prefix fits");
    let text = writer.finish();
    TaskBriefRead::Got(Box::new([TaskBriefFragment {
        left: u64::try_from(total.checked_sub(text.len()).expect("written prefix")).expect("usize fits u64"),
        bytes: text,
    }]))
}

fn delegate_phase(phase: &tasks::Phase) -> &'static [u8] {
    match phase {
        tasks::Phase::Waiting => b"waiting",
        tasks::Phase::Active(active) => match active {
            tasks::Active::Idle => b"idle",
            tasks::Active::Due | tasks::Active::Preparing => b"due",
            tasks::Active::Claimed { .. } | tasks::Active::Running { .. } => b"running",
            tasks::Active::BackingOff { .. } => b"backing off",
        },
        tasks::Phase::Closing(_) => b"closing",
        tasks::Phase::Held { .. } => b"held",
        tasks::Phase::Ended(_) => b"ended",
    }
}

fn delegates_read(delegates: &[tasks::DelegateState], bytes: u32) -> TaskBriefRead {
    let mut total = 0_usize;
    for delegate in delegates {
        total = total
            .checked_add(b"delegate ".len())
            .expect("bounded delegate text")
            .checked_add(Decimal::of(delegate.task).as_bytes().len())
            .expect("bounded delegate ID")
            .checked_add(b": ".len())
            .expect("bounded delegate text")
            .checked_add(delegate_phase(&delegate.phase).len())
            .expect("bounded delegate phase")
            .checked_add(1)
            .expect("delegate newline");
    }
    let mut writer = Writer::new(total);
    for delegate in delegates {
        writer.put(b"delegate ").expect("measured delegate text");
        writer.put(Decimal::of(delegate.task).as_bytes()).expect("measured delegate ID");
        writer.put(b": ").expect("measured delegate text");
        writer.put(delegate_phase(&delegate.phase)).expect("measured delegate phase");
        writer.put(b"\n").expect("measured delegate newline");
    }
    let text = writer.finish();
    TaskBriefRead::Got(Box::new([text_part(&text, b"", b"", b"", bytes)]))
}

fn result_label(kind: tasks::ResultKind) -> &'static [u8] {
    match kind {
        tasks::ResultKind::Report => b"report",
        tasks::ResultKind::Verdict { .. } => b"verdict",
        tasks::ResultKind::Change { .. } => b"change",
        tasks::ResultKind::Failed => b"failed",
        tasks::ResultKind::Cancelled => b"cancelled",
    }
}

fn dependency_read(results: &[HistoricalResult], bytes: u32) -> TaskBriefRead {
    let mut total = 0_usize;
    for result in results {
        total = total
            .checked_add(b"task ".len())
            .expect("bounded result text")
            .checked_add(Decimal::of(result.task).as_bytes().len())
            .expect("bounded result ID")
            .checked_add(b": ".len())
            .expect("bounded result text")
            .checked_add(result_label(result.kind).len())
            .expect("bounded result kind")
            .checked_add(result.words.len())
            .expect("bounded result words")
            .checked_add(3)
            .expect("result separators");
        match result.kind {
            tasks::ResultKind::Verdict { code } => {
                total = total
                    .checked_add(Decimal::of(u64::from(code)).as_bytes().len())
                    .expect("bounded verdict code")
                    .checked_add(1)
                    .expect("verdict space");
            }
            tasks::ResultKind::Report
            | tasks::ResultKind::Change { .. }
            | tasks::ResultKind::Failed
            | tasks::ResultKind::Cancelled => {}
        }
    }
    let mut writer = Writer::new(total);
    for result in results {
        writer.put(b"task ").expect("measured result text");
        writer.put(Decimal::of(result.task).as_bytes()).expect("measured result ID");
        writer.put(b": ").expect("measured result text");
        writer.put(result_label(result.kind)).expect("measured result kind");
        match result.kind {
            tasks::ResultKind::Verdict { code } => {
                writer.put(b" ").expect("measured verdict space");
                writer.put(Decimal::of(u64::from(code)).as_bytes()).expect("measured verdict code");
            }
            tasks::ResultKind::Report
            | tasks::ResultKind::Change { .. }
            | tasks::ResultKind::Failed
            | tasks::ResultKind::Cancelled => {}
        }
        writer.put(b"\n").expect("measured result newline");
        writer.put(&result.words).expect("measured result words");
        writer.put(b"\n\n").expect("measured result separator");
    }
    let text = writer.finish();
    TaskBriefRead::Got(Box::new([text_part(&text, b"", b"", b"", bytes)]))
}

fn tail_read(transcript: &Transcript, bytes: u32) -> TaskBriefRead {
    let kept = u64::from(bytes).min(transcript.kept);
    let skip = transcript.kept.checked_sub(kept).expect("tail within kept bytes");
    let mut writer = Writer::new(usize::try_from(kept).expect("u32 bound fits usize"));
    let mut passed = 0_u64;
    for turn in &transcript.turns {
        let end = passed.checked_add(u64::try_from(turn.len()).expect("bounded turn")).expect("bounded retained tail");
        if end > skip {
            let from = usize::try_from(skip.saturating_sub(passed)).expect("bounded offset");
            writer.put(turn.get(from..).expect("tail starts inside retained turn")).expect("tail fits chosen budget");
        }
        passed = end;
    }
    let text = writer.finish();
    TaskBriefRead::Got(Box::new([TaskBriefFragment {
        left: transcript.bytes.saturating_sub(u64::try_from(text.len()).expect("bounded tail")),
        bytes: text,
    }]))
}

fn add_text_len(total: &mut usize, fragment: &[u8]) {
    *total = total.checked_add(fragment.len()).expect("bounded brief text");
}

fn contract_text(contract: &tasks::Contract) -> Box<[u8]> {
    let mut room = 0_usize;
    match contract {
        tasks::Contract::Report { words } => {
            for fragment in [b"[Report: at most ".as_slice(), Decimal::of(u64::from(*words)).as_bytes(), b" bytes]\n"] {
                add_text_len(&mut room, fragment);
            }
        }
        tasks::Contract::Verdict { choices } => {
            add_text_len(&mut room, b"[Verdict choices:\n");
            for choice in choices {
                for fragment in [
                    b"  ".as_slice(),
                    Decimal::of(u64::from(choice.code)).as_bytes(),
                    b": at most ",
                    Decimal::of(u64::from(choice.words)).as_bytes(),
                    b" bytes\n",
                ] {
                    add_text_len(&mut room, fragment);
                }
            }
            add_text_len(&mut room, b"]\n");
        }
        tasks::Contract::Change { connector, kind, words } => {
            for fragment in [
                b"[Change connector ".as_slice(),
                Decimal::of(u64::from(*connector)).as_bytes(),
                b", kind ",
                Decimal::of(u64::from(*kind)).as_bytes(),
                b", at most ",
                Decimal::of(u64::from(*words)).as_bytes(),
                b" bytes]\n",
            ] {
                add_text_len(&mut room, fragment);
            }
        }
    }
    let mut writer = Writer::new(room);
    match contract {
        tasks::Contract::Report { words } => {
            writer.put(b"[Report: at most ").expect("contract text room");
            writer.put(Decimal::of(u64::from(*words)).as_bytes()).expect("contract text room");
            writer.put(b" bytes]\n").expect("contract text room");
        }
        tasks::Contract::Verdict { choices } => {
            writer.put(b"[Verdict choices:\n").expect("contract text room");
            for choice in choices {
                writer.put(b"  ").expect("contract text room");
                writer.put(Decimal::of(u64::from(choice.code)).as_bytes()).expect("contract text room");
                writer.put(b": at most ").expect("contract text room");
                writer.put(Decimal::of(u64::from(choice.words)).as_bytes()).expect("contract text room");
                writer.put(b" bytes\n").expect("contract text room");
            }
            writer.put(b"]\n").expect("contract text room");
        }
        tasks::Contract::Change { connector, kind, words } => {
            writer.put(b"[Change connector ").expect("contract text room");
            writer.put(Decimal::of(u64::from(*connector)).as_bytes()).expect("contract text room");
            writer.put(b", kind ").expect("contract text room");
            writer.put(Decimal::of(u64::from(*kind)).as_bytes()).expect("contract text room");
            writer.put(b", at most ").expect("contract text room");
            writer.put(Decimal::of(u64::from(*words)).as_bytes()).expect("contract text room");
            writer.put(b" bytes]\n").expect("contract text room");
        }
    }
    writer.finish()
}

/// Render an agent task's spec and typed contract from its activation snapshot.
fn task_read(record: &tasks::RunContext, parts: u32, bytes: u32) -> TaskBriefRead {
    if parts == 0 {
        return TaskBriefRead::Failed;
    }
    let contract = contract_text(&record.contract);
    let first = text_part(&record.spec.words, b"\n", &contract, b"", bytes);
    let remaining =
        bytes.checked_sub(u32::try_from(first.bytes.len()).expect("bounded task part")).expect("part within read");
    let part = match record.requester {
        tasks::Party::Person(person) => {
            let person = Decimal::of(person);
            text_part(b"[Requested by person ", person.as_bytes(), b"]\n", b"", remaining)
        }
        tasks::Party::Task(task) => {
            let task = Decimal::of(task);
            text_part(b"[Requested by task ", task.as_bytes(), b"]\n", b"", remaining)
        }
        tasks::Party::Deployment { project } => {
            let project = Decimal::of(u64::from(project));
            text_part(b"[Requested by deployment for project ", project.as_bytes(), b"]\n", b"", remaining)
        }
    };
    let mut gathered = List::with_capacity(parts);
    gathered.push(first).expect("positive part room");
    if parts > 1 {
        gathered.push(part).expect("requester part room");
    } else {
        let last = gathered.get_mut(0).expect("first part");
        last.left = last
            .left
            .checked_add(u64::try_from(part.bytes.len()).expect("usize fits u64"))
            .expect("bounded omitted bytes")
            .checked_add(part.left)
            .expect("bounded omitted bytes");
    }
    TaskBriefRead::Got(gathered.into_boxed())
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
        | Startup::Adopting
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

fn remember_unpriced_terminal(domain: &mut Domain, run: Token, attempt: Token, end: tasks::End) {
    domain.core.remember_unpriced_terminal(run, attempt, end);
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
            policy::restore(domain, project, value);
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
