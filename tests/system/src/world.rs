//! One deterministic typed crossing: a committed root assignment enters a
//! scripted Smith agent; its concrete turns and terminal return to the root
//! as the hub's link records, which the engine world's protocol layer plays.
//! The fake store and person remain outside both domains.

use jig_domain_host as host;
use jig_domain_people as people;
use skein_fake_checkout::{Checkout, Exit, Program};
use skein_fake_llm_domain::api::Script;
use skein_lib::{Duration, ReplyTo, Token};
use smith_agent_world::{Job, Settings, World as Agent};
use smith_domain as smith;
use smith_domain_run as run;
use smith_domain_session::record;
use temper_engine_domain as engine;
use temper_engine_domain::Delivery;
use temper_engine_domain_world::{direct::Driver, protocol, store::Store, walking};

/// The one worker channel of the narrow fixtures.
pub const CHANNEL: Token = Token::new(7);

/// Composed root and agent with the same selected task charter.
pub struct World {
    pub root: Driver,
    pub assignment: engine::Assignment,
    pub agent: Agent,
}

/// The deployment's one charter as smith's fixtures price it: smith's fake
/// model at a unit per million tokens, with room for a scripted conversation (jig's
/// `domain/engine.md`, 7.1).
#[must_use]
pub fn charter() -> engine::Charter {
    let mut charter = walking::charter();
    charter.policy.model = engine::Model {
        program: None,
        dialect: temper_worker_domain_world::DIALECT,
        endpoint: 0,
        name: b"fake-1".as_slice().into(),
        window: 65_536,
        output: 4096,
        input_price: 1,
        cached_price: 1,
        cache_write_price: None,
        output_price: 1,
        price_unit: 1_000_000,
        efforts: Box::new([]),
        effort: None,
    };
    charter.policy.turns = 64;
    charter.policy.time = Duration::from_secs(3600);
    // A change's reserve covers the agent's checks (smith's run limits).
    charter.policy.reserve_time = CHECKS;
    charter
}

/// How long smith's agent may run a change's checks, which its fixtures set
/// (`smith_agent_world::LIMITS`, run `check_timeout`).
pub const CHECKS: Duration = Duration::from_secs(60);

/// The walking configuration with [`charter`] and room to resume its turns.
#[must_use]
pub fn config(seed: u64) -> engine::Config {
    let mut config = walking::config(seed);
    let charter = charter();
    // The charter's model is a configured one, as jig's core world sets it (tests/core charters.rs).
    config.core.settings.models = Box::new([charter.policy.model.clone()]);
    config.core.settings.llm_providers.providers.first_mut().expect("walking fixture's provider").models =
        Box::new([charter.policy.model.name.clone()]);
    config.core.defaults.charters = Box::new([charter]);
    config.core.settings.resume_bytes = 8192;
    config
}

/// The system configuration with a chat ended only by its requester.
#[must_use]
pub fn open_config(seed: u64) -> engine::Config {
    let mut config = config(seed);
    config.core.defaults.templates =
        temper_engine_domain_world::fixtures::templates_ending(jig_domain_tasks::EndedBy::Requester);
    config
}

/// What a worker says it can host: wide enough for [`charter`] and for the
/// configured spend, message and inbox bounds (jig's `domain/hosts.md`, 8).
#[must_use]
pub fn bounds() -> host::Bounds {
    let model = charter().policy.model;
    host::Bounds {
        program_endpoints: Box::new([]),
        models: Box::new([host::ModelBound { name: model.name, window: model.window, output: model.output }]),
        turns: 64,
        spend: 1000,
        time: Duration::from_secs(3600),
        waiting: Duration::from_secs(60),
        // Checks finish within a changing charter's reserved time.
        checks: CHECKS,
        message_bytes: 1024,
        inbox: 16,
        unacknowledged: walking::HUB_TURNS,
    }
}

/// One worker's hello on [`CHANNEL`] with `slots` slots and [`bounds`].
#[must_use]
pub fn hello(slots: u32) -> engine::Event {
    engine::Event::Worker(engine::WorkerInput::Link {
        channel: CHANNEL,
        link: engine::Link::Hello {
            worker: 0,
            hello: host::Hello {
                slots,
                stop_bound: Duration::from_secs(1),
                workstreams: Box::new([]),
                hosting: Box::new([]),
                bounds: bounds(),
            },
        },
    })
}

/// Start one person's chat, wait for a committed assignment, and pass its
/// translated charter to the in-process Smith domain and scripted fake LLM.
#[must_use]
pub fn chat(words: &[u8], job: Job) -> World {
    let mut root = Driver::configured(Store::new(), [config(901), config(901)], &walking::limits());
    root.send(hello(1));
    root.settle();
    root.sign_in();
    root.settle();
    root.send(engine::Event::Party(engine::PartyInput::Ask {
        models: Box::new([]),
        reply_to: ReplyTo::new(Token::new(401)),
        sign_in: root.session(),
        key: [41; 16],
        ask: temper_engine_domain_world::walking::chat(1, words),
    }));
    root.settle();
    let assignment = root.last_assignment();
    let agent = agent_for(&assignment, None, job);
    World { root, assignment, agent }
}

