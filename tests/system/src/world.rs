//! One deterministic typed crossing: a committed root assignment enters a
//! scripted Smith agent; its concrete turns and terminal return to the root.
//! The fake store and person remain outside both domains.

use skein_fake_checkout::{Checkout, Exit, Program};
use skein_fake_llm_domain::api::Script;
use skein_lib::{Duration, ReplyTo, Token};
use smith_agent_world::{Job, Settings, World as Agent};
use smith_domain as smith;
use smith_domain_run as run;
use temper_engine_domain::{Delivery, engine};
use temper_engine_domain_fleet as fleet;
use temper_engine_domain_people as people;
use temper_engine_domain_world::{commits::Store, direct::Driver, walking};

/// Composed root and agent with the same selected task charter.
pub struct World {
    pub root: Driver,
    pub assignment: engine::Assignment,
    pub agent: Agent,
}

/// Start one person's chat, wait for a committed assignment, and pass its
/// translated charter to the in-process Smith domain and scripted fake LLM.
#[must_use]
#[expect(clippy::wildcard_enum_match_arm, reason = "the world selects only committed assignments")]
pub fn chat(words: &[u8], job: Job) -> World {
    let mut config = walking::config(901);
    config.run.model.account = 1;
    config.run.model.endpoint = 0;
    config.run.model.name = b"fake-1".as_slice().into();
    config.run.model.input_price = 0;
    config.run.model.cached_price = 0;
    config.run.model.output_price = 0;
    config.run.model.price_unit = 1;
    config.run.model.max_tokens = 4096;
    config.run.turns = 64;
    config.run.time = Duration::from_secs(3600);
    config.resume_bytes = 8192;
    let limits = walking::limits();
    let mut root = Driver::configured(Store::new(), config, &limits);
    root.send(engine::Event::Hello {
        channel: Token::new(7),
        hello: fleet::Hello {
            graces: Some(Duration::from_secs(1)),
            slots: 1,
            workstreams: Box::new([]),
            hosting: Box::new([]),
        },
    });
    root.settle();
    root.sign_in();
    root.settle();
    root.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(401)),
        sign_in: root.session(),
        key: [41; 16],
        ask: people::Ask::StartChat { project: 1, words: words.into() },
    });
    root.settle();
    let assignment = root
        .delivered
        .iter()
        .find_map(|delivery| match delivery {
            Delivery::Assigned { assignment, .. } => Some(assignment.clone()),
            _ => None,
        })
        .expect("durable assignment");
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
    disk.program(b"cargo test", Program {
        duration: std::time::Duration::from_millis(200),
        output: b"test result: ok. 1 passed\n".to_vec(),
        exit: Exit::Code(0),
        changes: Vec::new(),
    });
    let workspace = Some(run::Workspace {
        directories: Box::new([run::Directory {
            name: b"work".as_slice().into(),
            root: Token::new(disk.root(b"work")),
            writable: true,
            git: true,
            conflicts: Box::new([]),
        }]),
    });
    let resume = transcript.is_some();
    let start = temper_engine_smith::start(
        assignment,
        workspace,
        transcript,
        ReplyTo::new(Token::new(402)),
        Token::new(assignment.task),
    );
    let smith::Event::Start { ref charter, .. } = start else { panic!("typed Smith start") };
    let mut settings = Settings::calm(901);
    settings.job = job;
    settings.resume = resume;
    settings.cancel_at = cancel_at;
    settings.budget = charter.budget;
    settings.limits.run.budget = charter.budget;
    settings.limits.run.host_tools = 32;
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
    #[expect(clippy::wildcard_enum_match_arm, reason = "the world selects only the next committed assignment")]
    pub fn say_and_resume(&mut self, words: &[u8], key: u8, job: Job) {
        let first = self.agent.turns().first().expect("parked run has a concrete turn");
        let transcript = smith::Transcript {
            version: first.version,
            endpoint: first.endpoint,
            dialect: first.dialect,
            turns: self.agent.turns().into(),
            after: Box::new([]),
        };
        self.root.send(engine::Event::Ask {
            reply_to: ReplyTo::new(Token::new(2000 + u64::from(key))),
            sign_in: self.root.session(),
            key: [key; 16],
            ask: people::Ask::Say { project: 1, task: self.assignment.task, words: words.into() },
        });
        self.root.settle();
        let assignment = self
            .root
            .delivered
            .iter()
            .rev()
            .find_map(|delivery| match delivery {
                Delivery::Assigned { assignment, .. } if assignment.attempt > self.assignment.attempt => {
                    Some(assignment.clone())
                }
                _ => None,
            })
            .expect("new durable assignment");
        self.agent = agent_for(&assignment, Some(transcript), job);
        self.assignment = assignment;
    }

    /// Give each live Smith host call to the root, return its committed answer,
    /// then commit Smith's concrete turns and terminal.
    pub fn run(&mut self) {
        self.agent.enable_parent_host_calls();
        loop {
            if self.agent.drive(1000) {
                break;
            }
            let pending: Vec<_> = self.agent.pending_host_calls().into_iter().cloned().collect();
            assert!(!pending.is_empty(), "Smith must either settle or yield a host call");
            for submission in pending {
                let body = temper_engine_smith::call(submission.name, &submission.tool, &submission.input)
                    .expect("script uses the declared host schema");
                let before = self.root.delivered.len();
                self.root.send(engine::Event::Call {
                    channel: Token::new(7),
                    task: self.assignment.task,
                    attempt: self.assignment.attempt,
                    call: submission.relay.owner,
                    body,
                });
                self.root.settle();
                let answer = self.root.delivered[before..]
                    .iter()
                    .find_map(|delivery| match delivery {
                        Delivery::CallAnswer { call, answer, .. } if *call == submission.relay.owner => Some(answer),
                        _ => None,
                    })
                    .expect("committed root call answer");
                self.agent
                    .return_host_reply(submission.relay, run::HostReply::Answered(temper_engine_smith::answer(answer)))
                    .expect("one pending Smith host relay");
            }
        }
        for (number, read, spent) in self.agent.turn_metadata() {
            let index = usize::try_from(*number - 1).expect("positive turn");
            let record = self.agent.turns()[index].clone();
            let request = smith::Request::Turn {
                host_run: Token::new(self.assignment.task),
                number: *number,
                read: *read,
                spent: *spent,
                turn: record.clone(),
            };
            let bytes = format!("{record:?}").into_bytes().into_boxed_slice();
            let turn = temper_engine_smith::turn(request, bytes).expect("typed turn");
            self.root.send(engine::Event::Turn {
                channel: Token::new(7),
                task: self.assignment.task,
                attempt: self.assignment.attempt,
                turn,
            });
            self.root.settle();
        }
        let terminal = temper_engine_smith::result(copy_answer(self.agent.answer()), None).expect("typed terminal");
        self.root.send(engine::Event::Answer {
            saved: None,
            channel: Token::new(7),
            task: self.assignment.task,
            attempt: self.assignment.attempt,
            cumulative: terminal.cumulative,
            end: terminal.end,
        });
        self.root.settle();
    }
}

/// Copy a settled Smith terminal into the typed Temper result translator.
#[must_use]
pub fn copy_answer(answer: &run::Answer) -> run::Answer {
    match answer {
        run::Answer::Refused(refusal) => run::Answer::Refused(*refusal),
        run::Answer::Parked { spent, turns } => run::Answer::Parked { spent: *spent, turns: *turns },
        run::Answer::Failed { failure, spent, turns } => {
            run::Answer::Failed { failure: *failure, spent: *spent, turns: *turns }
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
