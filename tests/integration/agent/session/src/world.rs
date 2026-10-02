use std::collections::{BTreeMap, BTreeSet};

use temper_agent_model_session as agent;
use temper_agent_model_session::llm::{Answer, Block, Decoded, Descriptor, Endpoint, Failure, Prompt, Returned, Usage};
use temper_agent_model_tools::{self as tools, Authority, Done, Effect, Fault, Grants, Op, Repo};
use temper_agent_model_tools_tests::translate as io;
use temper_checkout_fake::Checkout;
use temper_lib::{Duration, ReplyTo, Rng, Time, Token};
use temper_llm_model as provider;
use temper_world::{Key, Ledger, Schedule, Span, Stage, Trace};

use crate::fixture;
use crate::tickets::{SERVED, Ticketed, Tickets};
use crate::translate;

/// Room in each model's output queue beyond what one step may emit. Small, so
/// the loop's flow control (take an event only while there is room for what
/// it may produce) is exercised.
const SLACK: u32 = 3;

/// What the fake opener says when it nudges a session on.
const NUDGE: &[u8] = b"You have not finished: carry on.";

/// The budget [`spec`] asks for: what the calm limits allow at most.
pub const BUDGET: agent::Budget = agent::Budget {
    turns: 16,
    input: 1 << 20,
    output: 1 << 20,
    cache_read: 1 << 20,
    cache_write: 1 << 20,
    time: Duration::from_secs(1800),
};

/// The tools' limits in the calm world: a kit for each session, as many calls
/// a kit as the session runs at once, and room for the fixture's files.
pub const TOOLS: tools::Limits = tools::Limits {
    kits: 4,
    calls: 4,
    repos: 1,
    path_bytes: 256,
    known_files: 16,
    file_bytes: 1 << 16,
    read_bytes: 4096,
    list_entries: 64,
    match_lines: 8,
    file_timeout: Duration::from_secs(30),
    env_bytes: 256,
    shell_timeout: Duration::from_secs(30),
    shell_timeout_max: Duration::from_secs(120),
    shell_head: 256,
    shell_tail: 256,
    search_hits: 16,
    search_bytes: 1024,
    search_timeout: Duration::from_secs(30),
    facts: 256,
};

/// Counts drawn uniformly from `min..=max`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Count {
    pub min: u32,
    pub max: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Settings {
    /// Seeds the world, which seeds both models.
    pub seed: u64,
    pub agent: agent::Limits,
    pub provider: provider::Config,
    /// One-way latency between the agent and the provider.
    pub network: Span,
    /// How long io takes over an operation of the tools, a command's
    /// included.
    pub tool: Span,
    /// The chance, per mille, that io fails an operation of the tools.
    pub tool_errors: u32,
    /// How many times the opener nudges a session that yields before it
    /// closes it, drawn for each session.
    pub nudges: Count,
    /// How long the opener takes to answer a yield.
    pub think: Span,
    /// The chance, per mille, that the opener closes a session at a moment of
    /// its own, whatever the session is doing then.
    pub abandon: u32,
    /// When such a close comes, after the session opens.
    pub abandon_after: Span,
    /// The chance, per mille, that a cancel loses its race: the call, the
    /// tools' operation or the delegated call it was for ends of itself, and
    /// that is its terminal event. A cancel that wins is told after a network
    /// draw.
    pub cancels_lost: u32,
    /// The chance, per mille, that the opener sends a close twice, the second
    /// a network draw after the first.
    pub double_close: u32,
    /// The chance, per mille, that the opener serves its own tools, a finish
    /// and a lookup, to a session it opens.
    pub serve: u32,
    /// How long the opener takes to answer a call it serves.
    pub serving: Span,
    /// The grid every delivery is rounded up to, so that some come at the
    /// same instant; zero for none.
    pub granule: Duration,
}

impl Settings {
    /// A world where nothing goes wrong: no failures, answers well within
    /// every deadline, room for a few sessions, and an opener that closes a
    /// session when it first yields.
    #[must_use]
    pub const fn calm(seed: u64) -> Settings {
        Settings {
            seed,
            agent: agent::Limits {
                sessions: 4,
                messages: 32,
                session_bytes: 1 << 20,
                budget: BUDGET,
                max_tokens: 4096,
                retries: 3,
                backoff_base: Duration::from_millis(200),
                backoff_max: Duration::from_secs(5),
                call_timeout: Duration::from_secs(60),
                tool_timeout: Duration::from_secs(60),
                facts: 256,
                parallel_tools: 4,
                tools: TOOLS,
            },
            provider: provider::Config {
                calls: 16,
                latency_min: Duration::from_millis(100),
                latency_max: Duration::from_millis(2000),
                overloaded: 0,
                rate_limited: 0,
                retry_after: Duration::from_secs(1),
                unavailable: 0,
                too_long: 0,
                unauthorized: 0,
                refused: 0,
                no_calls: 0,
                answer_tokens: 1,
                calls_per_answer: 1,
                malformed: 0,
                tool_rounds: 2,
            },
            network: Span::millis(1, 20),
            tool: Span::millis(10, 500),
            tool_errors: 0,
            nudges: Count { min: 0, max: 0 },
            think: Span::millis(0, 50),
            abandon: 0,
            abandon_after: Span::millis(0, 10_000),
            cancels_lost: 0,
            double_close: 0,
            serve: 0,
            serving: Span::millis(10, 2_000),
            granule: Duration::ZERO,
        }
    }
}

/// A spec that grants every family of tools, with the calm budget, in one
/// writable repository at `/work`, also the working directory. The world
/// gives each session a repository of its own, seeded with the fixture, and
/// names it in the spec as io names its root.
#[must_use]
pub fn spec(prompt: &[u8]) -> agent::Spec {
    let repo = Repo { mount: io::names(b"/work"), root: Token::new(0), writable: true };
    let grants = Grants { inspect: true, modify: true, shell: true };
    let authority = Authority { cwd: io::names(b"/work"), repos: Box::new([repo]), grants, env: Box::new([]) };
    agent::Spec {
        endpoint: Endpoint(0),
        model: b"fake-1"[..].into(),
        system: b"You are a coding agent."[..].into(),
        authority,
        delegated: Box::new([]),
        prompt: prompt.into(),
        max_tokens: 1024,
        budget: BUDGET,
    }
}

