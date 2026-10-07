//! Root forge stories with its durable store and the independent fake Forgejo.
#![expect(clippy::wildcard_enum_match_arm, reason = "the fixture selects only the forge rows relevant to each story")]
use jig_core_authority as authority;
use jig_core_fleet as fleet;
use skein_lib::{Duration, Env, Queue, ReplyTo, Time, Token, Wall};
use std::collections::{BTreeMap, VecDeque};
use temper_engine_domain::{Delivery, Key, Record, engine};
use temper_engine_domain_forge as forge_top;
use temper_engine_domain_forge_client as client;
use temper_engine_domain_people as people;
use temper_engine_domain_tasks as tasks;
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
    forge_calls: u32,
    adopted: Vec<people::Outcome>,
    signed_in: Option<u64>,
    assigned: Vec<engine::Assignment>,
    answers: Vec<temper_engine_domain::CallAnswer>,
    fail_job_reads: bool,
    slow_brief_reads: bool,
}

#[derive(Clone, Copy, Debug)]
enum Until {
    Ready,
    SignedIn,
    Adopted,
    Assigned,
    Subscribed,
    News,
    Read,
    Effect,
    Delegated,
    SecondAssignment,
}

impl World {
    fn new() -> Self {
        Self::configured(false, false)
    }

    fn configured(with_read: bool, with_change: bool) -> Self {
        Self::configured_ci(with_read, with_change, false)
    }

    fn configured_ci(with_read: bool, with_change: bool, silent_ci: bool) -> Self {
        Self::configured_checks(with_read, with_change, silent_ci, 1000)
    }

