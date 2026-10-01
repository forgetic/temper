use std::collections::{BTreeMap, BTreeSet, VecDeque};

use temper_agent_model as agent;
use temper_agent_model::llm::{Endpoint, Failure, Tool};
use temper_lib::{Duration, Env, Queue, ReplyTo, Rng, Time, Token};
use temper_llm_model as provider;

use crate::translate;

/// Room in each model's output queue. Small, so the loop's flow control (take
/// an event only while there is room for what it may produce) is exercised.
const OUT: u32 = 4;

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
}

impl Settings {
    /// A world where nothing goes wrong: no failures, answers well within
    /// every deadline, room for a few sessions.
    #[must_use]
    pub const fn calm(seed: u64) -> Settings {
        Settings {
            seed,
            agent: agent::Limits {
                sessions: 4,
                messages: 32,
                session_bytes: 1 << 20,
                turns: 16,
                max_tokens: 4096,
                retries: 3,
                backoff_base: Duration::from_millis(200),
                backoff_max: Duration::from_secs(5),
                call_timeout: Duration::from_secs(60),
                session_timeout: Duration::from_secs(1800),
            },
            provider: provider::Config {
                calls: 16,
                latency_min: Duration::from_millis(100),
                latency_max: Duration::from_millis(2000),
                overloaded: 0,
                rate_limited: 0,
                retry_after: Duration::from_secs(1),
                tool_rounds: 2,
            },
            network: Span::millis(1, 20),
            tool: Span::millis(10, 500),
            tool_errors: 0,
        }
    }
}

