//! Root forge stories with its durable store and the independent fake Forgejo.
#![expect(clippy::wildcard_enum_match_arm, reason = "the fixture selects only the forge rows relevant to each story")]
use jig_core_authority as authority;
use jig_core_fleet as fleet;
use jig_core_people as people;
use jig_core_tasks as tasks;
use skein_lib::{Duration, Env, List, Queue, ReplyTo, Time, Token, Wall};
use std::collections::{BTreeMap, VecDeque};
use temper_engine_domain::{Delivery, Key, Record, engine};
use temper_engine_domain_forge as forge_top;
use temper_engine_domain_forge_client as client;
use temper_engine_domain_world::{commits::Store, walking};
use temper_engine_forge_world::{self as forge_world, translate};
use temper_fake_forge_domain::{self as fake, api as raw};

struct World {
    root: engine::Domain,
    env: Env<engine::Limits>,
    out: Queue<engine::Request>,
    events: VecDeque<engine::Event>,
    store: Store,
    fake: fake::Domain,
    fake_env: Env<fake::Config>,
    fake_out: Queue<fake::Request>,
    pending: BTreeMap<Token, client::api::Op>,
    adopted: Vec<people::Outcome>,
    signed_in: Option<u64>,
    assigned: Vec<engine::Assignment>,
    answers: Vec<temper_engine_domain::CallAnswer>,
}

#[derive(Clone, Copy, Debug)]
enum Until {
    Ready,
    SignedIn,
    Adopted,
    Assigned,
    Delegated,
    SecondAssignment,
}