    fn configured_checks(with_read: bool, with_change: bool, silent_ci: bool, passes: u32) -> Self {
        Self::configured_policy(with_read, with_change, silent_ci, passes, None, false)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one fixture configures the policy, fake, and bounds for root forge stories"
    )]
    #[expect(clippy::fn_params_excessive_bools, reason = "the fixture varies independent forge and gate conditions")]
    fn configured_policy(
        with_read: bool,
        with_change: bool,
        silent_ci: bool,
        passes: u32,
        approval: Option<people::Freshness>,
        agent_gate: bool,
    ) -> Self {
        let mut limits = walking::limits();
        limits.people.people = 4;
        limits.people.holdings = 4;
        limits.journal.writes += (people::max_out(&limits.people) - people::max_out(&walking::limits().people)) * 4;
        let mut config = walking::config(71);
        if approval.is_some() || agent_gate {
            limits.authority.roles = 3;
        }
        if with_change {
            limits.authority.requirements = 8;
            limits.authority.facts = 8;
        }
        if with_change {
            limits.brief.gather = Duration::from_secs(5);
            limits.brief.read_bytes = 512;
            limits.brief.budgets.pull = 384;
            limits.brief.budgets.ci = 384;
            limits.brief.brief_bytes = 768;
            limits.tasks.tasks = 12;
            limits.tasks.project_tasks = 12;
            limits.tasks.delegates = 4;
            limits.tasks.tree_tasks = 12;
            limits.tasks.depth = 3;
            limits.tasks.parameters = 8;
            limits.tasks.spec_bytes = 512;
            limits.tasks.executor_kinds = 2;
            limits.tasks.authority_grants = 5;
            limits.tasks.authority_segments = 9;
            limits.tasks.authority_bytes = 512;
            limits.tasks.contract_choices = 2;
            limits.tasks.inbox_messages = 8;
            limits.tasks.inbox_bytes = 1024;
            limits.authority.executors = 2;
            limits.authority.batch = 2;
            limits.authority.grants = 5;
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
            if !with_change {
                limits.authority.grants = 2;
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
                // The chat's 100-unit allotment must retain room for its 20-unit delegate
                // after the run reserves its capped allowance at claim.
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
            rules.ceiling.grants.clone_from(&grants);
            if approval.is_some() || agent_gate {
                let landing: Box<[people::LandingRule]> = Box::new([people::LandingRule {
                    connector: 0,
                    kind: 4,
                    pattern: people::Pattern {
                        segments: Box::new([
                            Box::from(&b"forge"[..]),
                            Box::from(&b"forge.example"[..]),
                            Box::from(&b"org"[..]),
                            Box::from(&b"repo"[..]),
                            Box::from(&b"branch"[..]),
                        ]),
                        last: people::Last::Exact(Box::from(&b"main"[..])),
                    },
                    ci: true,
                    up_to_date: true,
                    gates: if agent_gate {
                        Box::new([people::Gate { number: 1, blocking: true, freshness: people::Freshness::Exact }])
                    } else {
                        Box::new([])
                    },
                    approvals: match approval {
                        Some(freshness) => Box::new([people::Approval { role: 2, people: 1, freshness }]),
                        None => Box::new([]),
                    },
                }]);
                config.landing.deployment.clone_from(&landing);
                assert!(config.landing.projects.insert(1, landing).is_ok());
            }
            if with_change {
                rules.ceiling.delegation.kinds =
                    Box::new([authority::Executor::Charter(1), authority::Executor::Procedure(2)]);
                rules.ceiling.delegation.tasks = 12;
                rules.ceiling.delegation.depth = 3;
            }
            let mut policy = config.authority.policy(1).expect("walk policy").clone();
            policy.ceiling.tools = authority::Tools(1);
            policy.ceiling.grants.clone_from(&grants);
            if approval.is_some() || agent_gate {
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
                config.chat_authority.delegation.tasks = 8;
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
            forge_calls: 0,
            adopted: Vec::new(),
            signed_in: None,
            assigned: Vec::new(),
            answers: Vec::new(),
            fail_job_reads: false,
            slow_brief_reads: false,
        }
    }

    fn send(&mut self, event: engine::Event) {
        engine::step(&mut self.root, &self.env, event);
        engine::release(&mut self.root, &self.env, &mut self.out);
        self.collect();
        self.root.reclaim();
    }

    /// Lose the process while retaining only the store, external forge and
    /// worker's hosted attempt. Outstanding API terminals belong to the old
    /// process; startup has to rediscover the outbox by its durable key.
    fn restart(&mut self, with_read: bool, with_change: bool) {
        assert!(self.store.pending.is_empty(), "the chosen cut resolves store uncertainty first");
        assert!(self.pending.is_empty(), "old API terminals are drained at this cut");
        let mut fresh = Self::configured(with_read, with_change);
        std::mem::swap(&mut self.root, &mut fresh.root);
        std::mem::swap(&mut self.out, &mut fresh.out);
        self.events.clear();
        self.answers.clear();
        self.adopted.clear();
        self.events.push_back(engine::Event::Start);
        if !self.assigned.is_empty() {
            self.events.push_back(engine::Event::Hello {
                channel: Token::new(7),
                hello: fleet::Hello {
                    stop_bound: Duration::from_secs(1),
                    slots: if with_change { 2 } else { 1 },
                    workstreams: Box::new([]),
                    hosting: self
                        .assigned
                        .iter()
                        .map(|assignment| fleet::Hosted {
                            run: Token::new(assignment.task),
                            attempt: Token::new(assignment.attempt),
                            phase: fleet::Phase::Active,
                        })
                        .collect(),
                },
            });
        }
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
                    let (rows, next) = self.store.page(range, after, most);
                    self.events.push_back(engine::Event::Loaded { owner, rows, next });
                }
                engine::Request::Forge { call, repository, op } => {
                    self.forge_calls += 1;
                    if self.fail_job_reads && matches!(op, client::api::Op::Read(client::api::Read::Job { .. })) {
                        self.events.push_back(engine::Event::ForgeAnswered {
                            call,
                            cost: 1,
                            result: Err(client::api::Error::MissingJob),
                        });
                        continue;
                    }
                    let name: &[u8] = if repository == forge_world::REPO {
                        b"org/repo"
                    } else if repository == (client::api::Repository { forge: 1, repository: 3 }) {
                        b"org/extra"
                    } else {
                        panic!("unknown test repository {repository:?}");
                    };
                    let slow =
                        self.slow_brief_reads && matches!(op, client::api::Op::Read(client::api::Read::Job { .. }));
                    let raw = translate::op(&op, &self.env.limits.forge.client);
                    self.pending.insert(call, op);
                    let mut limits = self.fake_env.limits;
                    if slow {
                        limits.latency_min = Duration::from_secs(20);
                        limits.latency_max = Duration::from_secs(20);
                    }
                    let fake_env = Env { now: self.fake_env.now, wall: self.fake_env.wall, limits };
                    fake::step(
                        &mut self.fake,
                        &fake_env,
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
                        if matches!(outcome, people::Outcome::RepositoryAdopted { .. }) =>
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
                Until::Subscribed => self
                    .answers
                    .iter()
                    .any(|answer| matches!(answer, temper_engine_domain::CallAnswer::Subscribed { .. })),
                Until::News => {
                    let task = self.assigned.first().expect("assigned subscriber").task;
                    match self.store.rows.get(&Key::Tasks(tasks::Key::Live(task))) {
                        Some(Record::Tasks(tasks::Stored::Live(row))) => {
                            row.inbox.iter().any(|word| matches!(word.kind, tasks::MessageKind::News { .. }))
                        }
                        _ => false,
                    }
                }
                Until::Read => {
                    self.answers.iter().any(|answer| matches!(answer, temper_engine_domain::CallAnswer::ForgeRead(_)))
                }
                Until::Effect => self
                    .answers
                    .iter()
                    .any(|answer| matches!(answer, temper_engine_domain::CallAnswer::ForgeEffect { .. })),
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
        self.adopt_with_checks(true, Box::new([]));
    }

    fn adopt_with_checks(&mut self, ci: bool, checks: Box<[u32]>) {
        self.until(Until::Ready);
        self.send(engine::Event::SignedIn {
            reply_to: ReplyTo::new(Token::new(90)),
            identity: people::Identity {
                key: people::IdentityKey { forge: 1, user: 7 },
                login: Box::from(&b"owner"[..]),
                name: Box::from(&b"Owner"[..]),
            },
        });
        self.until(Until::SignedIn);
        self.send(engine::Event::Ask {
            reply_to: ReplyTo::new(Token::new(91)),
            sign_in: self.signed_in.expect("signed in"),
            key: [91; 16],
            ask: people::Ask::AdoptRepository {
                project: 1,
                adoption: people::Adoption {
                    home: true,
                    forge: forge_world::REPO.forge,
                    repository: forge_world::REPO.repository,
                    host: Box::from(&b"forge.example"[..]),
                    owner: Box::from(&b"org"[..]),
                    name: Box::from(&b"repo"[..]),
                    prefix: Box::from(&b"temper/"[..]),
                    role: people::RepositoryRole::Owned,
                    landing: Box::from(&b"main"[..]),
                    ci,
                    checks,
                },
            },
        });
        self.until(Until::Adopted);
        assert!(matches!(self.adopted.as_slice(), [people::Outcome::RepositoryAdopted { .. }]));
    }
}

fn change_world(
    silent_ci: bool,
    passes: u32,
    approval: Option<people::Freshness>,
) -> (World, engine::Assignment, engine::Assignment, Box<[u8]>) {
    change_world_with_gate(silent_ci, passes, approval, false)
}

fn change_world_with_gate(
    silent_ci: bool,
    passes: u32,
    approval: Option<people::Freshness>,
    agent_gate: bool,
) -> (World, engine::Assignment, engine::Assignment, Box<[u8]>) {
    change_world_with_policy(silent_ci, passes, approval, agent_gate, false, false)
}

#[expect(clippy::too_many_lines, reason = "the root change fixture builds both producer and review routes")]
#[expect(clippy::fn_params_excessive_bools, reason = "independent CI, gate and policy settings select test worlds")]
fn change_world_with_policy(
    silent_ci: bool,
    passes: u32,
    approval: Option<people::Freshness>,
    agent_gate: bool,
    no_ci: bool,
    owner_gate: bool,
) -> (World, engine::Assignment, engine::Assignment, Box<[u8]>) {
    let mut world = World::configured_policy(true, true, silent_ci, passes, approval, agent_gate);
    if no_ci {
        world.adopt_with_checks(false, Box::new([1]));
    } else {
        world.adopt();
    }
    if owner_gate {
        world.send(engine::Event::Ask {
            reply_to: ReplyTo::new(Token::new(900)),
            sign_in: world.signed_in.expect("owner session"),
            key: [90; 16],
            ask: people::Ask::ChangePolicy {
                project: 1,
                change: people::PolicyChange::Landing {
                    rules: Box::new([people::LandingRule {
                        connector: 0,
                        kind: 4,
                        pattern: people::Pattern {
                            segments: Box::new([
                                Box::from(&b"forge"[..]),
                                Box::from(&b"forge.example"[..]),
                                Box::from(&b"org"[..]),
                                Box::from(&b"repo"[..]),
                                Box::from(&b"branch"[..]),
                            ]),
                            last: people::Last::Exact(Box::from(&b"main"[..])),
                        },
                        ci: true,
                        up_to_date: true,
                        gates: Box::new([people::Gate {
                            number: 1,
                            blocking: true,
                            freshness: people::Freshness::Exact,
                        }]),
                        approvals: Box::new([]),
                    }]),
                },
            },
        });
        world.until(Until::Ready);
        assert!(world.store.rows.contains_key(&Key::People(people::Key::Policy(1))), "owner policy committed");
        world.restart(true, true);
        world.until(Until::Ready);
    }
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
        ask: people::Ask::StartChat { project: 1, words: Box::from(&b"make the fix"[..]) },
    });
    world.until(Until::Assigned);
    let chat = world.assigned[0].clone();
    let grant = tasks::Grant {
        connector: 0,
        kind: 2,
        pattern: tasks::Pattern {
            segments: Box::new([Box::from(&b"forge"[..]), Box::from(&b"forge.example"[..]), Box::from(&b"org"[..])]),
            last: tasks::Last::Open(Box::from(&b"repo"[..])),
        },
    };
    let child_authority = tasks::Authority {
        tools: tasks::Tools(1),
        grants: Box::new([tasks::Grant { kind: 3, ..grant.clone() }, tasks::Grant { kind: 4, ..grant }]),
        delegation: tasks::Delegation { kinds: Box::new([tasks::AuthorityExecutor::Charter(1)]), tasks: 4, depth: 1 },
        budget: tasks::Budget { spend: 20, deadline: None },
        notes: tasks::Scopes(0),
        note_resources: Box::new([]),
    };
    world.send(engine::Event::Call {
        channel: Token::new(7),
        task: chat.task,
        attempt: chat.attempt,
        call: Token::new(93),
        body: engine::Call {
            completion: 1,
            position: 1,
            tool: engine::Tool::Delegate {
                batch: Box::new([engine::Delegate {
                    executor: tasks::Executor::Procedure { connector: 0, code: 2 },
                    spec: tasks::Spec {
                        words: Box::from(&b"Small fix"[..]),
                        parameters: Box::new([
                            tasks::Parameter::Resource {
                                name: 1,
                                connector: 0,
                                resource: u64::from(forge_world::REPO.repository),
                            },
                            tasks::Parameter::Bytes { name: 2, value: Box::from(&b"main"[..]) },
                        ]),
                        inputs: Box::new([]),
                    },
                    contract: tasks::Contract::Change { connector: 0, kind: 1, words: 32 },
                    authority: child_authority,
                    symbolic_grants: Box::new([tasks::Grant {
                        connector: 0,
                        kind: 2,
                        pattern: tasks::Pattern {
                            segments: Box::new([
                                Box::from(&b"forge"[..]),
                                Box::from(&b"forge.example"[..]),
                                Box::from(&b"org"[..]),
                                Box::from(&b"repo"[..]),
                                Box::from(&b"branch"[..]),
                                Box::from(&b"temper"[..]),
                                chat.task.to_string().into_bytes().into_boxed_slice(),
                            ]),
                            last: tasks::Last::Open(Box::from(&b"c"[..])),
                        },
                    }]),
                    dependencies: Box::new([]),
                    wake: tasks::WakePolicy::DEFAULT,
                }]),
            },
        },
    });
    world.until(Until::Delegated);
    world.until(Until::SecondAssignment);
    let producer = world.assigned[1].clone();
    let change = world
        .answers
        .iter()
        .find_map(|answer| match answer {
            temper_engine_domain::CallAnswer::Delegated(numbers) => numbers.first().copied(),
            _ => None,
        })
        .expect("change task number");
    assert!(
        world.store.rows.values().any(|stored| matches!(stored,
            Record::Tasks(tasks::Stored::Live(task))
                if task.number == producer.task && task.authority.grants.iter().any(|grant|
                    grant.connector == 0 && grant.kind == 2
                        && grant.pattern.segments.last().map(AsRef::as_ref)
                            == Some(chat.task.to_string().as_bytes())
                        && matches!(&grant.pattern.last, tasks::Last::Exact(prefix)
                            if prefix.as_ref() == format!("c{change}").as_bytes()))
        )),
        "producer inherits the change task's numbered branch grant"
    );
    let branch = producer.workspace.repositories[0].push.clone().expect("writable change branch");
    (world, chat, producer, branch)
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "the full chat-to-landing story carries its worker, forge and result assertions"
)]
fn a_small_fix_made_in_a_chat_lands() {
    let mut world = World::configured(true, true);
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
        ask: people::Ask::StartChat { project: 1, words: Box::from(&b"make the fix"[..]) },
    });
    world.until(Until::Assigned);
    let chat = world.assigned[0].clone();
    let grant = tasks::Grant {
        connector: 0,
        kind: 2,
        pattern: tasks::Pattern {
            segments: Box::new([Box::from(&b"forge"[..]), Box::from(&b"forge.example"[..]), Box::from(&b"org"[..])]),
            last: tasks::Last::Open(Box::from(&b"repo"[..])),
        },
    };
    let child_authority = tasks::Authority {
        tools: tasks::Tools(1),
        grants: Box::new([grant.clone(), tasks::Grant { kind: 3, ..grant.clone() }, tasks::Grant { kind: 4, ..grant }]),
        delegation: tasks::Delegation { kinds: Box::new([tasks::AuthorityExecutor::Charter(1)]), tasks: 2, depth: 1 },
        budget: tasks::Budget { spend: 20, deadline: None },
        notes: tasks::Scopes(0),
        note_resources: Box::new([]),
    };
    world.send(engine::Event::Call {
        channel: Token::new(7),
        task: chat.task,
        attempt: chat.attempt,
        call: Token::new(93),
        body: engine::Call {
            completion: 1,
            position: 1,
            tool: engine::Tool::Delegate {
                batch: Box::new([engine::Delegate {
                    executor: tasks::Executor::Procedure { connector: 0, code: 2 },
                    spec: tasks::Spec {
                        words: Box::from(&b"Small fix"[..]),
                        parameters: Box::new([
                            tasks::Parameter::Resource {
                                name: 1,
                                connector: 0,
                                resource: u64::from(forge_world::REPO.repository),
                            },
                            tasks::Parameter::Bytes { name: 2, value: Box::from(&b"main"[..]) },
                        ]),
                        inputs: Box::new([]),
                    },
                    contract: tasks::Contract::Change { connector: 0, kind: 1, words: 32 },
                    authority: child_authority,
                    symbolic_grants: Box::new([]),
                    dependencies: Box::new([]),
                    wake: tasks::WakePolicy::DEFAULT,
                }]),
            },
        },
    });
    world.until(Until::Delegated);
    world.until(Until::SecondAssignment);
    assert_ne!(world.assigned[1].task, chat.task, "change's producer is a separate task");
    let producer = world.assigned[1].clone();
    let repository = producer.workspace.repositories.first().expect("adopted forge workspace");
    assert!(matches!(repository.start, engine::ForgeStart::Base(ref branch) if branch.as_ref() == b"main"));
    let branch = repository.push.as_ref().expect("change's branch is writable");
    assert!(branch.starts_with(b"temper/"));
    assert!(world.store.rows.values().any(|stored| matches!(stored,
        Record::Forge { row, .. } if matches!(row.as_ref(), forge_top::Stored::Hold(hold)
            if matches!(hold.writer, Some(forge_top::Writer::Run { task, attempt })
                if task == producer.task && attempt == producer.attempt))
    )));
    let branch = branch.clone();
    let created = world.external(2, raw::Op::Git(raw::Git::Create { branch: branch.clone(), commit: 1 }));
    assert!(matches!(created, raw::Answer::Branch(raw::Created::Created)), "created branch: {created:?}");
    fake::advance(&mut world.fake, &world.fake_env, b"org/repo", &branch, b"file", b"fixed", 1)
        .expect("producer pushed its branch");
    world.send(engine::Event::Answer {
        channel: Token::new(7),
        task: producer.task,
        attempt: producer.attempt,
        cumulative: 5,
        end: tasks::End::Finished {
            result: tasks::TaskResult::Change {
                connector: 0,
                kind: 2,
                resource: u64::from(forge_world::REPO.repository),
                words: Box::from(&b"pushed"[..]),
            },
            cancel_delegates: false,
        },
        saved: None,
    });
    for _ in 0..200 {
        world.tick();
        if world.store.rows.values().any(|stored| matches!(stored,
            Record::Forge { row, .. } if matches!(row.as_ref(), forge_top::Stored::Change(change)
                if change.task != producer.task && matches!(change.change.state, temper_engine_domain_forge_change::State::Landed { .. }))
        )) { break; }
    }
    assert!(
        world.store.rows.values().any(|stored| matches!(stored,
            Record::Forge { row, .. } if matches!(row.as_ref(), forge_top::Stored::Change(change)
                if matches!(change.change.state, temper_engine_domain_forge_change::State::Landed { .. }))
        )),
        "change did not land: {:?}",
        world.store.rows.values().filter(|row| matches!(row, Record::Forge { .. })).collect::<Vec<_>>()
    );
    for _ in 0..20 {
        world.tick();
    }
    assert!(
        world.store.rows.values().any(|stored| matches!(stored,
            Record::Tasks(tasks::Stored::Ended(row)) if row.requester == tasks::Party::Task(chat.task)
                && matches!(row.phase, tasks::Phase::Ended(tasks::Ending::Done(tasks::TaskResult::Change { .. })))
        )),
        "change task did not report its landing"
    );
}