/// A task with two tools for the LLM to call.
#[must_use]
pub fn task(prompt: &[u8]) -> agent::Task {
    let tool = |name: &[u8], description: &[u8]| Tool {
        name: name.into(),
        description: description.into(),
        schema: br#"{"type":"object"}"#[..].into(),
    };
    agent::Task {
        endpoint: Endpoint(0),
        model: b"fake-1"[..].into(),
        system: b"You are a coding agent."[..].into(),
        tools: Box::new([tool(b"read_file", b"Reads a file."), tool(b"run_tests", b"Runs the tests.")]),
        prompt: prompt.into(),
        max_tokens: 1024,
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
}

/// Something on its way, delivered at its time.
enum Delivery {
    /// A run arrives at the agent from outside.
    Run { run: u64, task: agent::Task },
    /// A call arrives at the provider.
    Query { call: u64, query: provider::api::Query },
    /// The provider's answer arrives back at the agent's side.
    Answer { call: u64, result: Result<provider::api::Answer, provider::api::Error> },
    /// The agent's side gives up on a call.
    Deadline { call: u64 },
    /// A tool run finishes.
    ToolDone { owner: Token, output: Box<[u8]>, error: bool },
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
    /// Names for runs, calls and deliveries.
    serial: u64,
    /// The agent's calls in flight, and the call each session has in flight.
    calls: BTreeMap<u64, Call>,
    calling: BTreeMap<Token, u64>,
    /// The tool run each session has in flight, by its result's delivery.
    tools: BTreeMap<Token, (Time, u64)>,
    /// Calls the provider has not answered yet.
    serving: BTreeSet<u64>,
    /// Runs made, and their reports once answered.
    runs: BTreeMap<u64, Option<agent::Report>>,

    stats: Stats,
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
            agent_out: Queue::with_capacity(OUT),
            provider,
            provider_env: Env { now: Time::ZERO, limits: settings.provider },
            provider_in: VecDeque::new(),
            provider_out: Queue::with_capacity(OUT),
            wire: BTreeMap::new(),
            serial: 0,
            calls: BTreeMap::new(),
            calling: BTreeMap::new(),
            tools: BTreeMap::new(),
            serving: BTreeSet::new(),
            runs: BTreeMap::new(),
            stats: Stats::default(),
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

    /// What crossed between the models and the world, in order, with times.
    #[must_use]
    pub fn trace(&self) -> &[String] {
        &self.trace
    }

    /// Sends a run of `task` to the agent, to arrive at `at`. Returns the
    /// run's name.
    pub fn submit(&mut self, at: Time, task: agent::Task) -> u64 {
        let run = self.next_serial();
        self.schedule(at, Delivery::Run { run, task });
        run
    }

    /// The answer to `run`, once it has come.
    #[must_use]
    pub fn report(&self, run: u64) -> Option<&agent::Report> {
        self.runs.get(&run)?.as_ref()
    }

    /// Every run made so far, with its answer once it has come.
    pub fn reports(&self) -> impl Iterator<Item = (u64, Option<&agent::Report>)> {
        self.runs.iter().map(|(run, report)| (*run, report.as_ref()))
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
            agent::step(&mut self.agent, &self.agent_env, event, &mut self.agent_out);
        }
        while self.agent_out.room() >= agent::MAX_OUT && self.agent.is_due(self.now) {
            self.log("agent alarm");
            agent::fire(&mut self.agent, &self.agent_env, &mut self.agent_out);
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

    /// The agent's requests, carried out the way its protocol layer and the
    /// tools would.
    fn agent_request(&mut self, request: agent::Request) {
        self.log(&format!("agent -> {}", describe_agent_request(&request)));
        match request {
            agent::Request::Reply { to, report } => {
                let run = to.into_token().raw();
                let answer = self.runs.get_mut(&run).expect("a reply answers a run that was made");
                assert!(answer.is_none(), "a run is answered once");
                *answer = Some(report);
            }
            agent::Request::Complete { owner, prompt, timeout } => {
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
                let error = self.rng.chance(self.settings.tool_errors);
                let output = if error { b"the tool failed"[..].into() } else { [b"ran ", &*call.name].concat().into() };
                let at = self.now.saturating_add(self.draw(self.settings.tool));
                let delivery = self.schedule(at, Delivery::ToolDone { owner, output, error });
                assert!(self.tools.insert(owner, delivery).is_none(), "a session has one tool run in flight");
                self.stats.tool_runs += 1;
            }
            agent::Request::CancelTool { owner } => {
                if let Some(delivery) = self.tools.remove(&owner) {
                    self.wire.remove(&delivery).expect("a tool run in flight has its result on the way");
                    self.agent_in.push_back(agent::Event::ToolCancelled { owner });
                    self.stats.tool_cancels += 1;
                }
            }
        }
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
                Delivery::Run { run, task } => {
                    assert!(self.runs.insert(run, None).is_none(), "runs have distinct names");
                    let reply_to = ReplyTo::new(Token::new(run));
                    self.agent_in.push_back(agent::Event::Run { reply_to, task });
                }
                Delivery::Query { call, query } => {
                    self.serving.insert(call);
                    self.provider_in
                        .push_back(provider::Event::Call { reply_to: ReplyTo::new(Token::new(call)), query });
                    self.stats.provider_calls += 1;
                }
                Delivery::Answer { call, result } => {
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
                Delivery::ToolDone { owner, output, error } => {
                    let run = self.tools.remove(&owner);
                    assert!(run.is_some(), "a cancelled tool run's result is withdrawn");
                    self.agent_in.push_back(agent::Event::ToolDone { owner, output, error });
                }
            }
        }
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

    /// The invariants of a world where nothing is left to happen.
    fn assert_settled(&self) {
        assert_eq!(self.agent.sessions(), 0, "every session has ended and been reclaimed");
        assert_eq!(self.agent.next_deadline(), None, "no alarm outlives its session");
        assert_eq!(self.provider.calls(), 0, "the provider holds no call");
        assert!(self.calls.is_empty() && self.calling.is_empty(), "no call is in flight");
        assert!(self.tools.is_empty(), "no tool is running");
        assert!(self.serving.is_empty(), "the provider answered every call");
        assert!(
            self.wire.is_empty() && self.agent_in.is_empty() && self.provider_in.is_empty(),
            "nothing is on its way"
        );
        for (run, report) in &self.runs {
            assert!(report.is_some(), "run {run} was answered");
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
        agent::Event::Run { reply_to: _, task } => format!("run {:?}", String::from_utf8_lossy(&task.prompt)),
        agent::Event::Completed { owner, completion } => {
            format!("completed {} {:?} with {} blocks", owner.raw(), completion.stop, completion.content.len())
        }
        agent::Event::Failed { owner, failure } => format!("failed {} {failure:?}", owner.raw()),
        agent::Event::Cancelled { owner } => format!("cancelled {}", owner.raw()),
        agent::Event::ToolDone { owner, output, error } => {
            format!("tool done {} {:?} error={error}", owner.raw(), String::from_utf8_lossy(output))
        }
        agent::Event::ToolCancelled { owner } => format!("tool cancelled {}", owner.raw()),
    }
}

fn describe_agent_request(request: &agent::Request) -> String {
    match request {
        agent::Request::Reply { to, report } => format!("reply {to:?} {report:?}"),
        agent::Request::Complete { owner, prompt, timeout } => {
            format!("complete {} with {} messages within {timeout:?}", owner.raw(), prompt.messages.len())
        }
        agent::Request::Cancel { owner } => format!("cancel {}", owner.raw()),
        agent::Request::Tool { owner, call } => {
            format!("tool {} {:?}", owner.raw(), String::from_utf8_lossy(&call.name))
        }
        agent::Request::CancelTool { owner } => format!("cancel tool {}", owner.raw()),
    }
}