/// What the world counted.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Stats {
    /// Calls the agent made.
    pub calls: u32,
    /// Calls that reached the provider.
    pub provider_calls: u32,
    /// Calls that ran out of time.
    pub timeouts: u32,
    /// Calls the agent cancelled.
    pub cancels: u32,
    /// Answers that arrived after their call had ended, and were dropped.
    pub late_answers: u32,
    /// Operations the tools asked of io; those a cancel ended; those that ran
    /// out of time; those io failed.
    pub ops: u32,
    pub op_cancels: u32,
    pub op_timeouts: u32,
    pub op_faults: u32,
    /// Results of calls to the tools that went back to the LLM, each checked
    /// against its call.
    pub results: u32,
    /// Tool calls the session answered as not run, as the LLM stopped
    /// before it could use them.
    pub not_run: u32,
    /// Times a session yielded.
    pub yields: u32,
    /// Messages the opener sent to yielded sessions.
    pub continues: u32,
    /// Closes the opener sent.
    pub closes: u32,
    /// Continues and closes that reached a session after it had ended.
    pub stale: u32,
    /// The most operations and delegated calls one session had in flight at
    /// once.
    pub most_parallel: u32,
    /// Cancels of calls and of operations that lost their race.
    pub cancels_lost: u32,
    pub op_cancels_lost: u32,
    /// What ended a call or an operation whose cancel lost: an answer, a
    /// failure (a deadline included), the operation's own end.
    pub answered_after_cancel: u32,
    pub failed_after_cancel: u32,
    pub done_after_cancel: u32,
    /// Closes that reached a session already closing.
    pub closed_while_closing: u32,
    /// Cancels and withdraws for operations and delegated calls that ended in
    /// the iteration they were sent.
    pub cancels_crossed: u32,
    /// Calls delegated to the opener; withdrawn, or whose answer won the race
    /// with the withdraw; answered as timed out by the opener itself.
    pub delegates: u32,
    pub withdraws: u32,
    pub withdraws_lost: u32,
    pub answered_after_withdraw: u32,
    pub delegate_timeouts: u32,
    /// Finishes the opener refused, and accepted.
    pub finishes_refused: u32,
    pub finishes_accepted: u32,
}

/// The facts the sessions told, by kind, as the loop drained them.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Told {
    pub opened: u32,
    pub completions_started: u32,
    pub completions_answered: u32,
    pub completions_failed: u32,
    pub completions_cancelled: u32,
    pub completions_retried: u32,
    /// What the session's tools told: kits opened and closed, calls started
    /// (those that asked io for something) and answered.
    pub kits_opened: u32,
    pub kits_closed: u32,
    pub tools_started: u32,
    pub tools_answered: u32,
    pub yielded: u32,
    pub used: u32,
    pub ended: u32,
    pub delegates_started: u32,
    pub delegates_answered: u32,
    pub delegates_cancelled: u32,
    /// Tool calls the completions made, and those that could not be decoded.
    pub calls: u32,
    pub invalid_calls: u32,
}

/// A session as its opener saw it.
#[derive(Debug)]
pub struct Session {
    /// The session's name, once it opened.
    pub session: Option<Token>,
    /// What it was opened with: its budget, and the most tokens an answer
    /// may take.
    pub budget: agent::Budget,
    pub max_tokens: u32,
    /// Each time it yielded: why, and what the LLM said.
    pub yields: Vec<(agent::Yield, Box<[u8]>)>,
    /// What its `Used` added up to: completions, and their tokens.
    pub turns: u32,
    pub usage: Usage,
    /// When its time budget runs out, once it opened.
    expires: Option<Time>,
    /// How it ended, once it has.
    pub ended: Option<Ended>,
    /// Nudges the opener has left to give.
    nudges: u32,
    /// Whether the opener closes it at a moment of its own.
    abandon: bool,
    /// It yielded, and the opener has not continued it yet.
    waiting: bool,
    /// The opener has closed it.
    closed: bool,
    /// Finishes it has called.
    finishes: u32,
}

/// How a session ended.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Ended {
    pub end: agent::End,
    pub turns: u32,
    pub usage: Usage,
}

/// Something on its way, delivered at its time.
enum Delivery {
    /// The opener opens a session.
    Open {
        opener: u64,
        spec: agent::Spec,
    },
    /// The opener continues a yielded session.
    Continue {
        opener: u64,
        content: Box<[u8]>,
    },
    /// The opener closes a session.
    Close {
        opener: u64,
    },
    /// A call arrives at the provider.
    Query {
        call: u64,
        query: provider::api::Query,
    },
    /// The provider's answer arrives back at the agent's side.
    Answer {
        call: u64,
        result: Result<provider::api::Answer, provider::api::Error>,
    },
    /// The agent's side gives up on a call.
    Deadline {
        call: u64,
    },
    /// A cancel that won its race is told.
    Cancelled {
        owner: Token,
    },
    AnswerCancelled {
        owner: Token,
    },
    /// The opener's answer to a delegated call arrives.
    Answered {
        owner: Token,
        answer: Answer,
    },
    /// An operation of the tools ends.
    Ran {
        owner: Token,
    },
}

/// A delegated call in flight, as the opener keeps it.
struct Running {
    /// The answer's delivery, withdrawn if the call is withdrawn.
    delivery: Key,
    /// The session that started it.
    session: Token,
    effect: Effect,
    /// A finish the opener accepts, after which it closes the session.
    accepts: bool,
}

/// An operation of the tools in flight, as io keeps it.
struct Pending {
    /// Its end's delivery, moved up when a cancel wins the race.
    delivery: Key,
    /// The session whose tools asked for it, by the repository it is in.
    session: Token,
    /// A store or a command, which only a call that writes asks for.
    writes: bool,
    work: Work,
}

/// What an operation does when it ends.
enum Work {
    /// A file operation, run on the checkout then.
    File(Op),
    /// A command started, finished then.
    Command(io::Started),
    /// Nothing more: it ends so (it failed to start, timed out, or was
    /// cancelled).
    Ending(Done),
}

/// A call of the agent in flight, as its protocol layer would keep it.
struct Call {
    owner: Token,
    /// The deadline's delivery, withdrawn when the call ends first.
    deadline: Key,
    /// The session's opener, and the tools it served in the query.
    opener: u64,
    served: Box<[Descriptor]>,
}

pub struct World {
    now: Time,
    rng: Rng,
    settings: Settings,

    agent: agent::Model,
    agent_stage: Stage<agent::Limits, agent::Event, agent::Request>,

    provider: provider::Model,
    provider_stage: Stage<provider::Config, provider::Event, provider::Request>,

