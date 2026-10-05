//! The concrete 06a root: bounded child ownership and synchronous decision
//! routing for a charged person chat (domain/engine.md, 3–7 and 5.7).
//! [`Domain`] keeps task/funding state in tasks, policy in authority, identities
//! and keyed requests in people, placement in fleet, task text gathering in
//! brief and secret-free credential lifetimes in accounts. Its own state is
//! commits, loads, candidate numbers, bounded current-claim replay evidence and
//! unfinished handoffs between children (domain/engine.md, 7.5).
//!
//! The shell/protocol supplies [`Event`]s to [`step`], drains durable effects
//! through [`resume`], fires child timers through [`fire`] and reclaims at the
//! iteration boundary. Every writing route closes into one atomic commit;
//! store terminals are accepted under pressure and a failed commit stops
//! release. Startup reads the header first, pages child state and current root
//! proofs, validates their identities/expense/terminal correlation, then permits
//! child restoration consequences. It adopts every restored claim before fleet
//! `Loaded` allows new placement (domain/engine.md, 6 and 7.5).
//!
//! The root never knows file descriptors, wire encodings, kernel races,
//! repository contents or secret credential bytes (domain/engine.md, 2 and 5.5).
//! Its closed input vocabulary has no blanket child-event pass-through. Tools,
//! connectors and the broader people/run routes remain later increments (5.7).
//! Child facts are disposable observations; [`Domain::quiescent`] reports
//! internal idleness, while an external referee establishes final story results.
use crate::{
    Decision, Delivery, Family, Journal, JournalLimits, Key, Output, Range, Record, RunProof, TerminalRecord,
    TurnProof, TurnRecord, Write, loads,
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
    /// Ordered commit/write/delivery room, including all synchronous child routes (domain/engine.md, 5).
    pub journal: JournalLimits,
    /// Paged store reads; every startup page must fit whole (domain/engine.md, 5.3).
    pub loads: loads::Limits,
    /// Authentic task/funding room, also bounding root current-claim proof slots;
    /// transport evidence belongs to root (domain/tasks.md, 2 and 14; domain/engine.md, 7.5).
    pub tasks: tasks::Limits,
    /// Deployment/project policy and finding room, supplied at startup (domain/authority.md, 6).
    pub authority: authority::Limits,
    /// Identity, session and keyed request room (domain/people.md, 11).
    pub people: people::Limits,
    /// Worker attempts and unacknowledged turns (domain/worker.md, 2).
    pub fleet: fleet::Limits,
    /// Required task section gathering and cuts (domain/engine.md, 9).
    pub brief: brief::Limits,
    /// Secret-free credential policy (domain/engine.md, section 8; docs/design/credentials.md, 5).
    pub accounts: accounts::Limits,
}

/// Startup configuration owned by the root, bounded by the corresponding
/// child limits; the shell supplies it once (domain/engine.md, 4).
#[derive(Debug)]
pub struct Config {
    /// Identity committed at the first start; ignored when a header exists (domain/engine.md, 5.4).
    pub deployment: [u8; 16],
    /// Injected deterministic task backoff seed (domain/tasks.md, 5).
    pub seed: u64,
    /// Bootstrap project/identity owners supplied by deployment configuration,
    /// at most people `initial_owners`; sign-in authentication matches their
    /// identity keys rather than trusting a request's role (domain/people.md, 3.1).
    pub owners: Box<[people::InitialOwner]>,
    /// Validated deployment and project policy; no duplicated policy values (domain/authority.md, 6).
    pub authority: authority::Domain,
    /// Charter selected for chats; admitted by tasks as the configured executor (domain/people.md, 7).
    pub charter: u32,
    /// Finite period number used to address project/person funding ledgers;
    /// restoring an existing ledger never resets it (domain/authority.md, 7; domain/engine.md, 5.7).
    pub period: u64,
    /// Initial project period budget, checked against current project policy
    /// before its first opening; restored funding wins (domain/authority.md, 7; domain/engine.md, 5.7).
    pub period_budget: u64,
    /// Initial person pool budget, checked against the authenticated role's
    /// period ceiling before carving; restored funding wins (domain/people.md, 7; domain/engine.md, 5.7).
    pub person_budget: u64,
    /// Exact task authority checked for each authenticated chat (domain/authority.md, 8.4).
    pub chat_authority: authority::Authority,
    /// Account required by the chat charter, configured through the accounts child (domain/engine.md, section 8; docs/design/credentials.md, 5).
    pub account: u32,
    /// Existing secret-free generation at startup (domain/engine.md, section 8; docs/design/credentials.md, 5).
    pub account_generation: u64,
    /// Startup credential lifetime; a refresh is required when absent (domain/engine.md, section 8; docs/design/credentials.md, 5).
    pub account_valid: Option<skein_lib::Duration>,
}

/// A complete claim's assignment, root to worker after durability; its
/// task section is bounded by brief limits (domain/engine.md, 7.1 and 9).
/// Its attempt ends through the worker answer route; the worker keeps its
/// terminal body until a durable ACK, with channel loss handled by fleet grace.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Assignment {
    /// Root-issued durable task number (domain/tasks.md, 2).
    pub task: u64,
    /// Fresh root-issued attempt number (domain/engine.md, 7.1).
    pub attempt: u64,
    /// Configured charter, never selected by the worker (domain/people.md, 7).
    pub charter: u32,
    /// Owned required task section, at most brief `sections` and `brief_bytes`;
    /// includes specification, report contract and the actual person requester.
    /// Task/deployment lineage awaits its actual root route (domain/engine.md, 5.7, 7.5 and 9).
    pub sections: Box<[brief::Section]>,
    /// Secret-free account grant; token bytes stay in the protocol (domain/engine.md, section 8; docs/design/credentials.md, 5).
    pub grant: accounts::Grant,
}

