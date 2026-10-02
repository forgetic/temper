use std::collections::{BTreeMap, BTreeSet};

use temper_agent_model_run as run;
use temper_lib::{Duration, ReplyTo, Rng, Time, Token};
use temper_world::{Key, Ledger, Schedule, Span, Stage, Trace};

use crate::host::{self, Host};
use crate::partner::{Out, Partner, Script, Tally};

/// Room in each model's output queue. Small, so the loop's flow control (take
/// an event only while there is room for what it may produce) is exercised.
const OUT: u32 = 4;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Settings {
    /// Seeds the world, which seeds the host and the partner.
    pub seed: u64,
    pub run: run::Limits,
    pub host: host::Script,
    pub partner: Script,
    /// One-way latency between the host and the agent.
    pub network: Span,
    /// One-way latency between the run and its conversations. Within the
    /// agent, the top level hands records over in the step that makes them;
    /// a latency here lets them cross in every order.
    pub hop: Span,
    /// What the checkouts hold, and how io works on them.
    pub checkout: Checkouts,
    /// The chance, per mille, that a check in flight wins the race with its
    /// abort. A push's race with its cancel is the host's.
    pub races: u32,
    /// The chance, per mille, that as the run is handed an event, the host's
    /// cancel of that event's run comes right before it or right after it.
    pub inject: u32,
}

/// The checkouts the world makes for runs, and io's way with them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Checkouts {
    /// The chance, per mille, that a repository has an `AGENTS.md`, of a
    /// length drawn from `1..=guide_max`.
    pub guides: u32,
    pub guide_max: u32,
    /// The chance, per mille, that a repository has checks, and how many
    /// times they fail before they pass: drawn from `0..=check_failures`.
    pub checks: u32,
    pub check_failures: u32,
    /// The chance, per mille, that a guide is not UTF-8 text.
    pub not_text: u32,
    /// How long io takes for each read or probe, and the chance, per mille,
    /// that it fails.
    pub io: Span,
    pub io_failures: u32,
    /// How long checks run.
    pub check: Span,
}

impl Settings {
    /// A world where nothing goes wrong at the agent's side: room for every
    /// run, charters well within the limits, an LLM that works and yields now
    /// and then but never finishes, and no cancels.
    #[must_use]
    pub const fn calm(seed: u64) -> Settings {
        Settings {
            seed,
            run: run::Limits {
                runs: 4,
                conversations: 16,
                run_bytes: 1 << 16,
                repositories: 4,
                outlets: 4,
                verdicts: 4,
                calls: 16,
                budget: run::Budget {
                    turns: 1000,
                    input: 1 << 32,
                    output: 1 << 32,
                    cache_read: 1 << 32,
                    cache_write: 1 << 32,
                    time: Duration::from_secs(24 * 3600),
                },
                max_tokens: 8192,
                models: 4,
                depth: 2,
                run_conversations: 4,
                answer_bytes: 512,
                nudges: 2,
                guide_bytes: 1024,
                io_timeout: Duration::from_secs(5),
                outcome_bytes: 4096,
                check_timeout: Duration::from_secs(600),
                check_tail: 256,
                facts: 64,
            },
            host: host::Script {
                jobs: 4,
                window: Duration::from_secs(10),
                cancels: 0,
                cancel: Span::millis(1_000, 60_000),
                recancels: 0,
                late_cancels: 0,
                brief_min: 100,
                brief_max: 2000,
                turns_min: 20,
                turns_max: 40,
                tokens_min: 1 << 20,
                tokens_max: 1 << 21,
                time: Span { min: Duration::from_secs(3600), max: Duration::from_secs(7200) },
                max_tokens: 4096,
                writable: 500,
                changes: 700,
                checks: 800,
                verdicts: 700,
                agents: 0,
                push: Span::millis(10, 500),
                moved: 0,
                push_failures: 0,
            },
            partner: Script {
                conversations: 16,
                invalid: 0,
                turn: Span::millis(100, 5_000),
                input: 4_000,
                output: 1_000,
                cache: 2_000,
                faults: 0,
                finishes: 0,
                asks: 0,
                yields: 200,
                bad_asks: 0,
                shares: 0,
                parallel: 1,
                changes: 500,
                good: 1000,
                odd_stops: 0,
                settle: Span::millis(1, 500),
                races: 500,
            },
            network: Span::millis(1, 20),
            hop: Span::millis(0, 2),
            checkout: Checkouts {
                guides: 500,
                guide_max: 2000,
                checks: 500,
                check_failures: 0,
                not_text: 0,
                io: Span::millis(0, 50),
                io_failures: 0,
                check: Span::millis(100, 5_000),
            },
            races: 500,
            inject: 0,
        }
    }
}

/// What the world counted.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Stats {
    /// Runs the host started, and cancels it sent.
    pub starts: u32,
    pub cancels: u32,
    /// Reads, probes and checks the run asked io for, and checks it aborted.
    pub reads: u32,
    pub probes: u32,
    pub checks: u32,
    pub aborts: u32,
    /// Pushes the run asked the host for, and host calls it cancelled.
    pub pushes: u32,
    pub host_cancels: u32,
    /// Conversations the run opened, sub-agents among them, nudges it said,
    /// and closes it sent.
    pub opens: u32,
    pub children: u32,
    pub says: u32,
    pub closes: u32,
    /// The most conversations a run had live at once.
    pub peak: u32,
    /// What the host and the partner counted.
    pub host: host::Tally,
    pub partner: Tally,
}