    /// Deliveries in flight, whose count names openers and calls too.
    wire: Schedule<Delivery>,
    /// The agent's calls in flight, and the call each session has in flight.
    calls: Ledger<u64, Call>,
    calling: BTreeMap<Token, u64>,
    /// The delegated calls in flight, by their tokens; and the session of
    /// each the agent has started or has yet to hear the end of.
    tools: Ledger<Token, Running>,
    runs: BTreeMap<Token, Token>,
    /// The checkout the sessions' tools work on, a repository for each
    /// session, and the opener of each by io's name for its root.
    checkout: Checkout,
    roots: BTreeMap<u64, u64>,
    /// The tools' operations in flight, by their tokens, and every token an
    /// operation has had (a call's, for each operation it asks for).
    ops: Ledger<Token, Pending>,
    owners: BTreeSet<Token>,
    /// How many messages of each session's transcript have had their results
    /// counted.
    counted: BTreeMap<Token, usize>,
    /// The sessions whose call, and the delegated calls and operations, whose
    /// cancel lost its race; and the sessions the agent is closing, with a
    /// cancel sent and no end yet.
    cancel_lost: BTreeSet<Token>,
    run_cancel_lost: BTreeSet<Token>,
    op_cancel_lost: BTreeSet<Token>,
    closing: BTreeSet<Token>,
    /// The opener's tickets, as the top level would keep them.
    tickets: Tickets,
    /// Calls the provider has not answered yet.
    serving: Ledger<u64, ()>,
    /// The sessions opened, by the opener's name for each, and the opener's
    /// name for each session by the session's own while it lives.
    sessions: BTreeMap<u64, Session>,
    openers: BTreeMap<Token, u64>,

    stats: Stats,
    told: Told,
    trace: Trace,
}

impl World {
    #[must_use]
    pub fn new(settings: Settings) -> World {
        assert!(agent::worst_case(&settings.agent).is_some(), "the shell refuses limits it cannot provision");
        let mut rng = Rng::new(settings.seed);
        let agent = agent::Model::new(&settings.agent, rng.next_u64());
        let provider = provider::Model::new(&settings.provider, rng.next_u64());
        let max_out = agent::max_out(&settings.agent);
        let mut checkout = Checkout::new();
        fixture::script(&mut checkout);
        World {
            now: Time::ZERO,
            rng,
            settings,
            agent,
            agent_stage: Stage::new(settings.agent, max_out, max_out + SLACK),
            provider,
            provider_stage: Stage::new(settings.provider, provider::MAX_OUT, provider::MAX_OUT + SLACK),
            wire: Schedule::new(),
            calls: Ledger::new("call to the provider"),
            calling: BTreeMap::new(),
            tools: Ledger::new("delegated call"),
            runs: BTreeMap::new(),
            checkout,
            roots: BTreeMap::new(),
            ops: Ledger::new("operation"),
            owners: BTreeSet::new(),
            counted: BTreeMap::new(),
            cancel_lost: BTreeSet::new(),
            run_cancel_lost: BTreeSet::new(),
            op_cancel_lost: BTreeSet::new(),
            closing: BTreeSet::new(),
            tickets: Tickets::default(),
            serving: Ledger::new("call the provider serves"),
            sessions: BTreeMap::new(),
            openers: BTreeMap::new(),
            stats: Stats::default(),
            told: Told::default(),
            trace: Trace::default(),
        }
    }

    #[must_use]
    pub fn now(&self) -> Time {
        self.now
    }

    #[must_use]
    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// The facts the sessions told, and how many they dropped for want of
    /// room.
    #[must_use]
    pub fn told(&self) -> (Told, u64) {
        (self.told, self.agent.facts_lost())
    }

    /// What crossed between the models and the world, in order, with times.
    #[must_use]
    pub fn trace(&self) -> &[String] {
        self.trace.lines()
    }

    /// Has the opener open a session for `spec` at `at`. Returns the opener's
    /// name for it.
    pub fn submit(&mut self, at: Time, mut spec: agent::Spec) -> u64 {
        let opener = self.wire.name();
        if self.rng.chance(self.settings.serve) {
            let mut served = Vec::new();
            for (name, effect, schema) in SERVED {
                let ticket = self.tickets.issue(opener, Ticketed::Tool { name, effect, schema });
                served.push(Descriptor { ticket, effect });
            }
            spec.delegated = served.into();
        }
        self.schedule(at, Delivery::Open { opener, spec });
        opener
    }

    /// The session the opener named `opener`, once the open has been sent.
    #[must_use]
    pub fn session(&self, opener: u64) -> &Session {
        self.sessions.get(&opener).expect("the session was submitted and its open sent")
    }

    /// Every session opened so far, by the opener's name for it.
    pub fn sessions(&self) -> impl Iterator<Item = (u64, &Session)> {
        self.sessions.iter().map(|(opener, session)| (*opener, session))
    }

    /// Runs until nothing is left to happen, then checks the invariants of a
    /// settled world. Panics if it takes more than `iterations`.
    pub fn run(&mut self, iterations: u32) {
        for _ in 0..iterations {
            self.iterate();
            if self.has_work_now() {
                continue;
            }
            let Some(next) = self.next_time() else {
                self.assert_settled();
                return;
            };
            assert!(next > self.now, "time moves forward");
            self.now = next;
        }
        panic!("the world did not settle in {iterations} iterations");
    }

    /// One iteration of the loop, as the shell would run it.
    fn iterate(&mut self) {
        self.agent_stage.tick(self.now);
        self.provider_stage.tick(self.now);
        self.deliver();

        // Each stage resumes what is ready, then takes its events, then fires
        // its alarms, while it has room for what one more may produce.
        while self.agent_stage.has_room() && self.agent.is_ready() {
            self.log("agent ready");
            let from = self.agent_stage.out.len();
            agent::resume(&mut self.agent, &self.agent_stage.env, &mut self.agent_stage.out);
            self.check_sent(from);
        }
        while let Some(event) = self.agent_stage.next_event() {
            self.log(&format!("agent <- {}", describe_agent_event(&event)));
            let ended = ended_run(&event);
            let from = self.agent_stage.out.len();
            agent::step(&mut self.agent, &self.agent_stage.env, event, &mut self.agent_stage.out);
            self.check_sent(from);
            if let Some(run) = ended {
                self.runs.remove(&run);
            }
        }
        while self.agent_stage.has_room() && self.agent.is_due(self.now) {
            self.log("agent alarm");
            let from = self.agent_stage.out.len();
            agent::fire(&mut self.agent, &self.agent_stage.env, &mut self.agent_stage.out);
            self.check_sent(from);
        }
        // The facts, drained as the shell would write them out.
        while let Some(fact) = self.agent.pop_fact() {
            self.tell(fact);
        }
        while let Some(event) = self.provider_stage.next_event() {
            provider::step(&mut self.provider, &self.provider_stage.env, event, &mut self.provider_stage.out);
        }
        while self.provider_stage.has_room() && self.provider.is_due(self.now) {
            provider::fire(&mut self.provider, &self.provider_stage.env, &mut self.provider_stage.out);
        }

        // What the steps asked for, submitted at the end of the iteration.
        while let Some(request) = self.agent_stage.out.pop() {
            self.agent_request(request);
        }
        while let Some(request) = self.provider_stage.out.pop() {
            self.provider_request(request);
        }

        // The reclaim point.
        self.agent.reclaim();
        self.provider.reclaim();
        assert!(self.agent.sessions() <= self.settings.agent.sessions, "sessions stay within their slots");
        let runs = self.settings.agent.sessions * self.settings.agent.parallel_tools * 2;
        assert!(self.agent.runs() <= runs, "runs stay within two batches a session");
        assert!(self.agent.kits() <= self.settings.agent.sessions, "a kit at most for each session");
        assert!(self.provider.calls() <= self.settings.provider.calls, "calls stay within their slots");
    }