/// Worker to root: one numbered transcript and cumulative spend, retained
/// by its sender until the matching ACK (domain/engine.md, 7.2).
#[derive(PartialEq, Eq, Debug)]
pub struct Turn {
    /// Positive consecutive worker turn; fleet fences accepted/pending duplicates
    /// before child admission using the restored kept turn. Root owns durable
    /// latest-turn evidence (domain/engine.md, 7.2 and 7.5; domain/tasks.md, 14).
    pub number: u32,
    /// Worker-priced cumulative spend for this attempt, not a new delta. Tasks
    /// validates monotonicity and charges the accepted delta atomically
    /// (domain/authority.md, 7; domain/engine.md, 5.7 and 7.2).
    pub cumulative: u64,
    /// Must be `None` in this actual route; nonempty read fences are refused
    /// until a real inbox route joins (domain/tasks.md, 14; domain/engine.md, 7.5).
    pub read: Option<u64>,
    /// Owned transcript, bounded by journal transcript bytes before fleet admission (domain/engine.md, 7.2).
    pub transcript: Box<[u8]>,
}

/// External inputs to the walking root; store terminals always enter,
/// worker bodies are bounded before retention (domain/engine.md, 5–7).
/// Web calls own one terminal reply right. Valid current worker bodies are
/// acknowledged after durability or refused for retry; stale worker input may
/// be dropped by fleet. Store and refresh variants are terminals, not new calls.
#[derive(Debug)]
pub enum Event {
    /// Shell to root: begin cold paged restoration and account setup. Repeated
    /// starts are inert; readiness follows all restored/adopted claims, or a
    /// terminal startup/storage failure emits `Stop` (domain/engine.md, 5.7 and 6).
    Start,
    /// Store's cumulative successful terminal for issued commits (domain/engine.md, 5.1).
    Committed {
        /// Store-echoed positive commit number, at most the latest issued commit;
        /// older cumulative answers are inert (domain/engine.md, 5.1).
        number: u64,
    },
    /// Store's failed terminal; root stops without releasing held work (domain/engine.md, 5.1).
    Uncommitted {
        /// Store-echoed issued commit that failed; failures at/before durable progress
        /// or after stop are inert (domain/engine.md, 5.1).
        number: u64,
    },
    /// Store's one page terminal for an issued load; decoded bounds are rechecked (domain/engine.md, 5.3).
    Loaded {
        /// Echoed generational load token, fenced after its one terminal and reclaim (domain/engine.md, 5.3).
        owner: Token,
        /// Owned decoded rows, at most `loads::Limits::rows` and `loads::Limits::reply_bytes` before restoration (domain/engine.md, 5.3).
        rows: Box<[Record]>,
        /// Exclusive last-row continuation in the requested range, or terminal page (domain/engine.md, 5.3).
        next: Option<Key>,
    },
    /// Store's failed terminal for a page (domain/engine.md, 5.3).
    Unloaded {
        /// Echoed generational load token, fenced after its one terminal and reclaim (domain/engine.md, 5.3).
        owner: Token,
    },
    /// Web supplies authenticated identity; root allocates person and sign-in candidates (domain/people.md, 3).
    SignedIn {
        /// Web-issued right to this one sign-in reply, returned busy immediately or
        /// moved through people to a durable terminal (domain/people.md, 3 and 11).
        reply_to: ReplyTo,
        /// Protocol-authenticated forge/user key and display bytes, bounded by people `identity_bytes` (domain/people.md, 3).
        identity: people::Identity,
    },
    /// Web's keyed chat request; the people child authenticates session and role (domain/people.md, 5.1).
    Ask {
        /// Web-issued right to one keyed request terminal; duplicates can join bounded
        /// people waiters, each still replied to once (domain/people.md, 5.1.1 and 11).
        reply_to: ReplyTo,
        /// Root-issued durable secret-free session number; authenticated and expired by people (domain/people.md, 3).
        sign_in: u64,
        /// Web-issued fixed request key, scoped by person and persisted with the exact typed ask (domain/people.md, 5.1.1).
        key: [u8; 16],
        /// Typed chat request with words bounded by people words before routing (domain/people.md, 5.1).
        ask: people::Ask,
    },
    /// Worker first reports its bounded slots and hosted attempts (domain/worker.md, 2).
    Hello {
        /// Protocol-issued opaque channel identity; at most fleet `workers` are retained
        /// cold, and fleet admits the live channel (domain/engine.md, 5.7; domain/worker.md, 2).
        channel: Token,
        /// Worker slot/host/workstream report, bounded by fleet limits before cold retention (domain/worker.md, 2).
        hello: fleet::Hello,
    },
    /// Protocol to root: channel closed. Known cold losses coalesce; unknown
    /// losses are inert. Live loss changes fleet topology/deadlines immediately
    /// without a commit or outward terminal (domain/engine.md, 5.7; domain/worker.md, 2).
    Lost {
        /// Protocol-issued identity of the channel that closed; duplicates/unknown
        /// channels consume no queued handoff room (domain/engine.md, 5.7).
        channel: Token,
    },
    /// Worker to root: numbered body for its current claim. A valid admitted
    /// turn ends in a durable ACK; pressure is a retry notice and stale bodies
    /// may be dropped (domain/engine.md, 5.1 and 7.2).
    Turn {
        /// Worker protocol sender; fleet validates it against the current attempt
        /// before handing the owned body to tasks (domain/worker.md, 2 and 8).
        channel: Token,
        /// Positive durable task number owning this turn; fleet treats it as an
        /// opaque run identity (domain/engine.md, 7.2; domain/tasks.md, 2).
        task: u64,
        /// Positive root-issued activation number; fleet fences stale worker bodies (domain/engine.md, 7.1).
        attempt: u64,
        /// Owned numbered transcript and cumulative charge; the root bounds bytes
        /// before payload retention (domain/engine.md, 5.7 and 7.2).
        turn: Turn,
    },
    /// Worker to root: attempt terminal retained until ACK. Accepted charge and
    /// task state commit first; pressure asks for retry. A refused charged
    /// terminal becomes an uncharged invalid activation before its durable ACK
    /// (domain/engine.md, 5.7 and 7.4).
    Answer {
        /// Worker protocol sender checked by fleet against the hosted attempt
        /// (domain/worker.md, 2 and 8).
        channel: Token,
        /// Durable task number for the answered attempt; it is not a fresh task
        /// allocation (domain/engine.md, 7.4; domain/tasks.md, 2).
        task: u64,
        /// Positive root-issued activation number; fleet fences stale worker bodies (domain/engine.md, 7.1).
        attempt: u64,
        /// Whole priced spend for this attempt; tasks checks monotonicity and
        /// semantic admission atomically, while root/fleet owns transport replay
        /// evidence and fencing (domain/engine.md, 7.2, 7.4 and 7.5).
        cumulative: u64,
        /// Owned task terminal; root retained payload bytes are at most twice tasks
        /// `result_bytes`, then tasks checks result/contract admission against
        /// its stricter result bound (domain/engine.md, 5.7 and 7.4).
        end: tasks::End,
    },
    /// Web to root: read a named historical result. People validates the session;
    /// the loaded ended task must name its person as requester. Ends once with
    /// `ResultReply` or a refused `WebReply` (domain/engine.md, 5.7; domain/people.md, 6).
    ReadResult {
        /// Web-issued right moved into one bounded result waiter; consumes one
        /// terminal result/refusal reply (domain/engine.md, 5.7; domain/people.md, 11).
        reply_to: ReplyTo,
        /// Root-issued durable secret-free session number; authenticated and expired by people (domain/people.md, 3).
        sign_in: u64,
        /// Positive ended task number selected by the reader; its stored requester
        /// must match the authenticated person (domain/people.md, 6).
        task: u64,
    },
    /// Account protocol completes a refresh with secret-free lifetime (domain/engine.md, section 8; docs/design/credentials.md, 5).
    Refreshed {
        /// Configured secret-free account number; bounded by accounts account room (domain/engine.md, 8).
        account: u32,
        /// Echoed account refresh generation; stale completions change nothing (domain/engine.md, 8).
        generation: u64,
        /// Remaining credential lifetime, represented by a u64 duration; no token bytes cross the domain (domain/engine.md, 8).
        valid: skein_lib::Duration,
    },
    /// Account protocol ends its refresh unsuccessfully (domain/engine.md, section 8; docs/design/credentials.md, 5).
    RefreshFailed {
        /// Configured secret-free account number; bounded by accounts account room (domain/engine.md, 8).
        account: u32,
        /// Echoed account refresh generation; stale completions change nothing (domain/engine.md, 8).
        generation: u64,
        /// Terminal account failure class, including retry delay where required (domain/engine.md, 8).
        failure: accounts::Failure,
    },
}