#[test]
fn a_pushed_change_is_recovered_before_its_worker_reports_the_head() {
    let (mut world, chat, producer, branch) = change_world(false, 1000, None);
    assert!(matches!(
        world.external(2, raw::Op::Git(raw::Git::Create { branch: branch.clone(), commit: 1 })),
        raw::Answer::Branch(raw::Created::Created)
    ));
    let pushed = fake::advance(&mut world.fake, &world.fake_env, b"org/repo", &branch, b"file", b"fixed", 1)
        .expect("worker push reached the forge");
    assert!(world.store.pending.is_empty(), "push precedes the worker's answer decision");
    world.restart(true, true);
    for _ in 0..30 {
        world.tick();
    }
    world.send(engine::Event::Answer {
        channel: Token::new(7),
        task: producer.task,
        attempt: producer.attempt,
        cumulative: 5,
        end: tasks::End::Finished {
            result: tasks::TaskResult::Change {
                connector: 0,
                kind: 2,
                resource: u64::from(forge_world::REPO.repository),
                words: Box::from(&b"pushed"[..]),
            },
            cancel_delegates: false,
        },
        saved: None,
    });
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
    assert!(
        world.store.rows.values().any(|stored| matches!(stored,
            Record::Forge { row, .. } if matches!(row.as_ref(), forge_top::Stored::Change(change)
                if matches!(change.change.state, temper_engine_domain_forge_change::State::Landed { .. }))
        )),
        "recovered push did not land"
    );
    let raw::Answer::Commit(found) = world.external(1, raw::Op::Read(raw::Read::Branch { branch: branch.clone() }))
    else {
        panic!("pushed branch remains readable")
    };
    assert_eq!(found, pushed, "the recovered procedure used the pushed head");
    assert!(
        world.store.rows.values().any(|stored| matches!(stored,
            Record::Tasks(tasks::Stored::Live(task)) if task.number == chat.task
        )),
        "the chat still owns the change result"
    );
}

#[test]
fn a_change_whose_ci_never_reports_is_stalled_and_held() {
    let (mut world, chat, producer, branch) = change_world(true, 1000, None);
    assert!(matches!(
        world.external(2, raw::Op::Git(raw::Git::Create { branch: branch.clone(), commit: 1 })),
        raw::Answer::Branch(raw::Created::Created)
    ));
    fake::advance(&mut world.fake, &world.fake_env, b"org/repo", &branch, b"file", b"fixed", 1)
        .expect("producer pushed its branch");
    world.send(engine::Event::Answer {
        channel: Token::new(7),
        task: producer.task,
        attempt: producer.attempt,
        cumulative: 5,
        end: tasks::End::Finished {
            result: tasks::TaskResult::Change {
                connector: 0,
                kind: 2,
                resource: u64::from(forge_world::REPO.repository),
                words: Box::from(&b"pushed"[..]),
            },
            cancel_delegates: false,
        },
        saved: None,
    });
    for _ in 0..450 {
        world.tick();
    }
    assert!(
        world.store.rows.values().any(|stored| matches!(stored,
            Record::Forge { row, .. } if matches!(row.as_ref(), forge_top::Stored::Change(change)
                if matches!(change.change.state, temper_engine_domain_forge_change::State::Held {
                    why: temper_engine_domain_forge_change::Hold::Stalled, .. }))
        )),
        "CI silence did not stall the change: {:?}",
        world.store.rows.values().filter(|row| matches!(row, Record::Forge { .. })).collect::<Vec<_>>()
    );
    assert!(
        !world.store.rows.values().any(|stored| matches!(stored,
            Record::Tasks(tasks::Stored::Ended(row)) if row.requester == tasks::Party::Task(chat.task)
        )),
        "stalled change must not report a landing"
    );
}

#[test]
fn an_agent_review_gate_runs_at_the_head_and_its_approval_lands_the_change() {
    check_gate_landing(false, false);
}

#[test]
fn a_change_to_a_repository_without_ci_lands_on_its_checks() {
    check_gate_landing(true, false);
}

#[test]
fn an_owner_adds_a_review_to_a_branchs_landing_rule_and_later_changes_wait_for_it() {
    check_gate_landing(false, true);
}

fn check_gate_landing(no_ci: bool, owner_gate: bool) {
    let (mut world, _chat, producer, branch) =
        change_world_with_policy(no_ci, 1000, None, !owner_gate, no_ci, owner_gate);
    assert!(matches!(
        world.external(2, raw::Op::Git(raw::Git::Create { branch: branch.clone(), commit: 1 })),
        raw::Answer::Branch(raw::Created::Created)
    ));
    fake::advance(&mut world.fake, &world.fake_env, b"org/repo", &branch, b"file", b"review this diff", 1)
        .expect("producer pushed review head");
    world.send(engine::Event::Answer {
        channel: Token::new(7),
        task: producer.task,
        attempt: producer.attempt,
        cumulative: 5,
        end: tasks::End::Finished {
            result: tasks::TaskResult::Change {
                connector: 0,
                kind: 2,
                resource: u64::from(forge_world::REPO.repository),
                words: Box::from(&b"pushed"[..]),
            },
            cancel_delegates: false,
        },
        saved: None,
    });
    for _ in 0..200 {
        world.tick();
        if world.assigned.len() >= 3 {
            break;
        }
    }
    let gate = world.assigned.get(2).expect("agent gate assigned").clone();
    if owner_gate {
        assert!(
            world.store.rows.values().all(|stored| !matches!(stored,
                Record::Forge { row, .. } if matches!(row.as_ref(), forge_top::Stored::Change(change)
                    if matches!(change.change.state, temper_engine_domain_forge_change::State::Landed { .. }))
            )),
            "the owner-added review holds landing until its verdict"
        );
    }
    assert!(
        gate.sections.iter().any(|section| section.kind == engine::BriefKind::Forge(engine::ForgeBriefKind::Pull)
            && matches!(&section.body, engine::BriefBody::Text(words)
            if words.windows(b"review this diff".len()).any(|part| part == b"review this diff"))),
        "sections={:?}",
        gate.sections
    );
    assert_eq!(gate.workspace.repositories[0].start, engine::ForgeStart::Branch(branch));
    world.send(engine::Event::Answer {
        channel: Token::new(7),
        task: gate.task,
        attempt: gate.attempt,
        cumulative: 5,
        end: tasks::End::Finished {
            result: tasks::TaskResult::Verdict { code: 1, words: Box::from(&b"approved"[..]) },
            cancel_delegates: false,
        },
        saved: None,
    });
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
                && change.verdicts.iter().any(|report| Some(report.head) == change.change.last_head
                    && report.status == temper_engine_domain_forge_change::Status::Passed))
    )));
}

#[test]
fn a_review_asking_for_changes_is_repaired_with_its_remarks_and_reviewed_again() {
    check_gate_repair(false);
}

#[test]
fn a_failing_check_is_repaired_and_checked_again() {
    check_gate_repair(true);
}

#[expect(clippy::too_many_lines, reason = "the check repair story spans both gate heads and their worker reports")]
fn check_gate_repair(no_ci: bool) {
    let (mut world, _chat, producer, branch) = change_world_with_policy(no_ci, 1000, None, true, no_ci, false);
    assert!(matches!(
        world.external(2, raw::Op::Git(raw::Git::Create { branch: branch.clone(), commit: 1 })),
        raw::Answer::Branch(raw::Created::Created)
    ));
    fake::advance(&mut world.fake, &world.fake_env, b"org/repo", &branch, b"file", b"first version", 1)
        .expect("first head pushed");
    world.send(engine::Event::Answer {
        channel: Token::new(7),
        task: producer.task,
        attempt: producer.attempt,
        cumulative: 5,
        end: tasks::End::Finished {
            result: tasks::TaskResult::Change {
                connector: 0,
                kind: 2,
                resource: u64::from(forge_world::REPO.repository),
                words: Box::from(&b"pushed"[..]),
            },
            cancel_delegates: false,
        },
        saved: None,
    });
    for _ in 0..200 {
        world.tick();
        if world.assigned.len() >= 3 {
            break;
        }
    }
    let first = world.assigned.get(2).expect("first review assigned").clone();
    world.send(engine::Event::Answer {
        channel: Token::new(7),
        task: first.task,
        attempt: first.attempt,
        cumulative: 5,
        end: tasks::End::Finished {
            result: tasks::TaskResult::Verdict { code: 2, words: Box::from(&b"Please fix the unsafe edge"[..]) },
            cancel_delegates: false,
        },
        saved: None,
    });
    for _ in 0..200 {
        world.tick();
        if world.assigned.len() >= 4 {
            break;
        }
    }
    let repair = world.assigned.get(3).expect("review repair assigned").clone();
    assert!(
        repair.sections.iter().any(|section| section.kind == engine::BriefKind::Forge(engine::ForgeBriefKind::Reviews)
            && matches!(&section.body, engine::BriefBody::Text(words)
            if words.windows(b"Please fix the unsafe edge".len()).any(|part| part == b"Please fix the unsafe edge"))),
        "review remarks in repair brief: {:?}",
        repair.sections
    );
    fake::advance(&mut world.fake, &world.fake_env, b"org/repo", &branch, b"file", b"safe revision", 1)
        .expect("repair pushed second head");
    world.send(engine::Event::Answer {
        channel: Token::new(7),
        task: repair.task,
        attempt: repair.attempt,
        cumulative: 5,
        end: tasks::End::Finished {
            result: tasks::TaskResult::Change {
                connector: 0,
                kind: 2,
                resource: u64::from(forge_world::REPO.repository),
                words: Box::from(&b"repaired"[..]),
            },
            cancel_delegates: false,
        },
        saved: None,
    });
    for _ in 0..200 {
        world.tick();
        if world.assigned.len() >= 5 {
            break;
        }
    }
    let second = world.assigned.get(4).expect("second review assigned").clone();
    assert_ne!(first.task, second.task);
    world.send(engine::Event::Answer {
        channel: Token::new(7),
        task: second.task,
        attempt: second.attempt,
        cumulative: 5,
        end: tasks::End::Finished {
            result: tasks::TaskResult::Verdict { code: 1, words: Box::from(&b"Approved revision"[..]) },
            cancel_delegates: false,
        },
        saved: None,
    });
    for _ in 0..200 {
        world.tick();
        if world.store.rows.values().any(|stored| {
            matches!(stored,
            Record::Forge { row, .. } if matches!(row.as_ref(), forge_top::Stored::Change(change)
                if matches!(change.change.state, temper_engine_domain_forge_change::State::Landed { .. })))
        }) {
            break;
        }
    }
    assert!(world.store.rows.values().any(|stored| matches!(stored,
        Record::Forge { row, .. } if matches!(row.as_ref(), forge_top::Stored::Change(change)
            if matches!(change.change.state, temper_engine_domain_forge_change::State::Landed { .. })
                && change.change.repairs == 1))));
}