impl World {
    #[expect(
        clippy::too_many_lines,
        reason = "one fixture configures the policy, fake, and bounds for root forge stories"
    )]
    fn configured_policy(
        with_read: bool,
        with_change: bool,
        silent_ci: bool,
        passes: u32,
        approval: Option<engine::Freshness>,
    ) -> Self {
        let mut limits = walking::limits();
        limits.people.people = 4;
        limits.people.holdings = 4;
        limits.journal.writes += (people::max_out(&limits.people) - people::max_out(&walking::limits().people)) * 4;
        let mut config = walking::config(71);
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
        if approval.is_some() {
            limits.authority.roles = 3;
            limits.authority.requirements = 8;
            limits.authority.facts = 8;
        }
        if with_change {
            limits.tasks.tasks = 12;
            limits.tasks.project_tasks = 12;
            limits.tasks.delegates = 4;
            limits.tasks.tree_tasks = 12;
            limits.tasks.depth = 3;
            limits.tasks.parameters = 8;
            limits.tasks.spec_bytes = 512;
            limits.tasks.executor_kinds = 2;
            limits.tasks.authority_grants = 6;
            limits.tasks.authority_segments = 9;
            limits.tasks.authority_bytes = 512;
            limits.tasks.contract_choices = 2;
            limits.tasks.inbox_messages = 8;
            limits.tasks.inbox_bytes = 1024;
            limits.authority.executors = 2;
            limits.authority.batch = 2;
            limits.authority.grants = 6;
            limits.authority.segments = 9;
            limits.authority.segment_bytes = 64;
            limits.authority.writes = 2;
            limits.journal.writes += 64;
            limits.journal.writes = 4096;
            limits.journal.deliveries = 256;
            limits.journal.held = 1024;
            limits.journal.result_bytes = 4096;
            limits.journal.transcript_bytes = 32_768;
            limits.fleet.slots = 2;
            limits.fleet.workstreams = 2;
        }
        if with_read {
            limits.tasks.inbox_bytes = limits.tasks.inbox_bytes.max(256);
            if !with_change {
                limits.authority.grants = 3;
            }
            limits.authority.segments = 9;
            limits.authority.segment_bytes = 64;
            if !with_change {
                limits.tasks.authority_grants = 2;
            }
            limits.tasks.authority_segments = 9;
            if !with_change {
                limits.tasks.authority_bytes = 64;
            }
            let grant = authority::Grant {
                connector: 0,
                kind: 1,
                pattern: authority::Pattern {
                    segments: Box::new([
                        Box::from(&b"forge"[..]),
                        Box::from(&b"forge.example"[..]),
                        Box::from(&b"org"[..]),
                    ]),
                    last: authority::Last::Open(Box::from(&b"repo"[..])),
                },
            };
            let effect_grant = authority::Grant { kind: 8, ..grant.clone() };
            let mut rules = config.authority.rules().clone();
            if with_change {
                rules.maximum_run_spend = 60;
            }
            rules.ceiling.tools = authority::Tools(1);
            let grants: Box<[authority::Grant]> = if with_change {
                Box::new([
                    grant.clone(),
                    effect_grant.clone(),
                    authority::Grant { kind: 2, ..grant.clone() },
                    authority::Grant { kind: 3, ..grant.clone() },
                    authority::Grant { kind: 4, ..grant.clone() },
                ])
            } else {
                Box::new([grant.clone(), effect_grant.clone()])
            };
            let mut ceiling_grants = List::with_capacity(limits.authority.grants);
            for grant in &grants {
                ceiling_grants.push(grant.clone()).expect("configured task grants");
            }
            ceiling_grants.push(authority::Grant { kind: 7, ..grant.clone() }).expect("projection comments");
            rules.ceiling.grants = ceiling_grants.into_boxed();
            if let Some(freshness) = approval {
                let landing = Box::new([engine::LandingRule {
                    connector: 0,
                    kind: 4,
                    enforce: true,
                    pattern: authority::Pattern {
                        segments: Box::new([
                            Box::from(&b"forge"[..]),
                            Box::from(&b"forge.example"[..]),
                            Box::from(&b"org"[..]),
                            Box::from(&b"repo"[..]),
                            Box::from(&b"branch"[..]),
                        ]),
                        last: authority::Last::Exact(Box::from(&b"main"[..])),
                    },
                    ci: true,
                    up_to_date: true,
                    gates: Box::new([]),
                    approvals: Box::new([engine::Approval { role: 2, people: 1, freshness }]),
                }]);
                config.landing.deployment = landing.clone();
                assert!(config.landing.projects.insert(1, landing).is_ok());
            }
            if with_change {
                rules.ceiling.delegation.kinds =
                    Box::new([authority::Executor::Charter(1), authority::Executor::Procedure(2)]);
                rules.ceiling.delegation.tasks = 8;
                rules.ceiling.delegation.depth = 3;
            }
            let mut policy = config.authority.policy(1).expect("walk policy").clone();
            policy.ceiling.tools = authority::Tools(1);
            policy.ceiling.grants.clone_from(&rules.ceiling.grants);
            policy.projections.clone_from(&rules.ceiling.grants);
            if approval.is_some() {
                let mut maintainer = policy.roles[0].clone();
                maintainer.number = 1;
                let mut member = maintainer.clone();
                member.number = 2;
                policy.roles = Box::new([policy.roles[0].clone(), maintainer, member]);
            }
            policy.roles[0].authority.tools = authority::Tools(1);
            policy.roles[0].authority.grants.clone_from(&grants);
            if with_change {
                policy.ceiling.delegation = rules.ceiling.delegation.clone();
                policy.roles[0].authority.delegation = rules.ceiling.delegation.clone();
            }
            let mut domain = authority::Domain::new(rules, limits.authority).expect("read authority rules");
            let mut facts = Queue::with_capacity(authority::POLICY_MAX_OUT);
            authority::step(&mut domain, authority::Event::Policy { project: 1, policy }, &mut facts);
            assert_eq!(facts.pop(), Some(authority::PolicyFact::Added { project: 1 }));
            config.authority = domain;
            config.chat_authority.tools = authority::Tools(1);
            config.chat_authority.grants = grants;
            if with_change {
                config.chat_authority.delegation.kinds =
                    Box::new([authority::Executor::Charter(1), authority::Executor::Procedure(2)]);
                config.chat_authority.delegation.tasks = 4;
                config.chat_authority.delegation.depth = 2;
            }
        }
        let mut fake_config = forge_world::fake_config();
        fake_config.limits.repositories = 2;
        let mut fake = fake::Domain::new(&fake_config, 71);
        fake::repository(
            &mut fake,
            &fake_config,
            raw::Setup {
                name: Box::from(&b"org/repo"[..]),
                default: Box::from(&b"main"[..]),
                tree: Box::new([]),
                labels: Box::new([]),
                checks: raw::Checks {
                    contexts: Box::new([Box::from(&b"build"[..])]),
                    latency_min: Duration::ZERO,
                    latency_max: Duration::ZERO,
                    silent: if silent_ci { 1000 } else { 0 },
                    passes,
                    reruns: 0,
                    cue: None,
                },
                protection: None,
                hooked: false,
            },
        );
        fake::grant(&mut fake, b"org/repo", 1, raw::Permission::Admin);
        fake::grant(&mut fake, b"org/repo", 2, raw::Permission::Write);
        fake::grant(&mut fake, b"org/repo", 7, raw::Permission::Read);
        let fake_env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: fake_config };
        Self {
            root: engine::Domain::new(config, &limits),
            env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits },
            out: Queue::with_capacity(engine::max_out(&limits)),
            events: VecDeque::from([engine::Event::Start]),
            store: Store::new(),
            fake,
            fake_env,
            fake_out: Queue::with_capacity(fake::MAX_OUT),
            pending: BTreeMap::new(),
            adopted: Vec::new(),
            signed_in: None,
            assigned: Vec::new(),
            answers: Vec::new(),
        }
    }

    fn send(&mut self, event: engine::Event) {
        engine::step(&mut self.root, &self.env, event);
        engine::release(&mut self.root, &self.env, &mut self.out);
        self.collect();
        self.root.reclaim();
    }

    fn external(&mut self, user: u64, op: raw::Op) -> raw::Answer {
        let mut config = self.fake_env.limits;
        config.latency_min = Duration::ZERO;
        config.latency_max = Duration::ZERO;
        let env = Env { now: self.env.now, wall: self.env.wall, limits: config };
        fake::step(
            &mut self.fake,
            &env,
            fake::Event::Call {
                reply_to: ReplyTo::new(Token::new(10_000)),
                user,
                repository: Box::from(&b"org/repo"[..]),
                op,
            },
            &mut self.fake_out,
        );
        for _ in 0..8 {
            if self.fake.is_due(env.now) {
                fake::fire(&mut self.fake, &env, &mut self.fake_out);
            }
            while let Some(output) = self.fake_out.pop() {
                if let fake::Request::Reply { result, .. } = output {
                    return result.expect("external fake call");
                }
            }
        }
        panic!("external fake call did not return")
    }

    #[expect(clippy::wildcard_enum_match_arm, reason = "the world records only its scripted root deliveries")]
    fn collect(&mut self) {
        for _ in 0..self.out.len() {
            match self.out.pop().expect("counted root output") {
                engine::Request::Commit { number, writes } => self.store.pending.push_back((number, writes)),
                engine::Request::Load { owner, range, after, most, .. } => {
                    let (rows, next) = self.store.page(&range, after.as_ref(), most);
                    self.events.push_back(engine::Event::Loaded { owner, rows, next });
                }
                engine::Request::Forge { call, repository, op } => {
                    let name: &[u8] = if repository == forge_world::REPO {
                        b"org/repo"
                    } else if repository == (client::api::Repository { forge: 1, repository: 3 }) {
                        b"org/extra"
                    } else {
                        panic!("unknown test repository {repository:?}");
                    };
                    let raw = translate::op(&op, &self.env.limits.forge.client);
                    self.pending.insert(call, op);
                    fake::step(
                        &mut self.fake,
                        &self.fake_env,
                        fake::Event::Call {
                            reply_to: ReplyTo::new(call),
                            user: 1,
                            repository: Box::from(name),
                            op: raw,
                        },
                        &mut self.fake_out,
                    );
                }
                engine::Request::Deliver(delivery) => match delivery {
                    Delivery::WebReply { sign_in, reply: people::Reply::SignedIn { .. }, .. } => {
                        self.signed_in = sign_in;
                    }
                    Delivery::Assigned { assignment, .. } => self.assigned.push(assignment),
                    Delivery::CallAnswer { answer, .. } => self.answers.push(answer),
                    Delivery::WebReply { reply: people::Reply::Outcome(outcome), .. }
                        if matches!(outcome, people::Outcome::Adopted { .. }) =>
                    {
                        self.adopted.push(outcome);
                    }
                    _ => {}
                },
                engine::Request::Account(_)
                | engine::Request::View(_)
                | engine::Request::WatchRefused { .. }
                | engine::Request::CallBusy { .. }
                | engine::Request::TurnBusy { .. }
                | engine::Request::AnswerBusy { .. } => {}
                engine::Request::Host(_) => panic!("legacy forge world received typed input"),
                engine::Request::Stop => panic!("root startup or store failure"),
            }
        }
    }

    fn tick(&mut self) {
        self.env.now = self.env.now.saturating_add(Duration::from_millis(100));
        self.env.wall = Wall::from_nanos(self.env.now.as_nanos());
        self.fake_env.now = self.env.now;
        self.fake_env.wall = self.env.wall;
        if let Some(event) = self.events.pop_front() {
            self.send(event);
        }
        if !self.store.pending.is_empty() {
            let number = self.store.apply();
            self.send(engine::Event::Committed { number });
        }
        if self.fake.is_due(self.fake_env.now) {
            fake::fire(&mut self.fake, &self.fake_env, &mut self.fake_out);
        }
        for _ in 0..self.fake_out.len() {
            match self.fake_out.pop().expect("counted fake output") {
                fake::Request::Reply { to, result } => {
                    let call = to.into_token();
                    let op = self.pending.remove(&call).expect("one pending forge call");
                    let result = match result {
                        Ok(answer) => Ok(translate::answer(&op, answer, &self.env.limits.forge.client)),
                        Err(error) => {
                            Err(translate::error(error, fake::time(&self.fake_env.limits, self.fake_env.now)))
                        }
                    };
                    self.events.push_back(engine::Event::ForgeAnswered { call, cost: 1, result });
                }
                fake::Request::Hook { .. } => {}
            }
        }
        engine::release(&mut self.root, &self.env, &mut self.out);
        self.collect();
        engine::fire(&mut self.root, &self.env);
        engine::release(&mut self.root, &self.env, &mut self.out);
        self.collect();
        self.root.reclaim();
        self.fake.reclaim();
    }

    fn until(&mut self, target: Until) {
        for _ in 0..200 {
            self.tick();
            let done = match target {
                Until::Ready => self.root.quiescent() && self.events.is_empty() && self.store.pending.is_empty(),
                Until::SignedIn => self.signed_in.is_some(),
                Until::Adopted => !self.adopted.is_empty(),
                Until::Assigned => !self.assigned.is_empty(),
                Until::Delegated => {
                    self.answers.iter().any(|answer| matches!(answer, temper_engine_domain::CallAnswer::Delegated(_)))
                }
                Until::SecondAssignment => self.assigned.len() >= 2,
            };
            if done {
                return;
            }
        }
        panic!(
            "forge root did not reach {target:?}; answers={:?}, assignments={:?}, task2={:?}, task3={:?}, forge={:?}",
            self.answers,
            self.assigned,
            self.store.rows.get(&Key::Tasks(tasks::Key::Live(2))),
            self.store.rows.get(&Key::Tasks(tasks::Key::Live(3))),
            self.store.rows.values().filter(|row| matches!(row, Record::Forge { .. })).collect::<Vec<_>>()
        );
    }

    fn adopt(&mut self) {
        self.until(Until::Ready);
        self.send(engine::Event::SignedIn {
            reply_to: ReplyTo::new(Token::new(90)),
            identity: people::Identity {
                key: people::IdentityKey { provider: 0, subject: 7_u64.to_be_bytes().into() },
                login: Box::from(&b"owner"[..]),
                name: Box::from(&b"Owner"[..]),
            },
        });
        self.until(Until::SignedIn);
        self.send(engine::Event::Ask {
            reply_to: ReplyTo::new(Token::new(91)),
            sign_in: self.signed_in.expect("signed in"),
            key: [91; 16],
            ask: engine::adopt_repository_ask(
                forge_top::Adoption {
                    project: 1,
                    home: true,
                    provider: client::api::Repository {
                        forge: forge_world::REPO.forge,
                        repository: forge_world::REPO.repository,
                    },
                    host: Box::from(&b"forge.example"[..]),
                    owner: Box::from(&b"org"[..]),
                    name: Box::from(&b"repo"[..]),
                    prefix: Box::from(&b"temper/"[..]),
                    role: forge_top::Role::Owned,
                    landing: Box::from(&b"main"[..]),
                    ci: true,
                    checks: Box::new([]),
                },
                0,
            )
            .expect("forge adoption shape"),
        });
        self.until(Until::Adopted);
        assert!(matches!(self.adopted.as_slice(), [people::Outcome::Adopted { .. }]));
    }
}