    /// The agent's requests, carried out the way its opener, its protocol
    /// layer and the tools would.
    /// Checks what the entry point that emitted the requests past `from`
    /// sent the opener, which a parent counts on: the records that name it,
    /// and the delegated calls and their withdraws.
    fn check_sent(&self, from: u32) {
        let mut sent = 0;
        for request in self.agent_stage.out.iter().skip(usize::try_from(from).expect("small")) {
            match request {
                agent::Request::Opened { .. }
                | agent::Request::Yielded { .. }
                | agent::Request::Used { .. }
                | agent::Request::Ended { .. }
                | agent::Request::Delegate { .. }
                | agent::Request::Withdraw { .. } => sent += 1,
                agent::Request::Complete { .. }
                | agent::Request::Cancel { .. }
                | agent::Request::Io { .. }
                | agent::Request::CancelIo { .. } => {}
            }
        }
        let most = agent::max_to_opener(&self.agent_stage.env.limits);
        assert!(sent <= most, "an entry point sent its opener {sent} requests, more than {most}");
    }

    fn agent_request(&mut self, request: agent::Request) {
        self.log(&format!("agent -> {}", describe_agent_request(&request)));
        match request {
            agent::Request::Opened { opener, session } => self.opened(opener.raw(), session),
            agent::Request::Yielded { opener, stop, text } => self.yielded(opener.raw(), stop, text),
            agent::Request::Used { opener, usage } => self.used(opener.raw(), usage),
            agent::Request::Ended { opener, end, turns, usage } => {
                self.ended(opener.raw(), Ended { end, turns, usage });
            }
            agent::Request::Complete { owner, prompt, timeout } => {
                self.affordable(owner, &prompt);
                in_call_order(&prompt);
                self.check_results(owner, &prompt);
                self.stats.not_run += not_run(&prompt);
                let call = self.wire.name();
                let deadline = self.schedule(self.now.saturating_add(timeout), Delivery::Deadline { call });
                let opener = *self.openers.get(&owner).expect("a session calls the LLM while it lives");
                let served = prompt.delegated.clone();
                self.calls.open(call, Call { owner, deadline, opener, served });
                assert!(self.calling.insert(owner, call).is_none(), "a session has one call in flight");
                let query = translate::query(prompt, &self.tickets);
                self.send(Delivery::Query { call, query });
                self.stats.calls += 1;
            }
            agent::Request::Cancel { owner } => {
                self.closing.insert(owner);
                // A call that has already ended has its terminal event on the
                // way: the cancel lost the race and changes nothing. One still
                // in flight may end of itself all the same.
                if let Some(&call) = self.calling.get(&owner) {
                    if self.rng.chance(self.settings.cancels_lost) {
                        self.cancel_lost.insert(owner);
                        self.stats.cancels_lost += 1;
                    } else {
                        self.end_call(call);
                        self.send(Delivery::Cancelled { owner });
                        self.stats.cancels += 1;
                    }
                }
            }
            agent::Request::Io { owner, op, deadline } => self.start_op(owner, op, deadline),
            agent::Request::CancelIo { owner } => self.cancel_op(owner),
            agent::Request::Delegate { owner, opener, call, deadline } => {
                let session = self.sessions.get(&opener.raw()).and_then(|session| session.session);
                let session = session.expect("a session delegates once it has opened");
                assert!(self.runs.insert(owner, session).is_none(), "each delegated call has a token of its own");
                self.serve(owner, opener.raw(), call, deadline);
            }
            agent::Request::Withdraw { owner } => {
                let Some(&session) = self.runs.get(&owner) else {
                    self.stats.cancels_crossed += 1;
                    return;
                };
                self.closing.insert(session);
                if self.tools.contains(owner) && self.rng.chance(self.settings.cancels_lost) {
                    self.run_cancel_lost.insert(owner);
                    self.stats.withdraws_lost += 1;
                } else if let Some(Running { delivery, .. }) = self.tools.take(owner) {
                    self.wire.withdraw(delivery).expect("a served call in flight has its answer on the way");
                    self.send(Delivery::AnswerCancelled { owner });
                    self.stats.withdraws += 1;
                }
            }
        }
    }

    /// The opener learns its session's name, and plans a close of its own if
    /// it is to abandon the session.
    fn opened(&mut self, opener: u64, name: Token) {
        let session = self.sessions.get_mut(&opener).expect("a session opens for an open that was sent");
        assert!(session.session.is_none() && session.ended.is_none(), "a session opens once, before it ends");
        session.session = Some(name);
        session.expires = Some(self.now.saturating_add(session.budget.time));
        self.openers.insert(name, opener);
        if session.abandon {
            let at = self.now.saturating_add(self.draw(self.settings.abandon_after));
            self.schedule(at, Delivery::Close { opener });
        }
    }

    /// The opener answers a yield: it nudges the session on while it has
    /// nudges left, and closes it after.
    fn yielded(&mut self, opener: u64, stop: agent::Yield, text: Box<[u8]>) {
        let session = self.sessions.get_mut(&opener).expect("a session yields to its opener");
        let name = session.session.expect("a session yields once it has opened");
        assert!(session.ended.is_none(), "a session yields before it ends");
        assert!(!session.waiting, "a session yields once for each message");
        session.yields.push((stop, text));
        self.stats.yields += 1;
        assert!(self.idle(name), "a session yields with nothing in flight");
        let session = self.sessions.get_mut(&opener).expect("looked up above");
        if session.closed {
            // A close of the opener's own crossed the yield.
            return;
        }
        let at = self.now.saturating_add(self.draw(self.settings.think));
        let session = self.sessions.get_mut(&opener).expect("looked up above");
        if session.nudges > 0 {
            session.nudges -= 1;
            session.waiting = true;
            self.schedule(at, Delivery::Continue { opener, content: NUDGE.into() });
        } else {
            self.schedule(at, Delivery::Close { opener });
        }
    }

    /// A completion came back: the opener adds up what it used.
    fn used(&mut self, opener: u64, usage: Usage) {
        let session = self.sessions.get_mut(&opener).expect("a session reports to its opener");
        assert!(session.session.is_some() && session.ended.is_none(), "a session uses tokens while it lives");
        session.turns += 1;
        session.usage = session.usage.saturating_add(usage);
    }