/// Something on its way, delivered at its time.
enum Delivery {
    /// The host's start reaches the agent.
    Start {
        reply_to: ReplyTo,
        worker: Token,
        charter: run::Charter,
    },
    /// The host's cancel reaches the agent.
    Cancel {
        run: Token,
    },
    /// The end of a host call reaches the run.
    Host(run::Event),
    /// The run's word reaches the host.
    Admitted {
        worker: Token,
        run: Token,
    },
    Answered {
        worker: Token,
    },
    Checking {
        worker: Token,
    },
    Push {
        worker: Token,
        owner: Token,
    },
    CancelHost {
        owner: Token,
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
    Return {
        call: Token,
        result: run::Returned,
    },
    /// A conversation's event reaches the run.
    Event(run::Event),
    /// io's answer reaches the run.
    Io(run::Event),
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
    Host,
    Conversations,
    Run,
}

/// A start, as the world tracks it: the budget its run keeps to, and the
/// answer once it has come.
struct Start {
    budget: run::Budget,
    /// The roots whose checks a change must pass, once its checkout is made.
    checks: BTreeSet<Token>,
    answer: Option<run::Answer>,
}

/// An open, as the world tracks it.
#[derive(Default)]
struct Open {
    started: bool,
    ended: bool,
}

/// A run as the world sees it from outside, to tell what state a cancel finds
/// it in.
struct RunView {
    budget: run::Budget,
    deadline: Time,
    /// Its main conversation, once opened.
    main: Option<Token>,
    started: bool,
    /// Whether it decided how it ends: it closed main, or a cancel or its
    /// deadline came while it prepared or worked.
    decided: bool,
    spent: run::Spend,
    /// Its conversations opened and not ended, and the most it had at once.
    live: u32,
    peak: u32,
    /// The roots whose checks a change must pass before it is pushed: of the
    /// writable repositories, when the spec wants checks, those with checks,
    /// less those io failed to look in.
    checks: BTreeSet<Token>,
    /// The iteration it answered in.
    answered: Option<u64>,
}

/// What one step or alarm of the run made, for the world to attribute: the run
/// it was about, if known, how many requests it made, and the state a cancel
/// it took found the run in.
struct Made {
    run: Option<Token>,
    requests: u32,
    cancel: Option<(Token, &'static str)>,
    /// The call, if the step took an ask for a sub-agent, and what its run had
    /// left as it took it.
    asked: Option<(Token, run::Budget)>,
}

/// A conversation's call, as the world tracks it.
struct Call {
    conversation: Token,
    returned: bool,
}

/// A push, as the world tracks it: for which job.
struct Pushing {
    job: Token,
}

pub struct World {
    now: Time,
    rng: Rng,
    settings: Settings,

    run: run::Model,
    run_stage: Stage<run::Limits, run::Event, run::Request>,

    host: Host,
    partner: Partner,

    /// Deliveries in flight.
    wire: Schedule<Delivery>,
    /// When each lane delivers its latest, which the next may not overtake.
    lanes: [Time; 4],
    /// Every start, by the host's name for it; every open, by the run's
    /// name for the conversation; every conversation's call, by its own name.
    starts: BTreeMap<Token, Start>,
    opens: BTreeMap<Token, Open>,
    calls: BTreeMap<Token, Call>,
    /// The checkouts' files, by their roots and paths, which of them are
    /// executable, and how many more times each repository's checks fail.
    files: BTreeMap<(Token, Vec<u8>), Vec<u8>>,
    executables: BTreeSet<(Token, Vec<u8>)>,
    failures: BTreeMap<Token, u64>,
    /// io's operations in flight, by their owners: the runs' looks and the
    /// calls' checks (with the root each runs in), apart, as their tokens are
    /// of different kinds; and where the result of each check in flight is on
    /// the wire.
    looks: BTreeSet<Token>,
    checking: BTreeMap<Token, Token>,
    checks: BTreeMap<Token, Key>,
    /// Pushes in flight, by the run's owner; and the jobs whose runs pushed a
    /// change.
    pushes: Ledger<Token, Pushing>,
    pushed: BTreeSet<Token>,
    /// The roots whose checks passed, for each landing call, until it pushes
    /// or its run answers.
    passed: BTreeMap<Token, BTreeSet<Token>>,
    /// The runs as the world sees them, by the run's names for them; which run
    /// each host name, conversation and landing call is of; the
    /// conversation each peer is; and the landing calls with a check or push
    /// in flight.
    views: BTreeMap<Token, RunView>,
    run_of_owner: BTreeMap<Token, Token>,
    run_of_conversation: BTreeMap<Token, Token>,
    run_of_call: BTreeMap<Token, Token>,
    conversation_of_peer: BTreeMap<Token, Token>,
    landing: BTreeSet<Token>,
    /// The states cancels and deadlines found runs in, and how many times.
    cancel_cells: BTreeMap<&'static str, u32>,
    /// The facts the run told, by kind.
    facts: BTreeMap<&'static str, u32>,
    deadline_cells: BTreeMap<&'static str, u32>,
    iteration: u64,
    /// The run the last request answered, for the iteration's attribution;
    /// the call the step being attributed took, if it asked for a sub-agent,
    /// with what its run had left then; and the sub-agent each such call
    /// opened, until it returns.
    just_answered: Option<Token>,
    asked: Option<(Token, run::Budget)>,
    child_of_call: BTreeMap<Token, Token>,

    stats: Stats,
    trace: Trace,
}

impl World {
    #[must_use]
    pub fn new(settings: Settings) -> World {
        assert!(run::worst_case(&settings.run).is_some(), "the shell refuses limits it cannot provision");
        let mut rng = Rng::new(settings.seed);
        let host = Host::new(settings.host, rng.next_u64());
        let partner = Partner::new(settings.partner, rng.next_u64());
        World {
            now: Time::ZERO,
            rng,
            settings,
            run: run::Model::new(&settings.run),
            run_stage: Stage::new(settings.run, run::MAX_OUT, OUT),
            host,
            partner,
            wire: Schedule::new(),
            lanes: [Time::ZERO; 4],
            starts: BTreeMap::new(),
            opens: BTreeMap::new(),
            calls: BTreeMap::new(),
            files: BTreeMap::new(),
            executables: BTreeSet::new(),
            failures: BTreeMap::new(),
            looks: BTreeSet::new(),
            checking: BTreeMap::new(),
            checks: BTreeMap::new(),
            pushes: Ledger::new("push"),
            pushed: BTreeSet::new(),
            passed: BTreeMap::new(),
            views: BTreeMap::new(),
            run_of_owner: BTreeMap::new(),
            run_of_conversation: BTreeMap::new(),
            run_of_call: BTreeMap::new(),
            conversation_of_peer: BTreeMap::new(),
            landing: BTreeSet::new(),
            cancel_cells: BTreeMap::new(),
            facts: BTreeMap::new(),
            deadline_cells: BTreeMap::new(),
            iteration: 0,
            just_answered: None,
            asked: None,
            child_of_call: BTreeMap::new(),
            stats: Stats::default(),
            trace: Trace::default(),
        }
    }

