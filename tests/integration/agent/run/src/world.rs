use std::collections::{BTreeMap, VecDeque};

use temper_agent_model_run as run;
use temper_fake_worker_model as worker;
use temper_lib::{Duration, Env, Queue, ReplyTo, Rng, Time, Token};

use crate::partner::{Out, Partner, Script, Tally};
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
    /// Seeds the world, which seeds the worker and the partner.
    pub seed: u64,
    pub run: run::Limits,
    pub worker: worker::Config,
    pub partner: Script,
    /// One-way latency between the worker and the agent.
    pub network: Span,
    /// One-way latency between the run and its conversations. Within the
    /// agent, the top level hands records over in the step that makes them;
    /// a latency here lets them cross in every order.
    pub hop: Span,
}

impl Settings {
    /// A world where nothing goes wrong at the agent's side: room for every
    /// run, charters well within the limits, an LLM that works and yields now
    /// and then, and no cancels.
    #[must_use]
    pub const fn calm(seed: u64) -> Settings {
        Settings {
            seed,
            run: run::Limits {
                runs: 4,
                conversations: 4,
                run_bytes: 1 << 16,
                repositories: 4,
                outlets: 4,
                verdicts: 4,
                budget: run::Budget {
                    turns: 1000,
                    input: 1 << 32,
                    output: 1 << 32,
                    cache_read: 1 << 32,
                    cache_write: 1 << 32,
                    time: Duration::from_secs(24 * 3600),
                },
                max_tokens: 8192,
                nudges: 2,
            },
            worker: worker::Config {
                jobs: 4,
                window: Duration::from_secs(10),
                cancels: 0,
                cancel_min: Duration::from_secs(1),
                cancel_max: Duration::from_secs(60),
                brief_min: 100,
                brief_max: 2000,
                turns_min: 20,
                turns_max: 40,
                tokens_min: 1 << 20,
                tokens_max: 1 << 21,
                time_min: Duration::from_secs(3600),
                time_max: Duration::from_secs(7200),
                max_tokens: 4096,
            },
            partner: Script {
                conversations: 16,
                invalid: 0,
                turn: Span::millis(100, 5_000),
                input: 4_000,
                output: 1_000,
                cache: 2_000,
                faults: 0,
                yields: 200,
                odd_stops: 0,
                settle: Span::millis(1, 500),
                races: 500,
            },
            network: Span::millis(1, 20),
            hop: Span::millis(0, 2),
        }
    }
}

/// What the world counted.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Stats {
    /// Runs the worker started, and cancels it sent.
    pub starts: u32,
    pub cancels: u32,
    /// Conversations the run opened, nudges it said, and closes it sent.
    pub opens: u32,
    pub says: u32,
    pub closes: u32,
    /// What the partner counted.
    pub partner: Tally,
}

/// Something on its way, delivered at its time.
enum Delivery {
    /// The worker's start reaches the agent.
    Start {
        owner: Token,
        charter: worker::api::Charter,
    },
    /// The worker's cancel reaches the agent.
    Cancel {
        run: Token,
    },
    /// The agent's word on a run reaches the worker.
    Admitted {
        owner: Token,
        run: Token,
    },
    Answered {
        owner: Token,
        answer: worker::api::Answer,
    },
    /// The run's requests reach its conversations.
    Open {
        conversation: Token,
        opening: run::Opening,
    },
    Say {
        peer: Token,
    },
    Close {
        peer: Token,
    },
    /// A conversation's event reaches the run.
    Event(run::Event),
    /// The partner's own timer.
    Wake {
        peer: Token,
        wake: u64,
    },
}

/// The four one-way channels, named by where they lead, each delivering in the
/// order it was given.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Lane {
    Agent,
    Worker,
    Conversations,
    Run,
}

/// A start, as the world tracks it: the budget its run keeps to, and the
/// answer once it has come.
struct Start {
    budget: run::Budget,
    answer: Option<run::Answer>,
}

/// An open, as the world tracks it.
#[derive(Default)]
struct Open {
    started: bool,
    ended: bool,
}