    /// The session that owns `owner` starts a completion: it must have turns,
    /// input and output tokens and time left, no tokens past their budget,
    /// and an answer that may take no more than the output budget left.
    fn affordable(&self, owner: Token, prompt: &Prompt) {
        let opener = self.openers.get(&owner).expect("a session calls the LLM while it lives");
        let session = &self.sessions[opener];
        let (budget, usage) = (&session.budget, &session.usage);
        assert!(session.turns < budget.turns, "session {opener} starts no turn past its budget");
        assert!(usage.input_tokens < budget.input, "session {opener} has input tokens left");
        assert!(usage.output_tokens < budget.output, "session {opener} has output tokens left");
        assert!(usage.cache_read_tokens <= budget.cache_read, "session {opener} is within its cache reads");
        assert!(usage.cache_write_tokens <= budget.cache_write, "session {opener} is within its cache writes");
        assert!(Some(self.now) < session.expires, "session {opener} has time left");
        let messages = u32::try_from(prompt.messages.len()).expect("a small transcript");
        assert!(messages < self.settings.agent.messages, "session {opener}'s transcript has room for the answer");
        let left = budget.output - usage.output_tokens;
        let most = u32::try_from(left).unwrap_or(u32::MAX).min(session.max_tokens);
        assert_eq!(prompt.max_tokens, most, "session {opener}'s answer takes no more than the output budget left");
    }

    fn ended(&mut self, opener: u64, ended: Ended) {
        let session = self.sessions.get(&opener).expect("a session ends for an open that was sent");
        assert!(session.ended.is_none(), "a session ends once");
        match ended.end {
            agent::End::Busy | agent::End::Invalid => {
                assert!(session.session.is_none(), "a session refused at the entrance never opened");
            }
            agent::End::Closed | agent::End::Failed { .. } | agent::End::Budget { .. } | agent::End::TranscriptFull => {
                let name = session.session.expect("a session that ran had opened");
                assert!(self.idle(name), "a session ends once nothing it asked for is in flight");
            }
        }
        if ended.end == agent::End::Closed {
            assert!(session.closed, "a session ends as closed only when its opener closed it");
        }
        if let agent::End::Budget { spent } = ended.end {
            assert!(self.spent(session, spent), "session {opener} ended when its {spent:?} budget was spent");
        }
        assert_eq!((ended.turns, ended.usage), (session.turns, session.usage), "an end adds up what was used");
        if let Some(name) = session.session {
            self.openers.remove(&name);
            self.closing.remove(&name);
        }
        self.tickets.free(opener);
        self.sessions.get_mut(&opener).expect("looked up above").ended = Some(ended);
    }

    /// Whether `session` has run out of its budget in `dimension` now: used it
    /// up, for what gates a completion; gone past it, for the cache.
    fn spent(&self, session: &Session, dimension: agent::Dimension) -> bool {
        let (budget, usage) = (&session.budget, &session.usage);
        match dimension {
            agent::Dimension::Turns => session.turns >= budget.turns,
            agent::Dimension::Input => usage.input_tokens >= budget.input,
            agent::Dimension::Output => usage.output_tokens >= budget.output,
            agent::Dimension::CacheRead => usage.cache_read_tokens > budget.cache_read,
            agent::Dimension::CacheWrite => usage.cache_write_tokens > budget.cache_write,
            agent::Dimension::Time => Some(self.now) >= session.expires,
        }
    }

    /// The opener serves the delegated call `call` for the session of `opener`,
    /// by `deadline`: a finish it refuses the first time, as its tests fail,
    /// and accepts after; a lookup, which is slow. It runs the race with the
    /// deadline itself.
    fn serve(&mut self, owner: Token, opener: u64, call: Token, deadline: Time) {
        let session = *self.runs.get(&owner).expect("a run is worked out from the step that started it");
        let Ticketed::Call { tool, arguments: _ } = self.tickets.resolve(call).clone() else {
            panic!("a delegated call's ticket names a call");
        };
        let effect = SERVED.iter().find(|(name, ..)| *name == tool).expect("a call to a served tool").1;
        self.batched(session, effect == Effect::Write);
        let state = self.sessions.get_mut(&opener).expect("the opener serves the sessions it opened");
        let (mut text, mut error, mut accepts): (&[u8], bool, bool) = match tool {
            b"finish" => {
                state.finishes += 1;
                if state.finishes == 1 {
                    (b"not finished: the tests fail", true, false)
                } else {
                    (b"accepted", false, true)
                }
            }
            _ => (b"the forge says: fine", false, false),
        };
        let mut at = self.now.saturating_add(self.draw(self.settings.serving));
        if at > deadline {
            (text, error, accepts, at) = (b"timed out", true, false, deadline);
            self.stats.delegate_timeouts += 1;
        } else if tool == b"finish" {
            if accepts {
                self.stats.finishes_accepted += 1;
            } else {
                self.stats.finishes_refused += 1;
            }
        }
        let ticket = self.tickets.issue(opener, Ticketed::Answer { text: text.into(), error });
        let answer = Answer { ticket, bytes: u64::try_from(text.len()).expect("a short answer"), error };
        let delivery = self.schedule(at, Delivery::Answered { owner, answer });
        let running = Running { delivery, session, effect, accepts };
        self.tools.open(owner, running);
        self.stats.delegates += 1;
    }

    /// io starts the tools' operation `op` for `owner`, to end by
    /// `deadline`: after a draw, or at the deadline if that comes first.
    fn start_op(&mut self, owner: Token, op: Op, deadline: Time) {
        // A call's operations, one after another, carry its token.
        self.owners.insert(owner);
        let at = match &op {
            Op::Load { at, .. } | Op::Scan { at, .. } | Op::Store { at, .. } | Op::Search { at, .. } => at.root,
            Op::Spawn { cwd, .. } => cwd.root,
        };
        let opener = *self.roots.get(&at.raw()).expect("an operation is in a session's repository");
        let session = self.sessions.get(&opener).and_then(|session| session.session);
        let session = session.expect("a session's tools ask io for something once it has opened");
        let writes = matches!(op, Op::Store { .. } | Op::Spawn { .. });
        self.batched(session, writes);
        let mut ends = self.now.saturating_add(self.draw(self.settings.tool));
        let mut work = if self.rng.chance(self.settings.tool_errors) {
            self.stats.op_faults += 1;
            Work::Ending(Done::Failed { fault: Fault::Other })
        } else {
            match op {
                Op::Spawn { cwd, command, env, roots, head, tail } => {
                    match io::spawn(&self.checkout, &cwd, &command, &env, &roots, (head, tail)) {
                        Ok(started) => Work::Command(started),
                        Err(done) => Work::Ending(done),
                    }
                }
                op @ (Op::Load { .. } | Op::Scan { .. } | Op::Store { .. } | Op::Search { .. }) => Work::File(op),
            }
        };
        // io runs the race with the deadline, and says it lost a moment
        // after the deadline passes: a command killed then tells what it wrote
        // by then (nothing, here).
        if ends > deadline {
            let done = match work {
                Work::Command(started) => io::exited(None, &[], started.head, started.tail),
                Work::File(_) | Work::Ending(_) => Done::TimedOut,
            };
            let noticed = deadline.saturating_add(self.draw(self.settings.network));
            (work, ends) = (Work::Ending(done), noticed);
            self.stats.op_timeouts += 1;
        }
        let delivery = self.schedule(ends, Delivery::Ran { owner });
        self.ops.open(owner, Pending { delivery, session, writes, work });
        self.stats.ops += 1;
    }