#[test]
#[expect(clippy::too_many_lines, reason = "the failed-CI story includes repair, review, and landing")]
fn a_change_failing_ci_is_repaired_reviewed_at_its_head_and_lands() {
    let (mut world, _chat, producer, branch) = change_world(false, 0, None);
    world.external(
        1,
        raw::Op::Write(raw::Write::Status { commit: 1, context: Box::from(&b"build"[..]), state: raw::Check::Passed }),
    );
    assert!(matches!(
        world.external(2, raw::Op::Git(raw::Git::Create { branch: branch.clone(), commit: 1 })),
        raw::Answer::Branch(raw::Created::Created)
    ));
    let head = fake::advance(&mut world.fake, &world.fake_env, b"org/repo", &branch, b"file", b"first", 1)
        .expect("producer pushed its branch");
    world.external(
        1,
        raw::Op::Write(raw::Write::Status {
            commit: head,
            context: Box::from(&b"build"[..]),
            state: raw::Check::Failed,
        }),
    );
    world.send(engine::Event::Answer {
        channel: Token::new(7),
        task: producer.task,
        attempt: producer.attempt,
        cumulative: 5,
        end: tasks::End::Finished {
            result: tasks::TaskResult::Change {
                connector: 0,
                kind: 2,
                resource: u64::from(forge_world::REPO.repository),
                words: Box::from(&b"pushed"[..]),
            },
            cancel_delegates: false,
        },
        saved: None,
    });
    for _ in 0..200 {
        world.tick();
        if world.assigned.len() >= 3 {
            break;
        }
    }
    assert!(
        world.assigned.len() >= 3,
        "failed CI did not assign repair: task2={:?}; task3={:?}; task4={:?}; forge={:?}",
        world.store.rows.get(&Key::Tasks(tasks::Key::Live(2))),
        world.store.rows.get(&Key::Tasks(tasks::Key::Live(3))),
        world.store.rows.get(&Key::Tasks(tasks::Key::Live(4))),
        world.store.rows.values().filter(|row| matches!(row, Record::Forge { .. })).collect::<Vec<_>>()
    );
    let repair = world.assigned[2].clone();
    assert!(
        matches!(repair.workspace.repositories[0].start, engine::ForgeStart::Branch(ref name) if name.as_ref() == branch.as_ref())
    );
    assert!(
        repair.sections.iter().any(|section| section.kind == engine::BriefKind::Forge(engine::ForgeBriefKind::Ci)
            && matches!(&section.body, engine::BriefBody::Text(words)
            if words.windows(b"failing check build 3".len()).any(|part| part == b"failing check build 3")
                && words.windows(b"[job log truncated]".len()).any(|part| part == b"[job log truncated]"))),
        "CI sections: {:?}",
        repair.sections
    );
    let repaired = fake::advance(&mut world.fake, &world.fake_env, b"org/repo", &branch, b"file", b"repaired", 1)
        .expect("repair pushed a new head");
    for _ in 0..5 {
        world.tick();
    }
    world.external(
        1,
        raw::Op::Write(raw::Write::Status {
            commit: repaired,
            context: Box::from(&b"build"[..]),
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
                body: Box::from(&b"Reviewed repaired head"[..]),
            })
        ),
        raw::Answer::Reviewed(_)
    ));
    world.send(engine::Event::Answer {
        channel: Token::new(7),
        task: repair.task,
        attempt: repair.attempt,
        cumulative: 5,
        end: tasks::End::Finished {
            result: tasks::TaskResult::Change {
                connector: 0,
                kind: 2,
                resource: u64::from(forge_world::REPO.repository),
                words: Box::from(&b"repaired"[..]),
            },
            cancel_delegates: false,
        },
        saved: None,
    });
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
    assert!(
        world.store.rows.values().any(|stored| matches!(stored,
            Record::Forge { row, .. } if matches!(row.as_ref(), forge_top::Stored::Change(change)
                if matches!(change.change.state, temper_engine_domain_forge_change::State::Landed { .. })
                    && change.change.repairs == 1)
        )),
        "repaired change did not land: task2={:?}; pending={:?}; forge={:?}",
        world.store.rows.get(&Key::Tasks(tasks::Key::Live(2))),
        world.pending,
        world.store.rows.values().filter(|row| matches!(row, Record::Forge { .. })).collect::<Vec<_>>()
    );
}

#[test]
fn an_unreadable_failed_job_log_is_named_and_repair_still_runs() {
    let (mut world, _chat, producer, branch) = change_world(false, 0, None);
    world.fail_job_reads = true;
    assert!(matches!(
        world.external(2, raw::Op::Git(raw::Git::Create { branch: branch.clone(), commit: 1 })),
        raw::Answer::Branch(raw::Created::Created)
    ));
    let head = fake::advance(&mut world.fake, &world.fake_env, b"org/repo", &branch, b"file", b"first", 1)
        .expect("producer pushed its branch");
    world.external(
        1,
        raw::Op::Write(raw::Write::Status {
            commit: head,
            context: Box::from(&b"build"[..]),
            state: raw::Check::Failed,
        }),
    );
    world.send(engine::Event::Answer {
        channel: Token::new(7),
        task: producer.task,
        attempt: producer.attempt,
        cumulative: 5,
        end: tasks::End::Finished {
            result: tasks::TaskResult::Change {
                connector: 0,
                kind: 2,
                resource: u64::from(forge_world::REPO.repository),
                words: Box::from(&b"pushed"[..]),
            },
            cancel_delegates: false,
        },
        saved: None,
    });
    for _ in 0..200 {
        world.tick();
        if world.assigned.len() >= 3 {
            break;
        }
    }
    let repair = world.assigned.get(2).expect("repair assigned despite missing job log");
    assert!(
        repair.sections.iter().any(|section| section.kind == engine::BriefKind::Forge(engine::ForgeBriefKind::Ci)
            && matches!(&section.body, engine::BriefBody::Text(words)
            if words.windows(b"[job log could not be read]".len()).any(|part| part == b"[job log could not be read]")
                && words.windows(b"Description:".len()).any(|part| part == b"Description:")
                && words.windows(b"Link:".len()).any(|part| part == b"Link:"))),
        "CI sections: {:?}",
        repair.sections
    );
}

fn failed_ci_with_delayed_brief() -> (World, u64, u64) {
    let (mut world, chat, producer, branch) = change_world(false, 0, None);
    assert!(matches!(
        world.external(2, raw::Op::Git(raw::Git::Create { branch: branch.clone(), commit: 1 })),
        raw::Answer::Branch(raw::Created::Created)
    ));
    let head = fake::advance(&mut world.fake, &world.fake_env, b"org/repo", &branch, b"file", b"first", 1)
        .expect("producer pushed its branch");
    world.external(
        1,
        raw::Op::Write(raw::Write::Status {
            commit: head,
            context: Box::from(&b"build"[..]),
            state: raw::Check::Failed,
        }),
    );
    world.slow_brief_reads = true;
    world.send(engine::Event::Answer {
        channel: Token::new(7),
        task: producer.task,
        attempt: producer.attempt,
        cumulative: 5,
        end: tasks::End::Finished {
            result: tasks::TaskResult::Change {
                connector: 0,
                kind: 2,
                resource: u64::from(forge_world::REPO.repository),
                words: Box::from(&b"pushed"[..]),
            },
            cancel_delegates: false,
        },
        saved: None,
    });
    (world, chat.task, producer.task)
}

#[test]
fn a_required_section_missing_sends_the_task_back_to_due_without_spending_a_try() {
    let (mut world, chat, producer) = failed_ci_with_delayed_brief();
    let mut repair = None;
    for _ in 0..150 {
        world.tick();
        for stored in world.store.rows.values() {
            let Record::Tasks(tasks::Stored::Live(row)) = stored else { continue };
            if row.number != chat
                && row.number != producer
                && matches!(row.executor, tasks::Executor::Agent { .. })
                && row.refusals > 0
            {
                repair = Some(row.clone());
                break;
            }
        }
        if repair.is_some() {
            break;
        }
    }
    let repair = repair.unwrap_or_else(|| {
        panic!(
            "required forge section timed out while preparing: tasks={:?}; pending={:?}; assigned={:?}",
            world
                .store
                .rows
                .values()
                .filter(|row| matches!(row, Record::Tasks(tasks::Stored::Live(_))))
                .collect::<Vec<_>>(),
            world.pending,
            world.assigned,
        )
    });
    assert_eq!(repair.attempt, 0, "no claim was made");
    assert_eq!(repair.tries, tasks::Tries::NONE, "preparation consumed no try");
    assert_eq!(world.assigned.len(), 2, "repair was never assigned");
}

#[test]
fn an_amendment_while_gathering_discards_the_old_brief_without_claiming() {
    let (mut world, chat, producer) = failed_ci_with_delayed_brief();
    let mut repair = None;
    for _ in 0..150 {
        world.tick();
        for stored in world.store.rows.values() {
            let Record::Tasks(tasks::Stored::Live(row)) = stored else { continue };
            if row.number != chat
                && row.number != producer
                && matches!(row.executor, tasks::Executor::Agent { .. })
                && row.phase == tasks::Phase::Active(tasks::Active::Preparing)
                && world.pending.values().any(|op| matches!(op, client::api::Op::Read(client::api::Read::Job { .. })))
            {
                repair = Some(row.number);
                break;
            }
        }
        if repair.is_some() {
            break;
        }
    }
    let repair = repair.unwrap_or_else(|| {
        panic!(
            "repair gathering its required section: phases={:?}; pending={:?}",
            world
                .store
                .rows
                .values()
                .filter_map(|stored| match stored {
                    Record::Tasks(tasks::Stored::Live(row)) => Some((row.number, row.phase.clone())),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            world.pending
        )
    });
    world.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(790)),
        sign_in: world.signed_in.expect("owner signed in"),
        key: [79; 16],
        ask: people::Ask::Amend {
            project: 1,
            task: repair,
            amendment: people::Amendment {
                spec: Some(people::Spec {
                    words: b"revised repair".as_slice().into(),
                    parameters: Box::new([]),
                    inputs: Box::new([]),
                }),
                wake: None,
                dependencies: None,
                authority: None,
                reason: b"new failure detail".as_slice().into(),
            },
        },
    });
    for _ in 0..40 {
        world.tick();
        let Some(Record::Tasks(tasks::Stored::Live(row))) = world.store.rows.get(&Key::Tasks(tasks::Key::Live(repair)))
        else {
            continue;
        };
        if row.refusals > 0 && row.last_message == 2 {
            assert_eq!(row.spec.words.as_ref(), b"revised repair");
            assert_eq!(row.attempt, 0, "old preparation was not claimed");
            assert_eq!(row.tries, tasks::Tries::NONE);
            assert_eq!(world.assigned.len(), 2);
            for _ in 0..250 {
                world.tick();
            }
            assert_eq!(world.assigned.len(), 2, "an abandoned brief cannot assign after its delayed read");
            return;
        }
    }
    panic!(
        "amended repair did not abandon its pending brief: {:?}",
        world.store.rows.get(&Key::Tasks(tasks::Key::Live(repair)))
    );
}