    #[must_use]
    pub fn now(&self) -> Time {
        self.now
    }

    #[must_use]
    pub fn stats(&self) -> Stats {
        Stats { host: self.host.tally(), partner: self.partner.tally(), ..self.stats }
    }

    /// What crossed between the models and the world, in order, with times.
    #[must_use]
    pub fn trace(&self) -> &[String] {
        self.trace.lines()
    }

    /// The states cancels found runs in: preparing, stopping, opening,
    /// working, landing, over (its budget), winding, answered (earlier in the
    /// same iteration) or gone; and how many times each.
    #[must_use]
    pub fn cancel_cells(&self) -> &BTreeMap<&'static str, u32> {
        &self.cancel_cells
    }

    /// The states runs' deadlines found them in, as for cancels.
    #[must_use]
    pub fn deadline_cells(&self) -> &BTreeMap<&'static str, u32> {
        &self.deadline_cells
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
        self.run_stage.tick(self.now);
        self.deliver();

        // Each stage takes its events, then fires its alarms, while it has room
        // for what one more may produce.
        self.iteration += 1;
        let mut made = Vec::new();
        while let Some(event) = self.run_stage.next_event() {
            self.log(&format!("run <- {event:?}"));
            let run = self.run_of(&event);
            let cancel = self.note(&event, run);
            // What the run has left as it takes an ask, before what else it
            // takes in this iteration is counted.
            let asked = if let run::Event::Delegated { call, ask: run::Ask::SubAgent { .. }, .. } = &event {
                let left = run.map(|run| self.views[&run].budget_left(self.now));
                left.map(|left| (*call, left))
            } else {
                None
            };
            let before = self.run_stage.out.len();
            run::step(&mut self.run, &self.run_stage.env, event, &mut self.run_stage.out);
            made.push(Made { run, requests: self.run_stage.out.len() - before, cancel, asked });
        }
        while self.run_stage.has_room() && self.run.is_due(self.now) {
            self.log("run alarm");
            let run = self.deadline_due();
            let before = self.run_stage.out.len();
            run::fire(&mut self.run, &self.run_stage.env, &mut self.run_stage.out);
            made.push(Made { run, requests: self.run_stage.out.len() - before, cancel: None, asked: None });
        }

        // What the steps asked for, submitted at the end of the iteration, each
        // attributed to the run its step was about.
        let mut answered = BTreeMap::new();
        for (index, made) in made.iter().enumerate() {
            let mut current = made.run;
            self.asked = made.asked;
            for _ in 0..made.requests {
                let request = self.run_stage.out.pop().expect("a step's requests are queued");
                current = self.run_request(request, current);
                if let Some(run) = self.just_answered.take() {
                    answered.insert(run, index);
                }
            }
        }
        // A cancel stepped after its run answered in this iteration found it
        // answered, not yet reclaimed.
        for (index, made) in made.iter().enumerate() {
            if let Some((run, mut cell)) = made.cancel {
                if answered.get(&run).is_some_and(|at| *at < index) {
                    cell = "answered";
                }
                *self.cancel_cells.entry(cell).or_insert(0) += 1;
            }
        }
        // The host's alarms.
        if self.host.is_due(self.now) {
            let mut out = Vec::new();
            self.host.fire(self.now, &mut out);
            self.host_out(out);
        }

        // The facts the run told, drained at the world's pace.
        while let Some(fact) = self.run.pop_fact() {
            self.fact(fact);
        }

        // The reclaim point.
        self.run.reclaim();
        assert!(self.run.runs() <= self.settings.run.runs, "runs stay within their slots");
        assert!(self.run.conversations() <= self.settings.run.conversations, "conversations stay within their slots");
        assert!(self.run.calls() <= self.settings.run.calls, "calls stay within their slots");
    }