    /// io is asked to cancel the operation of `owner`: it ends cancelled
    /// after a network draw, unless it ends of itself first.
    fn cancel_op(&mut self, owner: Token) {
        assert!(self.owners.contains(&owner), "a cancel names an operation io was asked for");
        let Some(pending) = self.ops.get(owner) else {
            // It ended in the iteration the cancel was sent.
            self.stats.cancels_crossed += 1;
            return;
        };
        self.closing.insert(pending.session);
        if self.rng.chance(self.settings.cancels_lost) {
            self.op_cancel_lost.insert(owner);
            self.stats.op_cancels_lost += 1;
            return;
        }
        let at = self.now.saturating_add(self.draw(self.settings.network));
        let delivery = self.schedule(at, Delivery::Ran { owner });
        let pending = self.ops.get_mut(owner).expect("looked up above");
        self.wire.withdraw(pending.delivery).expect("an operation in flight has its end on the way");
        (pending.delivery, pending.work) = (delivery, Work::Ending(Done::Cancelled));
        self.stats.op_cancels += 1;
    }

    /// The operation of `owner` ends, and io tells the tools how.
    fn ran(&mut self, owner: Token) {
        let pending = self.ops.end(owner);
        let done = match pending.work {
            Work::File(op) => io::perform(&mut self.checkout, op),
            Work::Command(started) => {
                self.checkout.finish(&started.process);
                let program = &started.process.program;
                io::exited(Some(program.exit), &program.output, started.head, started.tail)
            }
            Work::Ending(done) => done,
        };
        if self.op_cancel_lost.remove(&owner) {
            self.stats.done_after_cancel += 1;
        }
        self.agent_stage.push(agent::Event::Done { owner, done });
    }

    /// Checks every result of the session's own tools in `prompt` against
    /// the call it answers, and counts those of its last message once.
    fn check_results(&mut self, owner: Token, prompt: &Prompt) {
        for pair in prompt.messages.windows(2) {
            let calls = pair[0].content.iter().filter_map(|block| {
                let Block::ToolCall { call: Decoded::Owned { call }, id, .. } = block else { return None };
                Some((id, call))
            });
            for (id, call) in calls {
                let result = pair[1].content.iter().find_map(|block| {
                    let Block::ToolResult { id: answered, result: Returned::Owned { outcome } } = block else {
                        return None;
                    };
                    (answered == id).then_some(outcome)
                });
                if let Some(outcome) = result {
                    assert!(fixture::fits(call, outcome), "{outcome:?} is not what comes of {call:?}");
                }
            }
        }
        let counted = self.counted.entry(owner).or_default();
        if prompt.messages.len() > *counted {
            *counted = prompt.messages.len();
            let last = prompt.messages.last().expect("a prompt has a message");
            let answers = last
                .content
                .iter()
                .filter(|block| matches!(block, Block::ToolResult { result: Returned::Owned { .. }, .. }));
            self.stats.results += u32::try_from(answers.count()).expect("a small message");
        }
    }

    /// Whether the session `name` has nothing in flight.
    fn idle(&self, name: Token) -> bool {
        !self.calling.contains_key(&name)
            && self.tools.values().all(|run| run.session != name)
            && self.ops.values().all(|op| op.session != name)
    }

    /// Something starts for `session`, an operation of its tools or a
    /// delegated call, which `writes` or only reads: a write runs alone, and
    /// reads run together, as many as the limits allow. (The tools ask io for
    /// one operation at a time for each call, and only a call that writes
    /// stores or runs a command.)
    fn batched(&mut self, session: Token, writes: bool) {
        let runs = self.tools.values().filter(|run| run.session == session).map(|run| run.effect == Effect::Write);
        let ops = self.ops.values().filter(|op| op.session == session).map(|op| op.writes);
        let others: Vec<bool> = runs.chain(ops).collect();
        assert!(!others.contains(&true), "nothing runs beside a write");
        if writes {
            assert!(others.is_empty(), "a write runs alone");
        }
        let running = u32::try_from(others.len()).expect("a small batch") + 1;
        assert!(running <= self.settings.agent.parallel_tools, "no more runs at once than the limits allow");
        self.stats.most_parallel = self.stats.most_parallel.max(running);
    }

    /// The provider's requests, carried back the way its protocol layer would.
    fn provider_request(&mut self, request: provider::Request) {
        match request {
            provider::Request::Reply { to, result } => {
                let call = to.into_token().raw();
                self.serving.end(call);
                self.send(Delivery::Answer { call, result });
            }
        }
    }