#[test]
fn a_conflicting_update_is_resolved_from_a_merge_in_progress() {
    let (mut world, _chat, producer, branch) = change_world(false, 1000, None);
    assert!(matches!(
        world.external(2, raw::Op::Git(raw::Git::Create { branch: branch.clone(), commit: 1 })),
        raw::Answer::Branch(raw::Created::Created)
    ));
    fake::advance(&mut world.fake, &world.fake_env, b"org/repo", &branch, b"file", b"branch change", 1)
        .expect("producer pushed branch");
    let base = fake::advance(&mut world.fake, &world.fake_env, b"org/repo", b"main", b"file", b"base change", 1)
        .expect("another change moved the base");
    world.send(engine::Event::Answer {
        channel: Token::new(7),
        task: producer.task,
        attempt: producer.attempt,
        cumulative: 5,
        end: tasks::End::Finished {
            result: tasks::TaskResult::Change {
                connector: 0,
                kind: 2,
                resource: u64::from(forge_world::REPO.repository),
                words: Box::from(&b"pushed"[..]),
            },
            cancel_delegates: false,
        },
        saved: None,
    });
    for _ in 0..200 {
        world.tick();
        if world.assigned.len() >= 3 {
            break;
        }
    }
    assert!(
        world.assigned.len() >= 3,
        "conflict did not assign a resolver: task2={:?}; forge={:?}",
        world.store.rows.get(&Key::Tasks(tasks::Key::Live(2))),
        world.store.rows.values().filter(|row| matches!(row, Record::Forge { .. })).collect::<Vec<_>>()
    );
    let resolver = world.assigned[2].clone();
    assert!(matches!(resolver.workspace.repositories[0].start,
        engine::ForgeStart::Merge { branch: ref source, base: expected }
            if source.as_ref() == branch.as_ref() && expected == translate::commit(base)));
    assert!(
        resolver.sections.iter().any(|section| section.kind == engine::BriefKind::Forge(engine::ForgeBriefKind::Pull)
            && matches!(&section.body, engine::BriefBody::Text(words)
            if words.windows(b"What landed in the base".len()).any(|part| part == b"What landed in the base")
                && words.windows(b"Conflicting file".len()).any(|part| part == b"Conflicting file"))),
        "resolution sections: {:?}",
        resolver.sections
    );
    let old = world.fake.branch(b"org/repo", &branch).expect("existing branch head");
    let merge = fake::merge_commit(
        &mut world.fake,
        &world.fake_env.limits,
        old,
        base,
        Box::new([raw::File { path: Box::from(&b"file"[..]), content: Box::from(&b"resolved"[..]) }]),
        b"Resolve conflict",
    )
    .expect("resolved merge commit");
    assert!(matches!(
        world.external(1, raw::Op::Git(raw::Git::Push { branch: branch.clone(), commit: merge, expected: Some(old) })),
        raw::Answer::Pushed(raw::Pushed::Pushed)
    ));
    world.send(engine::Event::Answer {
        channel: Token::new(7),
        task: resolver.task,
        attempt: resolver.attempt,
        cumulative: 5,
        end: tasks::End::Finished {
            result: tasks::TaskResult::Change {
                connector: 0,
                kind: 2,
                resource: u64::from(forge_world::REPO.repository),
                words: Box::from(&b"resolved"[..]),
            },
            cancel_delegates: false,
        },
        saved: None,
    });
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
    assert!(
        world.store.rows.values().any(|stored| matches!(stored,
            Record::Forge { row, .. } if matches!(row.as_ref(), forge_top::Stored::Change(change)
                if matches!(change.change.state, temper_engine_domain_forge_change::State::Landed { .. })
                    && change.change.resolutions == 1)
        )),
        "resolved change did not land"
    );
}

