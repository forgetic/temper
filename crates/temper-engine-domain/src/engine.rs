//! The concrete 06a root: bounded child ownership and synchronous decision
//! routing for a charged person chat (domain/engine.md, sections 3–7).
//! [`Domain`] keeps task/funding state in tasks, policy in authority, identities
//! and keyed requests in people, placement in fleet, task text gathering in
//! brief and secret-free credential lifetimes in accounts. Its own state is
//! commits, loads, candidate numbers, bounded current-claim replay evidence and
//! unfinished handoffs between children.
//!
//! The shell/protocol supplies [`Event`]s to [`step`], drains durable effects
//! through [`resume`], fires child timers through [`fire`] and reclaims at the
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
mod proposals;
mod results;

mod roles;

use crate::{
    CallAnswer, CallKey, Decision, Delivery, Family, Journal, JournalLimits, Key, Output, Range, Record, RunProof,
    TerminalRecord, TurnProof, TurnRecord, Write, loads,
};
use alloc::boxed::Box;
use skein_lib::{Decimal, Env, Id, List, Map, Queue, ReplyTo, Slab, Token, Writer};
use temper_engine_domain_accounts as accounts;
use temper_engine_domain_authority as authority;
use temper_engine_domain_brief as brief;
use temper_engine_domain_fleet as fleet;
use temper_engine_domain_people as people;
use temper_engine_domain_tasks as tasks;

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
    pub brief: brief::Limits,
    /// Secret-free credential policy.
    pub accounts: accounts::Limits,
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
    /// Validated deployment and project policy; no duplicated policy values.
    pub authority: authority::Domain,
    /// Charter selected for chats; admitted by tasks as the configured executor.
    pub charter: u32,
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
    /// Owned task, attempt and transcript-tail sections, at most brief `sections` and
    /// `brief_bytes`. Task/deployment lineage awaits its root route.
    pub sections: Box<[brief::Section]>,
    /// Whole unread words offered to this attempt, oldest first.
    pub inbox: Box<[tasks::Word]>,
    /// Writable repository tags whose workspace starts at the task's saved-work branch.
    pub saved: Box<[u32]>,
    /// Ordered opaque committed turn bodies of this task, empty for a fresh run.
    pub transcript: Box<[Box<[u8]>]>,
    /// Named answers committed after the task's last accepted turn. The
    /// worker gives these to the new attempt before it can decide new calls.
    pub answered: Box<[crate::CallRecord]>,
    /// Secret-free account grant; token bytes stay in the protocol.
    pub grant: accounts::Grant,
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
    /// Ask a covering ancestor or person to carry one action the caller cannot take alone.
    Propose { action: ProposedAction, reason: Box<[u8]>, as_holder: bool },
    /// Decide one pending proposal currently addressed to this task.
    Decide { proposer: u64, proposal: u64, decision: ProposalChoice },
    /// Withdraw this task's still-pending proposal.
    Withdraw { proposal: u64 },
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
    pub dependencies: Box<[Dependency]>,
    pub wake: tasks::WakePolicy,
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
    People(people::Event),
    Fleet(fleet::Event),
    Brief(brief::Event),
    Activate(Box<tasks::RunContext>),
    EscalationLoaded { waiter: Token, rows: Box<[Record]> },
    EscalationFailed { waiter: Token },
    DelegateValidated { to: Token, key: CallKey, batch: Box<[Delegate]> },
    DelegateInputRefused { to: Token, key: CallKey },
}

#[derive(Debug)]
enum Payload {
    Call { key: CallKey, body: Call },
    CallAnswer(CallAnswer),
    Turn { task: u64, attempt: u64, body: Turn },
    Answer { task: u64, attempt: u64, cumulative: u64, end: tasks::End, saved: Option<Box<[u32]>> },
}

#[derive(Debug)]
enum Read {
    Result(results::Read),
    Escalation(escalation::Query),
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
}

#[derive(Debug)]
struct HistoricalResult {
    task: u64,
    kind: tasks::ResultKind,
    words: Box<[u8]>,
}

#[derive(Debug)]
struct DependencyRead {
    task: u64,
    ids: Box<[u64]>,
    at: u32,
    results: List<HistoricalResult>,
}

/// Bounded recent opaque conversation while a due task's store pages are read.
/// Empty turn bodies need no slot: they add nothing to a resumed conversation.
#[derive(Debug)]
struct Transcript {
    previous_attempt: u64,
    bytes: u64,
    kept: u64,
    turns: Queue<Box<[u8]>>,
}

#[derive(Debug)]
struct PendingRelay {
    previous: Option<u64>,
    word: tasks::Word,
}

#[derive(Debug)]
struct ResultPage {
    waiter: Token,
    rows: Box<[Record]>,
    next: Option<Key>,
}