    /// Hands every delivery that is due to its destination.
    fn deliver(&mut self) {
        while let Some(delivery) = self.wire.next(self.now) {
            match delivery {
                Delivery::Open { opener, spec } => self.open(opener, spec),
                Delivery::Continue { opener, content } => {
                    let session = self.sessions.get_mut(&opener).expect("the opener continues what it opened");
                    assert!(session.waiting, "the opener continues a session only while it is yielded");
                    session.waiting = false;
                    if session.closed {
                        // It closed the session while it was making up its mind.
                        continue;
                    }
                    let name = session.session.expect("a yielded session has opened");
                    if session.ended.is_some() {
                        self.stats.stale += 1;
                    }
                    self.agent_stage.push(agent::Event::Continue { session: name, content });
                    self.stats.continues += 1;
                }
                Delivery::Close { opener } => {
                    let session = self.sessions.get_mut(&opener).expect("the opener closes what it opened");
                    let again = !session.closed && self.rng.chance(self.settings.double_close);
                    let session = self.sessions.get_mut(&opener).expect("looked up above");
                    session.closed = true;
                    let name = session.session.expect("the opener closes a session once it has opened");
                    if session.ended.is_some() {
                        self.stats.stale += 1;
                    } else if self.closing.contains(&name) {
                        self.stats.closed_while_closing += 1;
                    }
                    self.agent_stage.push(agent::Event::Close { session: name });
                    self.stats.closes += 1;
                    if again {
                        self.send(Delivery::Close { opener });
                    }
                }
                Delivery::Query { call, query } => {
                    self.serving.open(call, ());
                    self.provider_stage.push(provider::Event::Call { reply_to: ReplyTo::new(Token::new(call)), query });
                    self.stats.provider_calls += 1;
                }
                Delivery::Answer { call, result } => {
                    // The fake refuses a transcript a real provider would:
                    // a call without its result, a result without its call.
                    assert!(result != Err(provider::api::Error::InvalidRequest), "the agent sends well-formed queries");
                    if let Some(Call { owner, opener, served, .. }) = self.end_call(call) {
                        if self.cancel_lost.remove(&owner) {
                            match result {
                                Ok(_) => self.stats.answered_after_cancel += 1,
                                Err(_) => self.stats.failed_after_cancel += 1,
                            }
                        }
                        let event = translate::outcome(owner, result, &mut self.tickets, opener, &served);
                        self.agent_stage.push(event);
                    } else {
                        self.stats.late_answers += 1;
                    }
                }
                Delivery::Deadline { call } => {
                    let Call { owner, .. } =
                        self.end_call(call).expect("a deadline is withdrawn when its call ends first");
                    if self.cancel_lost.remove(&owner) {
                        self.stats.failed_after_cancel += 1;
                    }
                    self.agent_stage.push(agent::Event::Failed { owner, failure: Failure::TimedOut });
                    self.stats.timeouts += 1;
                }
                Delivery::Cancelled { owner } => self.agent_stage.push(agent::Event::Cancelled { owner }),
                Delivery::AnswerCancelled { owner } => self.agent_stage.push(agent::Event::AnswerCancelled { owner }),
                Delivery::Answered { owner, answer } => {
                    // A withdrawn call's answer is withdrawn.
                    let run = self.tools.end(owner);
                    if self.run_cancel_lost.remove(&owner) {
                        self.stats.answered_after_withdraw += 1;
                    }
                    self.agent_stage.push(agent::Event::Answered { owner, answer });
                    // The opener has its finish, and closes the session.
                    if run.accepts {
                        let opener = *self.openers.get(&run.session).expect("a session lives while its calls run");
                        self.sessions.get_mut(&opener).expect("the opener's session").nudges = 0;
                        self.send(Delivery::Close { opener });
                    }
                }
                Delivery::Ran { owner } => self.ran(owner),
            }
        }
    }

    /// The opener opens a session, deciding how it will treat it.
    fn open(&mut self, opener: u64, spec: agent::Spec) {
        let Count { min, max } = self.settings.nudges;
        let nudges = u32::try_from(self.rng.between(min.into(), max.into())).expect("drawn between two u32s");
        let abandon = self.rng.chance(self.settings.abandon);
        let session = Session {
            session: None,
            budget: spec.budget,
            max_tokens: spec.max_tokens,
            yields: Vec::new(),
            turns: 0,
            usage: Usage::ZERO,
            expires: None,
            ended: None,
            nudges,
            abandon,
            waiting: false,
            closed: false,
            finishes: 0,
        };
        assert!(self.sessions.insert(opener, session).is_none(), "openers have distinct names");
        // A repository of its own, which io names as its root.
        let root = fixture::seed(&mut self.checkout, format!("s{opener}").as_bytes());
        self.roots.insert(root, opener);
        let mut spec = spec;
        for repo in &mut spec.authority.repos {
            repo.root = io::token(root);
        }
        self.agent_stage.push(agent::Event::Open { opener: Token::new(opener), spec });
    }

    /// Ends the agent's call `call` if it is still in flight, withdrawing its
    /// deadline, and returns its owner.
    fn end_call(&mut self, call: u64) -> Option<Call> {
        let ended = self.calls.take(call)?;
        self.calling.remove(&ended.owner);
        self.wire.withdraw(ended.deadline);
        Some(ended)
    }

    fn has_work_now(&self) -> bool {
        self.agent_stage.has_events()
            || self.provider_stage.has_events()
            || self.agent.is_ready()
            || self.agent.is_due(self.now)
            || self.provider.is_due(self.now)
            || self.wire.is_due(self.now)
    }

    fn next_time(&self) -> Option<Time> {
        [self.wire.next_time(), self.agent.next_deadline(), self.provider.next_deadline()].into_iter().flatten().min()
    }

    fn tell(&mut self, fact: agent::Fact) {
        let told = &mut self.told;
        if let agent::Fact::CompletionAnswered { calls, invalid, .. } = fact {
            told.calls += calls;
            told.invalid_calls += invalid;
        }
        let count = match fact {
            agent::Fact::Opened { .. } => &mut told.opened,
            agent::Fact::CompletionStarted { .. } => &mut told.completions_started,
            agent::Fact::CompletionAnswered { .. } => &mut told.completions_answered,
            agent::Fact::CompletionFailed { .. } => &mut told.completions_failed,
            agent::Fact::CompletionCancelled { .. } => &mut told.completions_cancelled,
            agent::Fact::CompletionRetried { .. } => &mut told.completions_retried,
            agent::Fact::Tools { fact } => match fact {
                tools::Fact::Opened { .. } => &mut told.kits_opened,
                tools::Fact::Closed { .. } => &mut told.kits_closed,
                tools::Fact::Started { .. } => &mut told.tools_started,
                tools::Fact::Answered { .. } => &mut told.tools_answered,
                tools::Fact::Refused { .. } | tools::Fact::Closing { .. } => return,
            },
            agent::Fact::DelegateStarted { .. } => &mut told.delegates_started,
            agent::Fact::DelegateAnswered { .. } => &mut told.delegates_answered,
            agent::Fact::DelegateCancelled { .. } => &mut told.delegates_cancelled,
            agent::Fact::Yielded { .. } => &mut told.yielded,
            agent::Fact::Used { .. } => &mut told.used,
            agent::Fact::Ended { .. } => &mut told.ended,
        };
        *count += 1;
    }

    /// What the facts must add up to when none were dropped: what the world
    /// saw cross the boundary.
    fn assert_told(&self) {
        let (told, stats) = (&self.told, &self.stats);
        let count = |n: usize| u32::try_from(n).expect("a small world");
        let opened = count(self.sessions.values().filter(|session| session.session.is_some()).count());
        let turns: u32 = self.sessions.values().map(|session| session.turns).sum();
        assert_eq!((told.opened, told.ended), (opened, count(self.sessions.len())), "an open and an end each");
        assert_eq!(told.completions_started, stats.calls, "a fact for every call");
        let ended = told.completions_answered + told.completions_failed + told.completions_cancelled;
        assert_eq!(ended, stats.calls, "a fact for the end of every call");
        assert_eq!(told.completions_cancelled, stats.cancels, "a fact for every cancelled call");
        assert_eq!(told.used, turns, "a fact for every completion's usage");
        assert_eq!((told.kits_opened, told.kits_closed), (opened, opened), "a kit opened and closed per session");
        assert!(told.tools_started <= stats.ops, "a call the tools start asks io for something");
        assert!(told.tools_answered >= told.tools_started, "a fact for the answer to every call started");
        assert!(told.tools_answered >= stats.results, "a fact for every result that went back");
        assert_eq!(told.yielded, stats.yields, "a fact for every yield");
        assert_eq!(told.delegates_started, stats.delegates, "a fact for every delegated call");
        let delegates_ended = told.delegates_answered + told.delegates_cancelled;
        assert_eq!(delegates_ended, stats.delegates, "a fact for the end of every delegated call");
        assert_eq!(told.delegates_cancelled, stats.withdraws, "a fact for every withdrawn call");
    }