#[cfg(test)]
mod system_stories {
    use super::*;
    use crate::world as smith_world;
    use skein_fake_llm_domain::api::{Finish, Line, Script, Turn};
    use smith_agent_world::{Job, World as Agent};
    use smith_domain as smith;
    use smith_domain_run as run;
    use temper_engine_smith::ChangeResource;

    const DELEGATE: &[u8] = br#"{"batch":[{"executor":{"kind":"procedure","connector":0,"code":2},"spec":{"words":"@coding Small fix","parameters":[{"name":1,"kind":"resource","connector":0,"resource":2},{"name":2,"kind":"bytes","value":"main"}]},"contract":{"kind":"change","connector":0,"change_kind":1,"words":32},"authority":{"tools":1,"grants":[{"connector":0,"kind":2,"segments":["forge","forge.example","org"],"terminal":"open","last":"repo"},{"connector":0,"kind":3,"segments":["forge","forge.example","org"],"terminal":"open","last":"repo"},{"connector":0,"kind":4,"segments":["forge","forge.example","org"],"terminal":"open","last":"repo"}],"delegation":{"kinds":[{"kind":"agent","number":1}],"tasks":2,"depth":1},"budget":{"spend":20},"notes":0}}]}"#;

    fn chat_script() -> Script {
        let call = |name: &[u8], arguments: &[u8]| Turn {
            lines: Box::new([Line::Call { name: name.into(), arguments: arguments.into() }]),
            finish: Finish::ToolCalls,
            tokens: 20,
        };
        Script {
            cue: b"@forge_chat".as_slice().into(),
            turns: Box::new([
                call(b"delegate", DELEGATE),
                call(b"wait", b"{}"),
                call(b"wait", b"{}"),
                call(b"wait", b"{}"),
            ]),
        }
    }