/// Root to shell/worker/web. Commit and load requests each have one store
/// terminal; deliveries are notices or consume a `ReplyTo` (domain/engine.md, 5).
#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    /// Ordered atomic transaction; store ends with Committed or Uncommitted (domain/engine.md, 5.1).
    Commit {
        /// Positive ordered commit number allocated by the journal, echoed by the
        /// store terminal; checked `u64` exhaustion stops admission (domain/engine.md, 5.1).
        number: u64,
        /// Owned unique-key transaction, at most journal writes including its deployment header; applied atomically (domain/engine.md, 5.1).
        writes: Box<[Write]>,
    },
    /// Paged read, issued only after its prerequisite commits are durable (domain/engine.md, 5.3).
    Load {
        /// Fresh generational load identity issued by root loads; store echoes it
        /// once through `Loaded` or `Unloaded` (domain/engine.md, 5.3).
        owner: Token,
        /// Closed store-key family; every page key is checked for membership (domain/engine.md, 5.3).
        range: Range,
        /// Exclusive cursor in this range, or its beginning (domain/engine.md, 5.3).
        after: Option<Key>,
        /// Positive page demand, at most the configured load row bound (domain/engine.md, 5.3).
        most: u32,
        /// Hard decoded-page byte limit, enforced before and after protocol decoding (domain/engine.md, 5.3).
        bytes: u32,
    },
    /// Held durable outward delivery; internal fleet callbacks never reach the shell (domain/engine.md, 5.2).
    Deliver(
        /// Owned bounded effect, released once after the commit it follows (domain/engine.md, 5.2).
        Delivery,
    ),
    /// Root to account protocol: refresh/keep actions or secret-free notices.
    /// Refresh/keep ends through `Refreshed` or `RefreshFailed`; grant and
    /// availability notices require no root terminal (domain/engine.md, 8; docs/design/credentials.md, 5–6).
    Account(
        /// Fixed-size accounts request/notice; token values are filled only by
        /// the protocol and never retained here (domain/engine.md, 8; docs/design/credentials.md, 4–6).
        accounts::Request,
    ),
    /// Retry notice for a turn that could not enter a decision; worker retains body (domain/worker.md, 2).
    TurnBusy {
        /// Worker protocol destination echoed from its refused turn; a fixed-size
        /// identity, without admitting another worker (domain/engine.md, 5.1).
        channel: Token,
        /// Task number echoed from the refused turn; this notice does not validate
        /// or allocate the task (domain/engine.md, 5.1 and 7.2).
        task: u64,
        /// Attempt number echoed for the worker to fence the retry; no activation
        /// is admitted by this notice (domain/engine.md, 5.1 and 7.2).
        attempt: u64,
        /// Turn number echoed from the refused body, represented by `u32`; worker
        /// retains that body for retry (domain/engine.md, 5.1 and 7.2).
        turn: u32,
    },
    /// Root to worker: answer could not enter a decision; retain and retry it
    /// after backoff, without charging (domain/engine.md, 5.1).
    AnswerBusy {
        /// Worker protocol destination echoed from the refused terminal; no new
        /// worker is retained (domain/engine.md, 5.1).
        channel: Token,
        /// Task number echoed from the refused answer; no task or charge is admitted
        /// (domain/engine.md, 5.1 and 7.4).
        task: u64,
        /// Attempt number echoed for the worker to fence its retained terminal
        /// and retry (domain/engine.md, 5.1 and 7.4).
        attempt: u64,
    },
    /// Root to shell: failed storage/startup requires stopping this process;
    /// no terminal is owed to this notice and held effects remain unreleased
    /// (domain/engine.md, 5.1 and 5.7).
    Stop,
}

