//! Root forge stories with its durable store and the independent fake Forgejo.
#![expect(clippy::wildcard_enum_match_arm, reason = "the fixture selects only the forge rows relevant to each story")]
use jig_domain_authority as authority;
use jig_domain_host as host;
use jig_domain_people as people;
use jig_domain_tasks as tasks;
use skein_fake_forge_domain::{self as fake, api as raw};
use skein_lib::{Duration, Env, List, Queue, ReplyTo, Time, Token, Wall};
use std::collections::{BTreeMap, VecDeque};
use temper_engine_domain as engine;
use temper_engine_domain::{Delivery, Key, Record};
use temper_engine_domain_forge as forge_top;
use temper_engine_domain_world::protocol::{self, Protocol};
use temper_engine_domain_world::{store::Store, walking};
use temper_engine_forge_world::{self as forge_world, translate};

struct World {
    creating: jig_core_world::fixtures::Creating,
    root: engine::Domain,
    env: Env<engine::Limits>,
    out: Queue<engine::Request>,
    events: VecDeque<engine::Event>,
    store: Store,
    fake: fake::Domain,
    fake_env: Env<fake::Config>,
    fake_out: Queue<fake::Request>,
    pending: BTreeMap<Token, skein_forge::api::Op>,
    adopted: Vec<forge_top::ApplicationOutcome>,
    signed_in: Option<u64>,
    assigned: Vec<engine::Assignment>,
    answers: Vec<temper_engine_domain::CallAnswer>,
    protocol: Protocol,
    immediate: VecDeque<engine::Event>,
    /// Whether the funding period and the git identity were sent to the root once it was ready.
    started: bool,
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
    fn chat(&self, project: u32, words: &[u8]) -> people::Ask {
        walking::configured_chat(&self.creating, &self.store.rows, self.env.limits.tasks.tree_tasks, project, words)
    }
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
        let [(bootstrap, limits), (config, _)] = std::array::from_fn(|_| {
            let mut limits = walking::limits();
            limits.people.amendment_bytes =
                jig_protocol_config::profile(jig_protocol_config::Profile::Small).core.people.amendment_bytes;
            limits.people.people = 4;
            limits.people.holdings = 4;
            limits.journal.writes += (people::max_out(&limits.people) - people::max_out(&walking::limits().people)) * 4;
            let mut config = crate::world::config(71);
            if approval.is_some() {
                limits.authority.roles = 3;
                limits.authority.requirements = 8;
                limits.authority.facts = 8;
            }
            if with_change {
                // The CI section holds its head, three check lines, failed step and 76-byte
                // log end (352 bytes), as forge_routes.rs's change stories size it.
                // Walking's CI budget 128 and read bound 256 lose the log's end.
                limits.brief.read_bytes = 512;
                limits.brief.budgets.ci = 384;
                limits.brief.brief_bytes = 768;
                limits.tasks.tasks = 12;
                // The demo's end may select every live task (temper's `worst_case`).
                limits.forge.tasks = limits.tasks.tasks;
                limits.tasks.project_tasks = 12;
                limits.tasks.delegates = 4;
                limits.tasks.tree_tasks = 12;
                limits.tasks.depth = 3;
                limits.tasks.parameters = 8;
                limits.tasks.spec_bytes = 512;
                limits.tasks.executor_kinds = 2;
                limits.tasks.authority_grants = 10;
                limits.journal.run_bytes = 8192;
                limits.tasks.authority_segments = 9;
                limits.tasks.authority_bytes = 512;
                limits.tasks.contract_choices = 2;
                limits.tasks.inbox_messages = 8;
                limits.tasks.inbox_bytes = 1024;
                limits.authority.executors = 2;
                limits.authority.batch = 2;
                limits.authority.grants = 10;
                limits.authority.segments = 8;
                limits.authority.segment_bytes = 64;
                limits.authority.writes = 2;
                limits.journal.writes += 64;
                // The tasks child's whole route grew with jig's task families.
                limits.journal.writes = 8192;
                limits.journal.deliveries = 256;
                limits.journal.held = 1024;
                limits.journal.result_bytes = 4096;
                limits.journal.transcript_bytes = 32_768;
                limits.fleet.slots = 2;
                limits.fleet.workstreams = 2;
                // The core's inbox census and waiting entries cover every live task.
                limits.people.inbox_tasks = 24;
                limits.people.inbox_recipients = 32;
            }
            if with_read {
                limits.tasks.inbox_bytes = limits.tasks.inbox_bytes.max(256);
                if !with_change {
                    limits.authority.grants = 4;
                }
                limits.authority.segments = 8;
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
                let mut rules = config.core.defaults.rules.clone();
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
                        authority::Grant { kind: 10, ..grant.clone() },
                        authority::Grant { kind: 11, ..grant.clone() },
                        authority::Grant { kind: 12, ..grant.clone() },
                    ])
                } else {
                    Box::new([grant.clone(), effect_grant.clone()])
                };
                let mut ceiling_grants = List::with_capacity(limits.authority.grants);
                for grant in &grants {
                    ceiling_grants.push(grant.clone()).expect("configured task grants");
                }
                ceiling_grants.push(authority::Grant { kind: 7, ..grant.clone() }).expect("projection comments");
                ceiling_grants.push(authority::Grant { kind: 13, ..grant.clone() }).expect("projection issue sets");
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
                let mut policy = config.core.seed_projects[0].policy.policy.clone();
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
                // A chat's authority is its role's, one level shallower (jig a2616181).
                config.core.seed_projects[0].policy.policy = domain.policy(1).expect("configured seed policy").clone();
                config.core.defaults.rules = domain.rules().clone();
            }
            limits.notes.scopes = 3 + limits.authority.grants;
            (config, limits)
        });
        let mut fake_config = forge_world::fake_config();
        fake_config.limits.repositories = 2;
        let mut fake = fake::Domain::new(&fake_config, 71);
        fake::repository(
            &mut fake,
            &fake_config,
            raw::Setup {
                id: 2,
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
        let store = temper_engine_domain_world::fixtures::restored_store(bootstrap, &limits);
        Self {
            creating: jig_core_world::fixtures::Creating::new(&config.core, limits.tasks.tree_tasks),
            root: engine::Domain::new(config, &limits).expect("valid engine configuration"),
            env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits },
            out: Queue::with_capacity(engine::max_out(&limits)),
            events: VecDeque::from([engine::Event::Restart]),
            store,
            fake,
            fake_env,
            fake_out: Queue::with_capacity(fake::MAX_OUT),
            pending: BTreeMap::new(),
            adopted: Vec::new(),
            signed_in: None,
            assigned: Vec::new(),
            answers: Vec::new(),
            protocol: Protocol::default(),
            immediate: VecDeque::new(),
            started: false,
        }
    }

    fn send(&mut self, event: engine::Event) {
        self.immediate.push_back(event);
        self.flush();
    }

    /// Step every input waiting at once: the one sent, then what the
    /// protocol layer answered at once, as the root's door gives them.
    fn flush(&mut self) {
        while let Some(event) = self.immediate.pop_front() {
            if let Err(event) = engine::try_step(&mut self.root, &self.env, event) {
                self.events.push_back(event);
            }
            engine::release(&mut self.root, &self.env, &mut self.out);
            self.collect();
            self.root.reclaim();
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
            let request = match self.protocol.play(self.out.pop().expect("counted root output")) {
                // The protocol layer answers at once (testing-strategy.md, 4).
                Ok(events) => {
                    self.immediate.extend(events);
                    continue;
                }
                Err(request) => *request,
            };
            match request {
                engine::Request::ToChild(released) => self.events.push_back(engine::Event::Released(released)),
                engine::Request::Store(engine::StoreRequest::Commit { number, writes }) => {
                    self.store.pending.push_back((number, writes));
                }
                engine::Request::Store(engine::StoreRequest::Load { owner, range, after, most, .. }) => {
                    let (rows, next) = self.store.page(&range, after.as_ref(), most);
                    self.events.push_back(engine::Event::Store(engine::StoreInput::Loaded {
                        owner,
                        rows,
                        next,
                        texts: None,
                    }));
                }
                engine::Request::Forge { call, repository, op } => {
                    let located = translate::locate_name(&op);
                    let name: &[u8] = if let Some(name) = &located {
                        name
                    } else if repository == forge_world::REPO {
                        b"org/repo"
                    } else if repository == (skein_forge::api::Repository { forge: 1, repository: 3 }) {
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
                engine::Request::Worker(engine::WorkerRequest::Deliver(delivery))
                | engine::Request::Party(engine::PartyRequest::Deliver(delivery)) => match delivery {
                    Delivery::WebReply { reply: people::Reply::SignedIn { sign_in, .. }, .. } => {
                        self.signed_in = Some(sign_in);
                    }
                    Delivery::Assigned { assignment, .. } => self.assigned.push(assignment),
                    // The run has its answer once the hub relays it down.
                    Delivery::Link {
                        down: host::LinkDown::Relayed { answer: host::Relayed::Answer { text, .. }, .. },
                        ..
                    } => {
                        let (_, answer) = self
                            .protocol
                            .answers
                            .iter()
                            .find(|(name, _)| *name == text)
                            .expect("the world rendered every relayed answer");
                        self.answers.push(answer.clone());
                    }
                    Delivery::Application { outcome, .. } => {
                        let Some(engine::ApplicationAnswer::Adopt(outcome)) = outcome else {
                            panic!("retained adoption")
                        };
                        self.adopted.push(outcome);
                    }
                    _ => {}
                },
                engine::Request::Account(_)
                | engine::Request::SetUpGroup { .. }
                | engine::Request::Connect { .. }
                | engine::Request::Manifest { .. }
                | engine::Request::Reachable { .. }
                | engine::Request::Install { .. }
                | engine::Request::EraseCarried { .. }
                | engine::Request::StopGroup { .. }
                | engine::Request::SetUp { .. }
                | engine::Request::Bound { .. }
                | engine::Request::Return { .. }
                | engine::Request::Setup { .. }
                | engine::Request::Party(_)
                | engine::Request::Store(_)
                | engine::Request::Busy(_)
                | engine::Request::Drained => {}
                engine::Request::Worker(other) => panic!("forge fixture received an unexpected request {other:?}"),
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
            self.send(engine::Event::Store(engine::StoreInput::Committed { number, fill: 0 }));
        }
        if !self.started && self.root.ready() {
            self.started = true;
            let period = engine::PartyInput::Period { project: 1, period: 1, budget: 1000, next: None };
            self.send(engine::Event::Party(period));
            // The repository's git identity, an application credential from
            // temper's own source (domain/root.md, 7.1).
            self.send(engine::Event::Credential { number: 1, generation: 1, valid: Duration::from_secs(3600) });
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
                    self.events.push_back(engine::Event::Forge(Box::new(temper_engine_domain_forge::Event::Client(
                        skein_forge_client::Event::Answered { free: 0, rate: None, call, cost: 1, result },
                    ))));
                }
                fake::Request::Hook { .. } => {}
            }
        }
        temper_engine_domain_world::poll(&mut self.root, &self.env, &mut self.out);
        self.collect();
        self.flush();
        engine::fire(&mut self.root, &self.env);
        engine::release(&mut self.root, &self.env, &mut self.out);
        self.collect();
        self.flush();
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
            self.store.rows.get(&Key::Core(jig_domain_core::Key::Tasks(tasks::Key::Live(2)))),
            self.store.rows.get(&Key::Core(jig_domain_core::Key::Tasks(tasks::Key::Live(3)))),
            self.store.rows.values().filter(|row| matches!(row, Record::Forge { .. })).collect::<Vec<_>>()
        );
    }

    fn adopt(&mut self) {
        self.until(Until::Ready);
        self.send(engine::Event::Party(engine::PartyInput::SignedIn {
            held: None,
            reply_to: ReplyTo::new(Token::new(90)),
            identity: people::Identity {
                key: people::IdentityKey { provider: 0, subject: 7_u64.to_be_bytes().into() },
                login: Box::from(&b"owner"[..]),
                name: Box::from(&b"Owner"[..]),
            },
        }));
        self.until(Until::SignedIn);
        self.send(engine::Event::Party(engine::PartyInput::Application {
            scope: jig_domain_people::ApplicationScope::Project(1),
            reply_to: ReplyTo::new(Token::new(91)),
            sign_in: self.signed_in.expect("signed in"),
            key: [91; 16],
            digest: [5; 32],
            carried: None,
            request: engine::Application::Forge(forge_top::Ask::Adopt(forge_top::Adoption {
                project: 1,
                home: true,
                forge: forge_world::REPO.forge,
                host: Box::from(&b"forge.example"[..]),
                owner: Box::from(&b"org"[..]),
                name: Box::from(&b"repo"[..]),
                prefix: Box::from(&b"temper/"[..]),
                role: forge_top::Role::Owned,
                landing: Box::from(&b"main"[..]),
                ci: true,
                checks: Box::new([]),
            })),
        }));
        self.until(Until::Adopted);
        assert!(matches!(self.adopted.as_slice(), [forge_top::ApplicationOutcome::Adopted(_)]));
    }
}

mod stories;