    fn repair_script() -> Script {
        Script {
            cue: b"Repair the failed check".as_slice().into(),
            turns: Box::new([
                Turn {
                    lines: Box::new([Line::Call {
                        name: b"read".as_slice().into(),
                        arguments: br#"{"path":"src/lib.rs"}"#.as_slice().into(),
                    }]),
                    finish: Finish::ToolCalls,
                    tokens: 20,
                },
                Turn {
                    lines: Box::new([Line::Call {
                        name: b"edit".as_slice().into(),
                        arguments: br#"{"path":"src/lib.rs","old":"42","new":"43"}"#.as_slice().into(),
                    }]),
                    finish: Finish::ToolCalls,
                    tokens: 20,
                },
                Turn {
                    lines: Box::new([Line::Call {
                        name: b"finish".as_slice().into(),
                        arguments:
                            br#"{"title":"Repair the failed check","body":"The answer is 43 now."}"#.as_slice().into(),
                    }]),
                    finish: Finish::ToolCalls,
                    tokens: 20,
                },
            ]),
        }
    }

    fn start_change_world(passes: u32) -> (World, engine::Assignment) {
        let mut world = World::configured_policy(true, true, false, passes, None);
        world.adopt();
        world.send(engine::Event::Hello {
            channel: Token::new(7),
            hello: fleet::Hello {
                stop_bound: Duration::from_secs(1),
                slots: 2,
                workstreams: Box::new([]),
                hosting: Box::new([]),
            },
        });
        world.send(engine::Event::Ask {
            reply_to: ReplyTo::new(Token::new(92)),
            sign_in: world.signed_in.expect("owner session"),
            key: [92; 16],
            ask: people::Ask::StartChat { project: 1, words: b"@forge_chat".as_slice().into() },
        });
        world.until(Until::Assigned);
        let chat = world.assigned[0].clone();
        (world, chat)
    }