#[derive(Debug)]
enum Work {
    Tasks(tasks::Event),
    People(people::Event),
    Fleet(fleet::Event),
    Brief(brief::Event),
    Activate(Box<tasks::RunContext>),
}

#[derive(Debug)]
enum Payload {
    Turn { task: u64, attempt: u64, body: Turn },
    Answer { task: u64, attempt: u64, cumulative: u64, end: tasks::End },
}

#[derive(Debug)]
struct ResultRead {
    to: ReplyTo,
    person: u64,
    task: u64,
}

#[derive(Debug)]
struct RestoringProof {
    attempt: u64,
    turn: u32,
    run_spent: u64,
    last_answer: Option<u64>,
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
/// stays in tasks (domain/engine.md, 3–5 and 7.5; domain/tasks.md, 14).
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
    result_reads: Slab<Option<ResultRead>>,
    made: Map<Token, u64>,
    claiming: Map<u64, u64>,
    contexts: Map<u64, Box<tasks::RunContext>>,
    proofs: Map<u64, RunProof>,
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
    /// Allocate the root and fixed child room from shell-supplied configuration
    /// and validated cross-child limits. Checks bounded authority/bootstrap
    /// configuration; issues no request. Startup pages/account setup begin only
    /// on `Event::Start` (domain/engine.md, 4, 5.7 and 6).
    #[must_use]
    pub fn new(mut config: Config, limits: &Limits) -> Domain {
        assert!(worst_case(limits).is_some(), "root limits are valid");
        assert!(*config.authority.limits() == limits.authority, "root prices its exact authority limits");
        assert!(authority_within(&config.chat_authority, limits), "chat authority shape bounded before copying");
        let mut projects = List::with_capacity(limits.people.projects);
        for owner in &config.owners {
            assert!(config.authority.policy(owner.project).is_some(), "bootstrap owner names configured policy");
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
            made: Map::with_capacity(limits.people.pending),
            claiming: Map::with_capacity(limits.tasks.tasks),
            contexts: Map::with_capacity(limits.tasks.tasks),
            proofs: Map::with_capacity(limits.tasks.tasks),
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

    /// Pure readiness query: true only after all child/current proof pages
    /// validate, tasks restoration consequences finish routing, and every
    /// restored claim reaches fleet before `Loaded`. New work enters only then
    /// (domain/engine.md, 6 and 7.5).
    #[must_use]
    pub fn ready(&self) -> bool {
        self.startup == Startup::Running && !self.journal.stopped()
    }

    /// Pure shell idle query: no immediate internal handoff or issued store operation
    /// remains unfinished. Call after the iteration's reclaim; sessions, idle
    /// workers, assigned workers awaiting external answers and future task/account
    /// timers are permitted. Story completion also requires the shell's independent
    /// final result condition. This query never cancels work (domain/engine.md, 5.7).
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
            && self.made.is_empty()
            && self.claiming.is_empty()
            && self.contexts.is_empty()
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

    /// Pure snapshot query for the latest allocated deployment counters; these
    /// may run ahead of store durability. Exposes neither children nor owned
    /// handoff bodies and emits no effect (domain/engine.md, 5.1 and 5.4).
    #[must_use]
    pub fn deployment(&self) -> crate::Deployment {
        self.journal.deployment()
    }

    /// Retired IO/body/child slots are reclaimed at iteration end, after
    /// every event and ready pass. Bounded child/slab bookkeeping releases only
    /// retired entries; issued loads still awaiting a terminal remain owned.
    /// Emits no request or terminal (domain/engine.md, 5; programming-model.md, 2).
    pub fn reclaim(&mut self) {
        self.tasks.reclaim();
        self.people.reclaim();
        self.fleet.reclaim();
        self.brief.reclaim();
        self.payloads.reclaim();
        self.result_reads.reclaim();
        loads::reclaim(&mut self.loads);
    }

    /// Shell/root caller discards every currently queued child observation,
    /// scanning at most each child's configured fact capacity. Emits no effect
    /// or terminal and changes no decision state (domain/engine.md, 14).
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

/// One outward commit, or one ready delivery, or a bounded account step;
/// reserve before every `step`, `resume` and `fire` call. Pure constant query,
/// four output slots; emits no effect or terminal (domain/engine.md, 5.2 and 5.7).
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

/// Route one admitted input and all synchronous child callbacks in one
/// decision; store terminals are accepted even under pressure (domain/engine.md, 5.1).
/// The shell reserves `max_out` output slots and supplies unchanged configured
/// limits with injected time. Refused web/worker calls get terminal/busy notices
/// before child mutation; admitted effects may wait in the journal until store
/// durability. Account operations keep their own secret-free terminal contract
/// (domain/engine.md, 5.7 and 8).
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
        Event::Answer { channel, task, attempt, cumulative, end } => {
            if !domain.ready() || !admits(domain, &env.limits) {
                out.push(Request::AnswerBusy { channel, task, attempt });
                return;
            }
            if end_bytes(&end) > u64::from(env.limits.tasks.result_bytes).checked_mul(2).expect("bounded result bytes")
            {
                out.push(Request::AnswerBusy { channel, task, attempt });
                return;
            }
            let Ok(id) = domain.payloads.insert(Some(Payload::Answer { task, attempt, cumulative, end })) else {
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
        Event::ReadResult { reply_to, sign_in, task } => {
            if task == 0 {
                out.push(Request::Deliver(Delivery::WebReply {
                    to: reply_to,
                    sign_in: None,
                    reply: people::Reply::Refused(people::Refusal::Unknown),
                }));
                return;
            }
            let Some(person) = domain.people.person(sign_in, env.now, env.wall) else {
                out.push(Request::Deliver(Delivery::WebReply {
                    to: reply_to,
                    sign_in: None,
                    reply: people::Reply::Refused(people::Refusal::SignIn),
                }));
                return;
            };
            if !domain.ready() || !admits(domain, &env.limits) {
                out.push(Request::Deliver(Delivery::WebReply {
                    to: reply_to,
                    sign_in: None,
                    reply: people::Reply::Refused(people::Refusal::Busy),
                }));
                return;
            }
            let id = match domain.result_reads.insert(Some(ResultRead { to: reply_to, person, task })) {
                Ok(id) => id,
                Err(read) => {
                    let read = read.expect("unadmitted result read retains reply");
                    out.push(Request::Deliver(Delivery::WebReply {
                        to: read.to,
                        sign_in: None,
                        reply: people::Reply::Refused(people::Refusal::Busy),
                    }));
                    return;
                }
            };
            let mut decision = Decision::new(&env.limits.journal);
            emit(&mut decision, &env.limits, Delivery::ReadResult { waiter: id.token() });
            close(domain, env, decision, out);
            return;
        }
    }
    let decision = route(domain, env);
    domain.signing_in = None;
    close(domain, env, decision, out);
}

fn admits(domain: &Domain, limits: &Limits) -> bool {
    crate::takes(&domain.journal, &limits.journal)
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

/// Release one held effect after durability, preserving internal callbacks
/// while journal pressure is full (domain/engine.md, 5.2).
/// The shell reserves `max_out` output slots. Deferred callbacks run before
/// another held callback is consumed; other ready work may produce one commit.
/// This pass never waits for IO or drops a retained callback on pressure
/// (domain/engine.md, 5.7).
pub fn resume(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    assert!(out.room() >= max_out(&env.limits), "root ready output room");
    if domain.journal.stopped() || domain.startup == Startup::Failed {
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
            Output::Deliver(Delivery::Load { waiter, range, after }) => {
                request_load(domain, waiter, range, after, out);
                return;
            }
            Output::Deliver(Delivery::ReadResult { waiter }) => {
                let Some(read) = domain.result_reads.get(Id::from_token(waiter)) else {
                    return;
                };
                let Some(read) = read else {
                    return;
                };
                request_load(domain, waiter, Range::TaskResult { task: read.task }, None, out);
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

/// Fire participating child timers through the same barrier; store
/// durability and inputs run before this pass (domain/engine.md, 5 and 7).
/// The shell reserves `max_out` slots and supplies injected monotonic/wall time.
/// Account timers may emit bounded protocol actions independently; root routes
/// that mutate tasks/fleet wait for whole-decision admission. Their store and
/// account outcomes enter later through `step` (domain/engine.md, 5.7 and 8).
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
    tasks_outputs(domain, env, &mut decision, &mut tasks_out);
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
                tasks_outputs(domain, env, decision, &mut out);
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
            Work::Activate(task) => activate(domain, env, task),
        }
    }
    assert!(domain.work.is_empty(), "finite synchronous root handoffs finish within the configured route bound");
}

fn people_outputs(domain: &mut Domain, env: &Env<Limits>, decision: &mut Decision, out: &mut Queue<people::Request>) {
    for _ in 0..out.len() {
        match out.pop().expect("people output count") {
            people::Request::Save { record } => save(decision, &env.limits, Write::Save(Record::People(record))),
            people::Request::Erase { key } => save(decision, &env.limits, Write::Erase(Key::People(key))),
            people::Request::Reply { to, reply } => {
                emit(decision, &env.limits, Delivery::WebReply { to, sign_in: domain.signing_in, reply });
            }
            people::Request::Route { request, person, project, role, ask } => {
                make_chat(domain, env, request, person, project, role, ask);
            }
            people::Request::RolesRefused { .. } | people::Request::RestoreRefused { .. } => {
                domain.startup = Startup::Failed;
            }
        }
    }
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
    let role_number = match role {
        people::Role::Owner => 0,
        people::Role::Maintainer => 1,
        people::Role::Member => 2,
        people::Role::Observer => 3,
    };
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
    if domain.config.period_budget > policy.period_spend || domain.config.person_budget > role_policy.period_spend {
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
        }]),
    }));
}