    /// The run's requests, carried out the way the top level, the protocol
    /// layer, io and the conversations would. `current` is the run the step
    /// that made it was about, if known; what it is after the request is
    /// returned.
    fn run_request(&mut self, request: run::Request, current: Option<Token>) -> Option<Token> {
        self.log(&format!("run -> {request:?}"));
        let mut current = current;
        match request {
            run::Request::Admitted { worker, run } => {
                let Start { budget, checks, .. } = &self.starts[&worker];
                let view = RunView {
                    budget: *budget,
                    checks: checks.clone(),
                    deadline: self.now.saturating_add(budget.time),
                    main: None,
                    started: false,
                    decided: false,
                    spent: run::Spend::ZERO,
                    live: 0,
                    peak: 0,
                    answered: None,
                };
                self.views.insert(run, view);
                self.run_of_owner.insert(worker, run);
                current = Some(run);
                self.send(Lane::Host, Delivery::Admitted { worker, run });
            }
            run::Request::Answer { to, answer } => {
                let owner = self.answer(to, answer);
                if let Some(&run) = self.run_of_owner.get(&owner) {
                    // What its landings that did not push had passed.
                    self.passed.retain(|call, _| self.run_of_call.get(call) != Some(&run));
                    self.views.get_mut(&run).expect("a view per admitted run").answered = Some(self.iteration);
                    self.just_answered = Some(run);
                }
            }
            run::Request::Open { conversation, opening } => self.open(conversation, opening, current),
            run::Request::Say { peer, text: _ } => {
                self.stats.says += 1;
                self.send(Lane::Conversations, Delivery::Say { peer });
            }
            run::Request::Close { peer } => {
                let conversation = self.conversation_of_peer[&peer];
                let run = self.run_of_conversation[&conversation];
                self.views.get_mut(&run).expect("a run outlives its conversations").decided = true;
                self.stats.closes += 1;
                self.send(Lane::Conversations, Delivery::Close { peer });
            }
            run::Request::Return { call, result } => {
                let ledger = self.calls.get_mut(&call).expect("a return is of a call that was made");
                assert!(!ledger.returned, "a call returns once");
                ledger.returned = true;
                if let Some(child) = self.child_of_call.remove(&call) {
                    assert!(self.opens[&child].ended, "a sub-agent has ended before its call returns");
                }
                self.send(Lane::Conversations, Delivery::Return { call, result });
            }
            request @ (run::Request::Read { .. } | run::Request::Probe { .. } | run::Request::Abort { .. }) => {
                self.io_request(request);
            }
            run::Request::Check { owner, program, deadline, tail } => {
                let run = current.expect("a check is made in a step about its run");
                self.assert_alone(run);
                self.run_of_call.insert(owner, run);
                self.landing.insert(owner);
                self.check(owner, &program, deadline, tail);
            }
            run::Request::Checking { worker, deadline: _ } => self.send(Lane::Host, Delivery::Checking { worker }),
            run::Request::Push { worker, owner, change: _ } => {
                let run = current.expect("a push is made in a step about its run");
                self.assert_alone(run);
                self.run_of_call.insert(owner, run);
                self.landing.insert(owner);
                self.pushes.open(owner, Pushing { job: worker });
                // Checked is pushed: every repository whose checks the change
                // must pass passed them, for this landing.
                let passed = self.passed.remove(&owner).unwrap_or_default();
                let run = &self.views[&self.run_of_call[&owner]];
                assert!(run.checks.is_subset(&passed), "a change is pushed once every repository's checks passed it");
                self.stats.pushes += 1;
                self.send(Lane::Host, Delivery::Push { worker, owner });
            }
            run::Request::CancelHost { owner } => {
                self.stats.host_cancels += 1;
                // A push the host has answered already won the race; the host
                // decides the race for one still in flight.
                if self.pushes.contains(owner) {
                    self.send(Lane::Host, Delivery::CancelHost { owner });
                }
            }
        }
        current
    }

    /// The run's reads, probes and aborts, carried out the way io would.
    fn io_request(&mut self, request: run::Request) {
        match request {
            run::Request::Read { owner, at, max, deadline } => {
                let (at_time, read) = match self.io_result(owner, deadline) {
                    Some(at_time) => {
                        let read = match self.files.get(&(at.root, at.path.to_vec())) {
                            // io reads text, cut where a character ends.
                            Some(content) => match std::str::from_utf8(content) {
                                Ok(text) => {
                                    let mut cut = text.len().min(usize::try_from(max).expect("a u32 fits"));
                                    while !text.is_char_boundary(cut) {
                                        cut -= 1;
                                    }
                                    run::Read::Text { text: text.as_bytes()[..cut].into(), whole: cut == text.len() }
                                }
                                Err(_) => run::Read::NotText,
                            },
                            None => run::Read::Missing,
                        };
                        (at_time, read)
                    }
                    None => (deadline, run::Read::Failed),
                };
                self.stats.reads += 1;
                self.schedule(at_time, Delivery::Io(run::Event::Read { owner, read }));
            }
            run::Request::Probe { owner, at, deadline } => {
                let (at_time, executable) = if let Some(at_time) = self.io_result(owner, deadline) {
                    (at_time, self.executables.contains(&(at.root, at.path.to_vec())))
                } else {
                    // The run cannot know of checks io failed to find.
                    self.views.get_mut(&owner).expect("a run looks once admitted").checks.remove(&at.root);
                    (deadline, false)
                };
                self.stats.probes += 1;
                self.schedule(at_time, Delivery::Io(run::Event::Probed { owner, executable }));
            }
            run::Request::Abort { owner } => {
                self.stats.aborts += 1;
                // A check whose result is on its way already won the race.
                if let Some(key) = self.checks.get(&owner).copied()
                    && !self.rng.chance(self.settings.races)
                {
                    self.wire.withdraw(key).expect("a check in flight has its result on the wire");
                    self.checks.remove(&owner);
                    let at = self.now.saturating_add(self.draw(self.settings.checkout.io));
                    self.schedule(at, Delivery::Io(run::Event::Aborted { owner }));
                }
            }
            run::Request::Admitted { .. }
            | run::Request::Answer { .. }
            | run::Request::Open { .. }
            | run::Request::Say { .. }
            | run::Request::Close { .. }
            | run::Request::Return { .. }
            | run::Request::Check { .. }
            | run::Request::Checking { .. }
            | run::Request::Push { .. }
            | run::Request::CancelHost { .. } => unreachable!("not one of io's requests"),
        }
    }

    /// The run an event is about, if the world knows it yet.
    fn run_of(&self, event: &run::Event) -> Option<Token> {
        match event {
            run::Event::Start { .. } => None,
            run::Event::Cancel { run } => Some(*run),
            run::Event::Read { owner, .. } | run::Event::Probed { owner, .. } => Some(*owner),
            run::Event::Started { conversation, .. }
            | run::Event::Yielded { conversation, .. }
            | run::Event::Used { conversation, .. }
            | run::Event::Ended { conversation, .. }
            | run::Event::Delegated { conversation, .. }
            | run::Event::Withdraw { conversation, .. } => self.run_of_conversation.get(conversation).copied(),
            run::Event::Checked { owner, .. }
            | run::Event::Aborted { owner }
            | run::Event::Pushed { owner, .. }
            | run::Event::HostCancelled { owner } => self.run_of_call.get(owner).copied(),
        }
    }