    fn run_smith(
        world: &mut World,
        assignment: &engine::Assignment,
        mut agent: Agent,
        change: Option<ChangeResource>,
    ) -> Agent {
        agent.enable_parent_host_calls();
        loop {
            if agent.drive(1000) {
                break;
            }
            let pending: Vec<_> = agent.pending_host_calls().into_iter().cloned().collect();
            assert!(!pending.is_empty(), "Smith either settles or yields a host call");
            for submission in pending {
                let body = temper_engine_smith::call(submission.name, &submission.tool, &submission.input)
                    .unwrap_or_else(|problem| {
                        panic!(
                            "declared Smith tool input: {problem:?} tool={} input={}",
                            String::from_utf8_lossy(&submission.tool),
                            String::from_utf8_lossy(submission.input.bytes())
                        )
                    });
                let before = world.answers.len();
                world.send(engine::Event::Call {
                    channel: Token::new(7),
                    task: assignment.task,
                    attempt: assignment.attempt,
                    call: submission.relay.owner,
                    body,
                });
                for _ in 0..200 {
                    world.tick();
                    if world.answers.len() > before {
                        break;
                    }
                }
                let answer = world.answers.get(before).expect("root committed host answer");
                agent
                    .return_host_reply(submission.relay, run::HostReply::Answered(temper_engine_smith::answer(answer)))
                    .expect("one pending Smith relay");
            }
        }
        for (number, read, spent) in agent.turn_metadata() {
            let record = agent.turns()[usize::try_from(*number - 1).expect("positive turn")].clone();
            let request = smith::Request::Turn {
                host_run: Token::new(assignment.task),
                number: *number,
                position: record.sequence,
                read: *read,
                spent: *spent,
                turn: record.clone(),
            };
            let turn = temper_engine_smith::turn(request, format!("{record:?}").into_bytes().into_boxed_slice())
                .expect("typed Smith turn");
            world.send(engine::Event::Turn {
                channel: Token::new(7),
                task: assignment.task,
                attempt: assignment.attempt,
                turn,
            });
            for _ in 0..8 {
                world.tick();
            }
        }
        let terminal =
            temper_engine_smith::result(smith_world::copy_answer(agent.answer()), change).expect("typed Smith result");
        world.send(engine::Event::Answer {
            pushed: Box::new([]),
            saved: None,
            channel: Token::new(7),
            task: assignment.task,
            attempt: assignment.attempt,
            cumulative: terminal.cumulative,
            end: terminal.end,
        });
        for _ in 0..30 {
            world.tick();
        }
        agent
    }