    /// The invariants of a world where nothing is left to happen.
    fn assert_settled(&self) {
        assert_eq!(self.agent.sessions(), 0, "every session has ended and been reclaimed");
        assert_eq!(self.agent.runs(), 0, "every run has ended and been reclaimed");
        assert_eq!((self.agent.kits(), self.agent.jobs()), (0, 0), "every kit has closed, its calls answered");
        assert!(self.ops.is_empty() && self.op_cancel_lost.is_empty(), "every operation has ended, once");
        assert_eq!(self.agent.next_deadline(), None, "no alarm outlives its session");
        assert_eq!(self.provider.calls(), 0, "the provider holds no call");
        assert!(self.calls.is_empty() && self.calling.is_empty(), "no call is in flight");
        assert!(self.tools.is_empty() && self.runs.is_empty(), "no tool is running, and every run's end was heard");
        assert!(self.tickets.is_empty(), "every session's tickets were freed when it ended");
        assert!(self.cancel_lost.is_empty() && self.run_cancel_lost.is_empty(), "every lost cancel's race ended");
        assert!(self.serving.is_empty(), "the provider answered every call");
        assert!(
            self.wire.is_empty() && !self.agent_stage.has_events() && !self.provider_stage.has_events(),
            "nothing is on its way"
        );
        for (opener, session) in &self.sessions {
            assert!(session.ended.is_some(), "session {opener} has ended");
        }
        if self.agent.facts_lost() == 0 {
            self.assert_told();
        }
    }

    fn send(&mut self, delivery: Delivery) {
        let at = self.now.saturating_add(self.draw(self.settings.network));
        self.schedule(at, delivery);
    }

    fn schedule(&mut self, at: Time, delivery: Delivery) -> Key {
        // On a coarse grid, if the world has one, so that deliveries coincide.
        let granule = self.settings.granule.as_nanos();
        let at =
            if granule > 0 { Time::from_nanos(at.as_nanos().div_ceil(granule).saturating_mul(granule)) } else { at };
        self.wire.send(at, delivery)
    }

    fn draw(&mut self, span: Span) -> Duration {
        span.draw(&mut self.rng)
    }

    fn log(&mut self, line: &str) {
        self.trace.log(self.now, line);
    }
}

/// The delegated call whose end `event` is, if it is one.
fn ended_run(event: &agent::Event) -> Option<Token> {
    match event {
        agent::Event::Answered { owner, .. } | agent::Event::AnswerCancelled { owner } => Some(*owner),
        agent::Event::Open { .. }
        | agent::Event::Continue { .. }
        | agent::Event::Close { .. }
        | agent::Event::Completed { .. }
        | agent::Event::Failed { .. }
        | agent::Event::Cancelled { .. }
        | agent::Event::Done { .. } => None,
    }
}

fn describe_agent_event(event: &agent::Event) -> String {
    match event {
        agent::Event::Open { opener, spec } => {
            format!("open {} {:?}", opener.raw(), String::from_utf8_lossy(&spec.prompt))
        }
        agent::Event::Continue { session, content } => {
            format!("continue {} {:?}", session.raw(), String::from_utf8_lossy(content))
        }
        agent::Event::Close { session } => format!("close {}", session.raw()),
        agent::Event::Completed { owner, completion } => {
            format!("completed {} {:?} with {} blocks", owner.raw(), completion.stop, completion.content.len())
        }
        agent::Event::Failed { owner, failure } => format!("failed {} {failure:?}", owner.raw()),
        agent::Event::Cancelled { owner } => format!("cancelled {}", owner.raw()),
        agent::Event::Done { owner, done } => format!("done {} {done:?}", owner.raw()),
        agent::Event::Answered { owner, answer } => format!("answered {} {answer:?}", owner.raw()),
        agent::Event::AnswerCancelled { owner } => format!("answer cancelled {}", owner.raw()),
    }
}

fn describe_agent_request(request: &agent::Request) -> String {
    match request {
        agent::Request::Opened { opener, session } => format!("opened {} as {}", opener.raw(), session.raw()),
        agent::Request::Yielded { opener, stop, text } => {
            format!("yielded {} {stop:?} {:?}", opener.raw(), String::from_utf8_lossy(text))
        }
        agent::Request::Used { opener, usage } => format!("used {} {usage:?}", opener.raw()),
        agent::Request::Ended { opener, end, turns, usage } => {
            format!("ended {} {end:?} after {turns} turns, {usage:?}", opener.raw())
        }
        agent::Request::Complete { owner, prompt, timeout } => {
            let (messages, most) = (prompt.messages.len(), prompt.max_tokens);
            format!("complete {} with {messages} messages, at most {most} tokens, within {timeout:?}", owner.raw())
        }
        agent::Request::Cancel { owner } => format!("cancel {}", owner.raw()),
        agent::Request::Io { owner, op, deadline } => format!("io {} {op:?} by {}", owner.raw(), deadline.as_nanos()),
        agent::Request::CancelIo { owner } => format!("cancel io {}", owner.raw()),
        agent::Request::Delegate { owner, opener, call, deadline } => {
            format!("delegate {} for {} {call:?} by {}", owner.raw(), opener.raw(), deadline.as_nanos())
        }
        agent::Request::Withdraw { owner } => format!("withdraw {}", owner.raw()),
    }
}

/// The results answered as not run in the last message of `prompt`.
fn not_run(prompt: &Prompt) -> u32 {
    let Some(last) = prompt.messages.last() else { return 0 };
    let unrun = last.content.iter().filter(|block| matches!(block, Block::ToolResult { result: Returned::NotRun, .. }));
    u32::try_from(unrun.count()).expect("a small message")
}

/// Checks that the results at the end of `prompt` answer the calls of the
/// message before them, in their order.
fn in_call_order(prompt: &Prompt) {
    let [.., asked, answered] = &*prompt.messages else { return };
    let calls: Vec<&[u8]> = asked
        .content
        .iter()
        .filter_map(|block| if let Block::ToolCall { id, .. } = block { Some(&**id) } else { None })
        .collect();
    let results: Vec<&[u8]> = answered
        .content
        .iter()
        .filter_map(|block| if let Block::ToolResult { id, .. } = block { Some(&**id) } else { None })
        .collect();
    assert_eq!(calls, results, "the results go back in call order");
}