    /// What an event the run is about to take tells the world of it; for a
    /// cancel, the state it finds the run in.
    fn note(&mut self, event: &run::Event, run: Option<Token>) -> Option<(Token, &'static str)> {
        let view = self.views.get_mut(&run?)?;
        match event {
            run::Event::Started { conversation, peer } => {
                self.conversation_of_peer.insert(*peer, *conversation);
                view.started = true;
            }
            run::Event::Used { spend, .. } => view.spent = view.spent.saturating_add(*spend),
            run::Event::Checked { owner, .. }
            | run::Event::Aborted { owner }
            | run::Event::Pushed { owner, .. }
            | run::Event::HostCancelled { owner } => {
                self.landing.remove(owner);
            }
            run::Event::Cancel { run } => {
                let cell = self.cell(*run);
                let view = self.views.get_mut(run).expect("looked up above");
                if matches!(cell, "preparing" | "opening" | "working" | "landing" | "over") {
                    view.decided = true;
                }
                return Some((*run, cell));
            }
            run::Event::Start { .. }
            | run::Event::Yielded { .. }
            | run::Event::Ended { .. }
            | run::Event::Delegated { .. }
            | run::Event::Withdraw { .. }
            | run::Event::Read { .. }
            | run::Event::Probed { .. } => {}
        }
        None
    }