#[derive(Debug)]
struct RestoringProof {
    attempt: u64,
    turn: u32,
    run_spent: u64,
    last_answer: Option<u64>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RoutedCall {
    Propose { key: CallKey, proposal: u64 },
    Decide { key: CallKey, proposal: u64 },
    Withdraw { key: CallKey, proposal: u64 },
    Accepting { key: CallKey, proposer: u64, proposal: u64, message: u64 },
    Message(CallKey),
    Introduce(CallKey),
    Subscribe { key: CallKey, subscription: u64 },
    Unsubscribe(CallKey),
    Control(CallKey),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PersonProposalRoute {
    Deciding { request: Token, proposer: u64, proposal: u64 },
    Accepting { request: Token, person: u64, proposer: u64, proposal: u64, message: u64 },
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
    config: Config,
    journal: Journal,
    startup: Startup,
    tasks: tasks::Domain,
    people: people::Domain,
    fleet: fleet::Domain,
    brief: brief::Domain,
    accounts: accounts::Domain,
    loads: loads::Loads,
    assignments: Map<u64, Assignment>,
    payloads: Slab<Option<Payload>>,
    result_reads: Slab<Option<Read>>,
    reading_results: Map<u64, Token>,
    result_pages: Queue<ResultPage>,
    dependency_results: Map<u64, Box<[HistoricalResult]>>,
    made: Map<Token, u64>,
    delegating: Map<Token, CallKey>,
    routing_calls: Map<Token, RoutedCall>,
    routing_people_proposals: Map<Token, PersonProposalRoute>,
    ending_positions: Map<u64, u64>,
    saying: Map<Token, u64>,
    relaying: Option<PendingRelay>,
    claiming: Map<u64, u64>,
    contexts: Map<u64, Box<tasks::RunContext>>,
    transcripts: Map<u64, Transcript>,
    proofs: Map<u64, RunProof>,
    calls: Map<CallKey, CallAnswer>,
    pending_calls: Map<CallKey, bool>,
    restoring_proofs: Map<u64, RestoringProof>,
    work: Queue<Work>,
    before_header: Queue<Work>,
    cold_channels: Map<Token, bool>,
    adopted: Queue<fleet::Event>,
    due: Queue<Box<tasks::RunContext>>,
    signing_in: Option<u64>,
    projects: List<u32>,
}

impl Domain {
    /// Allocate the root and fixed child room from shell-supplied configuration and validated
    /// cross-child limits. Checks bounded authority/bootstrap configuration; issues no request.
    /// Startup pages/account setup begin only on `Event::Start`.
    #[must_use]
    pub fn new(mut config: Config, limits: &Limits) -> Domain {
        assert!(worst_case(limits).is_some(), "root limits are valid");
        assert!(
            config.resume_bytes > 0 && config.resume_bytes <= limits.journal.transcript_bytes,
            "configured task transcript bound fits the root's owned-byte limit"
        );
        assert!(*config.authority.limits() == limits.authority, "root prices its exact authority limits");
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
        let owners = core::mem::replace(&mut config.owners, Box::new([]));
        let tasks = tasks::Domain::new(&limits.tasks, config.seed, Box::new([config.charter]));
        let people = people::Domain::new(&limits.people, owners);
        Domain {
            journal: Journal::bootstrap(config.deployment, &limits.journal),
            startup: Startup::Cold,
            tasks,
            people,
            fleet: fleet::Domain::new(&limits.fleet),
            brief: brief::Domain::new(&limits.brief),
            accounts: accounts::Domain::new(&limits.accounts),
            loads: loads::Loads::new(&limits.loads),
            assignments: Map::with_capacity(limits.tasks.tasks),
            payloads: Slab::with_capacity(payload_slots(limits).expect("valid payload room")),
            result_reads: Slab::with_capacity(limits.loads.loads),
            reading_results: Map::with_capacity(limits.loads.loads),
            result_pages: Queue::with_capacity(limits.loads.loads),
            dependency_results: Map::with_capacity(limits.tasks.tasks),
            made: Map::with_capacity(limits.people.pending),
            delegating: Map::with_capacity(limits.fleet.calls),
            routing_calls: Map::with_capacity(limits.fleet.calls),
            routing_people_proposals: Map::with_capacity(limits.people.pending),
            ending_positions: Map::with_capacity(limits.tasks.tasks),
            saying: Map::with_capacity(limits.people.pending),
            relaying: None,
            claiming: Map::with_capacity(limits.tasks.tasks),
            contexts: Map::with_capacity(limits.tasks.tasks),
            transcripts: Map::with_capacity(limits.tasks.tasks),
            proofs: Map::with_capacity(limits.tasks.tasks),
            calls: Map::with_capacity(limits.call_records),
            pending_calls: Map::with_capacity(limits.call_records),
            restoring_proofs: Map::with_capacity(limits.tasks.tasks),
            work: Queue::with_capacity(route_bound(limits).expect("valid routes")),
            before_header: Queue::with_capacity(limits.fleet.workers),
            cold_channels: Map::with_capacity(limits.fleet.workers),
            adopted: Queue::with_capacity(limits.tasks.tasks.checked_mul(2).expect("valid adoption room")),
            due: Queue::with_capacity(limits.tasks.tasks),
            signing_in: None,
            projects,
            config,
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
            && self.journal.quiescent()
            && self.loads.quiescent()
            && self.work.is_empty()
            && self.before_header.is_empty()
            && self.cold_channels.is_empty()
            && self.adopted.is_empty()
            && self.due.is_empty()
            && self.assignments.is_empty()
            && self.payloads.is_empty()
            && self.result_reads.is_empty()
            && self.reading_results.is_empty()
            && self.result_pages.is_empty()
            && self.dependency_results.is_empty()
            && self.pending_calls.is_empty()
            && self.routing_calls.is_empty()
            && self.routing_people_proposals.is_empty()
            && self.made.is_empty()
            && self.delegating.is_empty()
            && self.ending_positions.is_empty()
            && self.saying.is_empty()
            && self.relaying.is_none()
            && self.claiming.is_empty()
            && self.contexts.is_empty()
            && self.transcripts.is_empty()
            && self.restoring_proofs.is_empty()
            && self.signing_in.is_none()
            && self.brief.briefs() == 0
            && self.brief.reads() == 0
            && !self.fleet.is_ready()
            && self.fleet.turns() == 0
            && self.fleet.calls() == 0
            && self.fleet.next_deadline().is_none()
            && !self.accounts.waiting()
    }

    /// Pure snapshot query for the latest allocated deployment counters; these may run ahead of
    /// store durability. Exposes neither children nor owned handoff bodies and emits no effect.
    #[must_use]
    pub fn deployment(&self) -> crate::Deployment {
        self.journal.deployment()
    }

    /// Retired IO/body/child slots are reclaimed at iteration end, after every event and ready
    /// pass. Bounded child/slab bookkeeping releases only retired entries; issued loads still
    /// awaiting a terminal remain owned. Emits no request or terminal.
    pub fn reclaim(&mut self) {
        self.tasks.reclaim();
        self.people.reclaim();
        self.fleet.reclaim();
        self.brief.reclaim();
        self.payloads.reclaim();
        self.result_reads.reclaim();
        loads::reclaim(&mut self.loads);
    }

    /// Shell/root caller discards every currently queued child observation, scanning at most each
    /// child's configured fact capacity. Emits no effect or terminal and changes no decision state.
    pub fn drain_facts(&mut self) {
        for _ in 0..self.limits.tasks.facts {
            let _fact = self.tasks.pop_fact();
        }
        for _ in 0..self.limits.people.facts {
            let _fact = self.people.pop_fact();
        }
        for _ in 0..self.limits.fleet.facts {
            let _fact = self.fleet.pop_fact();
        }
        for _ in 0..self.limits.brief.facts {
            let _fact = self.brief.pop_fact();
        }
        for _ in 0..self.limits.accounts.facts {
            let _fact = self.accounts.pop_fact();
        }
    }
}

/// One outward commit, or one ready delivery, or a bounded account step; reserve before every
/// `step`, `resume` and `fire` call. Pure constant query, four output slots; emits no effect or
/// terminal.
#[must_use]
pub const fn max_out(_limits: &Limits) -> u32 {
    4
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

fn environment_brief(env: &Env<Limits>) -> Env<brief::Limits> {
    Env { now: env.now, wall: env.wall, limits: env.limits.brief }
}

fn environment_accounts(env: &Env<Limits>) -> Env<accounts::Limits> {
    Env { now: env.now, wall: env.wall, limits: env.limits.accounts }
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
/// are accepted even under pressure. The shell reserves `max_out` output slots and supplies
/// unchanged configured limits with injected time. Refused web/worker calls get terminal/busy
/// notices before child mutation; admitted effects may wait in the journal until store durability.
/// Account operations keep their own secret-free terminal contract.
#[expect(
    clippy::too_many_lines,
    reason = "one exhaustive admission match keeps every input before a single decision close"
)]
pub fn step(domain: &mut Domain, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    assert!(domain.limits == env.limits, "root uses configured limits");
    assert!(out.room() >= max_out(&env.limits), "root output room reserved");
    if domain.journal.stopped() || domain.startup == Startup::Failed {
        discard_after_stop(domain, event);
        return;
    }
    match event {
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
                    account: domain.config.account,
                    generation: domain.config.account_generation,
                    valid: domain.config.account_valid,
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
            if domain.journal.deployment().people == u64::MAX || domain.journal.deployment().sign_ins == u64::MAX {
                out.push(Request::Deliver(Delivery::WebReply {
                    to: reply_to,
                    sign_in: None,
                    reply: people::Reply::Refused(people::Refusal::Limit),
                }));
                return;
            }
            let person = crate::fresh(&mut domain.journal, Family::Person).expect("person counter available");
            let sign_in = crate::fresh(&mut domain.journal, Family::SignIn).expect("sign-in counter available");
            domain.signing_in = Some(sign_in);
            domain.work.push(Work::People(people::Event::SignedIn { reply_to, person, sign_in, identity }));
        }
        Event::Ask { reply_to, sign_in, key, ask } => {
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
                || domain.fleet.calls() >= env.limits.fleet.calls
                || domain.pending_calls.contains_key(&key)
                || (call_needs_input(&body.tool) && domain.result_reads.len() >= domain.result_reads.capacity())
                || (!domain.calls.contains_key(&key)
                    && domain.calls.len().saturating_add(domain.pending_calls.len()) >= env.limits.call_records)
            {
                out.push(Request::CallBusy { channel, task, attempt, call });
                return;
            }
            if let Some(why) = call_shape(&body.tool, &env.limits.tasks) {
                body.tool = match body.tool {
                    Tool::Message { .. } => Tool::RejectedMessage(why),
                    Tool::Amend { .. } | Tool::Cancel { .. } | Tool::Release { .. } => Tool::RejectedControl(why),
                    Tool::Propose { .. } | Tool::Decide { .. } | Tool::Withdraw { .. } => Tool::RejectedProposal(why),
                    Tool::Delegate { .. }
                    | Tool::Introduce { .. }
                    | Tool::Subscribe { .. }
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
                || !tasks_saved_within(saved.as_deref(), env.limits.tasks.saved_repositories)
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
    }
    let decision = route(domain, env);
    domain.signing_in = None;
    close(domain, env, decision, out);
}

fn admits(domain: &Domain, limits: &Limits) -> bool {
    crate::takes(&domain.journal, &limits.journal)
        && domain.journal.deployment().messages
            <= u64::MAX.checked_sub(u64::from(limits.tasks.tasks)).expect("task count fits u64")
        && domain.work.is_empty()
        && domain.journal.held_room() >= limits.journal.deliveries.checked_mul(3).expect("root held reserve bounded")
}

fn remember_due(domain: &mut Domain, context: Box<tasks::RunContext>) {
    for task in &domain.due {
        if task.task == context.task {
            return;
        }
    }
    domain.due.push(context);
}

fn lose_channel(domain: &mut Domain, env: &Env<Limits>, channel: Token) {
    let mut fleet_out = Queue::with_capacity(fleet::max_out(&env.limits.fleet));
    fleet::step(&mut domain.fleet, &environment_fleet(env), fleet::Event::Lost { channel }, &mut fleet_out);
    assert!(fleet_out.is_empty(), "loss changes topology and deadlines without child effects");
}

fn close(domain: &mut Domain, env: &Env<Limits>, decision: Decision, out: &mut Queue<Request>) {
    let mut journal_out = Queue::with_capacity(1);
    crate::accept(&mut domain.journal, &env.limits.journal, decision, &mut journal_out)
        .expect("root pressure reserved before child mutation");
    journal_outputs(&mut journal_out, out);
}

fn journal_outputs(journal_out: &mut Queue<Output>, out: &mut Queue<Request>) {
    for _ in 0..journal_out.len() {
        match journal_out.pop().expect("journal output count") {
            Output::Commit { number, writes } => out.push(Request::Commit { number, writes }),
            Output::Stop => out.push(Request::Stop),
            Output::Deliver(delivery) => out.push(Request::Deliver(delivery)),
        }
    }
}

/// Release one held effect after durability, preserving internal callbacks while journal pressure
/// is full. The shell reserves `max_out` output slots. Deferred callbacks run before another held
/// callback is consumed; other ready work may produce one commit. This pass never waits for IO or
/// drops a retained callback on pressure
#[expect(clippy::too_many_lines, reason = "root release path routes each held delivery exhaustively")]
pub fn resume(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    assert!(out.room() >= max_out(&env.limits), "root ready output room");
    if domain.journal.stopped() || domain.startup == Startup::Failed {
        return;
    }
    if !domain.result_pages.is_empty() && crate::takes(&domain.journal, &env.limits.journal) {
        let page = domain.result_pages.pop().expect("pending result page");
        results::page(domain, env, page.waiter, page.rows, page.next, out);
        return;
    }
    if !domain.work.is_empty() {
        if crate::takes(&domain.journal, &env.limits.journal) {
            let decision = route(domain, env);
            close(domain, env, decision, out);
        }
        return;
    }
    let mut journal_out = Queue::with_capacity(1);
    crate::resume(&mut domain.journal, &mut journal_out);
    if let Some(output) = journal_out.pop() {
        match output {
            Output::Deliver(Delivery::Fleet(event)) => domain.work.push(Work::Fleet(event)),
            Output::Deliver(Delivery::Relay { task, attempt, previous, word }) => {
                let event = Token::new(word.number);
                assert!(domain.relaying.replace(PendingRelay { previous, word }).is_none(), "one relay at a time");
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
                    | Read::Transcript { .. }
                    | Read::Dependency(_)
                    | Read::InputCheck(_)
                    | Read::Escalation(escalation::Query::Read { .. }) => {
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
                    Read::Escalation(_) | Read::Transcript { .. } | Read::Dependency(_) | Read::InputCheck(_) => {
                        unreachable!("result waiter")
                    }
                }
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
    if !crate::takes(&domain.journal, &env.limits.journal) {
        return;
    }
    if !domain.work.is_empty() {
        let decision = route(domain, env);
        close(domain, env, decision, out);
        return;
    }
    if domain.ready() {
        if domain.accounts.usable(domain.config.account) && !domain.due.is_empty() {
            for _ in 0..domain.due.len() {
                domain.work.push(Work::Activate(domain.due.pop().expect("waiting activation")));
            }
            let decision = route(domain, env);
            close(domain, env, decision, out);
            return;
        }
        let mut fleet_out = Queue::with_capacity(fleet::max_out(&env.limits.fleet));
        fleet::resume(&mut domain.fleet, &environment_fleet(env), &mut fleet_out);
        let mut decision = Decision::new(&env.limits.journal);
        fleet_outputs(domain, env, &mut decision, &mut fleet_out);
        route_into(domain, env, &mut decision);
        close(domain, env, decision, out);
    }
}

/// Fire participating child timers through the same barrier; store durability and inputs run before
/// this pass. The shell reserves `max_out` slots and supplies injected monotonic/wall time. Account
/// timers may emit bounded protocol actions independently; root routes that mutate tasks/fleet wait
/// for whole-decision admission. Their store and account outcomes enter later through `step`.
pub fn fire(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    if domain.journal.stopped() || domain.startup == Startup::Failed {
        return;
    }
    account_fire(domain, env, out);
    if !domain.ready() || !admits(domain, &env.limits) {
        return;
    }
    let mut decision = Decision::new(&env.limits.journal);
    let mut tasks_out = Queue::with_capacity(tasks::max_out(&env.limits.tasks));
    tasks::fire(&mut domain.tasks, &environment_tasks(env), &mut tasks_out);
    tasks_outputs(domain, env, &mut decision, &mut tasks_out, false);
    let mut fleet_out = Queue::with_capacity(fleet::max_out(&env.limits.fleet));
    fleet::fire(&mut domain.fleet, &environment_fleet(env), &mut fleet_out);
    fleet_outputs(domain, env, &mut decision, &mut fleet_out);
    let mut brief_out = Queue::with_capacity(brief::max_out(&env.limits.brief));
    brief::fire(&mut domain.brief, &environment_brief(env), &mut brief_out);
    brief_outputs(domain, env, &mut decision, &mut brief_out);
    route_into(domain, env, &mut decision);
    close(domain, env, decision, out);
}

fn route(domain: &mut Domain, env: &Env<Limits>) -> Decision {
    let mut decision = Decision::new(&env.limits.journal);
    route_into(domain, env, &mut decision);
    decision
}

fn route_into(domain: &mut Domain, env: &Env<Limits>, decision: &mut Decision) {
    for _ in 0..route_bound(&env.limits).expect("valid route bound") {
        let Some(work) = domain.work.pop() else {
            break;
        };
        match work {
            Work::Tasks(event) => {
                let mut out = Queue::with_capacity(tasks::max_out(&env.limits.tasks));
                tasks::step(&mut domain.tasks, &environment_tasks(env), event, &mut out);
                tasks_outputs(domain, env, decision, &mut out, false);
            }
            Work::PersonProposal(event) => {
                let mut out = Queue::with_capacity(tasks::max_out(&env.limits.tasks));
                tasks::step(&mut domain.tasks, &environment_tasks(env), event, &mut out);
                tasks_outputs(domain, env, decision, &mut out, true);
            }
            Work::People(event) => {
                let mut out = Queue::with_capacity(people::max_out(&env.limits.people));
                people::step(&mut domain.people, &environment_people(env), event, &mut out);
                people_outputs(domain, env, decision, &mut out);
            }
            Work::Fleet(event) => {
                let mut out = Queue::with_capacity(fleet::max_out(&env.limits.fleet));
                fleet::step(&mut domain.fleet, &environment_fleet(env), event, &mut out);
                fleet_outputs(domain, env, decision, &mut out);
            }
            Work::Brief(event) => {
                let mut out = Queue::with_capacity(brief::max_out(&env.limits.brief));
                brief::step(&mut domain.brief, &environment_brief(env), event, &mut out);
                brief_outputs(domain, env, decision, &mut out);
            }
            Work::Activate(task) => activate(domain, env, decision, task),
            Work::EscalationLoaded { waiter, rows } => escalation::loaded(domain, env, waiter, rows),
            Work::EscalationFailed { waiter } => escalation::failed(domain, waiter),
            Work::DelegateValidated { to, key, batch } => {
                delegate_call(domain, env, decision, ReplyTo::new(to), key, batch, true);
            }
            Work::DelegateInputRefused { to, key } => decide_call(
                domain,
                &env.limits,
                decision,
                ReplyTo::new(to),
                key,
                CallAnswer::DelegationRefused(tasks::Problem { task: Some(key.task), why: tasks::Refusal::Inputs }),
            ),
        }
    }
    assert!(domain.work.is_empty(), "finite synchronous root handoffs finish within the configured route bound");
}

fn append_word(existing: &[tasks::Word], word: &tasks::Word, capacity: u32) -> Box<[tasks::Word]> {
    let mut words = List::with_capacity(capacity);
    for item in existing {
        words.push(item.clone()).expect("accepted inbox count");
    }
    words.push(word.clone()).expect("accepted inbox count");
    words.into_boxed()
}

fn people_outputs(domain: &mut Domain, env: &Env<Limits>, decision: &mut Decision, out: &mut Queue<people::Request>) {
    for _ in 0..out.len() {
        match out.pop().expect("people output count") {
            people::Request::Save { record } => save(decision, &env.limits, Write::Save(Record::People(record))),
            people::Request::Erase { key } => save(decision, &env.limits, Write::Erase(Key::People(key))),
            people::Request::Reply { to, reply } => {
                emit(decision, &env.limits, Delivery::WebReply { to, sign_in: domain.signing_in, reply });
            }
            people::Request::Route { request, person, project, role, ask } => match ask {
                people::Ask::Say { task, words, .. } => {
                    let Some(message) = crate::fresh(&mut domain.journal, Family::Message) else {
                        domain.work.push(Work::People(people::Event::Decided {
                            request,
                            outcome: people::Outcome::Refused(people::Refusal::Limit),
                        }));
                        continue;
                    };
                    assert!(domain.saying.insert(request, task) == Ok(None), "one pending say flight");
                    domain.work.push(Work::Tasks(tasks::Event::Message {
                        reply_to: ReplyTo::new(request),
                        project,
                        task,
                        word: tasks::Word {
                            number: message,
                            from: tasks::Party::Person(person),
                            kind: tasks::MessageKind::Words,
                            words,
                            at: env.wall,
                            hits: 1,
                            eligible: false,
                        },
                    }));
                }
                people::Ask::StartChat { .. } => {
                    let Some(role) = role else { unreachable!("chat membership admitted") };
                    make_chat(domain, env, request, person, project, role, ask);
                }
                people::Ask::SetRoles { holdings, .. } => {
                    roles::begin(domain, env, decision, request, person, project, holdings);
                }
                people::Ask::DecideEscalation { task, revision, decision, .. } => {
                    escalation::begin(domain, request, person, role, project, task, revision, decision);
                }
                people::Ask::DecideProposal { proposer, proposal, decision: choice, .. } => {
                    proposals::person_decide(
                        domain, env, decision, request, person, role, project, proposer, proposal, choice,
                    );
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

#[expect(clippy::too_many_lines, reason = "chat admission keeps the keyed person request and task creation together")]
fn make_chat(
    domain: &mut Domain,
    env: &Env<Limits>,
    request: Token,
    person: u64,
    project: u32,
    role: people::Role,
    ask: people::Ask,
) {
    let role_number = escalation::role_number(role);
    let pool = tasks::Funder::Pool { project, person, period: domain.config.period };
    let pool_numbers = match domain.tasks.funding(pool) {
        Some(record) => record.numbers,
        None => tasks::Numbers { budget: domain.config.person_budget, spent: 0, spent_below: 0, reserved: 0 },
    };
    let mut findings =
        Queue::with_capacity(authority::max_out(domain.config.authority.limits()).expect("authority check bound"));
    let checked = authority::check_request(
        &domain.config.authority,
        &authority::PersonAsk {
            project,
            role: role_number,
            pool: authority_numbers(pool_numbers),
            tasks_left: env.limits.tasks.tree_tasks,
            request: authority::PersonRequest::Create(Box::new([authority::Delegate {
                executor: authority::Executor::Charter(domain.config.charter),
                authority: domain.config.chat_authority.clone(),
            }])),
        },
        &mut findings,
    );
    if checked.answer != authority::Answer::Allow {
        domain.work.push(Work::People(people::Event::Decided {
            request,
            outcome: people::Outcome::Refused(people::Refusal::Authority),
        }));
        return;
    }
    let Some(policy) = domain.config.authority.policy(project) else { unreachable!("checked project policy") };
    let Some(role_policy) = domain.config.authority.role(project, role_number) else {
        unreachable!("checked role policy")
    };
    if match policy.escalation_role {
        Some(role) => role > 3,
        None => true,
    } || domain.config.period_budget > policy.period_spend
        || domain.config.person_budget > role_policy.period_spend
    {
        domain.work.push(Work::People(people::Event::Decided {
            request,
            outcome: people::Outcome::Refused(people::Refusal::Authority),
        }));
        return;
    }
    let period = tasks::Funder::Period { project, period: domain.config.period };
    if domain.tasks.funding(period).is_none() {
        domain.work.push(Work::Tasks(tasks::Event::OpenPeriod {
            reply_to: internal(0),
            project,
            period: domain.config.period,
            budget: domain.config.period_budget,
        }));
    }
    if domain.tasks.funding(pool).is_none() {
        domain.work.push(Work::Tasks(tasks::Event::CarvePool {
            reply_to: internal(0),
            project,
            person,
            period: domain.config.period,
            budget: domain.config.person_budget,
        }));
    }
    let Some(number) = crate::fresh(&mut domain.journal, Family::Task) else {
        domain.work.push(Work::People(people::Event::Decided {
            request,
            outcome: people::Outcome::Refused(people::Refusal::Limit),
        }));
        return;
    };
    assert!(domain.made.insert(request, number) == Ok(None), "people route has unique pending key");
    let words = match ask {
        people::Ask::StartChat { words, .. } => words,
        people::Ask::DecideEscalation { .. }
        | people::Ask::DecideProposal { .. }
        | people::Ask::SetRoles { .. }
        | people::Ask::Say { .. } => {
            unreachable!("other asks routed separately")
        }
    };
    domain.work.push(Work::Tasks(tasks::Event::Make {
        reply_to: ReplyTo::new(request),
        creator: tasks::Party::Person(person),
        batch: Box::new([tasks::New {
            number,
            project,
            executor: tasks::Executor::Agent { charter: domain.config.charter },
            spec: tasks::Spec { words, parameters: Box::new([]), inputs: Box::new([]) },
            contract: tasks::Contract::Report { words: env.limits.tasks.result_bytes },
            authority: task_authority(&domain.config.chat_authority),
            numbers: tasks::Numbers {
                budget: domain.config.chat_authority.budget.spend,
                spent: 0,
                spent_below: 0,
                reserved: 0,
            },
            funder: pool,
            dependencies: Box::new([]),
            wake: tasks::WakePolicy::DEFAULT,
        }]),
    }));
}

/// Consume the actual child activation context for authority/account readiness and the person-chat
/// brief; retain only credential waits and drop the context on claim/failure.
#[expect(clippy::too_many_lines, reason = "authority check and transcript preparation are one activation route")]
fn activate(domain: &mut Domain, env: &Env<Limits>, decision: &mut Decision, task: Box<tasks::RunContext>) {
    let number = task.task;
    if !domain.ready() {
        remember_due(domain, task);
        return;
    }
    let mut findings =
        Queue::with_capacity(authority::max_out(domain.config.authority.limits()).expect("authority check bound"));
    let checked = authority::check_run(
        &domain.config.authority,
        &authority::RunAsk {
            project: task.project,
            authority: authority_value(&task.authority),
            numbers: authority_numbers(task.numbers),
            budget: authority::left(authority_numbers(task.numbers)),
            wall: env.wall,
            accounts: Box::new([domain.accounts.usable(domain.config.account)]),
            writes: Box::new([]),
        },
        &mut findings,
    );
    match checked {
        authority::Answer::Allow => {}
        authority::Answer::Wait | authority::Answer::Propose | authority::Answer::Refuse => {
            let mut account = false;
            let mut hold = None;
            for _ in 0..findings.len() {
                match findings.pop().expect("authority finding count") {
                    authority::Finding::Account => account = true,
                    authority::Finding::Deadline => hold = Some(tasks::Hold::Deadline),
                    authority::Finding::RunBudget
                    | authority::Finding::RunCap
                    | authority::Finding::Arithmetic
                    | authority::Finding::Spend { .. } => {
                        if hold != Some(tasks::Hold::Deadline) {
                            hold = Some(tasks::Hold::Budget);
                        }
                    }
                    authority::Finding::Oversized
                    | authority::Finding::UnknownProject
                    | authority::Finding::UnknownRole
                    | authority::Finding::Authority { .. }
                    | authority::Finding::Executor { .. }
                    | authority::Finding::Tasks { .. }
                    | authority::Finding::Writer
                    | authority::Finding::Tool
                    | authority::Finding::Grant { .. }
                    | authority::Finding::Reference
                    | authority::Finding::Scope { .. }
                    | authority::Finding::Required { .. }
                    | authority::Finding::Failed { .. }
                    | authority::Finding::Unpermitted
                    | authority::Finding::Undecidable
                    | authority::Finding::PeriodSpend
                    | authority::Finding::LandingMissing
                    | authority::Finding::LandingPin
                    | authority::Finding::Ci { .. }
                    | authority::Finding::Behind { .. }
                    | authority::Finding::Gate { .. }
                    | authority::Finding::Approval { .. }
                    | authority::Finding::ReviewFailed { .. } => {
                        if hold.is_none() {
                            hold = Some(tasks::Hold::Effects);
                        }
                    }
                }
            }
            if let Some(why) = hold {
                domain.work.push(Work::Tasks(tasks::Event::Hold { task: number, why }));
            } else if account {
                remember_due(domain, task);
            } else {
                domain.work.push(Work::Tasks(tasks::Event::Hold { task: number, why: tasks::Hold::Effects }));
            }
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
    assert!(domain.contexts.insert(number, task).is_ok(), "bounded activation context");
    assert!(
        domain
            .transcripts
            .insert(
                number,
                Transcript {
                    previous_attempt,
                    bytes: 0,
                    kept: 0,
                    turns: Queue::with_capacity(domain.config.resume_bytes),
                }
            )
            .is_ok(),
        "one transcript preparation per live task"
    );
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
    let mut retired = List::with_capacity(limits.call_records);
    for (&key, _) in &domain.calls {
        if key.task == task && (key.attempt < attempt || (key.attempt == attempt && key.completion < turn)) {
            retired.push(key).expect("all retained call names fit their configured bound");
        }
    }
    for &key in &retired {
        let _answer = domain.calls.remove(&key);
        save(decision, limits, Write::Erase(Key::Call(key)));
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
        | Tool::Unsubscribe { .. }
        | Tool::Cancel { .. }
        | Tool::Release { .. }
        | Tool::Amend { .. }
        | Tool::Propose { .. }
        | Tool::Decide { .. }
        | Tool::Withdraw { .. } => false,
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
        Tool::Withdraw { .. }
        | Tool::Unavailable
        | Tool::Rejected(_)
        | Tool::RejectedMessage(_)
        | Tool::RejectedControl(_)
        | Tool::RejectedProposal(_)
        | Tool::Introduce { .. }
        | Tool::Subscribe { .. }
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
    let _pending = domain.pending_calls.remove(&key);
    if !current_proof(domain, key.task, key.attempt) {
        relay_call(domain, limits, decision, to, answer);
        return;
    }
    let _number = crate::fresh(&mut domain.journal, Family::Call).expect("admitted call counter");
    assert!(domain.calls.insert(key, answer.clone()) == Ok(None), "call record room reserved");
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
    let Some(context) = domain.tasks.delegation(key.task) else {
        decide_call(
            domain,
            &env.limits,
            decision,
            to,
            key,
            CallAnswer::MessageRefused(tasks::Problem { task: Some(key.task), why: tasks::Refusal::Unknown }),
        );
        return;
    };
    let Some(number) = crate::fresh(&mut domain.journal, Family::Message) else {
        decide_call(
            domain,
            &env.limits,
            decision,
            to,
            key,
            CallAnswer::MessageRefused(tasks::Problem { task: Some(target), why: tasks::Refusal::Busy }),
        );
        return;
    };
    let token = to.into_token();
    assert!(domain.pending_calls.insert(key, true).is_ok(), "call record room reserved");
    assert!(domain.routing_calls.insert(token, RoutedCall::Message(key)) == Ok(None), "one live routed call");
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
    assert!(domain.pending_calls.insert(key, true).is_ok(), "call record room reserved");
    assert!(domain.routing_calls.insert(token, RoutedCall::Introduce(key)) == Ok(None), "one live routed call");
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
    let Some(subscription) = crate::fresh(&mut domain.journal, Family::Message) else {
        decide_call(
            domain,
            &env.limits,
            decision,
            to,
            key,
            CallAnswer::SubscriptionRefused(tasks::Problem { task: Some(key.task), why: tasks::Refusal::Busy }),
        );
        return;
    };
    let token = to.into_token();
    assert!(domain.pending_calls.insert(key, true).is_ok(), "call record room reserved");
    assert!(
        domain.routing_calls.insert(token, RoutedCall::Subscribe { key, subscription }) == Ok(None),
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
    assert!(domain.pending_calls.insert(key, true).is_ok(), "call record room reserved");
    assert!(domain.routing_calls.insert(token, RoutedCall::Unsubscribe(key)) == Ok(None), "one live routed call");
    domain.work.push(Work::Tasks(tasks::Event::Unsubscribe {
        reply_to: ReplyTo::new(token),
        task: key.task,
        subscription,
    }));
}

fn control_call(domain: &mut Domain, to: ReplyTo, key: CallKey, target: u64, control: tasks::Control) {
    let token = to.into_token();
    assert!(domain.pending_calls.insert(key, true).is_ok(), "call record room reserved");
    assert!(domain.routing_calls.insert(token, RoutedCall::Control(key)) == Ok(None), "one live routed call");
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
    let Some(holder) = domain.tasks.delegation(key.task) else {
        return decide_call(
            domain,
            &env.limits,
            decision,
            to,
            key,
            CallAnswer::ControlRefused(tasks::Problem { task: Some(key.task), why: tasks::Refusal::Unknown }),
        );
    };
    let Some(current) = domain.tasks.delegation(target) else {
        return decide_call(
            domain,
            &env.limits,
            decision,
            to,
            key,
            CallAnswer::ControlRefused(tasks::Problem { task: Some(target), why: tasks::Refusal::Unknown }),
        );
    };
    if current.requester != tasks::Party::Task(key.task) || current.project != holder.project {
        return decide_call(
            domain,
            &env.limits,
            decision,
            to,
            key,
            CallAnswer::ControlRefused(tasks::Problem { task: Some(target), why: tasks::Refusal::Reference }),
        );
    }
    let mut stop_run = false;
    if let Some(after) = &amendment.authority {
        let before = authority_value(&current.authority);
        let after = authority_value(after);
        let implies = &domain.config.authority.rules().implies;
        stop_run = !authority::at_most(&before, &after, implies);
        if !authority::at_most(&after, &before, implies) {
            let ceiling = domain.config.authority.policy(current.project);
            let hard = match ceiling {
                Some(policy) => {
                    authority::at_most(&after, &policy.ceiling, implies)
                        && authority::at_most(&after, &domain.config.authority.rules().ceiling, implies)
                }
                None => false,
            };
            let answer = if hard {
                if authority::at_most(&after, &authority_value(&holder.authority), implies) {
                    authority::Answer::Allow
                } else {
                    authority::Answer::Propose
                }
            } else {
                authority::Answer::Refuse
            };
            if answer != authority::Answer::Allow {
                return decide_call(domain, &env.limits, decision, to, key, CallAnswer::ControlDenied { answer });
            }
        }
    }
    let Some(message) = crate::fresh(&mut domain.journal, Family::Message) else {
        return decide_call(
            domain,
            &env.limits,
            decision,
            to,
            key,
            CallAnswer::ControlRefused(tasks::Problem { task: Some(target), why: tasks::Refusal::Busy }),
        );
    };
    let token = to.into_token();
    assert!(domain.pending_calls.insert(key, true).is_ok(), "call record room reserved");
    assert!(domain.routing_calls.insert(token, RoutedCall::Control(key)) == Ok(None), "one amendment route");
    domain.work.push(Work::Tasks(tasks::Event::Amend {
        reply_to: ReplyTo::new(token),
        by: tasks::Party::Task(key.task),
        task: target,
        message,
        stop_run,
        amendment,
    }));
}

#[expect(
    clippy::too_many_lines,
    reason = "one delegated call checks authority, inputs and the atomic batch before routing"
)]
fn delegate_call(
    domain: &mut Domain,
    env: &Env<Limits>,
    decision: &mut Decision,
    to: ReplyTo,
    key: CallKey,
    batch: Box<[Delegate]>,
    validated: bool,
) {
    if !current_proof(domain, key.task, key.attempt) {
        decide_call(
            domain,
            &env.limits,
            decision,
            to,
            key,
            CallAnswer::DelegationRefused(tasks::Problem { task: Some(key.task), why: tasks::Refusal::State }),
        );
        return;
    }
    let Some(context) = domain.tasks.delegation(key.task) else {
        decide_call(
            domain,
            &env.limits,
            decision,
            to,
            key,
            CallAnswer::DelegationRefused(tasks::Problem { task: Some(key.task), why: tasks::Refusal::Unknown }),
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
            CallAnswer::DelegationRefused(tasks::Problem { task: None, why: tasks::Refusal::Batch }),
        );
        return;
    }
    let mut asked = List::with_capacity(env.limits.tasks.batch);
    for member in &batch {
        let executor = match member.executor {
            tasks::Executor::Agent { charter } => authority::Executor::Charter(charter),
        };
        asked
            .push(authority::Delegate { executor, authority: authority_value(&member.authority) })
            .expect("bounded delegation request");
    }
    let mut findings =
        Queue::with_capacity(authority::max_out(domain.config.authority.limits()).expect("findings bound"));
    let checked = authority::check_batch(
        &domain.config.authority,
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
            List::with_capacity(authority::max_out(domain.config.authority.limits()).expect("findings bound"));
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
                    CallAnswer::DelegationRefused(tasks::Problem { task: None, why: tasks::Refusal::Inputs }),
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
            let read = InputCheck { to, key, batch, ids: ids.into_boxed(), at: 0, project: context.project };
            let waiter =
                domain.result_reads.insert(Some(Read::InputCheck(read))).expect("preflighted input read slot").token();
            assert!(domain.pending_calls.insert(key, true).is_ok(), "reserved call record room");
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
        let Some(number) = crate::fresh(&mut domain.journal, Family::Task) else {
            decide_call(
                domain,
                &env.limits,
                decision,
                to,
                key,
                CallAnswer::DelegationRefused(tasks::Problem { task: None, why: tasks::Refusal::Live }),
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
                    }),
                );
                return;
            }
        }
        created
            .push(tasks::New {
                number: *numbers.get(index).expect("one ID per member"),
                project: context.project,
                executor: member.executor,
                spec: member.spec,
                contract: member.contract,
                authority: member.authority.clone(),
                numbers: tasks::Numbers {
                    budget: member.authority.budget.spend,
                    spent: 0,
                    spent_below: 0,
                    reserved: 0,
                },
                funder: tasks::Funder::Task(key.task),
                dependencies: dependencies.into_boxed(),
                wake: member.wake,
            })
            .expect("bounded delegation batch");
    }
    let token = to.into_token();
    if !validated {
        assert!(domain.pending_calls.insert(key, true).is_ok(), "reserved call record room");
    }
    assert!(domain.delegating.insert(token, key) == Ok(None), "one pending delegated call");
    domain.work.push(Work::Tasks(tasks::Event::Make {
        reply_to: ReplyTo::new(token),
        creator: tasks::Party::Task(key.task),
        batch: created.into_boxed(),
    }));
}

#[expect(clippy::too_many_lines, reason = "the closed child vocabulary is routed exhaustively inside one decision")]
fn tasks_outputs(
    domain: &mut Domain,
    env: &Env<Limits>,
    decision: &mut Decision,
    out: &mut Queue<tasks::Request>,
    person_proposal: bool,
) {
    for _ in 0..out.len() {
        match out.pop().expect("tasks output count") {
            tasks::Request::Notify { task, subscription, target, state, words } => {
                let number = crate::fresh(&mut domain.journal, Family::Message).expect("notification number admitted");
                domain.work.push(Work::Tasks(tasks::Event::Notice {
                    task,
                    word: tasks::Word {
                        number,
                        from: tasks::Party::Task(target),
                        kind: tasks::MessageKind::Notice { subscription, target, state },
                        words,
                        at: env.wall,
                        hits: 1,
                        eligible: false,
                    },
                }));
            }
            tasks::Request::Timer { task, subscription } => {
                let number = crate::fresh(&mut domain.journal, Family::Message).expect("timer number admitted");
                domain.work.push(Work::Tasks(tasks::Event::Notice {
                    task,
                    word: tasks::Word {
                        number,
                        from: tasks::Party::Task(task),
                        kind: tasks::MessageKind::Timer { subscription },
                        words: Box::new([]),
                        at: env.wall,
                        hits: 1,
                        eligible: false,
                    },
                }));
            }
            tasks::Request::Sent { reply_to, task, word } => {
                let request = reply_to.into_token();
                if let Some(context) = domain.contexts.get_mut(&task) {
                    let capacity = env
                        .limits
                        .tasks
                        .inbox_messages
                        .checked_add(env.limits.tasks.tasks)
                        .expect("validated virtual proposal room");
                    context.inbox = append_word(&context.inbox, &word, capacity);
                    context.last_message = word.number;
                }
                match domain.routing_calls.remove(&request) {
                    Some(RoutedCall::Message(key)) => decide_call(
                        domain,
                        &env.limits,
                        decision,
                        ReplyTo::new(request),
                        key,
                        CallAnswer::Sent { message: word.number },
                    ),
                    Some(
                        RoutedCall::Introduce(_)
                        | RoutedCall::Propose { .. }
                        | RoutedCall::Decide { .. }
                        | RoutedCall::Withdraw { .. }
                        | RoutedCall::Accepting { .. }
                        | RoutedCall::Subscribe { .. }
                        | RoutedCall::Unsubscribe(_)
                        | RoutedCall::Control(_),
                    ) => {
                        unreachable!("only message calls produce Sent")
                    }
                    None => {
                        assert!(domain.saying.remove(&request) == Some(task), "matching pending say flight");
                        domain.work.push(Work::People(people::Event::Decided {
                            request,
                            outcome: people::Outcome::Said { task, message: word.number },
                        }));
                    }
                }
            }
            tasks::Request::Relay { task, attempt, previous, word } => {
                let previous = match word.kind {
                    tasks::MessageKind::Proposal { .. } => match domain.proofs.get(&task) {
                        Some(proof) => proof.offered,
                        None => None,
                    },
                    tasks::MessageKind::ProposalDecision { .. }
                    | tasks::MessageKind::Words
                    | tasks::MessageKind::Amendment { .. }
                    | tasks::MessageKind::Question
                    | tasks::MessageKind::Answer { .. }
                    | tasks::MessageKind::Notice { .. }
                    | tasks::MessageKind::Timer { .. }
                    | tasks::MessageKind::News { .. }
                    | tasks::MessageKind::Result(_) => previous,
                };
                emit(decision, &env.limits, Delivery::Relay { task, attempt, previous, word });
            }
            tasks::Request::EscalationsInspected { .. } | tasks::Request::EscalationsRechecked { .. } => {
                unreachable!("serialized roles route consumes project terminals")
            }
            tasks::Request::EscalationNeeded { context } => escalation::needed(domain, context),
            tasks::Request::EscalationInspected { reply_to, context } => {
                escalation::inspected(domain, env, decision, reply_to.into_token(), context);
            }
            tasks::Request::EscalationDecided { reply_to, task, revision, outcome } => {
                escalation::completed(domain, env, decision, reply_to.into_token(), task, revision, outcome);
            }
            tasks::Request::ProposalDecided { reply_to, proposer, number, outcome } => {
                let token = reply_to.into_token();
                let person_route = if person_proposal { domain.routing_people_proposals.remove(&token) } else { None };
                if let Some(route) = person_route {
                    let request = match route {
                        PersonProposalRoute::Deciding { request, proposer: named, proposal }
                            if named == proposer && proposal == number =>
                        {
                            request
                        }
                        PersonProposalRoute::Deciding { .. } | PersonProposalRoute::Accepting { .. } => {
                            unreachable!("matching person proposal decision")
                        }
                    };
                    let choice = match outcome {
                        tasks::ProposalOutcome::Accepted => people::ProposalChoice::Accepted,
                        tasks::ProposalOutcome::Rejected => people::ProposalChoice::Rejected,
                        tasks::ProposalOutcome::Passed => people::ProposalChoice::Passed,
                        tasks::ProposalOutcome::Withdrawn => people::ProposalChoice::Withdrawn,
                        tasks::ProposalOutcome::Stale => people::ProposalChoice::Stale,
                    };
                    domain.work.push(Work::People(people::Event::Decided {
                        request,
                        outcome: people::Outcome::ProposalDecided { proposer, proposal: number, choice },
                    }));
                    continue;
                }
                let route = domain.routing_calls.remove(&token).expect("pending proposal decision route");
                let key = match route {
                    RoutedCall::Decide { key, proposal } | RoutedCall::Withdraw { key, proposal }
                        if proposal == number =>
                    {
                        key
                    }
                    RoutedCall::Propose { .. }
                    | RoutedCall::Accepting { .. }
                    | RoutedCall::Decide { .. }
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
                if let Some(pending) = domain.tasks.proposal(proposer, proposal)
                    && let tasks::ProposalState::Pending { holder: current, .. } = pending.state
                    && current == holder
                    && let Some(next) = proposals::holder(domain, proposer, &pending.action, Some(holder))
                {
                    domain.work.push(Work::Tasks(tasks::Event::StalledProposal { proposer, proposal, holder: next }));
                }
            }
            tasks::Request::Save { record } => {
                let record = match record {
                    tasks::Stored::Ended(mut task) => {
                        let position = crate::fresh(&mut domain.journal, Family::Message)
                            .expect("ending position preflighted before mutation");
                        task.result_position = position;
                        assert!(
                            domain.ending_positions.insert(task.number, position) == Ok(None),
                            "one ending position per ended task"
                        );
                        if let tasks::Party::Person(person) = task.requester {
                            domain.people.remember_result(
                                &env.limits.people,
                                person,
                                people::ResultRef { task: task.number, position },
                            );
                        }
                        tasks::Stored::Ended(task)
                    }
                    tasks::Stored::Live(_) | tasks::Stored::Ledger(_) | tasks::Stored::History(_) => record,
                };
                save(decision, &env.limits, Write::Save(Record::Tasks(record)));
            }
            tasks::Request::Erase { key } => save(decision, &env.limits, Write::Erase(Key::Tasks(key))),
            tasks::Request::Made { reply_to, tasks } => {
                let request = reply_to.into_token();
                let person_route =
                    if person_proposal { domain.routing_people_proposals.remove(&request) } else { None };
                if let Some(PersonProposalRoute::Accepting { request: named, person, proposer, proposal, message }) =
                    person_route
                {
                    assert!(request == named, "person acceptance correlation");
                    domain
                        .routing_people_proposals
                        .insert(request, PersonProposalRoute::Deciding { request, proposer, proposal })
                        .expect("person route room");
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
                if let Some(RoutedCall::Accepting { key, proposer, proposal, message }) =
                    domain.routing_calls.remove(&request)
                {
                    domain
                        .routing_calls
                        .insert(request, RoutedCall::Decide { key, proposal })
                        .expect("acceptance route room");
                    domain.work.push(Work::Tasks(tasks::Event::DecideProposal {
                        reply_to: ReplyTo::new(request),
                        proposer,
                        proposal,
                        message: Some(message),
                        by: tasks::Party::Task(key.task),
                        decision: tasks::ProposalDecision::Accept,
                    }));
                    continue;
                }
                match domain.delegating.remove(&request) {
                    Some(key) => decide_call(
                        domain,
                        &env.limits,
                        decision,
                        ReplyTo::new(request),
                        key,
                        CallAnswer::Delegated(tasks),
                    ),
                    None => {
                        let expected = domain.made.remove(&request).expect("pending make route");
                        assert!(tasks.as_ref() == [expected], "one exact chat created");
                        domain.work.push(Work::People(people::Event::Decided {
                            request,
                            outcome: people::Outcome::Started { task: expected },
                        }));
                    }
                }
            }
            tasks::Request::Refused { reply_to, problem } => {
                let token = reply_to.into_token();
                let person_route = if person_proposal { domain.routing_people_proposals.remove(&token) } else { None };
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
                            | tasks::Refusal::Turn => people::Refusal::Limit,
                        }),
                    }));
                    continue;
                }
                if let Some(route) = domain.routing_calls.remove(&token) {
                    let (key, answer) = match route {
                        RoutedCall::Message(key) | RoutedCall::Introduce(key) => {
                            (key, CallAnswer::MessageRefused(problem))
                        }
                        RoutedCall::Subscribe { key, .. } | RoutedCall::Unsubscribe(key) => {
                            (key, CallAnswer::SubscriptionRefused(problem))
                        }
                        RoutedCall::Control(key) => (key, CallAnswer::ControlRefused(problem)),
                        RoutedCall::Propose { key, .. }
                        | RoutedCall::Decide { key, .. }
                        | RoutedCall::Withdraw { key, .. }
                        | RoutedCall::Accepting { key, .. } => (key, CallAnswer::ProposalRefused(problem)),
                    };
                    decide_call(domain, &env.limits, decision, ReplyTo::new(token), key, answer);
                } else if let Some(key) = domain.delegating.remove(&token) {
                    decide_call(
                        domain,
                        &env.limits,
                        decision,
                        ReplyTo::new(token),
                        key,
                        CallAnswer::DelegationRefused(problem),
                    );
                } else if domain.made.remove(&token).is_some() {
                    domain.work.push(Work::People(people::Event::Decided {
                        request: token,
                        outcome: people::Outcome::Refused(people::Refusal::Limit),
                    }));
                } else if domain.saying.remove(&token).is_some() {
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
                            | tasks::Refusal::Funding => people::Refusal::Limit,
                        }),
                    }));
                } else if domain.claiming.remove(&token.raw()).is_some() {
                    drop(domain.assignments.remove(&token.raw()));
                    drop(domain.proofs.remove(&token.raw()));
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
                                let proof = domain.proofs.get_mut(&task).expect("answer proof reserved");
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
                let proof = domain.proofs.get_mut(&task).expect("turn proof reserved before child mutation");
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
                if let Some(proof) = domain.proofs.get(&task) {
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
            }
            tasks::Request::Activate { context } => domain.work.push(Work::Activate(context)),
            tasks::Request::Adopt { task, attempt, kept } => {
                domain.adopted.push(fleet::Event::Adopt {
                    reply_to: internal(task),
                    run: Token::new(task),
                    attempt: Token::new(attempt),
                    kept,
                });
            }
            tasks::Request::Stop { task, attempt } => emit(
                decision,
                &env.limits,
                Delivery::Fleet(fleet::Event::Cancel { run: Token::new(task), attempt: Token::new(attempt) }),
            ),
            tasks::Request::Close { task, .. } => domain.work.push(Work::Tasks(tasks::Event::Settled { task })),
            tasks::Request::Ended { task, requester, ending } => {
                let position = domain.ending_positions.remove(&task).expect("ended row assigned its result position");
                retire_calls(domain, &env.limits, decision, task, u64::MAX, u32::MAX);
                drop(domain.proofs.remove(&task));
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
                let person_route =
                    if person_proposal { domain.routing_people_proposals.remove(&Token::new(task)) } else { None };
                if let Some(PersonProposalRoute::Accepting { request, person, proposer, proposal, message }) =
                    person_route
                {
                    domain
                        .routing_people_proposals
                        .insert(request, PersonProposalRoute::Deciding { request, proposer, proposal })
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
                if let Some(route) = domain.routing_calls.remove(&Token::new(task)) {
                    let (key, answer) = match route {
                        RoutedCall::Propose { key, proposal } => (key, CallAnswer::Proposed { proposal }),
                        RoutedCall::Accepting { key, proposer, proposal, message } => {
                            domain
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
                        RoutedCall::Decide { .. } | RoutedCall::Withdraw { .. } => {
                            unreachable!("proposal decisions produce their own terminal")
                        }
                    };
                    decide_call(domain, &env.limits, decision, ReplyTo::new(Token::new(task)), key, answer);
                } else if let Some(attempt) = domain.claiming.remove(&task) {
                    let proof = domain.proofs.get(&task).expect("claim proof pre-reserved");
                    save(decision, &env.limits, Write::Save(Record::RunProof(proof.clone())));
                    emit(
                        decision,
                        &env.limits,
                        Delivery::Fleet(fleet::Event::Start {
                            reply_to: internal(task),
                            run: Token::new(task),
                            attempt: Token::new(attempt),
                            workstream: Box::new(task.to_be_bytes()),
                        }),
                    );
                }
            }
            tasks::Request::RestoreRefused { .. } => domain.startup = Startup::Failed,
        }
    }
}

fn brief_outputs(domain: &mut Domain, env: &Env<Limits>, _decision: &mut Decision, out: &mut Queue<brief::Request>) {
    for _ in 0..out.len() {
        match out.pop().expect("brief output count") {
            brief::Request::Read { owner, source, parts, bytes, .. } => {
                let read = match source {
                    brief::Source::Task { task, part } => task_section(domain, task, part, parts, bytes),
                    brief::Source::Item(_)
                    | brief::Source::Comments { .. }
                    | brief::Source::Dependencies(_)
                    | brief::Source::Ci { .. }
                    | brief::Source::Reviews { .. }
                    | brief::Source::Pull { .. }
                    | brief::Source::Attempts(_)
                    | brief::Source::Plan { .. }
                    | brief::Source::Notes { .. }
                    | brief::Source::Template(_) => unreachable!("06a only asks the task section"),
                };
                domain.work.push(Work::Brief(brief::Event::Read { owner, read }));
            }
            brief::Request::Rendered { reply_to, sections } => {
                let task = reply_to.into_token().raw();
                let context = domain.contexts.remove(&task).expect("rendered task owns context");
                drop(domain.dependency_results.remove(&task));
                let transcript = domain.transcripts.remove(&task).expect("rendered task owns loaded transcript");
                let Some(attempt) = crate::fresh(&mut domain.journal, Family::Run) else {
                    domain.work.push(Work::Tasks(tasks::Event::PreparationFailed { task }));
                    continue;
                };
                let Some(grant) = domain.accounts.grant(domain.config.account, env.now) else {
                    domain.work.push(Work::Tasks(tasks::Event::PreparationFailed { task }));
                    continue;
                };
                if !domain.proofs.contains_key(&task) && domain.proofs.len() == domain.proofs.capacity() {
                    domain.work.push(Work::Tasks(tasks::Event::PreparationFailed { task }));
                    continue;
                }
                let offered = if context.last_message == 0 { None } else { Some(context.last_message) };
                assert!(
                    domain.proofs.insert(task, RunProof { task, attempt, offered, turn: None, terminal: None }).is_ok(),
                    "claim proof reserved before child mutation"
                );
                let mut turns = List::with_capacity(transcript.turns.len());
                if transcript.bytes <= u64::from(domain.config.resume_bytes) {
                    let mut kept = transcript.turns;
                    for _ in 0..kept.len() {
                        turns.push(kept.pop().expect("counted transcript turn")).expect("bounded transcript turns");
                    }
                }
                let assignment = Assignment {
                    task,
                    attempt,
                    charter: domain.config.charter,
                    sections,
                    inbox: context.inbox,
                    saved: context.saved,
                    transcript: turns.into_boxed(),
                    answered: {
                        let mut answered = List::with_capacity(domain.limits.call_records);
                        for (&key, answer) in &domain.calls {
                            if key.task == task && key.attempt < attempt {
                                answered
                                    .push(crate::CallRecord { key, answer: answer.clone() })
                                    .expect("retained call bound");
                            }
                        }
                        answered.into_boxed()
                    },
                    grant,
                };
                assert!(domain.assignments.insert(task, assignment).is_ok(), "assignment fits live task room");
                assert!(domain.claiming.insert(task, attempt) == Ok(None), "one pending claim per task");
                domain.work.push(Work::Tasks(tasks::Event::Claim { reply_to: internal(task), task, attempt }));
            }
            brief::Request::Failed { reply_to, .. } | brief::Request::Refused { reply_to, .. } => {
                let task = reply_to.into_token().raw();
                drop(domain.contexts.remove(&task));
                drop(domain.dependency_results.remove(&task));
                drop(domain.transcripts.remove(&task));
                domain.work.push(Work::Tasks(tasks::Event::PreparationFailed { task }));
            }
            brief::Request::Room => {}
        }
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
            fleet::Request::Assign { channel, run, attempt } => {
                let assignment = domain.assignments.remove(&run.raw()).expect("durable claim has prepared assignment");
                assert!(assignment.attempt == attempt.raw(), "assignment names current attempt");
                emit(decision, &env.limits, Delivery::Assigned { channel, assignment });
            }
            fleet::Request::Placed { run, attempt } => {
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
                    offered: domain.proofs.get(&run.raw()).expect("current proof").offered,
                    cumulative: payload.cumulative,
                }));
            }
            fleet::Request::Answered { run, attempt, payload, to, .. } => {
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
                let proof = domain.proofs.get_mut(&run.raw()).expect("terminal proof pre-reserved");
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
                let proof = domain.proofs.get(&run.raw()).expect("lost claim has reserved durable evidence");
                assert!(proof.attempt == attempt.raw(), "lost callback belongs to current proof");
                let end = tasks::End::Failed(tasks::Class::Lost);
                remember_unpriced_terminal(domain, run, attempt, end.clone());
                domain.work.push(Work::Tasks(tasks::Event::Activation {
                    reply_to: internal(u64::MAX),
                    task: run.raw(),
                    attempt: attempt.raw(),
                    end,
                    saved: None,
                    cause: tasks::Cause::Unpriced,
                }));
            }
            fleet::Request::Withdrawn { to, run, attempt, .. } | fleet::Request::Refused { to, run, attempt, .. } => {
                let _answered = to.into_token();
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
                let relay = domain.relaying.take().expect("fleet inbound follows committed word");
                assert!(event.raw() == relay.word.number, "relay event identifies word");
                if let Some(proof) = domain.proofs.get_mut(&run.raw())
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
                drop(domain.relaying.take());
            }
            fleet::Request::Relay { reply_to, run, attempt, body } => {
                let Some(Payload::Call { key, body }) = take_payload(domain, body) else {
                    unreachable!("fleet returns admitted call payload")
                };
                assert!(key.task == run.raw() && key.attempt == attempt.raw(), "fleet call envelope is unchanged");
                assert!(current_proof(domain, key.task, key.attempt), "fleet only relays a current claim");
                match domain.calls.get(&key).cloned() {
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
                            CallAnswer::DelegationRefused(tasks::Problem { task: None, why }),
                        ),
                        Tool::RejectedMessage(why) => decide_call(
                            domain,
                            &env.limits,
                            decision,
                            reply_to,
                            key,
                            CallAnswer::MessageRefused(tasks::Problem { task: None, why }),
                        ),
                        Tool::RejectedControl(why) => decide_call(
                            domain,
                            &env.limits,
                            decision,
                            reply_to,
                            key,
                            CallAnswer::ControlRefused(tasks::Problem { task: None, why }),
                        ),
                        Tool::RejectedProposal(why) => decide_call(
                            domain,
                            &env.limits,
                            decision,
                            reply_to,
                            key,
                            CallAnswer::ProposalRefused(tasks::Problem { task: None, why }),
                        ),
                        Tool::Delegate { batch } => delegate_call(domain, env, decision, reply_to, key, batch, false),
                        Tool::Propose { action, reason, as_holder } => {
                            proposals::propose_call(domain, env, decision, reply_to, key, action, reason, as_holder);
                        }
                        Tool::Decide { proposer, proposal, decision: choice } => {
                            proposals::decide_call(domain, env, decision, reply_to, key, proposer, proposal, choice);
                        }
                        Tool::Withdraw { proposal } => {
                            proposals::withdraw_call(domain, env, decision, reply_to, key, proposal);
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
            fleet::Request::Grant { .. }
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
        Range::Deployment | Range::TaskResult { .. } | Range::EscalationDecision { .. } => 1,
        Range::Calls
        | Range::Tasks
        | Range::EndedResults
        | Range::People
        | Range::RunProofs
        | Range::Turns { .. }
        | Range::TaskTranscript { .. } => domain.limits.loads.rows,
    };
    if loads::begin(&mut domain.loads, waiter, range, after, most, &mut load_out).is_none() {
        if let Range::TaskTranscript { .. } = range {
            transcript_failed(domain, waiter);
            return;
        }
        match range {
            Range::EscalationDecision { .. } => {
                domain.work.push(Work::EscalationFailed { waiter });
                return;
            }
            Range::Calls
            | Range::Deployment
            | Range::Tasks
            | Range::EndedResults
            | Range::People
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
        Some(Some(Read::Result(_) | Read::Escalation(_) | Read::Dependency(_) | Read::InputCheck(_)) | None) | None => {
            false
        }
    }
}

fn dependency_waiter(domain: &Domain, waiter: Token) -> bool {
    match domain.result_reads.get(Id::from_token(waiter)) {
        Some(Some(Read::Dependency(_))) => true,
        Some(Some(Read::Result(_) | Read::Escalation(_) | Read::Transcript { .. } | Read::InputCheck(_)) | None)
        | None => false,
    }
}

fn input_waiter(domain: &Domain, waiter: Token) -> bool {
    match domain.result_reads.get(Id::from_token(waiter)) {
        Some(Some(Read::InputCheck(_))) => true,
        Some(Some(Read::Result(_) | Read::Escalation(_) | Read::Transcript { .. } | Read::Dependency(_)) | None)
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
        | Record::Turn(_)
        | Record::RunProof(_)
        | Record::Terminal(_)
        | Record::Tasks(_)
        | Record::People(_) => {
            input_failed(domain, waiter);
            return;
        }
    };
    if row.number != wanted
        || row.project != project
        || row.requester != tasks::Party::Task(creator)
        || !match row.phase {
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
    read.at = read.at.checked_add(1).expect("bounded input index");
    if let Some(&next) = read.ids.get(usize::try_from(read.at).expect("bounded input index")) {
        request_load(domain, waiter, Range::TaskResult { task: next }, None, out);
        return;
    }
    let Some(Read::InputCheck(read)) = take_read(domain, waiter) else { unreachable!("completed input read") };
    domain.result_reads.retire(Id::from_token(waiter));
    domain.work.push(Work::DelegateValidated { to: read.to, key: read.key, batch: read.batch });
}

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
                if cut.is_some() {
                    let archive = match domain.result_reads.get(Id::from_token(waiter)) {
                        Some(Some(Read::Escalation(_))) => true,
                        Some(
                            Some(Read::Result(_) | Read::Transcript { .. } | Read::Dependency(_) | Read::InputCheck(_))
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
                        Some(
                            Some(Read::Result(_) | Read::Transcript { .. } | Read::Dependency(_) | Read::InputCheck(_))
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
                if waiter == Token::new(u64::MAX) {
                    domain.startup = Startup::Failed;
                    out.push(Request::Stop);
                } else {
                    let archive = match domain.result_reads.get(Id::from_token(waiter)) {
                        Some(Some(Read::Escalation(_))) => true,
                        Some(
                            Some(Read::Result(_) | Read::Transcript { .. } | Read::Dependency(_) | Read::InputCheck(_))
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
    drop(domain.contexts.remove(&task));
    drop(domain.transcripts.remove(&task));
    domain.work.push(Work::Tasks(tasks::Event::PreparationFailed { task }));
}

fn begin_dependency_read(domain: &mut Domain, task: u64) -> Option<(Token, u64)> {
    let context = domain.contexts.get(&task).expect("preparing task context");
    let count = context.dependencies.len().checked_add(context.spec.inputs.len()).expect("bounded input count");
    if count == 0 {
        start_brief(domain, task);
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
        drop(domain.contexts.remove(&task));
        drop(domain.transcripts.remove(&task));
        domain.work.push(Work::Tasks(tasks::Event::PreparationFailed { task }));
        return None;
    };
    Some((waiter.token(), first))
}

fn dependency_failed(domain: &mut Domain, waiter: Token) {
    let Some(Read::Dependency(read)) = take_read(domain, waiter) else { return };
    domain.result_reads.retire(Id::from_token(waiter));
    drop(domain.contexts.remove(&read.task));
    drop(domain.transcripts.remove(&read.task));
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
    let project = domain.contexts.get(&read.task).expect("preparing task context").project;
    if next.is_some() || rows.len() != 1 {
        dependency_failed(domain, waiter);
        return;
    }
    let row = match rows.into_iter().next().expect("one dependency result row") {
        Record::Tasks(tasks::Stored::Ended(row)) => row,
        Record::Call(_)
        | Record::Deployment(_)
        | Record::EscalationDecision(_)
        | Record::Turn(_)
        | Record::RunProof(_)
        | Record::Terminal(_)
        | Record::Tasks(_)
        | Record::People(_) => {
            dependency_failed(domain, waiter);
            return;
        }
    };
    if row.number != wanted || row.project != project {
        dependency_failed(domain, waiter);
        return;
    }
    let tasks::Phase::Ended(ending) = row.phase else {
        dependency_failed(domain, waiter);
        return;
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
        domain.dependency_results.insert(read.task, read.results.into_boxed()).is_ok(),
        "one preparation result set"
    );
    start_brief(domain, read.task);
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
    let transcript = domain.transcripts.get_mut(&task).expect("load belongs to prepared task");
    let bound = u64::from(domain.config.resume_bytes);
    for row in rows {
        let Record::Turn(turn) = row else {
            transcript_failed(domain, waiter);
            return;
        };
        if turn.task != task || turn.attempt > transcript.previous_attempt || turn.attempt == 0 || turn.turn == 0 {
            transcript_failed(domain, waiter);
            return;
        }
        let length = u64::try_from(turn.transcript.len()).expect("stored turn size fits u64");
        let Some(total) = transcript.bytes.checked_add(length) else {
            transcript_failed(domain, waiter);
            return;
        };
        transcript.bytes = total;
        if length == 0 {
            continue;
        }
        let body = if length > bound {
            let start = turn
                .transcript
                .len()
                .checked_sub(usize::try_from(bound).expect("u32 bound fits usize"))
                .expect("oversize turn has tail");
            Box::<[u8]>::from(turn.transcript.get(start..).expect("tail starts inside stored turn"))
        } else {
            turn.transcript
        };
        let body_len = u64::try_from(body.len()).expect("bounded turn fits u64");
        for _ in 0..transcript.turns.len() {
            if transcript.kept.checked_add(body_len).expect("two bounded tails") <= bound {
                break;
            }
            let old = transcript.turns.pop().expect("overfull transcript has an older turn");
            transcript.kept = transcript
                .kept
                .checked_sub(u64::try_from(old.len()).expect("bounded old turn"))
                .expect("old turn was counted");
        }
        transcript.turns.push(body);
        transcript.kept = transcript.kept.checked_add(body_len).expect("bounded transcript tail");
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

fn start_brief(domain: &mut Domain, task: u64) {
    let context = domain.contexts.get(&task).expect("loaded task context");
    let transcript = domain.transcripts.get(&task).expect("prepared transcript state");
    let oversized = transcript.bytes > u64::from(domain.config.resume_bytes);
    let mut wanted = List::with_capacity(domain.limits.brief.sections);
    wanted
        .push(brief::Wanted { source: brief::Source::Task { task, part: brief::TaskPart::Spec }, required: true })
        .expect("task brief room");
    if !context.dependencies.is_empty() || !context.spec.inputs.is_empty() {
        wanted
            .push(brief::Wanted {
                source: brief::Source::Task { task, part: brief::TaskPart::Dependencies },
                required: true,
            })
            .expect("dependency result section room");
    }
    if oversized {
        wanted
            .push(brief::Wanted {
                source: brief::Source::Task { task, part: brief::TaskPart::TranscriptTail },
                required: true,
            })
            .expect("tail brief room");
    }
    if !context.delegates.is_empty() && wanted.room() > 0 {
        wanted
            .push(brief::Wanted {
                source: brief::Source::Task { task, part: brief::TaskPart::Delegates },
                required: false,
            })
            .expect("delegate section room");
    }
    if context.tries != tasks::Tries::NONE && wanted.room() > 0 {
        wanted
            .push(brief::Wanted {
                source: brief::Source::Task { task, part: brief::TaskPart::Attempts },
                required: false,
            })
            .expect("attempt brief room");
    }
    domain.work.push(Work::Brief(brief::Event::Render { reply_to: internal(task), sections: wanted.into_boxed() }));
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
        Range::Tasks => Some(Range::RunProofs),
        Range::RunProofs => Some(Range::Calls),
        Range::Calls => None,
        Range::EscalationDecision { .. }
        | Range::Turns { .. }
        | Range::TaskTranscript { .. }
        | Range::TaskResult { .. }
        | Range::EndedResults => {
            unreachable!("startup range")
        }
    };
    if range == Range::Tasks {
        for project in &domain.projects {
            if !domain.people.has_project(*project) {
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
    if !domain.restoring_proofs.is_empty() {
        domain.startup = Startup::Failed;
        out.push(Request::Stop);
        return;
    }
    domain.startup = Startup::Adopting;
    domain.work.push(Work::Tasks(tasks::Event::Restored));
    route_into(domain, env, &mut decision);
    if domain.startup == Startup::Failed {
        out.push(Request::Stop);
        return;
    }
    for _ in 0..domain.adopted.len() {
        domain.work.push(Work::Fleet(domain.adopted.pop().expect("all restored claims")));
    }
    domain.work.push(Work::Fleet(fleet::Event::Loaded));
    route_into(domain, env, &mut decision);
    if domain.startup == Startup::Failed {
        out.push(Request::Stop);
        return;
    }
    domain.startup = Startup::Running;
    for _ in 0..domain.due.len() {
        domain.work.push(Work::Activate(domain.due.pop().expect("restored due tasks")));
    }
    route_into(domain, env, &mut decision);
    close(domain, env, decision, out);
}

fn account_event(domain: &mut Domain, env: &Env<Limits>, event: accounts::Event, out: &mut Queue<Request>) {
    let mut account_out = Queue::with_capacity(accounts::MAX_OUT);
    accounts::step(&mut domain.accounts, &environment_accounts(env), event, &mut account_out);
    for _ in 0..account_out.len() {
        out.push(Request::Account(account_out.pop().expect("account output count")));
    }
}

fn account_fire(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    let mut account_out = Queue::with_capacity(accounts::MAX_OUT);
    accounts::fire(&mut domain.accounts, &environment_accounts(env), &mut account_out);
    for _ in 0..account_out.len() {
        out.push(Request::Account(account_out.pop().expect("account output count")));
    }
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
    match end {
        tasks::End::Finished { result, .. } => match result {
            tasks::TaskResult::Report { words }
            | tasks::TaskResult::Verdict { words, .. }
            | tasks::TaskResult::Change { words, .. } => u64::try_from(words.len()).expect("usize fits u64"),
            tasks::TaskResult::Failure { reason } => u64::try_from(reason.len()).expect("usize fits u64"),
        },
        tasks::End::Parked | tasks::End::Failed(_) | tasks::End::Refused => 0,
    }
}

fn authority_numbers(numbers: tasks::Numbers) -> authority::Numbers {
    authority::Numbers {
        budget: numbers.budget,
        spent: numbers.spent,
        spent_below: numbers.spent_below,
        reserved: numbers.reserved,
    }
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
        // One serialized role cohort revisits each Waiting task, then answers.
        .checked_add(limits.tasks.tasks.checked_add(4)?)
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
    let task_bytes = tasks::worst_case(&limits.tasks)?;
    let fleet_bytes = fleet::worst_case(&limits.fleet)?;
    let people_bytes = people::worst_case(&limits.people)?;
    let authority_bytes = authority::worst_case(&limits.authority)?;
    let brief_bytes = brief::worst_case(&limits.brief)?;
    let account_bytes = accounts::worst_case(&limits.accounts)?;
    let load_bytes = loads::worst_case(&limits.loads)?;
    let routes = route_bound(limits)?;
    let tool_bytes = u64::from(limits.tasks.batch).checked_mul(row_bound(limits)?)?;
    let inbox_bytes = u64::from(limits.people.inbox_entries).checked_mul(
        u64::try_from(size_of::<crate::ResultEntry>()).ok()?.checked_add(u64::from(limits.journal.result_bytes))?,
    )?;
    if limits.journal.writes < routes
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
        || limits.journal.deliveries < limits.tasks.saved_repositories
        || limits.journal.result_bytes < limits.tasks.result_bytes
        || limits.journal.result_bytes < limits.people.words
        || limits.fleet.workstream_bytes < 8
        || limits.people.inbox_entries == 0
        || inbox_bytes > u64::from(limits.journal.transcript_bytes)
        || u64::from(limits.journal.transcript_bytes) < row_bound(limits)?
    {
        return None;
    }
    let mut bytes = crate::worst_case(&limits.journal)?;
    bytes = bytes.checked_add(role_scratch_bytes(limits)?)?;
    let cold = limits.fleet.workers;
    let hello = u64::from(limits.fleet.slots)
        .checked_mul(u64::try_from(size_of::<fleet::Hosted>()).ok()?)?
        .checked_add(u64::from(limits.fleet.workstreams).checked_mul(
            u64::try_from(size_of::<Box<[u8]>>()).ok()?.checked_add(u64::from(limits.fleet.workstream_bytes))?,
        )?)?;
    bytes = bytes
        .checked_add(Queue::<Work>::worst_case(cold)?)?
        .checked_add(Map::<Token, bool>::worst_case(cold)?)?
        .checked_add(List::<(Token, bool)>::worst_case(cold)?)?
        .checked_add(u64::from(cold).checked_mul(hello)?)?
        .checked_add(List::<u32>::worst_case(limits.people.projects)?)?;
    for child in
        [task_bytes, authority_bytes.checked_mul(3)?, people_bytes, fleet_bytes, brief_bytes, account_bytes, load_bytes]
    {
        bytes = bytes.checked_add(child)?;
    }
    bytes = bytes
        .checked_add(Queue::<Work>::worst_case(routes)?)?
        .checked_add(Queue::<fleet::Event>::worst_case(limits.tasks.tasks.checked_mul(2)?)?)?
        .checked_add(Queue::<Box<tasks::RunContext>>::worst_case(limits.tasks.tasks)?)?;
    bytes = bytes.checked_add(
        u64::from(limits.tasks.tasks).checked_mul(u64::from(limits.tasks.saved_repositories).checked_mul(4)?)?,
    )?;
    bytes = bytes.checked_add(Slab::<Option<Payload>>::worst_case(payload_slots(limits)?)?)?.checked_add(
        u64::from(payload_slots(limits)?).checked_mul(
            u64::from(limits.journal.transcript_bytes)
                .max(u64::from(limits.tasks.result_bytes).checked_mul(2)?)
                .max(tool_bytes),
        )?,
    )?;
    bytes = bytes.checked_add(
        u64::from(payload_slots(limits)?).checked_mul(u64::from(limits.tasks.saved_repositories).checked_mul(4)?)?,
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
        .checked_add(Map::<Token, CallKey>::worst_case(limits.fleet.calls)?)?
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
        .checked_add(Map::<u64, Box<tasks::RunContext>>::worst_case(limits.tasks.tasks)?)?
        .checked_add(
            u64::from(limits.tasks.tasks).checked_mul(u64::from(limits.tasks.saved_repositories).checked_mul(4)?)?,
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
                .checked_add(List::<brief::Section>::worst_case(limits.brief.sections)?)?
                .checked_add(u64::from(limits.tasks.inbox_bytes))?
                .checked_add(u64::from(limits.tasks.tasks).checked_mul(u64::from(limits.tasks.message_bytes))?)?
                .checked_add(u64::from(limits.tasks.saved_repositories).checked_mul(4)?)?
                .checked_add(u64::from(limits.journal.transcript_bytes))?
                .checked_add(List::<Box<[u8]>>::worst_case(limits.journal.transcript_bytes)?)?
                .checked_add(
                    u64::from(limits.tasks.inbox_messages.checked_add(limits.tasks.tasks)?)
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
        .checked_add(Queue::<brief::Request>::worst_case(brief::max_out(&limits.brief))?)?
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
        || value.delegation.kinds.len()
            > usize::try_from(limits.authority.executors.min(limits.tasks.executor_kinds)).expect("u32 fits usize")
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
    bytes <= usize::try_from(limits.tasks.authority_bytes).expect("u32 fits usize")
}

fn text_part(first: &[u8], second: &[u8], third: &[u8], fourth: &[u8], available: u32) -> brief::Part {
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
    brief::Part {
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

fn task_section(domain: &Domain, task: u64, part: brief::TaskPart, parts: u32, bytes: u32) -> brief::Read {
    if parts == 0 {
        return brief::Read::Failed;
    }
    match part {
        brief::TaskPart::Spec => match domain.contexts.get(&task) {
            Some(context) => task_read(context, parts, bytes),
            None => brief::Read::Failed,
        },
        brief::TaskPart::Delegates => match domain.contexts.get(&task) {
            Some(context) => delegates_read(&context.delegates, bytes),
            None => brief::Read::Failed,
        },
        brief::TaskPart::Dependencies => match domain.dependency_results.get(&task) {
            Some(results) => dependency_read(results, bytes),
            None => brief::Read::Failed,
        },
        brief::TaskPart::Attempts => match domain.contexts.get(&task) {
            Some(context) => attempt_read(context.tries, bytes),
            None => brief::Read::Failed,
        },
        brief::TaskPart::TranscriptTail => match domain.transcripts.get(&task) {
            Some(transcript) => tail_read(transcript, bytes),
            None => brief::Read::Failed,
        },
    }
}

fn attempt_read(tries: tasks::Tries, bytes: u32) -> brief::Read {
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
    let mut writer = Writer::new(total.min(usize::try_from(bytes).expect("u32 fits usize")));
    for (name, count) in classes {
        if count > 0 {
            for fragment in [name, Decimal::of(u64::from(count)).as_bytes(), b"\n"] {
                let kept = prefix(fragment, writer.room());
                writer.put(kept).expect("attempt prefix fits");
            }
        }
    }
    let text = writer.finish();
    brief::Read::Got(Box::new([brief::Part {
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

fn delegates_read(delegates: &[tasks::DelegateState], bytes: u32) -> brief::Read {
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
    brief::Read::Got(Box::new([text_part(&text, b"", b"", b"", bytes)]))
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

fn dependency_read(results: &[HistoricalResult], bytes: u32) -> brief::Read {
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
        if let tasks::ResultKind::Verdict { code } = result.kind {
            total = total
                .checked_add(Decimal::of(u64::from(code)).as_bytes().len())
                .expect("bounded verdict code")
                .checked_add(1)
                .expect("verdict space");
        }
    }
    let mut writer = Writer::new(total);
    for result in results {
        writer.put(b"task ").expect("measured result text");
        writer.put(Decimal::of(result.task).as_bytes()).expect("measured result ID");
        writer.put(b": ").expect("measured result text");
        writer.put(result_label(result.kind)).expect("measured result kind");
        if let tasks::ResultKind::Verdict { code } = result.kind {
            writer.put(b" ").expect("measured verdict space");
            writer.put(Decimal::of(u64::from(code)).as_bytes()).expect("measured verdict code");
        }
        writer.put(b"\n").expect("measured result newline");
        writer.put(&result.words).expect("measured result words");
        writer.put(b"\n\n").expect("measured result separator");
    }
    let text = writer.finish();
    brief::Read::Got(Box::new([text_part(&text, b"", b"", b"", bytes)]))
}

fn tail_read(transcript: &Transcript, bytes: u32) -> brief::Read {
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
    brief::Read::Got(Box::new([brief::Part {
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
fn task_read(record: &tasks::RunContext, parts: u32, bytes: u32) -> brief::Read {
    if parts == 0 {
        return brief::Read::Failed;
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
    brief::Read::Got(gathered.into_boxed())
}

fn header_loaded(startup: Startup) -> bool {
    match startup {
        Startup::Cold | Startup::Loading(Range::Deployment) | Startup::Failed => false,
        Startup::Loading(
            Range::Calls
            | Range::Tasks
            | Range::EndedResults
            | Range::People
            | Range::RunProofs
            | Range::EscalationDecision { .. }
            | Range::Turns { .. }
            | Range::TaskTranscript { .. }
            | Range::TaskResult { .. },
        )
        | Startup::Adopting
        | Startup::Running => true,
    }
}

fn hello_within(hello: &fleet::Hello, limits: &fleet::Limits) -> bool {
    let stop_before_grace = match hello.graces {
        Some(duration) => duration < limits.grace,
        None => false,
    };
    if hello.hosting.len() > usize::try_from(limits.slots).expect("u32 fits usize")
        || hello.workstreams.len() > usize::try_from(limits.workstreams).expect("u32 fits usize")
        || !stop_before_grace
    {
        return false;
    }
    for key in &hello.workstreams {
        if key.len() > usize::try_from(limits.workstream_bytes).expect("u32 fits usize") {
            return false;
        }
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
        u64::from(tasks.saved_repositories).checked_mul(4)?,
        u64::from(tasks.parameters).checked_mul(u64::try_from(size_of::<tasks::Parameter>()).ok()?)?,
        u64::from(tasks.inputs)
            .checked_add(u64::from(tasks.dependencies).checked_mul(2)?)?
            .checked_add(u64::from(tasks.delegates))?
            .checked_mul(8)?,
        u64::from(tasks.contract_choices).checked_mul(u64::try_from(size_of::<tasks::Verdict>()).ok()?)?,
        u64::from(tasks.authority_grants).checked_mul(u64::try_from(size_of::<tasks::Grant>()).ok()?.checked_add(
            u64::from(tasks.authority_segments).checked_mul(u64::try_from(size_of::<Box<[u8]>>()).ok()?)?,
        )?)?,
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
        Event::Start
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
        | Event::Refreshed { .. }
        | Event::RefreshFailed { .. } => {}
    }
}

fn current_proof(domain: &Domain, task: u64, attempt: u64) -> bool {
    match domain.proofs.get(&task) {
        Some(proof) => proof.attempt == attempt,
        None => false,
    }
}

fn proof_turn(proof: &RunProof) -> u32 {
    match proof.turn {
        Some(turn) => turn.turn,
        None => 0,
    }
}

/// Validate one root row against transient metadata from the owned live-task
/// startup page. A current answered attempt requires a matching typed terminal;
/// topology terminals retain unchanged accepted expense.
fn valid_proof(proof: &RunProof, expected: &RestoringProof, limits: &Limits) -> bool {
    if proof.task == 0 || proof.attempt == 0 {
        return false;
    }
    let spent = match proof.turn {
        Some(turn) => {
            let read_valid = match turn.read {
                Some(number) => match proof.offered {
                    Some(high) => number <= high,
                    None => false,
                },
                None => true,
            };
            if turn.turn == 0 || !read_valid || turn.cumulative > expected.run_spent {
                return false;
            }
            turn.cumulative
        }
        None => 0,
    };
    match &proof.terminal {
        Some(terminal) => {
            terminal.task == proof.task
                && terminal.attempt == proof.attempt
                && terminal.cumulative == expected.run_spent
                && expected.last_answer == Some(proof.attempt)
                && end_bytes(&terminal.end)
                    <= u64::from(limits.tasks.result_bytes).checked_mul(2).expect("bounded terminal proof")
        }
        None => spent == expected.run_spent && expected.last_answer != Some(proof.attempt),
    }
}

fn supported_task(task: &tasks::TaskRecord, charter: u32) -> bool {
    let executor = match task.executor {
        tasks::Executor::Agent { charter: configured } => charter == configured,
    };
    task.number != 0 && task.result_position == 0 && executor
}

fn supported_proposal(task: &tasks::TaskRecord, deployment: &crate::Deployment) -> bool {
    let Some(proposal) = &task.proposal else { return true };
    if proposal.number == 0 || proposal.number > deployment.messages || proposal.proposer != task.number {
        return false;
    }
    let holder = match proposal.state {
        tasks::ProposalState::Pending { holder, .. } => holder,
        tasks::ProposalState::Accepted { .. }
        | tasks::ProposalState::Rejected { .. }
        | tasks::ProposalState::Withdrawn => return false,
    };
    let holder_valid = match holder {
        tasks::ProposalHolder::Task(number) => number != 0 && number <= deployment.tasks,
        tasks::ProposalHolder::Person(number) => number != 0 && number <= deployment.people,
        tasks::ProposalHolder::Policy { project, .. } => project == task.project,
    };
    if !holder_valid {
        return false;
    }
    match &proposal.action {
        tasks::ProposalAction::Batch(batch) => {
            for member in batch {
                if member.number == 0 || member.number > deployment.tasks {
                    return false;
                }
            }
            true
        }
        tasks::ProposalAction::Amend { task, .. }
        | tasks::ProposalAction::Widen { task, .. }
        | tasks::ProposalAction::Release { task } => *task != 0 && *task <= deployment.tasks,
    }
}

fn remember_unpriced_terminal(domain: &mut Domain, run: Token, attempt: Token, end: tasks::End) {
    let proof = domain.proofs.get_mut(&run.raw()).expect("current fleet terminal has pre-reserved proof");
    assert!(proof.attempt == attempt.raw(), "fleet terminal belongs to current proof");
    let cumulative = match proof.turn {
        Some(turn) => turn.cumulative,
        None => 0,
    };
    proof.terminal = Some(TerminalRecord { task: run.raw(), attempt: attempt.raw(), cumulative, end });
}

fn supported_requester(requester: tasks::Party, people: u64, tasks: u64) -> bool {
    match requester {
        tasks::Party::Person(person) => person != 0 && person <= people,
        tasks::Party::Task(task) => task != 0 && task <= tasks,
        tasks::Party::Deployment { .. } => true,
    }
}

fn valid_call_answer(answer: &CallAnswer, deployment: &crate::Deployment, limits: &Limits) -> bool {
    match answer {
        CallAnswer::Unavailable
        | CallAnswer::Introduced
        | CallAnswer::Unsubscribed
        | CallAnswer::Controlled
        | CallAnswer::ControlDenied { .. } => true,
        CallAnswer::Proposed { proposal } | CallAnswer::ProposalDecided { proposal, .. } => {
            *proposal != 0 && *proposal <= deployment.messages
        }
        CallAnswer::Sent { message } => *message != 0 && *message <= deployment.messages,
        CallAnswer::Subscribed { subscription } => *subscription != 0 && *subscription <= deployment.messages,
        CallAnswer::Delegated(numbers) => {
            if numbers.is_empty() || numbers.len() > usize::try_from(limits.tasks.batch).expect("u32 fits usize") {
                return false;
            }
            for (at, &number) in numbers.iter().enumerate() {
                if number == 0 || number > deployment.tasks {
                    return false;
                }
                for &earlier in numbers.iter().take(at) {
                    if number == earlier {
                        return false;
                    }
                }
            }
            true
        }
        CallAnswer::DelegationDenied { answer, findings } => {
            *answer != authority::Answer::Allow
                && findings.len()
                    <= usize::try_from(authority::max_out(&limits.authority).expect("valid authority bound"))
                        .expect("u32 fits usize")
        }
        CallAnswer::MessageRefused(problem)
        | CallAnswer::ProposalRefused(problem)
        | CallAnswer::SubscriptionRefused(problem)
        | CallAnswer::DelegationRefused(problem)
        | CallAnswer::ControlRefused(problem) => match problem.task {
            Some(task) => task != 0 && task <= deployment.tasks,
            None => true,
        },
    }
}

/// Reject unsupported root shapes and identities above durable high-water marks before child
/// restoration. Proof rows consume exact transient live-row correlations and never load archive
/// history into the live map.
fn restore_page_row(domain: &mut Domain, env: &Env<Limits>, row: Record) {
    match row {
        Record::Call(record) => {
            let key = record.key;
            let valid = key.task != 0
                && key.attempt != 0
                && key.completion != 0
                && key.task <= domain.journal.deployment().tasks
                && key.attempt <= domain.journal.deployment().runs
                && valid_call_answer(&record.answer, &domain.journal.deployment(), &env.limits)
                && match domain.proofs.get(&key.task) {
                    Some(proof) => proof.attempt >= key.attempt,
                    None => false,
                };
            if !valid || domain.calls.insert(key, record.answer).is_err() {
                domain.startup = Startup::Failed;
            }
        }
        Record::Deployment(deployment) => domain.journal = Journal::new(deployment, &env.limits.journal),
        Record::People(record) => domain.work.push(Work::People(people::Event::Restore { record })),
        Record::Tasks(record) => match record {
            tasks::Stored::Ended(_) => {
                unreachable!("historical child rows excluded from startup")
            }
            tasks::Stored::History(_) => unreachable!("history rows excluded from startup"),
            tasks::Stored::Live(ref task) => {
                if !supported_task(task, domain.config.charter)
                    || !supported_proposal(task, &domain.journal.deployment())
                    || !escalation::supported(domain, task)
                    || task.number > domain.journal.deployment().tasks
                    || task.attempt > domain.journal.deployment().runs
                    || !supported_requester(
                        task.requester,
                        domain.journal.deployment().people,
                        domain.journal.deployment().tasks,
                    )
                {
                    domain.startup = Startup::Failed;
                    return;
                }
                if task.attempt != 0
                    && domain
                        .restoring_proofs
                        .insert(
                            task.number,
                            RestoringProof {
                                attempt: task.attempt,
                                turn: task.turn,
                                run_spent: task.run_spent,
                                last_answer: task.last_answer,
                            },
                        )
                        .is_err()
                {
                    domain.startup = Startup::Failed;
                }
                domain.work.push(Work::Tasks(tasks::Event::Restore { record }));
            }
            tasks::Stored::Ledger(_) => domain.work.push(Work::Tasks(tasks::Event::Restore { record })),
        },
        Record::RunProof(proof) => {
            let valid = match domain.restoring_proofs.remove(&proof.task) {
                Some(expected) => {
                    proof.attempt == expected.attempt
                        && proof_turn(&proof) == expected.turn
                        && valid_proof(&proof, &expected, &env.limits)
                }
                None => false,
            };
            if !valid || domain.proofs.insert(proof.task, proof).is_err() {
                domain.startup = Startup::Failed;
            }
        }
        Record::Turn(_) | Record::Terminal(_) | Record::EscalationDecision(_) => {
            unreachable!("startup excludes archive families")
        }
    }
}