/// Consume the actual child activation context for authority/account readiness
/// and the person-chat brief; retain only credential waits and drop the context
/// on claim/failure (domain/engine.md, 5.7, 7.1, 7.5 and 9).
fn activate(domain: &mut Domain, env: &Env<Limits>, task: Box<tasks::RunContext>) {
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
    assert!(domain.contexts.insert(number, task).is_ok(), "bounded activation context");
    domain.work.push(Work::Tasks(tasks::Event::Prepare { reply_to: internal(number), task: number }));
    domain.work.push(Work::Brief(brief::Event::Render {
        reply_to: internal(number),
        sections: Box::new([brief::Wanted { source: brief::Source::Task { task: number }, required: true }]),
    }));
}

#[expect(clippy::too_many_lines, reason = "the closed child vocabulary is routed exhaustively inside one decision")]
fn tasks_outputs(domain: &mut Domain, env: &Env<Limits>, decision: &mut Decision, out: &mut Queue<tasks::Request>) {
    for _ in 0..out.len() {
        match out.pop().expect("tasks output count") {
            tasks::Request::Save { record } => save(decision, &env.limits, Write::Save(Record::Tasks(record))),
            tasks::Request::Erase { key } => save(decision, &env.limits, Write::Erase(Key::Tasks(key))),
            tasks::Request::Made { reply_to, tasks } => {
                let request = reply_to.into_token();
                let expected = domain.made.remove(&request).expect("pending make route");
                assert!(tasks.as_ref() == [expected], "one exact chat created");
                domain.work.push(Work::People(people::Event::Decided {
                    request,
                    outcome: people::Outcome::Started { task: expected },
                }));
            }
            tasks::Request::Refused { reply_to, problem } => {
                let token = reply_to.into_token();
                if domain.made.remove(&token).is_some() {
                    domain.work.push(Work::People(people::Event::Decided {
                        request: token,
                        outcome: people::Outcome::Refused(people::Refusal::Limit),
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
                                    cause: tasks::Cause::Unpriced,
                                }));
                            }
                        }
                    }
                }
            }
            tasks::Request::TurnAcknowledged { reply_to, task, attempt, turn, accepted } => {
                let token = reply_to.into_token();
                let payload = take_payload(domain, token).expect("charged turn owns payload");
                let body = match payload {
                    Payload::Turn { body, .. } => body,
                    Payload::Answer { .. } => unreachable!("turn family"),
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
                drop(domain.proofs.remove(&task));
                save(decision, &env.limits, Write::Erase(Key::RunProof { task }));
                match requester {
                    tasks::Party::Person(person) => {
                        emit(decision, &env.limits, Delivery::Result { person, task, words: ending_words(ending) });
                    }
                    tasks::Party::Task(_) | tasks::Party::Deployment { .. } => {
                        unreachable!("06a only makes person chats")
                    }
                }
            }
            tasks::Request::Done { reply_to } => {
                let task = reply_to.into_token().raw();
                if let Some(attempt) = domain.claiming.remove(&task) {
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
                    brief::Source::Task { task } => match domain.contexts.get(&task) {
                        Some(record) => task_read(record, parts, bytes),
                        None => brief::Read::Failed,
                    },
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
                drop(domain.contexts.remove(&task));
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
                assert!(
                    domain.proofs.insert(task, RunProof { task, attempt, turn: None, terminal: None }).is_ok(),
                    "claim proof reserved before child mutation"
                );
                let assignment = Assignment { task, attempt, charter: domain.config.charter, sections, grant };
                assert!(domain.assignments.insert(task, assignment).is_ok(), "assignment fits live task room");
                assert!(domain.claiming.insert(task, attempt) == Ok(None), "one pending claim per task");
                domain.work.push(Work::Tasks(tasks::Event::Claim { reply_to: internal(task), task, attempt }));
            }
            brief::Request::Failed { reply_to, .. } | brief::Request::Refused { reply_to, .. } => {
                let task = reply_to.into_token().raw();
                drop(domain.contexts.remove(&task));
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
                    Payload::Answer { .. } => unreachable!("fleet returns turn family"),
                };
                domain.work.push(Work::Tasks(tasks::Event::Turn {
                    reply_to: ReplyTo::new(body),
                    task: run.raw(),
                    attempt: attempt.raw(),
                    turn,
                    read: payload.read,
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
                let (cumulative, end) = match body.as_mut().expect("fleet returns owned payload") {
                    Payload::Answer { cumulative, end, .. } => (*cumulative, end.clone()),
                    Payload::Turn { .. } => unreachable!("fleet returns answer family"),
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
                remember_unpriced_terminal(domain, run, attempt, tasks::End::Failed(tasks::Class::Lost));
                domain.work.push(Work::Tasks(tasks::Event::Activation {
                    reply_to: internal(u64::MAX),
                    task: run.raw(),
                    attempt: attempt.raw(),
                    end: tasks::End::Failed(tasks::Class::Lost),
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
                    cause: tasks::Cause::Unpriced,
                }));
            }
            fleet::Request::Grant { .. }
            | fleet::Request::Rejected { .. }
            | fleet::Request::Exhausted { .. }
            | fleet::Request::Inbound { .. }
            | fleet::Request::Relayed { .. }
            | fleet::Request::Relay { .. }
            | fleet::Request::Bounced { .. }
            | fleet::Request::Undelivered { .. }
            | fleet::Request::Told { .. } => unreachable!("06a does not route tool/credential worker messages"),
        }
    }
}

fn request_load(domain: &mut Domain, waiter: Token, range: Range, after: Option<Key>, out: &mut Queue<Request>) {
    let mut load_out = Queue::with_capacity(1);
    let most = match range {
        Range::Deployment | Range::TaskResult { .. } => 1,
        Range::Tasks | Range::People | Range::RunProofs | Range::Turns { .. } => domain.limits.loads.rows,
    };
    assert!(
        loads::begin(&mut domain.loads, waiter, range, after, most, &mut load_out).is_some(),
        "startup/read load room reserved"
    );
    match load_out.pop().expect("load issued") {
        loads::Request::Load { owner, range, after, most, bytes } => {
            out.push(Request::Load { owner, range, after, most, bytes });
        }
        loads::Request::Loaded { .. } | loads::Request::Unloaded { .. } => unreachable!("begin only issues IO"),
    }
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
                if cut.is_some() {
                    domain.startup = Startup::Failed;
                    out.push(Request::Stop);
                    return;
                }
                if waiter == Token::new(u64::MAX) {
                    startup_page(domain, env, rows, next, out);
                } else {
                    result_page(domain, waiter, rows, out);
                }
            }
            loads::Request::Unloaded { waiter, .. } => {
                if waiter == Token::new(u64::MAX) {
                    domain.startup = Startup::Failed;
                    out.push(Request::Stop);
                } else if let Some(read) = take_read(domain, waiter) {
                    domain.result_reads.retire(Id::from_token(waiter));
                    out.push(Request::Deliver(Delivery::WebReply {
                        to: read.to,
                        sign_in: None,
                        reply: people::Reply::Refused(people::Refusal::Unknown),
                    }));
                }
            }
            loads::Request::Load { .. } => unreachable!("terminal methods never issue IO"),
        }
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
        Range::RunProofs => None,
        Range::Turns { .. } | Range::TaskResult { .. } => unreachable!("startup range"),
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

fn result_page(domain: &mut Domain, waiter: Token, rows: Box<[Record]>, out: &mut Queue<Request>) {
    let id = Id::from_token(waiter);
    let Some(read) = take_read(domain, waiter) else {
        return;
    };
    domain.result_reads.retire(id);
    let mut record = None;
    for row in rows {
        match row {
            Record::Tasks(tasks::Stored::Ended(task)) => record = Some(task),
            Record::Tasks(tasks::Stored::Live(_) | tasks::Stored::Closure(_) | tasks::Stored::Ledger(_))
            | Record::Deployment(_)
            | Record::People(_)
            | Record::Turn(_)
            | Record::RunProof(_)
            | Record::Terminal(_) => {
                unreachable!("result range only contains ended row")
            }
        }
    }
    if let Some(task) = record
        && task.number == read.task
        && task.requester == tasks::Party::Person(read.person)
    {
        let ending = match task.phase {
            tasks::Phase::Ended(ending) => Some(ending),
            tasks::Phase::Waiting | tasks::Phase::Active(_) | tasks::Phase::Closing(_) | tasks::Phase::Held { .. } => {
                None
            }
        };
        let Some(ending) = ending else {
            out.push(Request::Deliver(Delivery::WebReply {
                to: read.to,
                sign_in: None,
                reply: people::Reply::Refused(people::Refusal::Unknown),
            }));
            return;
        };
        let words = ending_words(ending);
        if words.len() > usize::try_from(domain.limits.journal.result_bytes).expect("u32 fits usize") {
            out.push(Request::Deliver(Delivery::WebReply {
                to: read.to,
                sign_in: None,
                reply: people::Reply::Refused(people::Refusal::Limit),
            }));
            return;
        }
        out.push(Request::Deliver(Delivery::ResultReply { to: read.to, person: read.person, task: read.task, words }));
        return;
    }
    out.push(Request::Deliver(Delivery::WebReply {
        to: read.to,
        sign_in: None,
        reply: people::Reply::Refused(people::Refusal::Unknown),
    }));
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
            authority::Last::None => tasks::Last::None,
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
            tasks::Last::None => authority::Last::None,
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
    limits.fleet.turns.checked_add(limits.fleet.attempts)?.checked_mul(2)
}

fn route_bound(limits: &Limits) -> Option<u32> {
    tasks::max_out(&limits.tasks)
        .checked_mul(8)?
        .checked_add(people::max_out(&limits.people).checked_mul(4)?)?
        .checked_add(fleet::max_out(&limits.fleet).checked_mul(4)?)
}

/// Count participating child state, fixed handoffs, decoded input and all
/// simultaneously retained owned payloads, including current proofs, terminal
/// scratch copies, transient restore correlations and preparation contexts.
/// Child limits validate before route capacity arithmetic; malformed or
/// unrepresentable bounds return `None` (domain/engine.md, 4, 5.3 and 7.5).
/// Pure startup heap calculation, excluding allocator overhead. Child declared
/// output bounds must already be representable; checked root arithmetic or
/// refused child/cross-route bounds return `None`. It allocates no state and
/// emits no request or terminal (domain/engine.md, 4 and 5.7).
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    let task_bytes = tasks::worst_case(&limits.tasks)?;
    let fleet_bytes = fleet::worst_case(&limits.fleet)?;
    let people_bytes = people::worst_case(&limits.people)?;
    let authority_bytes = authority::worst_case(&limits.authority)?;
    let brief_bytes = brief::worst_case(&limits.brief)?;
    let account_bytes = accounts::worst_case(&limits.accounts)?;
    let load_bytes = loads::worst_case(&limits.loads)?;
    let routes = route_bound(limits)?;
    if limits.journal.writes < routes
        || limits.journal.deliveries < limits.tasks.tasks.checked_mul(4)?.checked_add(8)?
        || limits.loads.loads < 2
        || limits.journal.held < limits.journal.deliveries.checked_mul(3)?
        || limits.journal.deliveries
            < limits.fleet.workers.checked_mul(fleet::max_out(&limits.fleet))?.checked_add(1)?
        || limits.loads.rows.checked_add(limits.fleet.workers)? > routes
        || limits.journal.result_bytes < limits.brief.brief_bytes
        || limits.journal.deliveries < limits.brief.sections
        || limits.journal.result_bytes < limits.tasks.result_bytes
        || limits.fleet.workstream_bytes < 8
        || u64::from(limits.journal.transcript_bytes) < row_bound(limits)?
    {
        return None;
    }
    let mut bytes = crate::worst_case(&limits.journal)?;
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
    bytes = bytes.checked_add(Slab::<Option<Payload>>::worst_case(payload_slots(limits)?)?)?.checked_add(
        u64::from(payload_slots(limits)?).checked_mul(
            u64::from(limits.journal.transcript_bytes).max(u64::from(limits.tasks.result_bytes).checked_mul(2)?),
        )?,
    )?;
    bytes = bytes.checked_add(Slab::<Option<ResultRead>>::worst_case(limits.loads.loads)?)?.checked_add(Map::<
        Token,
        u64,
    >::worst_case(
        limits.people.pending,
    )?)?;
    bytes = bytes
        .checked_add(Map::<u64, u64>::worst_case(limits.tasks.tasks)?)?
        .checked_add(Map::<u64, RunProof>::worst_case(limits.tasks.tasks)?)?
        .checked_add(Map::<u64, RestoringProof>::worst_case(limits.tasks.tasks)?)?
        .checked_add(Map::<u64, Box<tasks::RunContext>>::worst_case(limits.tasks.tasks)?)?
        .checked_add(u64::from(limits.tasks.tasks).checked_mul(u64::from(limits.tasks.result_bytes).checked_mul(2)?)?)?
        .checked_add(u64::from(limits.tasks.result_bytes).checked_mul(6)?)?
        .checked_add(u64::from(limits.tasks.tasks).checked_mul(row_bound(limits)?.checked_mul(2)?)?)?;
    bytes = bytes.checked_add(Map::<u64, Assignment>::worst_case(limits.tasks.tasks)?)?.checked_add(
        u64::from(limits.tasks.tasks).checked_add(u64::from(limits.journal.held))?.checked_mul(
            u64::from(limits.brief.brief_bytes)
                .checked_add(List::<brief::Section>::worst_case(limits.brief.sections)?)?,
        )?,
    )?;
    bytes
        .checked_add(Queue::<authority::Finding>::worst_case(authority::max_out(&limits.authority)?)?)?
        .checked_add(Queue::<tasks::Request>::worst_case(tasks::max_out(&limits.tasks))?)?
        .checked_add(Queue::<people::Request>::worst_case(people::max_out(&limits.people))?)?
        .checked_add(Queue::<fleet::Request>::worst_case(fleet::max_out(&limits.fleet))?)?
        .checked_add(Queue::<brief::Request>::worst_case(brief::max_out(&limits.brief))?)?
        .checked_add(Queue::<Request>::worst_case(max_out(limits))?)?
        .checked_add(Queue::<Output>::worst_case(1)?)?
        .checked_add(Queue::<loads::Request>::worst_case(1)?)?
        .checked_add(Queue::<accounts::Request>::worst_case(accounts::MAX_OUT)?)
}