pub struct World {
    now: Time,
    rng: Rng,
    settings: Settings,

    run: run::Model,
    run_env: Env<run::Limits>,
    run_in: VecDeque<run::Event>,
    run_out: Queue<run::Request>,

    worker: worker::Model,
    worker_env: Env<worker::Config>,
    worker_in: VecDeque<worker::Event>,
    worker_out: Queue<worker::Request>,

    partner: Partner,

    /// Deliveries in flight, by time and then by the order they were sent.
    wire: BTreeMap<(Time, u64), Delivery>,
    serial: u64,
    /// When each lane delivers its latest, which the next may not overtake.
    lanes: [Time; 4],
    /// Every start, by the worker's name for it, and every open, by the run's
    /// name for the conversation.
    starts: BTreeMap<Token, Start>,
    opens: BTreeMap<Token, Open>,

    stats: Stats,
    trace: Vec<String>,
}

impl World {
    #[must_use]
    pub fn new(settings: Settings) -> World {
        assert!(run::worst_case(&settings.run).is_some(), "the shell refuses limits it cannot provision");
        let mut rng = Rng::new(settings.seed);
        let worker = worker::Model::new(&settings.worker, rng.next_u64());
        let partner = Partner::new(settings.partner, rng.next_u64());
        World {
            now: Time::ZERO,
            rng,
            settings,
            run: run::Model::new(&settings.run),
            run_env: Env { now: Time::ZERO, limits: settings.run },
            run_in: VecDeque::new(),
            run_out: Queue::with_capacity(OUT),
            worker,
            worker_env: Env { now: Time::ZERO, limits: settings.worker },
            worker_in: VecDeque::new(),
            worker_out: Queue::with_capacity(OUT),
            partner,
            wire: BTreeMap::new(),
            serial: 0,
            lanes: [Time::ZERO; 4],
            starts: BTreeMap::new(),
            opens: BTreeMap::new(),
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
        Stats { partner: self.partner.tally(), ..self.stats }
    }

    /// What crossed between the models and the world, in order, with times.
    #[must_use]
    pub fn trace(&self) -> &[String] {
        &self.trace
    }

    /// Every run started, with the run's answer once it has come.
    pub fn answers(&self) -> impl Iterator<Item = Option<&run::Answer>> {
        self.starts.values().map(|start| start.answer.as_ref())
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
        self.run_env.now = self.now;
        self.worker_env.now = self.now;
        self.deliver();

        // Each stage takes its events, then fires its alarms, while it has room
        // for what one more may produce.
        while self.run_out.room() >= run::MAX_OUT {
            let Some(event) = self.run_in.pop_front() else { break };
            self.log(&format!("run <- {event:?}"));
            run::step(&mut self.run, &self.run_env, event, &mut self.run_out);
        }
        while self.run_out.room() >= run::MAX_OUT && self.run.is_due(self.now) {
            self.log("run alarm");
            run::fire(&mut self.run, &self.run_env, &mut self.run_out);
        }
        while self.worker_out.room() >= worker::MAX_OUT {
            let Some(event) = self.worker_in.pop_front() else { break };
            worker::step(&mut self.worker, &self.worker_env, event, &mut self.worker_out);
        }
        while self.worker_out.room() >= worker::MAX_OUT && self.worker.is_due(self.now) {
            worker::fire(&mut self.worker, &self.worker_env, &mut self.worker_out);
        }

        // What the steps asked for, submitted at the end of the iteration.
        while let Some(request) = self.run_out.pop() {
            self.run_request(request);
        }
        while let Some(request) = self.worker_out.pop() {
            self.worker_request(request);
        }

        // The reclaim point.
        self.run.reclaim();
        self.worker.reclaim();
        assert!(self.run.runs() <= self.settings.run.runs, "runs stay within their slots");
        assert!(self.run.conversations() <= self.settings.run.conversations, "conversations stay within their slots");
    }

    /// The run's requests, carried out the way the top level, the protocol
    /// layer and the conversations would.
    fn run_request(&mut self, request: run::Request) {
        self.log(&format!("run -> {request:?}"));
        match request {
            run::Request::Admitted { worker, run } => {
                self.send(Lane::Worker, Delivery::Admitted { owner: worker, run });
            }
            run::Request::Answer { to, answer } => {
                let owner = to.into_token();
                let start = self.starts.get_mut(&owner).expect("an answer is to a start that was made");
                assert!(start.answer.is_none(), "a start is answered once");
                assert_within(&start.budget, &answer, self.partner.turn_max());
                let translated = translate::answer(&answer);
                start.answer = Some(answer);
                self.send(Lane::Worker, Delivery::Answered { owner, answer: translated });
            }
            run::Request::Open { conversation, opening } => {
                assert!(
                    self.opens.insert(conversation, Open::default()).is_none(),
                    "conversations have distinct names"
                );
                self.stats.opens += 1;
                self.send(Lane::Conversations, Delivery::Open { conversation, opening });
            }
            run::Request::Say { peer, text: _ } => {
                self.stats.says += 1;
                self.send(Lane::Conversations, Delivery::Say { peer });
            }
            run::Request::Close { peer } => {
                self.stats.closes += 1;
                self.send(Lane::Conversations, Delivery::Close { peer });
            }
        }
    }

    /// The worker's requests, carried to the agent the way the two protocol
    /// layers would.
    fn worker_request(&mut self, request: worker::Request) {
        match request {
            worker::Request::Start { owner, charter } => {
                let start = Start { budget: translate::budget(charter.budget), answer: None };
                assert!(self.starts.insert(owner, start).is_none(), "jobs have distinct names");
                self.stats.starts += 1;
                self.send(Lane::Agent, Delivery::Start { owner, charter });
            }
            worker::Request::Cancel { run } => {
                self.stats.cancels += 1;
                self.send(Lane::Agent, Delivery::Cancel { run });
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
                Delivery::Start { owner, charter } => {
                    let charter = translate::charter(charter);
                    self.run_in.push_back(run::Event::Start { reply_to: ReplyTo::new(owner), worker: owner, charter });
                }
                Delivery::Cancel { run } => self.run_in.push_back(run::Event::Cancel { run }),
                Delivery::Admitted { owner, run } => self.worker_in.push_back(worker::Event::Admitted { owner, run }),
                Delivery::Answered { owner, answer } => {
                    self.worker_in.push_back(worker::Event::Answered { owner, answer });
                }
                Delivery::Open { conversation, opening } => {
                    let mut out = Vec::new();
                    self.partner.open(self.now, conversation, &opening, &mut out);
                    self.partner_out(out);
                }
                Delivery::Say { peer } => {
                    let mut out = Vec::new();
                    self.partner.say(self.now, peer, &mut out);
                    self.partner_out(out);
                }
                Delivery::Close { peer } => {
                    let mut out = Vec::new();
                    self.partner.close(self.now, peer, &mut out);
                    self.partner_out(out);
                }
                Delivery::Wake { peer, wake } => {
                    let mut out = Vec::new();
                    self.partner.woken(self.now, peer, wake, &mut out);
                    self.partner_out(out);
                }
                Delivery::Event(event) => {
                    self.check_conversation(&event);
                    self.run_in.push_back(event);
                }
            }
        }
    }

    /// What the partner asked for: events on their way to the run, and wakes.
    fn partner_out(&mut self, out: Vec<Out>) {
        for item in out {
            match item {
                Out::Event(event) => self.send(Lane::Run, Delivery::Event(event)),
                Out::Wake { at, peer, wake } => {
                    self.schedule(at, Delivery::Wake { peer, wake });
                }
            }
        }
    }

    /// The conversations' contract, as the run receives it: `Started` at most
    /// once and first, one `Ended` per open, nothing after it.
    fn check_conversation(&mut self, event: &run::Event) {
        let (conversation, started, ended) = match event {
            run::Event::Started { conversation, .. } => (conversation, true, false),
            run::Event::Ended { conversation, .. } => (conversation, false, true),
            run::Event::Yielded { conversation, .. } | run::Event::Used { conversation, .. } => {
                (conversation, false, false)
            }
            run::Event::Start { .. } | run::Event::Cancel { .. } => unreachable!("not a conversation's event"),
        };
        let open = self.opens.get_mut(conversation).expect("events are about conversations the run opened");
        assert!(!open.ended, "nothing comes after a conversation's end");
        if started {
            assert!(!open.started, "a conversation starts once");
            open.started = true;
        }
        if !started && !ended {
            assert!(open.started, "a conversation starts before anything else");
        }
        open.ended = ended;
    }

    fn has_work_now(&self) -> bool {
        !self.run_in.is_empty()
            || !self.worker_in.is_empty()
            || self.run.is_due(self.now)
            || self.worker.is_due(self.now)
            || self.wire.first_key_value().is_some_and(|((at, _), _)| *at <= self.now)
    }

    fn next_time(&self) -> Option<Time> {
        let wire = self.wire.first_key_value().map(|((at, _), _)| *at);
        [wire, self.run.next_deadline(), self.worker.next_deadline()].into_iter().flatten().min()
    }

    /// The invariants of a world where nothing is left to happen.
    fn assert_settled(&self) {
        assert_eq!(self.run.runs(), 0, "every run has answered and been reclaimed");
        assert_eq!(self.run.conversations(), 0, "every conversation has ended and been reclaimed");
        assert_eq!(self.run.next_deadline(), None, "no alarm outlives its run");
        assert_eq!(self.worker.jobs(), 0, "the worker took every answer");
        assert_eq!(self.worker.answered(), self.settings.worker.jobs, "every job was started and answered");
        assert_eq!(self.partner.live(), 0, "every conversation has ended");
        assert!(self.wire.is_empty() && self.run_in.is_empty() && self.worker_in.is_empty(), "nothing is on its way");
        let mut answered = run::Spend::ZERO;
        for (owner, start) in &self.starts {
            let answer = start.answer.as_ref().unwrap_or_else(|| panic!("start {owner:?} was answered"));
            if let run::Answer::Failed { spent, .. } = answer {
                answered = answered.saturating_add(*spent);
            }
        }
        for (conversation, open) in &self.opens {
            assert!(open.ended, "conversation {conversation:?} ended");
        }
        assert_eq!(answered, self.partner.spent(), "every turn spent is in exactly one answer");
    }

    /// Sends on `lane`, after its latency, and after whatever it carries
    /// already.
    fn send(&mut self, lane: Lane, delivery: Delivery) {
        let span = match lane {
            Lane::Agent | Lane::Worker => self.settings.network,
            Lane::Conversations | Lane::Run => self.settings.hop,
        };
        let latency = self.draw(span);
        let last = &mut self.lanes[lane as usize];
        let at = self.now.saturating_add(latency).max(*last);
        *last = at;
        self.schedule(at, delivery);
    }

    fn schedule(&mut self, at: Time, delivery: Delivery) {
        self.serial += 1;
        self.wire.insert((at, self.serial), delivery);
    }

    fn draw(&mut self, span: Span) -> Duration {
        Duration::from_nanos(self.rng.between(span.min.as_nanos(), span.max.as_nanos()))
    }

    fn log(&mut self, line: &str) {
        self.trace.push(format!("{:>16} {line}", self.now.as_nanos()));
    }
}

/// A run spends within its budget, give or take one turn in flight: a
/// conversation keeps to its share of turns, and may go past its share of
/// tokens by the turn that crossed it.
fn assert_within(budget: &run::Budget, answer: &run::Answer, turn: run::Spend) {
    let run::Answer::Failed { spent, .. } = answer else { return };
    assert!(spent.turns <= budget.turns, "{spent:?} keeps to the turns of {budget:?}");
    let over = |spent: u64, budget: u64, turn: u64| spent <= budget.saturating_add(turn);
    assert!(
        over(spent.input, budget.input, turn.input)
            && over(spent.output, budget.output, turn.output)
            && over(spent.cache_read, budget.cache_read, turn.cache_read)
            && over(spent.cache_write, budget.cache_write, turn.cache_write),
        "{spent:?} is within {budget:?} and one turn of at most {turn:?}"
    );
}