    #[test]
    fn the_smith_delegate_input_creates_the_producer() {
        let (mut world, chat) = start_change_world(1000);
        let input = run::HostInput::attested(DELEGATE.into()).expect("bounded Smith host input");
        let body = temper_engine_smith::call(
            run::CallName { activation: chat.attempt, completion: 1, position: 1 },
            b"delegate",
            &input,
        )
        .expect("declared delegate shape");
        world.send(engine::Event::Call {
            channel: Token::new(7),
            task: chat.task,
            attempt: chat.attempt,
            call: Token::new(93),
            body,
        });
        world.until(Until::Delegated);
        world.until(Until::SecondAssignment);
        let producer = world.assigned[1].clone();
        let mut agent = smith_world::agent_for(&producer, None, Job::Coding);
        agent.run(1000);
        assert!(
            matches!(agent.answer(), run::Answer::Accepted { outcome: run::outcome::Declared::Change(_), .. }),
            "{:?}",
            agent.answer()
        );
    }

    #[test]
    fn a_small_fix_made_in_a_smith_chat_lands_through_the_fake_forge() {
        let (mut world, chat) = start_change_world(1000);
        let agent = smith_world::scripted_agent_for(&chat, chat_script());
        let parent = run_smith(&mut world, &chat, agent, None);
        assert!(matches!(parent.answer(), run::Answer::Parked { .. }));
        world.until(Until::SecondAssignment);
        let producer = world.assigned[1].clone();
        let branch = producer.workspace.repositories[0].push.clone().expect("writable change branch");
        assert!(matches!(
            world.external(2, raw::Op::Git(raw::Git::Create { branch: branch.clone(), commit: 1 })),
            raw::Answer::Branch(raw::Created::Created)
        ));
        let agent = smith_world::agent_for(&producer, None, Job::Coding);
        let pushed = fake::advance(&mut world.fake, &world.fake_env, b"org/repo", &branch, b"file", b"fixed", 1)
            .expect("worker pushed its branch");
        assert!(pushed > 1);
        let producer_agent =
            run_smith(&mut world, &producer, agent, Some(ChangeResource { connector: 0, kind: 2, resource: 2 }));
        assert!(matches!(
            producer_agent.answer(),
            run::Answer::Accepted { outcome: run::outcome::Declared::Change(_), .. }
        ));
        for _ in 0..200 {
            world.tick();
        }
        assert!(world.store.rows.values().any(|stored| matches!(stored,
            Record::Forge { row, .. } if matches!(row.as_ref(), forge_top::Stored::Change(change)
                if matches!(change.change.state, temper_engine_domain_forge_change::State::Landed { .. }))
        )));
    }