/// Run a claimed assignment with its actual concrete Smith history if the
/// previous activation parked within the root's resume bound.
#[must_use]
pub fn agent_for(assignment: &engine::Assignment, transcript: Option<smith::Transcript>, job: Job) -> Agent {
    agent_for_with_options(assignment, transcript, job, None, None)
}

/// A Smith run whose scripted host observes the person's cancellation.
#[must_use]
pub fn cancelled_agent_for(assignment: &engine::Assignment, job: Job) -> Agent {
    agent_for_with_options(assignment, None, job, Some(Duration::from_millis(1)), None)
}

/// A caller-supplied fake LLM script with a distinct cue in the task brief.
#[must_use]
pub fn scripted_agent_for(assignment: &engine::Assignment, script: Script) -> Agent {
    agent_for_with_options(assignment, None, Job::Reporting, None, Some(script))
}

/// A caller-supplied fake LLM script for a Smith coding assignment.
#[must_use]
pub fn scripted_coding_agent_for(assignment: &engine::Assignment, script: Script) -> Agent {
    agent_for_with_options(assignment, None, Job::Coding, None, Some(script))
}

/// A caller-supplied script for a Smith run that can park on `wait`.
#[must_use]
pub fn scripted_waiting_agent_for(
    assignment: &engine::Assignment,
    transcript: Option<smith::Transcript>,
    script: Script,
) -> Agent {
    agent_for_with_options(assignment, transcript, Job::Waiting, None, Some(script))
}

/// The concrete Smith history of an agent's turns, as a resume gives it.
#[must_use]
pub fn transcript(agent: &Agent) -> smith::Transcript {
    let first = agent.turns().first().expect("a concrete Smith turn");
    smith::Transcript {
        version: first.version,
        endpoint: first.endpoint,
        dialect: first.dialect,
        turns: agent.turns().into(),
    }
}

fn agent_for_with_options(
    assignment: &engine::Assignment,
    transcript: Option<smith::Transcript>,
    job: Job,
    cancel_at: Option<Duration>,
    script: Option<Script>,
) -> Agent {
    let mut disk = Checkout::new();
    disk.mkdir(b"work");
    disk.write(b"work/README.md", b"The answer is 42.\n");
    disk.write(b"work/AGENTS.md", b"Report what you found.\n");
    disk.write(b"work/src/lib.rs", b"pub fn answer() -> u32 { 42 }\n");
    disk.write(b"work/.temper/pre-pr", b"#!checks\nsrc/lib.rs 43\n");
    disk.program(
        b"cargo test",
        Program {
            duration: std::time::Duration::from_millis(200),
            output: b"test result: ok. 1 passed\n".to_vec(),
            exit: Exit::Code(0),
            changes: Vec::new(),
        },
    );
    let workspace = Some(run::Workspace {
        directories: Box::new([run::Directory {
            name: b"work".as_slice().into(),
            host: b"/work".as_slice().into(),
            root: Token::new(disk.root(b"work")),
            writable: true,
            git: true,
            conflicts: Box::new([]),
        }]),
    });
    let resume = transcript.is_some();
    let mut settings = Settings::calm(901);
    let largest = smith::max_turn_bytes(&settings.limits).expect("bounded Smith turn");
    // This composed story commits the run's turns after its answer, so its
    // window covers the charter's entire bounded activation.
    let window = smith::Window {
        turns: assignment.run.policy.turns,
        bytes: largest.checked_mul(u64::from(assignment.run.policy.turns)).expect("bounded activation"),
    };
    let start = temper_engine_smith::start(
        assignment,
        workspace,
        transcript,
        ReplyTo::new(Token::new(402)),
        Token::new(assignment.task),
        window,
    );
    let smith::Event::Start { ref charter, .. } = start else { panic!("typed Smith start") };
    settings.job = job;
    settings.resume = resume;
    settings.cancel_at = cancel_at;
    settings.budget = charter.budget;
    settings.limits.run.budget = charter.budget;
    settings.limits.run.host_tools =
        u32::try_from(temper_engine_smith::tools(Duration::from_secs(10)).len()).expect("tool count");
    settings.limits.run.brief_sections = 16;
    settings.limits.session.spend = charter.budget.spend;
    let scripts = match script {
        Some(script) => Box::new([script]),
        None => smith_agent_world::script::all(),
    };
    Agent::with_workspace_scripts_start(settings, disk, scripts, start)
}