    /// The state the world sees the run `run` in.
    fn cell(&self, run: Token) -> &'static str {
        let view = &self.views[&run];
        let landing = self.landing.iter().any(|owner| self.run_of_call.get(owner) == Some(&run));
        match view.answered {
            Some(at) if at < self.iteration => "gone",
            Some(_) => "answered",
            None if view.main.is_none() && view.decided => "stopping",
            None if view.main.is_none() => "preparing",
            None if view.decided => "winding",
            None if !view.started => "opening",
            // Past its budget is a state of the run's own, landing or not.
            None if view.budget_spent() => "over",
            None if landing => "landing",
            None => "working",
        }
    }

    /// The run whose deadline the run's alarm is about to fire, if one alone
    /// is due; its state, counted.
    fn deadline_due(&mut self) -> Option<Token> {
        let mut due =
            self.views.iter().filter(|(_, view)| view.answered.is_none() && !view.decided && view.deadline <= self.now);
        let (run, view) = due.next()?;
        let (run, deadline) = (*run, view.deadline);
        let tied = due.any(|(_, other)| other.deadline <= deadline);
        let earliest = self
            .views
            .iter()
            .all(|(other, view)| *other == run || view.answered.is_some() || view.decided || view.deadline > deadline);
        if tied || !earliest {
            return None;
        }
        let cell = self.cell(run);
        *self.deadline_cells.entry(cell).or_insert(0) += 1;
        self.views.get_mut(&run).expect("looked up above").decided = true;
        Some(run)
    }

    /// The run opens a conversation: main, or the sub-agent of the call the
    /// step took.
    fn open(&mut self, conversation: Token, opening: run::Opening, current: Option<Token>) {
        let fresh = self.opens.insert(conversation, Open::default()).is_none();
        assert!(fresh, "conversations have distinct names");
        let run = current.expect("a run opens a conversation in a step about it");
        self.run_of_conversation.insert(conversation, run);
        // Checked is pushed: nothing opens while a change is checked or pushed.
        let landing = self.landing.iter().any(|owner| self.run_of_call.get(owner) == Some(&run));
        assert!(!landing, "a run opens no conversation while its change is checked or pushed");
        let view = self.views.get_mut(&run).expect("a run is admitted before it opens a conversation");
        view.live += 1;
        view.peak = view.peak.max(view.live);
        self.stats.peak = self.stats.peak.max(view.peak);
        match self.asked.take() {
            None => {
                assert!(view.main.is_none(), "a run opens main once, and sub-agents when asked");
                view.main = Some(conversation);
            }
            Some((call, left)) => {
                // A sub-agent's share is within what its run had left as it
                // took the ask.
                let share = opening.budget;
                assert!(
                    share.turns <= left.turns
                        && share.input <= left.input
                        && share.output <= left.output
                        && share.cache_read <= left.cache_read
                        && share.cache_write <= left.cache_write
                        && share.time <= left.time,
                    "a sub-agent's share {share:?} is within what its run has left, {left:?}"
                );
                assert!(!opening.finish, "a sub-agent may not finish");
                self.child_of_call.insert(call, conversation);
                self.stats.children += 1;
            }
        }
        self.stats.opens += 1;
        self.send(Lane::Conversations, Delivery::Open { conversation, opening });
    }

    /// Checked is pushed: from a run's first check to its push, nothing but
    /// main is live in it, so nothing else may write to the checkout.
    fn assert_alone(&self, run: Token) {
        let view = &self.views[&run];
        assert_eq!(view.live, 1, "main is its run's only conversation while its change is checked and pushed");
    }

    /// The run's answer, checked against its budget and what it did: the
    /// host's name for the run.
    fn answer(&mut self, to: ReplyTo, answer: run::Answer) -> Token {
        let owner = to.into_token();
        let start = self.starts.get_mut(&owner).expect("an answer is to a start that was made");
        assert!(start.answer.is_none(), "a start is answered once");
        let peak = self.run_of_owner.get(&owner).map_or(0, |run| self.views[run].peak);
        assert_within(&start.budget, &answer, self.partner.turn_max(), peak);
        // A change is accepted only once it is pushed, and once it is pushed,
        // whatever the run was winding down for.
        let change = matches!(&answer, run::Answer::Accepted { outcome: run::outcome::Declared::Change(_), .. });
        assert_eq!(change, self.pushed.contains(&owner), "a change is accepted if and only if it is pushed");
        // A run answers once its main conversation, if it opened one, and
        // every other conversation it opened have ended.
        if let Some(run) = self.run_of_owner.get(&owner) {
            for (conversation, of) in &self.run_of_conversation {
                assert!(of != run || self.opens[conversation].ended, "no conversation outlives its run");
            }
        }
        start.answer = Some(answer);
        self.send(Lane::Host, Delivery::Answered { worker: owner });
        owner
    }

    /// Runs the checks at `program` as io would: they fail as many times as
    /// their repository was given, then pass, unless they outlast `deadline`.
    fn check(&mut self, owner: Token, program: &run::Place, deadline: Time, tail: u32) {
        assert!(self.checking.insert(owner, program.root).is_none(), "a call has one check in flight at a time");
        assert!(self.executables.contains(&(program.root, program.path.to_vec())), "checks are run where found");
        self.stats.checks += 1;
        let at = self.now.saturating_add(self.draw(self.settings.checkout.check));
        let failures = self.failures.entry(program.root).or_insert(0);
        let (at, exit, written) = if at > deadline {
            (deadline, run::Exit::TimedOut, 100)
        } else if *failures > 0 {
            *failures -= 1;
            (at, run::Exit::Code { code: 1 }, self.rng.between(0, u64::from(tail) * 3))
        } else {
            (at, run::Exit::Code { code: 0 }, 20)
        };
        // io keeps the tail of what the checks wrote.
        let kept = written.min(u64::from(tail));
        let output =
            b"test parse_tabs ... FAILED\n".iter().copied().cycle().take(usize::try_from(kept).expect("small"));
        let ran = run::Ran { exit, output: output.collect(), cut: written - kept };
        let key = self.schedule(at, Delivery::Io(run::Event::Checked { owner, ran }));
        self.checks.insert(owner, key);
    }

    /// When io answers a read or a probe of `owner`'s due by `deadline`, or
    /// `None` if it fails or runs out of time.
    fn io_result(&mut self, owner: Token, deadline: Time) -> Option<Time> {
        assert!(self.looks.insert(owner), "a run has one look in flight at a time");
        let at = self.now.saturating_add(self.draw(self.settings.checkout.io));
        let failed = self.rng.chance(self.settings.checkout.io_failures);
        (!failed && at <= deadline).then_some(at)
    }

    /// Lays out what each repository of a checkout the host made holds, as
    /// io finds it.
    fn checkout(&mut self, repositories: &[run::charter::Repository]) {
        let settings = self.settings.checkout;
        for &run::charter::Repository { root, .. } in repositories {
            if self.rng.chance(settings.guides) {
                let len = usize::try_from(self.rng.between(1, u64::from(settings.guide_max))).expect("small");
                // Text with characters of more than one byte, so a cut may
                // fall inside one; or, now and then, not text at all.
                let mut guide = String::new();
                for c in "Keep changes small — and run the tests, où qu'ils soient. ".chars().cycle() {
                    if guide.len() + c.len_utf8() > len {
                        break;
                    }
                    guide.push(c);
                }
                let mut guide = guide.into_bytes();
                if self.rng.chance(settings.not_text) {
                    guide.insert(0, 0xff);
                }
                self.files.insert((root, b"AGENTS.md".to_vec()), guide);
            }
            if self.rng.chance(settings.checks) {
                self.executables.insert((root, b".temper/pre-pr".to_vec()));
                self.failures.insert(root, self.rng.between(0, u64::from(settings.check_failures)));
            }
        }
    }

    /// What the host asked for: events on their way to the agent.
    fn host_out(&mut self, out: Vec<run::Event>) {
        for event in out {
            match event {
                run::Event::Start { reply_to, worker, charter } => {
                    let start = Start { budget: charter.budget, checks: BTreeSet::new(), answer: None };
                    assert!(self.starts.insert(worker, start).is_none(), "jobs have distinct names");
                    self.stats.starts += 1;
                    self.send(Lane::Agent, Delivery::Start { reply_to, worker, charter });
                }
                run::Event::Cancel { run } => {
                    self.stats.cancels += 1;
                    self.send(Lane::Agent, Delivery::Cancel { run });
                }
                run::Event::Pushed { owner, push } => {
                    let Pushing { job } = self.pushes.end(owner);
                    if push == run::Push::Done {
                        self.pushed.insert(job);
                    }
                    self.send(Lane::Agent, Delivery::Host(event));
                }
                run::Event::HostCancelled { owner } => {
                    self.pushes.end(owner);
                    self.send(Lane::Agent, Delivery::Host(event));
                }
                run::Event::Started { .. }
                | run::Event::Yielded { .. }
                | run::Event::Used { .. }
                | run::Event::Ended { .. }
                | run::Event::Read { .. }
                | run::Event::Probed { .. }
                | run::Event::Delegated { .. }
                | run::Event::Withdraw { .. }
                | run::Event::Checked { .. }
                | run::Event::Aborted { .. } => unreachable!("the host starts and cancels runs, and ends their pushes"),
            }
        }
    }

    /// Hands every delivery that is due to its destination.
    fn deliver(&mut self) {
        while let Some(delivery) = self.wire.next(self.now) {
            match delivery {
                Delivery::Start { reply_to, worker, charter } => {
                    self.checkout(&charter.checkout.repositories);
                    let wants = matches!(charter.outcome.change, Some(run::outcome::ChangeSpec { checks: true }));
                    let mut checks = BTreeSet::new();
                    for repository in &charter.checkout.repositories {
                        let executable = (repository.root, b".temper/pre-pr".to_vec());
                        if wants && repository.writable && self.executables.contains(&executable) {
                            checks.insert(repository.root);
                        }
                    }
                    self.starts.get_mut(&worker).expect("a start is tracked").checks = checks;
                    self.run_stage.push(run::Event::Start { reply_to, worker, charter });
                }
                Delivery::Cancel { run } => self.run_stage.push(run::Event::Cancel { run }),
                Delivery::Host(event) => self.hand(event),
                Delivery::Admitted { worker, run } => self.host.admitted(self.now, worker, run),
                Delivery::Answered { worker } => self.host.answered(self.now, worker),
                Delivery::Checking { worker } => self.host.checking(worker),
                Delivery::Push { worker, owner } => self.host.push(self.now, worker, owner),
                Delivery::CancelHost { owner } => {
                    let mut out = Vec::new();
                    self.host.cancel_host(owner, &mut out);
                    self.host_out(out);
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
                Delivery::Return { call, result } => {
                    let mut out = Vec::new();
                    self.partner.returned(self.now, call, &result, &mut out);
                    self.partner_out(out);
                }
                Delivery::Wake { peer, wake } => {
                    let mut out = Vec::new();
                    self.partner.woken(self.now, peer, wake, &mut out);
                    self.partner_out(out);
                }
                Delivery::Event(event) => {
                    self.check_conversation(&event);
                    self.hand(event);
                }
                Delivery::Io(event) => {
                    let answered = match &event {
                        run::Event::Read { owner, .. } | run::Event::Probed { owner, .. } => self.looks.remove(owner),
                        run::Event::Checked { owner, ran } => {
                            self.checks.remove(owner);
                            let root = self.checking.remove(owner);
                            if let Some(root) = root
                                && matches!(ran.exit, run::Exit::Code { code: 0 })
                            {
                                self.passed.entry(*owner).or_default().insert(root);
                            }
                            root.is_some()
                        }
                        run::Event::Aborted { owner } => {
                            self.checks.remove(owner);
                            self.checking.remove(owner).is_some()
                        }
                        run::Event::Start { .. }
                        | run::Event::Cancel { .. }
                        | run::Event::Started { .. }
                        | run::Event::Yielded { .. }
                        | run::Event::Used { .. }
                        | run::Event::Ended { .. }
                        | run::Event::Delegated { .. }
                        | run::Event::Withdraw { .. }
                        | run::Event::Pushed { .. }
                        | run::Event::HostCancelled { .. } => unreachable!("io answers reads, probes and checks"),
                    };
                    assert!(answered, "io answers each operation once");
                    self.hand(event);
                }
            }
        }
    }

    /// A fact the run told: of a run it admitted, and a conversation it
    /// opened.
    fn fact(&mut self, fact: run::facts::Fact) {
        use run::facts::Fact;
        let (Fact::Admitted { run }
        | Fact::Prepared { run, .. }
        | Fact::Opened { run, .. }
        | Fact::Ended { run, .. }
        | Fact::Called { run, .. }
        | Fact::Returned { run, .. }
        | Fact::CheckStarted { run, .. }
        | Fact::CheckFinished { run, .. }
        | Fact::Pushed { run, .. }
        | Fact::Answered { run, .. }) = fact;
        assert!(self.views.contains_key(&run), "a fact is of a run that was admitted");
        if let Fact::Opened { conversation, .. } | Fact::Ended { conversation, .. } = fact {
            assert!(self.opens.contains_key(&conversation), "a fact is of a conversation that was opened");
        }
        *self.facts.entry(kind(&fact)).or_insert(0) += 1;
    }

    /// The facts the run told, by kind, and how many it dropped.
    #[must_use]
    pub fn facts(&self) -> (&BTreeMap<&'static str, u32>, u64) {
        (&self.facts, self.run.facts_lost())
    }

    /// Hands the run `event`, and with the configured chance, the host's
    /// cancel of its run right before or right after it.
    fn hand(&mut self, event: run::Event) {
        let run = self.run_of(&event);
        let inject = match run {
            Some(_) => self.rng.chance(self.settings.inject),
            None => false,
        };
        let before = inject && self.rng.chance(500);
        if let Some(run) = run.filter(|_| before) {
            self.run_stage.push(run::Event::Cancel { run });
        }
        self.run_stage.push(event);
        if let Some(run) = run.filter(|_| inject && !before) {
            self.run_stage.push(run::Event::Cancel { run });
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
    /// once and first, one `Ended` per open once every call it made has
    /// returned, nothing after it; a withdraw only of a call it made.
    fn check_conversation(&mut self, event: &run::Event) {
        let (conversation, started, ended) = match event {
            run::Event::Started { conversation, .. } => (conversation, true, false),
            run::Event::Ended { conversation, .. } => (conversation, false, true),
            run::Event::Yielded { conversation, .. } | run::Event::Used { conversation, .. } => {
                (conversation, false, false)
            }
            run::Event::Delegated { conversation, call, .. } => {
                let fresh = self.calls.insert(*call, Call { conversation: *conversation, returned: false }).is_none();
                assert!(fresh, "calls have distinct names");
                (conversation, false, false)
            }
            run::Event::Withdraw { conversation, call } => {
                let ledger = self.calls.get(call).expect("a withdraw is of a call that was made");
                assert_eq!(ledger.conversation, *conversation, "a conversation withdraws its own calls");
                (conversation, false, false)
            }
            run::Event::Start { .. }
            | run::Event::Cancel { .. }
            | run::Event::Read { .. }
            | run::Event::Probed { .. }
            | run::Event::Checked { .. }
            | run::Event::Aborted { .. }
            | run::Event::Pushed { .. }
            | run::Event::HostCancelled { .. } => unreachable!("not a conversation's event"),
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
        if ended {
            let run = self.run_of_conversation[conversation];
            self.views.get_mut(&run).expect("a run outlives its conversations").live -= 1;
            for ledger in self.calls.values() {
                assert!(
                    ledger.conversation != *conversation || ledger.returned,
                    "a conversation ends once its calls have returned"
                );
            }
        }
    }

    fn has_work_now(&self) -> bool {
        self.run_stage.has_events()
            || self.run.is_due(self.now)
            || self.host.is_due(self.now)
            || self.wire.is_due(self.now)
    }

    fn next_time(&self) -> Option<Time> {
        [self.wire.next_time(), self.run.next_deadline(), self.host.next_deadline()].into_iter().flatten().min()
    }

    /// The invariants of a world where nothing is left to happen.
    fn assert_settled(&self) {
        assert_eq!(self.run.runs(), 0, "every run has answered and been reclaimed");
        assert_eq!(self.run.conversations(), 0, "every conversation has ended and been reclaimed");
        assert_eq!(self.run.calls(), 0, "every call has returned and been reclaimed");
        assert_eq!(self.run.next_deadline(), None, "no alarm outlives its run");
        self.host.assert_settled();
        assert_eq!(self.partner.live(), 0, "every conversation has ended");
        assert!(
            self.looks.is_empty() && self.checking.is_empty() && self.checks.is_empty(),
            "io answered every operation"
        );
        assert!(self.pushes.is_empty(), "every push was answered");
        assert!(self.passed.is_empty(), "every landing call returned");
        assert!(self.child_of_call.is_empty(), "every sub-agent's call returned");
        assert!(self.wire.is_empty() && !self.run_stage.has_events(), "nothing is on its way");
        let mut answered = run::Spend::ZERO;
        for (owner, start) in &self.starts {
            let answer = start.answer.as_ref().unwrap_or_else(|| panic!("start {owner:?} was answered"));
            match answer {
                run::Answer::Failed { spent, .. } | run::Answer::Accepted { spent, .. } => {
                    answered = answered.saturating_add(*spent);
                }
                run::Answer::Refused(_) => {}
            }
        }
        for (conversation, open) in &self.opens {
            assert!(open.ended, "conversation {conversation:?} ended");
        }
        for (call, ledger) in &self.calls {
            assert!(ledger.returned, "call {call:?} returned");
        }
        assert_eq!(answered, self.partner.spent(), "every turn spent is in exactly one answer");
    }

    /// Sends on `lane`, after its latency, and after whatever it carries
    /// already.
    fn send(&mut self, lane: Lane, delivery: Delivery) {
        let span = match lane {
            Lane::Agent | Lane::Host => self.settings.network,
            Lane::Conversations | Lane::Run => self.settings.hop,
        };
        let latency = self.draw(span);
        let last = &mut self.lanes[lane as usize];
        let at = self.now.saturating_add(latency).max(*last);
        *last = at;
        self.schedule(at, delivery);
    }

    fn schedule(&mut self, at: Time, delivery: Delivery) -> Key {
        self.wire.send(at, delivery)
    }

    fn draw(&mut self, span: Span) -> Duration {
        span.draw(&mut self.rng)
    }

    fn log(&mut self, line: &str) {
        self.trace.log(self.now, line);
    }
}

/// A fact's kind, to count it by.
fn kind(fact: &run::facts::Fact) -> &'static str {
    use run::facts::Fact;
    match fact {
        Fact::Admitted { .. } => "admitted",
        Fact::Prepared { .. } => "prepared",
        Fact::Opened { .. } => "opened",
        Fact::Ended { .. } => "ended",
        Fact::Called { .. } => "called",
        Fact::Returned { .. } => "returned",
        Fact::CheckStarted { .. } => "check started",
        Fact::CheckFinished { .. } => "check finished",
        Fact::Pushed { .. } => "pushed",
        Fact::Answered { .. } => "answered",
    }
}

impl RunView {
    /// What the run has left of its budget at `now`.
    fn budget_left(&self, now: Time) -> run::Budget {
        let (spent, budget) = (self.spent, self.budget);
        run::Budget {
            turns: budget.turns.saturating_sub(spent.turns),
            input: budget.input.saturating_sub(spent.input),
            output: budget.output.saturating_sub(spent.output),
            cache_read: budget.cache_read.saturating_sub(spent.cache_read),
            cache_write: budget.cache_write.saturating_sub(spent.cache_write),
            time: self.deadline.saturating_since(now),
        }
    }

    /// Whether its conversations spent past any part of its budget.
    fn budget_spent(&self) -> bool {
        let (spent, budget) = (self.spent, self.budget);
        spent.turns > budget.turns
            || spent.input > budget.input
            || spent.output > budget.output
            || spent.cache_read > budget.cache_read
            || spent.cache_write > budget.cache_write
    }
}

/// A run spends within its budget, give or take a turn per conversation it
/// had live at once, and two. The run sums `Used` and closes main once the
/// total crosses its budget: at main's next turn when main crossed it, at
/// once when a sub-agent did, the close cascading down the tree. A
/// conversation keeps to its own share, but a share is carved from what the
/// run had left when it opened, and the others spend from the same budget,
/// so none may have reached its own ceiling by then. After the crossing turn,
/// the conversation whose `Used` crossed may finish one more completion
/// before it is closed, and each conversation may finish the completion it
/// has in flight when its close comes, one that wins the race with it: the
/// partner starts a turn the moment one ends, so a close always finds one in
/// flight, where a session would run its completion's calls first.
fn assert_within(budget: &run::Budget, answer: &run::Answer, turn: run::Spend, peak: u32) {
    let (run::Answer::Failed { spent, .. } | run::Answer::Accepted { spent, .. }) = answer else { return };
    let turns = u64::from(peak) + 2;
    let over = |spent: u64, budget: u64, turn: u64| spent <= budget.saturating_add(turn.saturating_mul(turns));
    assert!(
        over(u64::from(spent.turns), u64::from(budget.turns), u64::from(turn.turns))
            && over(spent.input, budget.input, turn.input)
            && over(spent.output, budget.output, turn.output)
            && over(spent.cache_read, budget.cache_read, turn.cache_read)
            && over(spent.cache_write, budget.cache_write, turn.cache_write),
        "{spent:?} is within {budget:?} and {turns} turns of at most {turn:?} between them"
    );
}
