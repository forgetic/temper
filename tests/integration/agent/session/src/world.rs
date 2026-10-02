use std::collections::{BTreeMap, BTreeSet, VecDeque};

use temper_agent_model_session as agent;
use temper_agent_model_session::llm::{Block, Endpoint, Failure, Prompt, Returned, Usage};
use temper_agent_model_tools::{self as tools, Effect, Entry, Fault, Grants, Kind, Name, Outcome};
use temper_lib::{Duration, Env, Queue, ReplyTo, Rng, Time, Token};
use temper_llm_model as provider;

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

/// Durations drawn uniformly from `min..=max`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Span {
    pub min: Duration,
    pub max: Duration,
}

impl Span {
    #[must_use]
    pub const fn millis(min: u64, max: u64) -> Span {
        Span { min: Duration::from_millis(min), max: Duration::from_millis(max) }
    }
}

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
    /// How long a tool takes to run.
    pub tool: Span,
    /// The chance, per mille, that a tool run fails.
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
                facts: 256,
                parallel_tools: 4,
            },
            provider: provider::Config {
                calls: 16,
                latency_min: Duration::from_millis(100),
                latency_max: Duration::from_millis(2000),
                overloaded: 0,
                rate_limited: 0,
                retry_after: Duration::from_secs(1),
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
        }
    }
}