fn take_read(domain: &mut Domain, waiter: Token) -> Option<ResultRead> {
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
            authority::Last::None => 0,
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

/// Render only the current person Report task section from its temporary
/// activation context, without a raw child query or invented ancestor route
/// (domain/engine.md, 7.5 and 9).
fn task_read(record: &tasks::RunContext, parts: u32, bytes: u32) -> brief::Read {
    let words = match &record.contract {
        tasks::Contract::Report { words } => *words,
        tasks::Contract::Verdict { .. } | tasks::Contract::Change { .. } => return brief::Read::Failed,
    };
    if parts == 0 {
        return brief::Read::Failed;
    }
    let contract = Decimal::of(u64::from(words));
    let first = text_part(&record.spec.words, b"\n[Report: at most ", contract.as_bytes(), b" bytes]\n", bytes);
    let remaining =
        bytes.checked_sub(u32::try_from(first.bytes.len()).expect("bounded task part")).expect("part within read");
    let part = match record.requester {
        tasks::Party::Person(person) => {
            let person = Decimal::of(person);
            text_part(b"[Requested by person ", person.as_bytes(), b"]\n", b"", remaining)
        }
        tasks::Party::Task(_) | tasks::Party::Deployment { .. } => return brief::Read::Failed,
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
            Range::Tasks | Range::People | Range::RunProofs | Range::Turns { .. } | Range::TaskResult { .. },
        )
        | Startup::Adopting
        | Startup::Running => true,
    }
}