#[test]
#[expect(clippy::too_many_lines, reason = "the story checks both clean-update carryover and a new review after repair")]
fn an_approval_carries_over_a_clean_update_and_is_asked_again_after_a_repair() {
    let (mut world, _chat, producer, branch) = change_world(false, 1000, Some(people::Freshness::Clean));
    assert!(matches!(
        world.external(2, raw::Op::Git(raw::Git::Create { branch: branch.clone(), commit: 1 })),
        raw::Answer::Branch(raw::Created::Created)
    ));
    let first = fake::advance(&mut world.fake, &world.fake_env, b"org/repo", &branch, b"file", b"fix", 1)
        .expect("producer pushed branch");
    world.send(engine::Event::Answer {
        channel: Token::new(7),
        task: producer.task,
        attempt: producer.attempt,
        cumulative: 5,
        end: tasks::End::Finished {
            result: tasks::TaskResult::Change {
                connector: 0,
                kind: 2,
                resource: u64::from(forge_world::REPO.repository),
                words: Box::from(&b"pushed"[..]),
            },
            cancel_delegates: false,
        },
        saved: None,
    });
    for _ in 0..100 {
        world.tick();
        if world.store.rows.values().any(|stored| matches!(stored,
            Record::Forge { row, .. } if matches!(row.as_ref(), forge_top::Stored::Change(change) if change.pull.is_some())
        )) { break; }
    }
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
        .expect("opened pull before review");
    assert!(
        !world.store.rows.values().any(|stored| matches!(stored,
            Record::Forge { row, .. } if matches!(row.as_ref(), forge_top::Stored::Change(change)
                if matches!(change.change.state, temper_engine_domain_forge_change::State::Landed { .. }))
        )),
        "approval requirement holds the first head"
    );
    assert!(matches!(
        world.external(
            2,
            raw::Op::Write(raw::Write::Review {
                number: pull,
                verdict: Some(raw::Verdict::Approve),
                body: Box::from(&b"Approved"[..]),
            })
        ),
        raw::Answer::Reviewed(_)
    ));
    let moved = fake::advance(&mut world.fake, &world.fake_env, b"org/repo", b"main", b"other", b"unrelated", 1)
        .expect("base advanced cleanly");
    world.send(engine::Event::ForgeHint {
        hint: client::api::Hint {
            repository: forge_world::REPO,
            change: client::api::Change::Branch(Box::from(&b"main"[..])),
            key: None,
        },
    });
    for _ in 0..240 {
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
    assert!(
        world.store.rows.values().any(|stored| matches!(stored,
            Record::Forge { row, .. } if matches!(row.as_ref(), forge_top::Stored::Change(change)
                if matches!(change.change.state, temper_engine_domain_forge_change::State::Landed { .. })
                    && change.change.updates == 1 && change.change.last_head != Some(translate::commit(first)))
        )),
        "old approval did not carry across the clean update; base={moved:?}; forge={:?}",
        world.store.rows.values().filter(|row| matches!(row, Record::Forge { .. })).collect::<Vec<_>>()
    );

    let (mut world, _chat, producer, branch) = change_world(false, 0, Some(people::Freshness::Clean));
    world.external(
        1,
        raw::Op::Write(raw::Write::Status { commit: 1, context: Box::from(&b"build"[..]), state: raw::Check::Passed }),
    );
    assert!(matches!(
        world.external(2, raw::Op::Git(raw::Git::Create { branch: branch.clone(), commit: 1 })),
        raw::Answer::Branch(raw::Created::Created)
    ));
    let first = fake::advance(&mut world.fake, &world.fake_env, b"org/repo", &branch, b"file", b"broken", 1)
        .expect("producer pushed branch");
    world.external(
        1,
        raw::Op::Write(raw::Write::Status {
            commit: first,
            context: Box::from(&b"build"[..]),
            state: raw::Check::Failed,
        }),
    );
    world.send(engine::Event::Answer {
        channel: Token::new(7),
        task: producer.task,
        attempt: producer.attempt,
        cumulative: 5,
        end: tasks::End::Finished {
            result: tasks::TaskResult::Change {
                connector: 0,
                kind: 2,
                resource: u64::from(forge_world::REPO.repository),
                words: Box::from(&b"pushed"[..]),
            },
            cancel_delegates: false,
        },
        saved: None,
    });
    for _ in 0..200 {
        world.tick();
        if world.assigned.len() >= 3 {
            break;
        }
    }
    let repair = world.assigned.get(2).expect("CI failure assigned repair").clone();
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
        .expect("opened pull for repair");
    assert!(matches!(
        world.external(
            2,
            raw::Op::Write(raw::Write::Review {
                number: pull,
                verdict: Some(raw::Verdict::Approve),
                body: Box::from(&b"Old head approved"[..]),
            })
        ),
        raw::Answer::Reviewed(_)
    ));
    let repaired = fake::advance(&mut world.fake, &world.fake_env, b"org/repo", &branch, b"file", b"repaired", 1)
        .expect("repair pushed new head");
    for _ in 0..5 {
        world.tick();
    }
    world.external(
        1,
        raw::Op::Write(raw::Write::Status {
            commit: repaired,
            context: Box::from(&b"build"[..]),
            state: raw::Check::Passed,
        }),
    );
    world.send(engine::Event::Answer {
        channel: Token::new(7),
        task: repair.task,
        attempt: repair.attempt,
        cumulative: 5,
        end: tasks::End::Finished {
            result: tasks::TaskResult::Change {
                connector: 0,
                kind: 2,
                resource: u64::from(forge_world::REPO.repository),
                words: Box::from(&b"repaired"[..]),
            },
            cancel_delegates: false,
        },
        saved: None,
    });
    for _ in 0..100 {
        world.tick();
    }
    assert!(
        !world.store.rows.values().any(|stored| matches!(stored,
            Record::Forge { row, .. } if matches!(row.as_ref(), forge_top::Stored::Change(change)
                if matches!(change.change.state, temper_engine_domain_forge_change::State::Landed { .. }))
        )),
        "old review must not approve repaired head"
    );
    assert!(matches!(
        world.external(
            2,
            raw::Op::Write(raw::Write::Review {
                number: pull,
                verdict: Some(raw::Verdict::Approve),
                body: Box::from(&b"Repaired head approved"[..]),
            })
        ),
        raw::Answer::Reviewed(_)
    ));
    world.send(engine::Event::ForgeHint {
        hint: client::api::Hint { repository: forge_world::REPO, change: client::api::Change::Item(pull), key: None },
    });
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
    assert!(
        world.store.rows.values().any(|stored| matches!(stored,
            Record::Forge { row, .. } if matches!(row.as_ref(), forge_top::Stored::Change(change)
                if matches!(change.change.state, temper_engine_domain_forge_change::State::Landed { .. })
                    && change.change.repairs == 1)
        )),
        "new review must approve repaired head; forge={:?}",
        world.store.rows.values().filter(|row| matches!(row, Record::Forge { .. })).collect::<Vec<_>>()
    );
}

#[test]
fn a_worker_frozen_past_its_grace_resumes_with_a_push_in_hand_and_lands_nothing_twice() {
    let (mut world, chat, producer, branch) = change_world(false, 1000, None);
    assert!(matches!(
        world.external(2, raw::Op::Git(raw::Git::Create { branch: branch.clone(), commit: 1 })),
        raw::Answer::Branch(raw::Created::Created)
    ));
    let pushed = fake::advance(&mut world.fake, &world.fake_env, b"org/repo", &branch, b"file", b"already pushed", 1)
        .expect("worker pushed before freezing");
    world.send(engine::Event::Lost { channel: Token::new(7) });
    world.send(engine::Event::Hello {
        channel: Token::new(8),
        hello: fleet::Hello {
            stop_bound: Duration::from_secs(1),
            slots: 2,
            workstreams: Box::new([]),
            hosting: Box::new([]),
        },
    });
    for _ in 0..150 {
        world.tick();
        if world.assigned.len() >= 3 {
            break;
        }
    }
    let resumed = world.assigned.get(2).expect("lost worker's task reassigned").clone();
    assert_eq!(resumed.task, producer.task);
    assert_ne!(resumed.attempt, producer.attempt);
    assert!(
        matches!(resumed.workspace.repositories[0].start, engine::ForgeStart::Branch(ref name)
        if name.as_ref() == branch.as_ref()),
        "resumed worker sees its pushed branch: {:?}",
        resumed.workspace.repositories[0].start
    );
    assert_eq!(world.fake.branch(b"org/repo", &branch), Some(pushed));
    world.send(engine::Event::Answer {
        channel: Token::new(8),
        task: resumed.task,
        attempt: resumed.attempt,
        cumulative: 5,
        end: tasks::End::Finished {
            result: tasks::TaskResult::Change {
                connector: 0,
                kind: 2,
                resource: u64::from(forge_world::REPO.repository),
                words: Box::from(&b"pushed"[..]),
            },
            cancel_delegates: false,
        },
        saved: None,
    });
    for _ in 0..200 {
        world.tick();
    }
    let landed = world
        .store
        .rows
        .values()
        .filter(|stored| {
            matches!(stored,
                Record::Forge { row, .. } if matches!(row.as_ref(), forge_top::Stored::Change(change)
                    if matches!(change.change.state, temper_engine_domain_forge_change::State::Landed { .. }))
            )
        })
        .count();
    assert_eq!(landed, 1, "one landing after a frozen worker resumes");
    assert_eq!(
        world
            .store
            .rows
            .values()
            .filter(|stored| matches!(stored,
                Record::Tasks(tasks::Stored::Ended(row)) if row.requester == tasks::Party::Task(chat.task)
                    && matches!(row.phase, tasks::Phase::Ended(tasks::Ending::Done(tasks::TaskResult::Change { .. })))
            ))
            .count(),
        1,
        "one result after recovery"
    );
}

#[test]
#[expect(clippy::too_many_lines, reason = "the base repair story drives two consecutive change landings")]
fn a_broken_landing_branch_gets_one_deployment_repair_before_waiting_changes_land() {
    let (mut world, _chat, producer, branch) = change_world(false, 1000, None);
    assert!(matches!(
        world.external(
            1,
            raw::Op::Write(raw::Write::Status {
                commit: 1,
                context: Box::from(&b"build"[..]),
                state: raw::Check::Failed,
            })
        ),
        raw::Answer::Done
    ));
    assert!(matches!(
        world.external(2, raw::Op::Git(raw::Git::Create { branch: branch.clone(), commit: 1 })),
        raw::Answer::Branch(raw::Created::Created)
    ));
    fake::advance(&mut world.fake, &world.fake_env, b"org/repo", &branch, b"file", b"fix", 1)
        .expect("original change pushed");
    world.send(engine::Event::Answer {
        channel: Token::new(7),
        task: producer.task,
        attempt: producer.attempt,
        cumulative: 5,
        end: tasks::End::Finished {
            result: tasks::TaskResult::Change {
                connector: 0,
                kind: 2,
                resource: u64::from(forge_world::REPO.repository),
                words: Box::from(&b"pushed"[..]),
            },
            cancel_delegates: false,
        },
        saved: None,
    });
    for _ in 0..200 {
        world.tick();
        if world.assigned.len() >= 3 {
            break;
        }
    }
    let repair = world
        .assigned
        .get(2)
        .unwrap_or_else(|| {
            panic!(
                "base failure assigned deployment repair producer; task4={:?}; task5={:?}; assignments={:?}",
                world.store.rows.get(&Key::Tasks(tasks::Key::Live(4))),
                world.store.rows.get(&Key::Tasks(tasks::Key::Live(5))),
                world.assigned
            )
        })
        .clone();
    let repair_branch = repair.workspace.repositories[0].push.as_ref().expect("repair branch").clone();
    assert_ne!(repair_branch, branch);
    assert!(world.store.rows.values().any(|stored| matches!(stored,
        Record::Forge { row, .. } if matches!(row.as_ref(), forge_top::Stored::Change(change)
            if change.base_repair && change.queue_repair.is_none())
    )));
    assert!(matches!(
        world.external(2, raw::Op::Git(raw::Git::Create { branch: repair_branch.clone(), commit: 1 })),
        raw::Answer::Branch(raw::Created::Created)
    ));
    fake::advance(&mut world.fake, &world.fake_env, b"org/repo", &repair_branch, b"base", b"fixed", 1)
        .expect("base repair pushed");
    world.send(engine::Event::Answer {
        channel: Token::new(7),
        task: repair.task,
        attempt: repair.attempt,
        cumulative: 5,
        end: tasks::End::Finished {
            result: tasks::TaskResult::Change {
                connector: 0,
                kind: 2,
                resource: u64::from(forge_world::REPO.repository),
                words: Box::from(&b"repaired"[..]),
            },
            cancel_delegates: false,
        },
        saved: None,
    });
    for _ in 0..1000 {
        world.tick();
        if world
            .store
            .rows
            .values()
            .filter(|stored| {
                matches!(stored,
                    Record::Forge { row, .. } if matches!(row.as_ref(), forge_top::Stored::Change(change)
                        if matches!(change.change.state, temper_engine_domain_forge_change::State::Landed { .. }))
                )
            })
            .count()
            >= 2
        {
            break;
        }
    }
    assert_eq!(
        world
            .store
            .rows
            .values()
            .filter(|stored| matches!(stored,
                Record::Forge { row, .. } if matches!(row.as_ref(), forge_top::Stored::Change(change)
                    if matches!(change.change.state, temper_engine_domain_forge_change::State::Landed { .. }))
            ))
            .count(),
        2,
        "deployment repair and waiting change land; forge={:?}",
        world.store.rows.values().filter(|row| matches!(row, Record::Forge { .. })).collect::<Vec<_>>()
    );
}

#[test]
#[expect(clippy::wildcard_enum_match_arm, reason = "the story selects persisted people rows")]
fn a_repository_adopted_seeds_its_collaborators_into_roles() {
    let mut world = World::new();
    world.adopt();
    let mut roles = None;
    let mut people = BTreeMap::new();
    for row in world.store.rows.values() {
        match row {
            Record::People(temper_engine_domain_people::Stored::Person { number, identity }) => {
                people.insert(identity.key.user, *number);
            }
            Record::People(temper_engine_domain_people::Stored::Roles { project: 1, holdings }) => {
                roles = Some(holdings.clone());
            }
            _ => {}
        }
    }
    let roles = roles.expect("durable project roles");
    assert!(roles.contains(&people::Holding { person: people[&7], role: people::Role::Owner }));
    assert!(roles.contains(&people::Holding { person: people[&1], role: people::Role::Maintainer }));
    assert!(roles.contains(&people::Holding { person: people[&2], role: people::Role::Member }));
}

#[test]
fn an_adoption_sent_twice_across_a_restart_is_made_once() {
    let mut world = World::new();
    world.adopt();
    let first = world.adopted[0];
    let calls = world.forge_calls;
    world.restart(false, false);
    world.until(Until::Ready);
    world.signed_in = None;
    world.send(engine::Event::SignedIn {
        reply_to: ReplyTo::new(Token::new(92)),
        identity: people::Identity {
            key: people::IdentityKey { forge: 1, user: 7 },
            login: Box::from(&b"owner"[..]),
            name: Box::from(&b"Owner"[..]),
        },
    });
    world.until(Until::SignedIn);
    world.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(93)),
        sign_in: world.signed_in.expect("owner signed in again"),
        key: [91; 16],
        ask: people::Ask::AdoptRepository {
            project: 1,
            adoption: people::Adoption {
                home: true,
                forge: forge_world::REPO.forge,
                repository: forge_world::REPO.repository,
                host: Box::from(&b"forge.example"[..]),
                owner: Box::from(&b"org"[..]),
                name: Box::from(&b"repo"[..]),
                prefix: Box::from(&b"temper/"[..]),
                role: people::RepositoryRole::Owned,
                landing: Box::from(&b"main"[..]),
                ci: true,
                checks: Box::new([]),
            },
        },
    });
    world.until(Until::Adopted);
    assert_eq!(world.adopted, [first], "the committed answer replays after restart");
    assert_eq!(world.forge_calls, calls, "a replay never asks the forge to adopt again");
}

#[test]
#[expect(clippy::too_many_lines, reason = "saved-work checkout story includes a second adopted repository")]
fn a_saved_repository_tag_becomes_a_concrete_checkout_in_the_next_attempt() {
    let mut world = World::configured(true, false);
    world.adopt();
    fake::repository(
        &mut world.fake,
        &world.fake_env.limits,
        raw::Setup {
            name: Box::from(&b"org/extra"[..]),
            default: Box::from(&b"main"[..]),
            tree: Box::new([]),
            labels: Box::new([]),
            checks: raw::Checks {
                contexts: Box::new([]),
                latency_min: Duration::ZERO,
                latency_max: Duration::ZERO,
                silent: 0,
                passes: 1000,
                reruns: 0,
                cue: None,
            },
            protection: None,
            hooked: false,
        },
    );
    fake::grant(&mut world.fake, b"org/extra", 1, raw::Permission::Admin);
    world.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(98)),
        sign_in: world.signed_in.expect("owner session"),
        key: [98; 16],
        ask: people::Ask::AdoptRepository {
            project: 1,
            adoption: people::Adoption {
                home: false,
                forge: 1,
                repository: 3,
                host: Box::from(&b"forge.example"[..]),
                owner: Box::from(&b"org"[..]),
                name: Box::from(&b"extra"[..]),
                prefix: Box::from(&b"temper/"[..]),
                role: people::RepositoryRole::Owned,
                landing: Box::from(&b"main"[..]),
                ci: true,
                checks: Box::new([]),
            },
        },
    });
    for _ in 0..100 {
        world.tick();
        if world.adopted.len() == 2 {
            break;
        }
    }
    assert!(
        matches!(world.adopted.get(1), Some(people::Outcome::RepositoryAdopted { .. })),
        "second repository adopted: {:?}",
        world.adopted
    );
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
        reply_to: ReplyTo::new(Token::new(99)),
        sign_in: world.signed_in.expect("owner session"),
        key: [99; 16],
        ask: people::Ask::StartChat { project: 1, words: Box::from(&b"Work in two repositories"[..]) },
    });
    world.until(Until::Assigned);
    let first = world.assigned[0].clone();
    world.send(engine::Event::Answer {
        channel: Token::new(7),
        task: first.task,
        attempt: first.attempt,
        cumulative: 0,
        end: tasks::End::Parked,
        saved: Some(Box::new([3])),
    });
    for _ in 0..30 {
        world.tick();
    }
    world.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(100)),
        sign_in: world.signed_in.expect("owner session"),
        key: [100; 16],
        ask: people::Ask::Say { project: 1, task: first.task, words: Box::from(&b"Continue"[..]) },
    });
    for _ in 0..150 {
        world.tick();
        if world.assigned.len() >= 2 {
            break;
        }
    }
    let second = world.assigned.get(1).expect("saved task reassigned");
    assert_eq!(second.saved.as_ref(), [3]);
    assert!(second.workspace.repositories.iter().any(|repository| repository.tag == 3
        && matches!(repository.start, engine::ForgeStart::Saved(ref branch)
            if branch.as_ref() == b"temper/1/s1")));
}