impl World {
    /// Commit a person's new words after a parked activation and give the
    /// next Smith activation its concrete previous turns.
    pub fn say_and_resume(&mut self, words: &[u8], key: u8, job: Job) {
        let transcript = transcript(&self.agent);
        self.root.send(engine::Event::Party(engine::PartyInput::Ask {
            models: Box::new([]),
            reply_to: ReplyTo::new(Token::new(2000 + u64::from(key))),
            sign_in: self.root.session(),
            key: [key; 16],
            ask: people::Ask::Say { project: 1, task: self.assignment.task, words: words.into() },
        }));
        self.root.settle();
        let assignment = self.root.last_assignment();
        assert!(assignment.attempt > self.assignment.attempt, "new durable assignment");
        self.agent = agent_for(&assignment, Some(transcript), job);
        self.assignment = assignment;
    }

    /// Give each live Smith host call to the root, return its committed answer,
    /// then commit Smith's concrete turns and terminal.
    pub fn run(&mut self) {
        run_assignment(&mut self.root, &self.assignment, &mut self.agent);
    }
}

/// Relay one Smith host call to the root as the hub's record and return the
/// root's committed answer once the protocol layer rendered it.
pub fn relay(
    root: &mut Driver,
    assignment: &engine::Assignment,
    submission: &smith_agent_world::host_referee::Submission,
) -> engine::CallAnswer {
    let body = temper_engine_smith::call(submission.name, &submission.tool, &submission.input)
        .expect("script uses the declared host schema");
    let call = submission.relay.owner.raw();
    root.call(CHANNEL, assignment.task, assignment.attempt, call, body);
    root.settle();
    root.answered(call).expect("committed root call answer").clone()
}

/// Execute one root assignment in the Smith world and commit its typed output.
pub fn run_assignment(root: &mut Driver, assignment: &engine::Assignment, agent: &mut Agent) {
    agent.enable_parent_host_calls();
    loop {
        if agent.drive(1000) {
            break;
        }
        let pending: Vec<_> = agent.pending_host_calls().into_iter().cloned().collect();
        assert!(!pending.is_empty(), "Smith must either settle or yield a host call");
        for submission in pending {
            let answer = relay(root, assignment, &submission);
            agent
                .return_host_reply(submission.relay, run::HostReply::Answered(temper_engine_smith::answer(&answer)))
                .expect("one pending Smith host relay");
        }
    }
    for (number, read, spent) in agent.turn_metadata() {
        let index = usize::try_from(*number - 1).expect("positive turn");
        let turn = turn(assignment, *number, *read, *spent, &agent.turns()[index]);
        root.send(protocol::up(
            CHANNEL,
            host::LinkUp::Turn { run: Token::new(assignment.task), attempt: Token::new(assignment.attempt), turn },
            engine::Parts::none(),
        ));
        root.settle();
    }
    let terminal = temper_engine_smith::result(copy_answer(agent.answer()), None).expect("typed terminal");
    root.answer(CHANNEL, assignment.task, assignment.attempt, terminal.cumulative, terminal.end, engine::Parts::none());
    root.settle();
}

/// One of smith's concrete turns as the hub's turn record, its body the
/// record's own bytes as a codec would keep them.
#[must_use]
pub fn turn(
    assignment: &engine::Assignment,
    number: u32,
    read: Option<Token>,
    spent: run::Spend,
    record: &record::Turn,
) -> host::Turn {
    let request = smith::Request::Turn {
        host_run: Token::new(assignment.task),
        number,
        position: record.sequence,
        opens_window: matches!(record.kind, record::Kind::First(_)),
        read,
        spent,
        turn: record.clone(),
    };
    let bytes = format!("{record:?}").into_bytes().into_boxed_slice();
    temper_engine_smith::turn(request, bytes).expect("typed turn")
}

/// Copy a settled Smith terminal into the typed Temper result translator.
#[must_use]
pub fn copy_answer(answer: &run::Answer) -> run::Answer {
    match answer {
        run::Answer::Refused(refusal) => run::Answer::Refused(*refusal),
        run::Answer::Parked { spent, turns } => run::Answer::Parked { spent: *spent, turns: *turns },
        run::Answer::Failed { failure, spent, turns } => {
            run::Answer::Failed { failure: failure.clone(), spent: *spent, turns: *turns }
        }
        run::Answer::Accepted { outcome, spent, turns } => {
            let outcome = match outcome {
                run::outcome::Declared::Change(change) => run::outcome::Declared::Change(change.clone()),
                run::outcome::Declared::Verdict(verdict) => run::outcome::Declared::Verdict(verdict.clone()),
                run::outcome::Declared::Report(report) => run::outcome::Declared::Report(report.clone()),
                run::outcome::Declared::Failure(failure) => run::outcome::Declared::Failure(failure.clone()),
            };
            run::Answer::Accepted { outcome, spent: *spent, turns: *turns }
        }
    }
}

/// Every durable assignment the root delivered, oldest first.
#[must_use]
pub fn assignments(root: &Driver) -> Vec<engine::Assignment> {
    let mut assignments = Vec::new();
    for delivery in &root.delivered {
        if let Delivery::Assigned { assignment, .. } = delivery {
            assignments.push(assignment.clone());
        }
    }
    assignments
}