fn hello_within(hello: &fleet::Hello, limits: &fleet::Limits) -> bool {
    if hello.hosting.len() > usize::try_from(limits.slots).expect("u32 fits usize")
        || hello.workstreams.len() > usize::try_from(limits.workstreams).expect("u32 fits usize")
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
        u64::from(tasks.result_bytes).checked_mul(2)?,
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
        | Event::Answer { .. }
        | Event::ReadResult { .. }
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
/// topology terminals retain unchanged accepted expense (domain/engine.md, 7.5).
fn valid_proof(proof: &RunProof, expected: &RestoringProof, limits: &Limits) -> bool {
    if proof.task == 0 || proof.attempt == 0 {
        return false;
    }
    let spent = match proof.turn {
        Some(turn) => {
            if turn.turn == 0 || turn.read.is_some() || turn.cumulative > expected.run_spent {
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
    let requester = match task.requester {
        tasks::Party::Person(_) => true,
        tasks::Party::Task(_) | tasks::Party::Deployment { .. } => false,
    };
    let contract = match task.contract {
        tasks::Contract::Report { .. } => true,
        tasks::Contract::Verdict { .. } | tasks::Contract::Change { .. } => false,
    };
    let executor = match task.executor {
        tasks::Executor::Agent { charter: configured } => charter == configured,
    };
    task.number != 0
        && requester
        && contract
        && executor
        && task.delegates.is_empty()
        && task.dependencies.is_empty()
        && task.waiting_on.is_empty()
        && task.spec.inputs.is_empty()
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

fn supported_person(requester: tasks::Party, highest: u64) -> bool {
    match requester {
        tasks::Party::Person(person) => person != 0 && person <= highest,
        tasks::Party::Task(_) | tasks::Party::Deployment { .. } => false,
    }
}

/// Reject unsupported root shapes and identities above durable high-water
/// marks before child restoration. Proof rows consume exact transient live-row
/// correlations and never load archive history into the live map
/// (domain/engine.md, 6 and 7.5; domain/tasks.md, 14).
fn restore_page_row(domain: &mut Domain, env: &Env<Limits>, row: Record) {
    match row {
        Record::Deployment(deployment) => domain.journal = Journal::new(deployment, &env.limits.journal),
        Record::People(record) => domain.work.push(Work::People(people::Event::Restore { record })),
        Record::Tasks(record) => match record {
            tasks::Stored::Ended(_) | tasks::Stored::Closure(_) => {
                unreachable!("historical child rows excluded from startup")
            }
            tasks::Stored::Live(ref task) => {
                if !supported_task(task, domain.config.charter)
                    || task.number > domain.journal.deployment().tasks
                    || task.attempt > domain.journal.deployment().runs
                    || !supported_person(task.requester, domain.journal.deployment().people)
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
        Record::Turn(_) | Record::Terminal(_) => unreachable!("startup excludes archive families"),
    }
}