#[test]
fn a_tracked_goal_opens_its_issue_through_the_checked_outbox() {
    let mut world = World::configured(true, false);
    world.adopt();
    world.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(94)),
        sign_in: world.signed_in.expect("owner session"),
        key: [94; 16],
        ask: people::Ask::SetGoal {
            project: 1,
            spec: Box::from(&b"Fix the small bug"[..]),
            charter: 1,
            budget: 50,
            priority: 3,
        },
    });
    for _ in 0..100 {
        world.tick();
    }
    let issue = world
        .store
        .rows
        .values()
        .find_map(|stored| match stored {
            Record::Forge { row, .. } => match row.as_ref() {
                forge_top::Stored::Issue(issue) if issue.pending.is_none() => Some(issue.clone()),
                _ => None,
            },
            _ => None,
        })
        .expect("tracked goal's issue was opened and acknowledged");
    let number = issue.number.expect("provider issued an issue number");
    assert!(world.store.rows.values().any(|stored| matches!(stored,
        Record::Forge { row, .. } if matches!(row.as_ref(), forge_top::Stored::Subscription(sub)
            if sub.task == issue.goal && sub.topic == forge_top::Topic::Participation { repository: forge_world::REPO, number })
    )), "goal subscribed to its actual issue");
    fake::step(
        &mut world.fake,
        &world.fake_env,
        fake::Event::Call {
            reply_to: ReplyTo::new(Token::new(999)),
            user: 2,
            repository: Box::from(&b"org/repo"[..]),
            op: raw::Op::Write(raw::Write::Comment { number, body: Box::from(&b"Please check this"[..]) }),
        },
        &mut world.fake_out,
    );
    for _ in 0..10 {
        if !world.fake_out.is_empty() {
            break;
        }
        world.fake_env.now = world.fake_env.now.saturating_add(Duration::from_millis(100));
        world.fake_env.wall = Wall::from_nanos(world.fake_env.now.as_nanos());
        fake::fire(&mut world.fake, &world.fake_env, &mut world.fake_out);
    }
    let commented = world.fake_out.pop();
    assert!(matches!(commented, Some(fake::Request::Reply { result: Ok(_), .. })), "comment result: {commented:?}");
    world.send(engine::Event::ForgeHint {
        hint: client::api::Hint { repository: forge_world::REPO, change: client::api::Change::Item(number), key: None },
    });
    for _ in 0..100 {
        world.tick();
    }
    assert!(
        matches!(world.store.rows.get(&Key::Tasks(tasks::Key::Live(issue.goal))),
        Some(Record::Tasks(tasks::Stored::Live(row))) if row.inbox.iter().any(|word|
            matches!(word.kind, tasks::MessageKind::News { class: tasks::NewsClass::Wakes, .. }))),
        "goal row after issue hint: {:?}; forge rows: {:?}; pending: {:?}",
        world.store.rows.get(&Key::Tasks(tasks::Key::Live(issue.goal))),
        world.store.rows.values().filter(|row| matches!(row, Record::Forge { .. })).collect::<Vec<_>>(),
        world.pending
    );
}

#[test]
fn a_goal_ending_while_its_issue_is_opening_eventually_closes_that_issue() {
    let mut world = World::configured(true, false);
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
        reply_to: ReplyTo::new(Token::new(94)),
        sign_in: world.signed_in.expect("owner session"),
        key: [94; 16],
        ask: people::Ask::SetGoal {
            project: 1,
            spec: Box::from(&b"Finish the goal"[..]),
            charter: 1,
            budget: 50,
            priority: 3,
        },
    });
    world.until(Until::Assigned);
    let goal = world.assigned[0].clone();
    world.send(engine::Event::Answer {
        channel: Token::new(7),
        task: goal.task,
        attempt: goal.attempt,
        cumulative: 1,
        end: tasks::End::Finished {
            result: tasks::TaskResult::Report { words: Box::from(&b"done"[..]) },
            cancel_delegates: false,
        },
        saved: None,
    });
    for _ in 0..300 {
        world.tick();
        if world.store.rows.values().any(|stored| matches!(stored,
            Record::Forge { row, .. } if matches!(row.as_ref(), forge_top::Stored::Issue(issue) if issue.state.closed)
        )) { break; }
    }
    assert!(
        world.store.rows.values().any(|stored| matches!(stored,
            Record::Forge { row, .. } if matches!(row.as_ref(), forge_top::Stored::Issue(issue) if issue.state.closed)
        )),
        "ended goal's issue did not close"
    );
}

#[test]
fn a_goal_subscription_receives_the_connectors_landing_news() {
    let mut world = World::new();
    world.adopt();
    world.send(engine::Event::Hello {
        channel: Token::new(7),
        hello: fleet::Hello {
            stop_bound: Duration::from_secs(1),
            slots: 1,
            workstreams: Box::new([]),
            hosting: Box::new([]),
        },
    });
    world.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(92)),
        sign_in: world.signed_in.expect("owner session"),
        key: [92; 16],
        ask: people::Ask::StartChat { project: 1, words: Box::from(&b"watch the landing"[..]) },
    });
    world.until(Until::Assigned);
    let assignment = world.assigned[0].clone();
    world.send(engine::Event::Call {
        channel: Token::new(7),
        task: assignment.task,
        attempt: assignment.attempt,
        call: Token::new(93),
        body: engine::Call {
            completion: 1,
            position: 1,
            tool: engine::Tool::SubscribeForge {
                topic: forge_top::Topic::Landings { repository: forge_world::REPO, branch: Box::from(&b"main"[..]) },
                own_change: None,
                paths: Box::new([Box::from(&b"file"[..])]),
            },
        },
    });
    world.until(Until::Subscribed);
    for _ in 0..10 {
        world.tick();
    }
    let result = fake::advance(&mut world.fake, &world.fake_env, b"org/repo", b"main", b"file", b"changed", 1);
    assert!(result.is_ok(), "external landing in the fake forge");
    world.send(engine::Event::ForgeHint {
        hint: client::api::Hint {
            repository: forge_world::REPO,
            change: client::api::Change::Branch(Box::from(&b"main"[..])),
            key: None,
        },
    });
    world.until(Until::News);
    let Some(Record::Tasks(tasks::Stored::Live(row))) =
        world.store.rows.get(&Key::Tasks(tasks::Key::Live(assignment.task)))
    else {
        panic!("subscriber task remains live")
    };
    assert!(
        row.inbox
            .iter()
            .any(|word| matches!(word.kind, tasks::MessageKind::News { class: tasks::NewsClass::Wakes, .. }))
    );
}

fn subscribe_chat(world: &mut World, topic: forge_top::Topic, key: u8) -> u64 {
    world.send(engine::Event::Hello {
        channel: Token::new(7),
        hello: fleet::Hello {
            stop_bound: Duration::from_secs(1),
            slots: 1,
            workstreams: Box::new([]),
            hosting: Box::new([]),
        },
    });
    world.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(u64::from(key))),
        sign_in: world.signed_in.expect("owner session"),
        key: [key; 16],
        ask: people::Ask::StartChat { project: 1, words: Box::from(&b"watch the forge"[..]) },
    });
    world.until(Until::Assigned);
    let assignment = world.assigned[0].clone();
    world.send(engine::Event::Call {
        channel: Token::new(7),
        task: assignment.task,
        attempt: assignment.attempt,
        call: Token::new(u64::from(key) + 1),
        body: engine::Call {
            completion: 1,
            position: 1,
            tool: engine::Tool::SubscribeForge { topic, own_change: None, paths: Box::new([]) },
        },
    });
    world.until(Until::Subscribed);
    assignment.task
}

fn news_count(world: &World, task: u64) -> usize {
    match world.store.rows.get(&Key::Tasks(tasks::Key::Live(task))) {
        Some(Record::Tasks(tasks::Stored::Live(row))) => {
            let mut hits = 0;
            for word in &row.inbox {
                if matches!(word.kind, tasks::MessageKind::News { .. }) {
                    hits += usize::try_from(word.hits).expect("bounded news hits");
                }
            }
            hits
        }
        Some(_) | None => 0,
    }
}

fn await_new_forge_news(world: &mut World, task: u64, before: usize) {
    for _ in 0..120 {
        world.tick();
        if news_count(world, task) > before {
            return;
        }
    }
    panic!("connector news did not reach the subscribed root task");
}

#[test]
fn a_pull_subscription_receives_the_connectors_state_news() {
    let mut world = World::new();
    world.adopt();
    assert!(matches!(
        world.external(2, raw::Op::Git(raw::Git::Create { branch: Box::from(&b"work"[..]), commit: 1 })),
        raw::Answer::Branch(_)
    ));
    fake::advance(&mut world.fake, &world.fake_env, b"org/repo", b"work", b"file", b"change", 1).expect("head");
    let raw::Answer::Created(pull) = world.external(
        2,
        raw::Op::Write(raw::Write::OpenPull {
            title: Box::from(&b"change"[..]),
            body: Box::new([]),
            head: Box::from(&b"work"[..]),
            base: Box::from(&b"main"[..]),
        }),
    ) else {
        panic!("pull opened")
    };
    let task = subscribe_chat(&mut world, forge_top::Topic::Pull { repository: forge_world::REPO, number: pull }, 94);
    for _ in 0..10 {
        world.tick();
    }
    let before = news_count(&world, task);
    assert!(matches!(world.external(2, raw::Op::Write(raw::Write::Close { number: pull })), raw::Answer::Done));
    world.send(engine::Event::ForgeHint {
        hint: client::api::Hint { repository: forge_world::REPO, change: client::api::Change::Item(pull), key: None },
    });
    await_new_forge_news(&mut world, task, before);
}