    #[test]
    #[expect(clippy::too_many_lines, reason = "the scenario follows the Smith repair and forge landing sequence")]
    fn a_smith_change_with_failed_ci_is_repaired_reviewed_and_landed() {
        let (mut world, chat) = start_change_world(0);
        let agent = smith_world::scripted_agent_for(&chat, chat_script());
        let parent = run_smith(&mut world, &chat, agent, None);
        assert!(matches!(parent.answer(), run::Answer::Parked { .. }));
        world.until(Until::SecondAssignment);
        let producer = world.assigned[1].clone();
        let branch = producer.workspace.repositories[0].push.clone().expect("writable change branch");
        world.external(
            1,
            raw::Op::Write(raw::Write::Status {
                commit: 1,
                context: b"build".as_slice().into(),
                state: raw::Check::Passed,
            }),
        );
        assert!(matches!(
            world.external(2, raw::Op::Git(raw::Git::Create { branch: branch.clone(), commit: 1 })),
            raw::Answer::Branch(raw::Created::Created)
        ));
        let failed_head = fake::advance(&mut world.fake, &world.fake_env, b"org/repo", &branch, b"file", b"first", 1)
            .expect("producer pushed first head");
        world.external(
            1,
            raw::Op::Write(raw::Write::Status {
                commit: failed_head,
                context: b"build".as_slice().into(),
                state: raw::Check::Failed,
            }),
        );
        let producer_agent = run_smith(
            &mut world,
            &producer,
            smith_world::agent_for(&producer, None, Job::Coding),
            Some(ChangeResource { connector: 0, kind: 2, resource: 2 }),
        );
        assert!(matches!(
            producer_agent.answer(),
            run::Answer::Accepted { outcome: run::outcome::Declared::Change(_), .. }
        ));
        for _ in 0..200 {
            world.tick();
            if world.assigned.len() >= 3 {
                break;
            }
        }
        let repair = world.assigned.get(2).expect("failed CI assigned a repair").clone();
        assert!(
            repair.sections.iter().any(|section| section.kind == engine::BriefKind::Forge(engine::ForgeBriefKind::Ci)
                && matches!(&section.body, engine::BriefBody::Text(words)
                if words.windows(b"failing check build 3".len()).any(|part| part == b"failing check build 3"))),
            "CI sections: {:?}",
            repair.sections
        );
        let repaired_head =
            fake::advance(&mut world.fake, &world.fake_env, b"org/repo", &branch, b"file", b"repaired", 1)
                .expect("repair pushed a new head");
        for _ in 0..5 {
            world.tick();
        }
        world.external(
            1,
            raw::Op::Write(raw::Write::Status {
                commit: repaired_head,
                context: b"build".as_slice().into(),
                state: raw::Check::Passed,
            }),
        );
        let pull = world
            .store
            .rows
            .values()
            .find_map(|stored| match stored {
                Record::Forge { row, .. } => match row.as_ref() {
                    forge_top::Stored::Change(change) => change.pull,
                    _ => None,
                },
                _ => None,
            })
            .expect("opened pull request");
        assert!(matches!(
            world.external(
                2,
                raw::Op::Write(raw::Write::Review {
                    number: pull,
                    verdict: Some(raw::Verdict::Approve),
                    body: b"Reviewed repaired head".as_slice().into(),
                })
            ),
            raw::Answer::Reviewed(_)
        ));
        let repair_agent = run_smith(
            &mut world,
            &repair,
            smith_world::scripted_coding_agent_for(&repair, repair_script()),
            Some(ChangeResource { connector: 0, kind: 2, resource: 2 }),
        );
        assert!(
            matches!(repair_agent.answer(), run::Answer::Accepted { outcome: run::outcome::Declared::Change(_), .. }),
            "{:?}",
            repair_agent.answer()
        );
        for _ in 0..200 {
            world.tick();
            if world.store.rows.values().any(|stored| {
                matches!(stored,
                    Record::Forge { row, .. } if matches!(row.as_ref(), forge_top::Stored::Change(change)
                        if matches!(change.change.state, temper_engine_domain_forge_change::State::Landed { .. }))
                )
            }) {
                break;
            }
        }
        assert!(world.store.rows.values().any(|stored| matches!(stored,
            Record::Forge { row, .. } if matches!(row.as_ref(), forge_top::Stored::Change(change)
                if matches!(change.change.state, temper_engine_domain_forge_change::State::Landed { .. })
                    && change.change.repairs == 1)
        )));
    }
}