/// A spec that grants every family of tools, with the calm budget.
#[must_use]
pub fn spec(prompt: &[u8]) -> agent::Spec {
    agent::Spec {
        endpoint: Endpoint(0),
        model: b"fake-1"[..].into(),
        system: b"You are a coding agent."[..].into(),
        tools: Grants { inspect: true, modify: true, shell: true },
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
    /// Tool runs the agent asked for.
    pub tool_runs: u32,
    /// Tool runs the agent cancelled.
    pub tool_cancels: u32,
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
    /// The most tool runs one session had in flight at once.
    pub most_parallel: u32,
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
    pub tools_started: u32,
    pub tools_finished: u32,
    pub tools_cancelled: u32,
    pub yielded: u32,
    pub used: u32,
    pub ended: u32,
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
    Open { opener: u64, spec: agent::Spec },
    /// The opener continues a yielded session.
    Continue { opener: u64, content: Box<[u8]> },
    /// The opener closes a session.
    Close { opener: u64 },
    /// A call arrives at the provider.
    Query { call: u64, query: provider::api::Query },
    /// The provider's answer arrives back at the agent's side.
    Answer { call: u64, result: Result<provider::api::Answer, provider::api::Error> },
    /// The agent's side gives up on a call.
    Deadline { call: u64 },
    /// A tool run finishes.
    ToolDone { owner: Token, outcome: Outcome },
}

/// A tool run of the agent in flight, as the tools would keep it.
struct Running {
    /// The result's delivery, withdrawn if the run is cancelled.
    delivery: (Time, u64),
    /// The session that started it.
    session: Token,
    effect: Effect,
}

/// A call of the agent in flight, as its protocol layer would keep it.
struct Call {
    owner: Token,
    /// The deadline's delivery, withdrawn when the call ends first.
    deadline: (Time, u64),
}

pub struct World {
    now: Time,
    rng: Rng,
    settings: Settings,

    agent: agent::Model,
    agent_env: Env<agent::Limits>,
    agent_in: VecDeque<agent::Event>,
    agent_out: Queue<agent::Request>,

    provider: provider::Model,
    provider_env: Env<provider::Config>,
    provider_in: VecDeque<provider::Event>,
    provider_out: Queue<provider::Request>,

    /// Deliveries in flight, by time and then by the order they were sent.
    wire: BTreeMap<(Time, u64), Delivery>,
    /// Names for openers, calls and deliveries.
    serial: u64,
    /// The agent's calls in flight, and the call each session has in flight.
    calls: BTreeMap<u64, Call>,
    calling: BTreeMap<Token, u64>,
    /// The tool runs in flight, by their tokens; and the session of each run
    /// the agent has started or has yet to hear the end of, worked out from
    /// the step that started it.
    tools: BTreeMap<Token, Running>,
    runs: BTreeMap<Token, Token>,
    /// Calls the provider has not answered yet.
    serving: BTreeSet<u64>,
    /// The sessions opened, by the opener's name for each, and the opener's
    /// name for each session by the session's own while it lives.
    sessions: BTreeMap<u64, Session>,
    openers: BTreeMap<Token, u64>,

    stats: Stats,
    told: Told,
    trace: Vec<String>,
}

impl World {
    #[must_use]
    pub fn new(settings: Settings) -> World {
        assert!(agent::worst_case(&settings.agent).is_some(), "the shell refuses limits it cannot provision");
        let mut rng = Rng::new(settings.seed);
        let agent = agent::Model::new(&settings.agent, rng.next_u64());
        let provider = provider::Model::new(&settings.provider, rng.next_u64());
        World {
            now: Time::ZERO,
            rng,
            settings,
            agent,
            agent_env: Env { now: Time::ZERO, limits: settings.agent },
            agent_in: VecDeque::new(),
            agent_out: Queue::with_capacity(agent::MAX_OUT + SLACK),
            provider,
            provider_env: Env { now: Time::ZERO, limits: settings.provider },
            provider_in: VecDeque::new(),
            provider_out: Queue::with_capacity(provider::MAX_OUT + SLACK),
            wire: BTreeMap::new(),
            serial: 0,
            calls: BTreeMap::new(),
            calling: BTreeMap::new(),
            tools: BTreeMap::new(),
            runs: BTreeMap::new(),
            serving: BTreeSet::new(),
            sessions: BTreeMap::new(),
            openers: BTreeMap::new(),
            stats: Stats::default(),
            told: Told::default(),
            trace: Vec::new(),
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
        &self.trace
    }

    /// Has the opener open a session for `spec` at `at`. Returns the opener's
    /// name for it.
    pub fn submit(&mut self, at: Time, spec: agent::Spec) -> u64 {
        let opener = self.next_serial();
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
        self.agent_env.now = self.now;
        self.provider_env.now = self.now;
        self.deliver();

        // Each stage takes its events, then fires its alarms, while it has room
        // for what one more may produce.
        while self.agent_out.room() >= agent::MAX_OUT {
            let Some(event) = self.agent_in.pop_front() else { break };
            self.log(&format!("agent <- {}", describe_agent_event(&event)));
            let (session, ended) = self.session_of(&event);
            let made = self.agent_out.len();
            agent::step(&mut self.agent, &self.agent_env, event, &mut self.agent_out);
            self.attribute(made, session);
            if let Some(run) = ended {
                self.runs.remove(&run);
            }
        }
        while self.agent_out.room() >= agent::MAX_OUT && self.agent.is_due(self.now) {
            self.log("agent alarm");
            let made = self.agent_out.len();
            agent::fire(&mut self.agent, &self.agent_env, &mut self.agent_out);
            self.attribute(made, None);
        }
        // The facts, drained as the shell would write them out.
        while let Some(fact) = self.agent.pop_fact() {
            self.tell(fact);
        }
        while self.provider_out.room() >= provider::MAX_OUT {
            let Some(event) = self.provider_in.pop_front() else { break };
            provider::step(&mut self.provider, &self.provider_env, event, &mut self.provider_out);
        }
        while self.provider_out.room() >= provider::MAX_OUT && self.provider.is_due(self.now) {
            provider::fire(&mut self.provider, &self.provider_env, &mut self.provider_out);
        }

        // What the steps asked for, submitted at the end of the iteration.
        while let Some(request) = self.agent_out.pop() {
            self.agent_request(request);
        }
        while let Some(request) = self.provider_out.pop() {
            self.provider_request(request);
        }

        // The reclaim point.
        self.agent.reclaim();
        self.provider.reclaim();
        assert!(self.agent.sessions() <= self.settings.agent.sessions, "sessions stay within their slots");
        assert!(self.provider.calls() <= self.settings.provider.calls, "calls stay within their slots");
    }

    /// The agent's requests, carried out the way its opener, its protocol
    /// layer and the tools would.
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
                self.stats.not_run += not_run(&prompt);
                let call = self.next_serial();
                let deadline = self.schedule(self.now.saturating_add(timeout), Delivery::Deadline { call });
                self.calls.insert(call, Call { owner, deadline });
                assert!(self.calling.insert(owner, call).is_none(), "a session has one call in flight");
                let query = translate::query(prompt);
                self.send(Delivery::Query { call, query });
                self.stats.calls += 1;
            }
            agent::Request::Cancel { owner } => {
                // A call that has already ended has its terminal event on the
                // way: the cancel lost the race and changes nothing.
                if let Some(&call) = self.calling.get(&owner) {
                    self.end_call(call);
                    self.agent_in.push_back(agent::Event::Cancelled { owner });
                    self.stats.cancels += 1;
                }
            }
            agent::Request::Tool { owner, call } => {
                let session = *self.runs.get(&owner).expect("a run is worked out from the step that started it");
                let effect = tools::effect(&call);
                self.batched(session, effect);
                let outcome = self.run_tool(&call);
                let at = self.now.saturating_add(self.draw(self.settings.tool));
                let delivery = self.schedule(at, Delivery::ToolDone { owner, outcome });
                let running = Running { delivery, session, effect };
                assert!(self.tools.insert(owner, running).is_none(), "each run has a token of its own");
                self.stats.tool_runs += 1;
            }
            agent::Request::CancelTool { owner } => {
                if let Some(Running { delivery, .. }) = self.tools.remove(&owner) {
                    self.wire.remove(&delivery).expect("a tool run in flight has its result on the way");
                    self.agent_in.push_back(agent::Event::ToolCancelled { owner });
                    self.stats.tool_cancels += 1;
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
        }
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

    /// What the fake tools make of `call`: what a checkout would answer, or,
    /// with the configured chance, a failure.
    fn run_tool(&mut self, call: &tools::Call) -> Outcome {
        if self.rng.chance(self.settings.tool_errors) {
            return if self.rng.chance(500) { Outcome::NotFound } else { Outcome::Failed { fault: Fault::Other } };
        }
        match call {
            tools::Call::Read { .. } => {
                Outcome::Read { content: b"fn main() {}\n"[..].into(), skipped: 0, lines: 1, total: 1, cut: false }
            }
            tools::Call::List { .. } => {
                let name = Name::new(b"main.rs"[..].into()).expect("a name");
                Outcome::Listed { entries: Box::new([Entry { name, kind: Kind::File }]), more: 0 }
            }
            tools::Call::Write { .. } => Outcome::Written { created: true },
            tools::Call::Search { .. } | tools::Call::Edit { .. } | tools::Call::Shell { .. } => Outcome::Unsupported,
        }
    }

    /// Whether the session `name` has nothing in flight.
    fn idle(&self, name: Token) -> bool {
        !self.calling.contains_key(&name) && self.tools.values().all(|run| run.session != name)
    }

    /// The session an event is for, as far as tool runs go, and the run whose
    /// end it is.
    fn session_of(&self, event: &agent::Event) -> (Option<Token>, Option<Token>) {
        match event {
            agent::Event::Completed { owner, .. }
            | agent::Event::Failed { owner, .. }
            | agent::Event::Cancelled { owner } => (Some(*owner), None),
            agent::Event::ToolDone { owner, .. } | agent::Event::ToolCancelled { owner } => {
                (Some(*self.runs.get(owner).expect("a run's end is for a run the agent started")), Some(*owner))
            }
            agent::Event::Continue { session, .. } | agent::Event::Close { session } => (Some(*session), None),
            agent::Event::Open { .. } => (None, None),
        }
    }

    /// Notes the session of each tool run the step just made started, from
    /// `made` in the output: a step works on one session.
    fn attribute(&mut self, made: u32, session: Option<Token>) {
        let skip = usize::try_from(made).expect("a small queue");
        for request in self.agent_out.iter().skip(skip) {
            if let agent::Request::Tool { owner, .. } = request {
                let session = session.expect("tool runs start in a step for their session");
                assert!(self.runs.insert(*owner, session).is_none(), "each run has a token of its own");
            }
        }
    }

    /// A run with `effect` starts for `session`: a write runs alone, and reads
    /// run together, as many as the limits allow.
    fn batched(&mut self, session: Token, effect: Effect) {
        let others: Vec<&Running> = self.tools.values().filter(|run| run.session == session).collect();
        let writing = others.iter().any(|run| run.effect == Effect::Write);
        assert!(!writing, "nothing runs beside a write");
        if effect == Effect::Write {
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
                assert!(self.serving.remove(&call), "the provider answers each call once");
                self.send(Delivery::Answer { call, result });
            }
        }
    }

    /// Hands every delivery that is due to its destination.
    fn deliver(&mut self) {
        while let Some(entry) = self.wire.first_entry() {
            if entry.key().0 > self.now {
                break;
            }
            match entry.remove() {
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
                    self.agent_in.push_back(agent::Event::Continue { session: name, content });
                    self.stats.continues += 1;
                }
                Delivery::Close { opener } => {
                    let session = self.sessions.get_mut(&opener).expect("the opener closes what it opened");
                    session.closed = true;
                    let name = session.session.expect("the opener closes a session once it has opened");
                    if session.ended.is_some() {
                        self.stats.stale += 1;
                    }
                    self.agent_in.push_back(agent::Event::Close { session: name });
                    self.stats.closes += 1;
                }
                Delivery::Query { call, query } => {
                    self.serving.insert(call);
                    self.provider_in
                        .push_back(provider::Event::Call { reply_to: ReplyTo::new(Token::new(call)), query });
                    self.stats.provider_calls += 1;
                }
                Delivery::Answer { call, result } => {
                    // The fake refuses a transcript a real provider would:
                    // a call without its result, a result without its call.
                    assert!(result != Err(provider::api::Error::InvalidRequest), "the agent sends well-formed queries");
                    if let Some(owner) = self.end_call(call) {
                        self.agent_in.push_back(translate::outcome(owner, result));
                    } else {
                        self.stats.late_answers += 1;
                    }
                }
                Delivery::Deadline { call } => {
                    let owner = self.end_call(call).expect("a deadline is withdrawn when its call ends first");
                    self.agent_in.push_back(agent::Event::Failed { owner, failure: Failure::TimedOut });
                    self.stats.timeouts += 1;
                }
                Delivery::ToolDone { owner, outcome } => {
                    let run = self.tools.remove(&owner);
                    assert!(run.is_some(), "a cancelled tool run's result is withdrawn");
                    self.agent_in.push_back(agent::Event::ToolDone { owner, outcome });
                }
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
        };
        assert!(self.sessions.insert(opener, session).is_none(), "openers have distinct names");
        self.agent_in.push_back(agent::Event::Open { opener: Token::new(opener), spec });
    }

    /// Ends the agent's call `call` if it is still in flight, withdrawing its
    /// deadline, and returns its owner.
    fn end_call(&mut self, call: u64) -> Option<Token> {
        let Call { owner, deadline } = self.calls.remove(&call)?;
        self.calling.remove(&owner);
        self.wire.remove(&deadline);
        Some(owner)
    }

    fn has_work_now(&self) -> bool {
        !self.agent_in.is_empty()
            || !self.provider_in.is_empty()
            || self.agent.is_due(self.now)
            || self.provider.is_due(self.now)
            || self.wire.first_key_value().is_some_and(|((at, _), _)| *at <= self.now)
    }

    fn next_time(&self) -> Option<Time> {
        let wire = self.wire.first_key_value().map(|((at, _), _)| *at);
        [wire, self.agent.next_deadline(), self.provider.next_deadline()].into_iter().flatten().min()
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
            agent::Fact::ToolStarted { .. } => &mut told.tools_started,
            agent::Fact::ToolFinished { .. } => &mut told.tools_finished,
            agent::Fact::ToolCancelled { .. } => &mut told.tools_cancelled,
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
        assert_eq!(told.tools_started, stats.tool_runs, "a fact for every tool run");
        assert_eq!(told.tools_finished + told.tools_cancelled, stats.tool_runs, "a fact for the end of every run");
        assert_eq!(told.tools_cancelled, stats.tool_cancels, "a fact for every cancelled run");
        assert_eq!(told.yielded, stats.yields, "a fact for every yield");
    }

    /// The invariants of a world where nothing is left to happen.
    fn assert_settled(&self) {
        assert_eq!(self.agent.sessions(), 0, "every session has ended and been reclaimed");
        assert_eq!(self.agent.next_deadline(), None, "no alarm outlives its session");
        assert_eq!(self.provider.calls(), 0, "the provider holds no call");
        assert!(self.calls.is_empty() && self.calling.is_empty(), "no call is in flight");
        assert!(self.tools.is_empty() && self.runs.is_empty(), "no tool is running, and every run's end was heard");
        assert!(self.serving.is_empty(), "the provider answered every call");
        assert!(
            self.wire.is_empty() && self.agent_in.is_empty() && self.provider_in.is_empty(),
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

    fn schedule(&mut self, at: Time, delivery: Delivery) -> (Time, u64) {
        let key = (at, self.next_serial());
        self.wire.insert(key, delivery);
        key
    }

    fn draw(&mut self, span: Span) -> Duration {
        Duration::from_nanos(self.rng.between(span.min.as_nanos(), span.max.as_nanos()))
    }

    fn next_serial(&mut self) -> u64 {
        self.serial += 1;
        self.serial
    }

    fn log(&mut self, line: &str) {
        self.trace.push(format!("{:>16} {line}", self.now.as_nanos()));
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
        agent::Event::ToolDone { owner, outcome } => {
            format!("tool done {} {outcome:?}", owner.raw())
        }
        agent::Event::ToolCancelled { owner } => format!("tool cancelled {}", owner.raw()),
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
        agent::Request::Tool { owner, call } => format!("tool {} {call:?}", owner.raw()),
        agent::Request::CancelTool { owner } => format!("cancel tool {}", owner.raw()),
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