fn ci_subscription_story(restart: bool) {
    let mut world = World::new();
    world.adopt();
    assert!(matches!(
        world.external(2, raw::Op::Git(raw::Git::Create { branch: Box::from(&b"work"[..]), commit: 1 })),
        raw::Answer::Branch(_)
    ));
    let head =
        fake::advance(&mut world.fake, &world.fake_env, b"org/repo", b"work", b"file", b"change", 1).expect("head");
    let raw::Answer::Created(_pull) = world.external(
        2,
        raw::Op::Write(raw::Write::OpenPull {
            title: Box::from(&b"change"[..]),
            body: Box::new([]),
            head: Box::from(&b"work"[..]),
            base: Box::from(&b"main"[..]),
        }),
    ) else {
        panic!("pull opened")
    };
    let task = subscribe_chat(
        &mut world,
        forge_top::Topic::Ci { repository: forge_world::REPO, head: translate::commit(head) },
        96,
    );
    for _ in 0..10 {
        world.tick();
    }
    if restart {
        world.restart(false, false);
        for _ in 0..20 {
            world.tick();
        }
    }
    let before = news_count(&world, task);
    assert!(matches!(
        world.external(
            1,
            raw::Op::Write(raw::Write::Status {
                commit: head,
                context: Box::from(&b"build"[..]),
                state: raw::Check::Failed
            })
        ),
        raw::Answer::Done
    ));
    world.send(engine::Event::ForgeHint {
        hint: client::api::Hint {
            repository: forge_world::REPO,
            change: client::api::Change::Commit(translate::commit(head)),
            key: None,
        },
    });
    await_new_forge_news(&mut world, task, before);
}

#[test]
fn a_ci_subscription_receives_the_connectors_verdict_news() {
    ci_subscription_story(false);
}

#[test]
fn a_durable_ci_subscription_receives_verdict_news_after_a_cold_restart() {
    ci_subscription_story(true);
}

#[test]
fn a_participation_subscription_receives_the_connectors_comment_news() {
    let mut world = World::new();
    world.adopt();
    assert!(matches!(
        world.external(2, raw::Op::Git(raw::Git::Create { branch: Box::from(&b"work"[..]), commit: 1 })),
        raw::Answer::Branch(_)
    ));
    fake::advance(&mut world.fake, &world.fake_env, b"org/repo", b"work", b"file", b"change", 1).expect("head");
    let raw::Answer::Created(pull) = world.external(
        2,
        raw::Op::Write(raw::Write::OpenPull {
            title: Box::from(&b"change"[..]),
            body: Box::new([]),
            head: Box::from(&b"work"[..]),
            base: Box::from(&b"main"[..]),
        }),
    ) else {
        panic!("pull opened")
    };
    let task =
        subscribe_chat(&mut world, forge_top::Topic::Participation { repository: forge_world::REPO, number: pull }, 98);
    for _ in 0..10 {
        world.tick();
    }
    let before = news_count(&world, task);
    assert!(matches!(
        world.external(2, raw::Op::Write(raw::Write::Comment { number: pull, body: Box::from(&b"please check"[..]) })),
        raw::Answer::Commented(_)
    ));
    world.send(engine::Event::ForgeHint {
        hint: client::api::Hint { repository: forge_world::REPO, change: client::api::Change::Item(pull), key: None },
    });
    await_new_forge_news(&mut world, task, before);
}

#[test]
fn an_authorized_forge_read_returns_a_bounded_typed_answer() {
    let mut world = World::configured(true, false);
    world.adopt();
    world.send(engine::Event::Hello {
        channel: Token::new(7),
        hello: fleet::Hello {
            stop_bound: Duration::from_secs(1),
            slots: 1,
            workstreams: Box::new([]),
            hosting: Box::new([]),
        },
    });
    world.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(92)),
        sign_in: world.signed_in.expect("owner session"),
        key: [92; 16],
        ask: people::Ask::StartChat { project: 1, words: Box::from(&b"read forge"[..]) },
    });
    world.until(Until::Assigned);
    let assignment = world.assigned[0].clone();
    world.send(engine::Event::Call {
        channel: Token::new(7),
        task: assignment.task,
        attempt: assignment.attempt,
        call: Token::new(93),
        body: engine::Call {
            completion: 1,
            position: 1,
            tool: engine::Tool::ReadForge { repository: forge_world::REPO, read: client::api::Read::Branches },
        },
    });
    world.until(Until::Read);
    assert!(
        matches!(world.answers.as_slice(), [temper_engine_domain::CallAnswer::ForgeRead(result)] if result.is_ok())
    );
}

#[test]
#[expect(clippy::wildcard_enum_match_arm, reason = "the story prints an unexpected typed effect answer")]
fn a_named_forge_effect_is_committed_before_its_write_and_made_once() {
    let mut world = World::configured(true, false);
    world.adopt();
    world.send(engine::Event::Hello {
        channel: Token::new(7),
        hello: fleet::Hello {
            stop_bound: Duration::from_secs(1),
            slots: 1,
            workstreams: Box::new([]),
            hosting: Box::new([]),
        },
    });
    world.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(92)),
        sign_in: world.signed_in.expect("owner session"),
        key: [92; 16],
        ask: people::Ask::StartChat { project: 1, words: Box::from(&b"create an issue"[..]) },
    });
    world.until(Until::Assigned);
    let assignment = world.assigned[0].clone();
    world.send(engine::Event::Call {
        channel: Token::new(7),
        task: assignment.task,
        attempt: assignment.attempt,
        call: Token::new(93),
        body: engine::Call {
            completion: 1,
            position: 1,
            tool: engine::Tool::EffectForge {
                repository: forge_world::REPO,
                resource: forge_top::What::Repository,
                write: Box::new(client::api::Write::CreateIssue {
                    key: Box::new([]),
                    title: Box::from(&b"Small finding"[..]),
                    body: Box::from(&b"Fix this"[..]),
                }),
            },
        },
    });
    world.until(Until::Effect);
    let entry = match world.answers[0] {
        temper_engine_domain::CallAnswer::ForgeEffect { entry, outcome: None } => entry,
        ref other => panic!("unexpected effect answer: {other:?}"),
    };
    for _ in 0..30 {
        world.tick();
    }
    let key = temper_engine_domain::CallKey {
        task: assignment.task,
        attempt: assignment.attempt,
        completion: 1,
        position: 1,
    };
    assert!(
        matches!(world.store.rows.get(&Key::Call(key)), Some(Record::Call(row)) if matches!(row.answer, temper_engine_domain::CallAnswer::ForgeEffect { entry: observed, outcome: Some(client::Outcome::Made { .. }) } if observed == entry))
    );
    world.send(engine::Event::Call {
        channel: Token::new(7),
        task: assignment.task,
        attempt: assignment.attempt,
        call: Token::new(94),
        body: engine::Call {
            completion: 1,
            position: 1,
            tool: engine::Tool::EffectForge {
                repository: forge_world::REPO,
                resource: forge_top::What::Repository,
                write: Box::new(client::api::Write::CreateIssue {
                    key: Box::new([]),
                    title: Box::from(&b"Small finding"[..]),
                    body: Box::from(&b"Fix this"[..]),
                }),
            },
        },
    });
    for _ in 0..10 {
        world.tick();
    }
    assert!(
        matches!(world.answers.last(), Some(temper_engine_domain::CallAnswer::ForgeEffect { entry: observed, outcome: Some(client::Outcome::Made { .. }) }) if *observed == entry)
    );
}

fn issue_effect_call(assignment: &engine::Assignment) -> engine::Event {
    engine::Event::Call {
        channel: Token::new(7),
        task: assignment.task,
        attempt: assignment.attempt,
        call: Token::new(93),
        body: engine::Call {
            completion: 1,
            position: 1,
            tool: engine::Tool::EffectForge {
                repository: forge_world::REPO,
                resource: forge_top::What::Repository,
                write: Box::new(client::api::Write::CreateIssue {
                    key: Box::new([]),
                    title: Box::from(&b"Small finding"[..]),
                    body: Box::from(&b"Fix this"[..]),
                }),
            },
        },
    }
}

fn pending_issue_effect() -> (World, engine::Assignment) {
    let mut world = World::configured(true, false);
    world.adopt();
    world.send(engine::Event::Hello {
        channel: Token::new(7),
        hello: fleet::Hello {
            stop_bound: Duration::from_secs(1),
            slots: 1,
            workstreams: Box::new([]),
            hosting: Box::new([]),
        },
    });
    world.send(engine::Event::Ask {
        reply_to: ReplyTo::new(Token::new(92)),
        sign_in: world.signed_in.expect("owner session"),
        key: [92; 16],
        ask: people::Ask::StartChat { project: 1, words: Box::from(&b"create an issue"[..]) },
    });
    world.until(Until::Assigned);
    let assignment = world.assigned[0].clone();
    world.send(issue_effect_call(&assignment));
    assert!(!world.store.pending.is_empty(), "effect decision awaits durability");
    (world, assignment)
}

fn issue_count(world: &mut World) -> usize {
    let raw::Answer::Items { items, more: false, .. } = world.external(
        1,
        raw::Op::Read(raw::Read::Items {
            state: None,
            kind: Some(raw::Kind::Issue),
            labels: Box::new([]),
            author: None,
            since: Time::ZERO,
            page: 1,
            limit: 10,
        }),
    ) else {
        panic!("one bounded issue page")
    };
    items.len()
}

#[test]
fn effect_survives_each_commit_and_outbox_cut_without_a_second_issue() {
    for cut in 0..4 {
        let (mut world, assignment) = pending_issue_effect();
        if cut == 0 {
            // The decision was sent, but no record became durable.
            world.store.pending.clear();
        } else {
            let number = world.store.apply();
            if cut >= 2 {
                world.send(engine::Event::Committed { number });
                for _ in 0..60 {
                    if world.events.iter().any(|event| {
                        matches!(
                            event,
                            engine::Event::ForgeAnswered { result: Ok(client::api::Answer::Created(_)), .. }
                        )
                    }) {
                        break;
                    }
                    world.tick();
                }
                assert_eq!(
                    issue_count(&mut world),
                    1,
                    "the external write completed before the process cut: cut={cut} pending={:?} events={:?} answers={:?}",
                    world.pending,
                    world.events,
                    world.answers
                );
                if cut == 3 {
                    let index = world
                        .events
                        .iter()
                        .position(|event| {
                            matches!(
                                event,
                                engine::Event::ForgeAnswered { result: Ok(client::api::Answer::Created(_)), .. }
                            )
                        })
                        .expect("external outcome queued for the old process");
                    let event = world.events.remove(index).expect("queued outcome");
                    world.send(event);
                    assert!(!world.store.pending.is_empty(), "outcome waits for its commit");
                    world.store.pending.clear();
                }
            }
        }
        world.restart(true, false);
        for _ in 0..40 {
            world.tick();
        }
        world.send(issue_effect_call(&assignment));
        for _ in 0..40 {
            world.tick();
        }
        assert_eq!(issue_count(&mut world), 1, "cut {cut} created one keyed issue");
        let key = temper_engine_domain::CallKey {
            task: assignment.task,
            attempt: assignment.attempt,
            completion: 1,
            position: 1,
        };
        assert!(
            matches!(
                world.store.rows.get(&Key::Call(key)),
                Some(Record::Call(row))
                    if matches!(row.answer, temper_engine_domain::CallAnswer::ForgeEffect {
                        outcome: Some(client::Outcome::Made { .. }), ..
                    })
            ),
            "cut {cut} committed the settled named answer"
        );
    }
}
