//! Root forge stories with its durable store and the independent fake Forgejo.
#![expect(clippy::wildcard_enum_match_arm, reason = "the fixture selects only the forge rows relevant to each story")]
use crate as temper_engine_domain_world;
use crate::forge_routes as paths;
use jig_domain_authority as authority;
use jig_domain_core::documents;
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

pub struct World {
    creating: jig_core_world::fixtures::Creating,
    pub root: engine::Domain,
    pub env: Env<engine::Limits>,
    pub out: Queue<engine::Request>,
    pub events: VecDeque<engine::Event>,
    pub store: Store,
    pub fake: fake::Domain,
    pub fake_env: Env<fake::Config>,
    fake_out: Queue<fake::Request>,
    pub pending: BTreeMap<Token, skein_forge::api::Op>,
    pub forge_calls: u32,
    pub adopted: Vec<forge_top::ApplicationOutcome>,
    pub signed_in: Option<u64>,
    pub assigned: Vec<engine::Assignment>,
    pub answers: Vec<temper_engine_domain::CallAnswer>,
    pub reads: Vec<(ReplyTo, jig_domain_core::reads::Answer)>,
    pub protocol: Protocol,
    pub immediate: VecDeque<engine::Event>,
    /// The funding period sent to this process's root, once it is ready.
    period: Option<u64>,
    pub fail_job_reads: bool,
    slow_brief_reads: bool,
    /// A branch whose reads take twenty seconds, each decided when it is sent.
    pub slow_branch: Option<Box<[u8]>>,
    owner_gate: bool,
    pub held_reads: Option<Vec<engine::Request>>,
    pub merges: u32,
}

#[derive(Clone, Copy, Default)]
pub struct StoryLimits {
    pub brief: Option<engine::BriefLimits>,
    pub stall: Option<Duration>,
}

#[derive(Clone, Copy, Debug)]
pub enum Until {
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
    #[must_use]
    pub fn new() -> Self {
        Self::configured(false, false)
    }

    #[must_use]
    pub fn configured(with_read: bool, with_change: bool) -> Self {
        Self::configured_ci(with_read, with_change, false)
    }

    #[must_use]
    pub fn configured_ci(with_read: bool, with_change: bool, silent_ci: bool) -> Self {
        Self::configured_checks(with_read, with_change, silent_ci, 1000)
    }

    #[must_use]
    pub fn configured_checks(with_read: bool, with_change: bool, silent_ci: bool, passes: u32) -> Self {
        Self::configured_policy(with_read, with_change, silent_ci, passes, paths::Gate::default(), None)
    }

    #[must_use]
    pub fn configured_policy(
        with_read: bool,
        with_change: bool,
        silent_ci: bool,
        passes: u32,
        gate: paths::Gate,
        documents: Option<jig_domain_core::documents::DocumentLimits>,
    ) -> Self {
        Self::configured_brief(with_read, with_change, silent_ci, passes, gate, documents, None)
    }

    #[must_use]
    pub fn configured_brief(
        with_read: bool,
        with_change: bool,
        silent_ci: bool,
        passes: u32,
        gate: paths::Gate,
        documents: Option<jig_domain_core::documents::DocumentLimits>,
        brief: Option<engine::BriefLimits>,
    ) -> Self {
        Self::configured_limits(
            with_read,
            with_change,
            silent_ci,
            passes,
            gate,
            documents,
            StoryLimits { brief, ..StoryLimits::default() },
        )
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one fixture configures the policy, fake, and bounds for root forge stories"
    )]
    #[must_use]
    pub fn configured_limits(
        with_read: bool,
        with_change: bool,
        silent_ci: bool,
        passes: u32,
        gate: paths::Gate,
        documents: Option<jig_domain_core::documents::DocumentLimits>,
        timing: StoryLimits,
    ) -> Self {
        let StoryLimits { brief, stall } = timing;
        let paths::Gate { approval, agent: agent_gate, owner: owner_gate } = gate;
        let [(bootstrap, mut limits), (config, _)] = std::array::from_fn(|_| {
            let mut limits = walking::limits();
            limits.people.amendment_bytes =
                jig_protocol_config::profile(jig_protocol_config::Profile::Small).core.people.amendment_bytes;
            limits.people.people = 4;
            limits.people.holdings = 4;
            if owner_gate {
                limits.people.amendment_bytes = 2048;
            }
            limits.journal.writes += (people::max_out(&limits.people) - people::max_out(&walking::limits().people)) * 4;
            let mut config = walking::config(71);
            if approval.is_some() || agent_gate || owner_gate {
                limits.authority.roles = 3;
            }
            if documents.is_some() {
                limits.authority.roles = 2;
            }
            if with_change {
                limits.authority.requirements = 8;
                limits.authority.facts = 8;
            }
            if with_change {
                // One adoption, three chat creations and two Says survive the restart cut.
                limits.people.requests = 6;
                // Adoption stories ask in 128 bytes of words.
                limits.people.words = 128;
                limits.tasks.message_bytes = 128;
                limits.call_records = 4;
                limits.brief.gather = Duration::from_secs(5);
                limits.brief.read_bytes = 512;
                // Change delegates gather the larger CI and pull fixtures below.
                limits.brief.budgets.task = 384;
                limits.brief.budgets.pull = 384;
                limits.brief.budgets.ci = 384;
                limits.brief.brief_bytes = 768;
                limits.tasks.tasks = 12;
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
                    // The chat's 100-unit allotment must retain room for its 20-unit delegate
                    // after the run reserves its capped allowance at claim.
                    rules.maximum_run_spend = 60;
                }
                rules.ceiling.tools = authority::Tools(1023);
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
                if approval.is_some() || agent_gate || owner_gate {
                    let landing: Box<[engine::LandingRule]> = Box::new([engine::LandingRule {
                        connector: 0,
                        kind: 4,
                        enforce: !owner_gate,
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
                        gates: if agent_gate || owner_gate {
                            Box::new([engine::Gate { number: 1, blocking: true, freshness: engine::Freshness::Exact }])
                        } else {
                            Box::new([])
                        },
                        approvals: match approval {
                            Some(freshness) => Box::new([engine::Approval { role: 2, people: 1, freshness }]),
                            None => Box::new([]),
                        },
                    }]);
                    if !owner_gate {
                        config.landing.deployment.clone_from(&landing);
                    }
                    assert!(config.landing.projects.insert(1, landing).is_ok());
                }
                if with_change {
                    rules.ceiling.delegation.kinds =
                        Box::new([authority::Executor::Charter(1), authority::Executor::Procedure(2)]);
                    rules.ceiling.delegation.tasks = 12;
                    rules.ceiling.delegation.depth = 3;
                }
                let mut policy = config.core.seed_projects[0].policy.policy.clone();
                policy.ceiling.tools = authority::Tools(1023);
                policy.ceiling.grants.clone_from(&rules.ceiling.grants);
                policy.projections.clone_from(&rules.ceiling.grants);
                if approval.is_some() || agent_gate || owner_gate {
                    let mut maintainer = policy.roles[0].clone();
                    maintainer.number = 1;
                    let mut member = maintainer.clone();
                    member.number = 2;
                    policy.roles = Box::new([policy.roles[0].clone(), maintainer, member]);
                }
                policy.roles[0].authority.tools = authority::Tools(1023);
                policy.roles[0].authority.grants.clone_from(&grants);
                if with_change {
                    policy.ceiling.delegation = rules.ceiling.delegation.clone();
                    policy.roles[0].authority.delegation = rules.ceiling.delegation.clone();
                }
                if documents.is_some() {
                    paths::add_reader(&mut policy);
                }
                let mut domain = authority::Domain::new(rules, limits.authority).expect("read authority rules");
                let mut facts = Queue::with_capacity(authority::POLICY_MAX_OUT);
                authority::step(&mut domain, authority::Event::Policy { project: 1, policy }, &mut facts);
                assert_eq!(facts.pop(), Some(authority::PolicyFact::Added { project: 1 }));
                // A chat's authority is its role's, one level shallower (jig a2616181).
                config.core.seed_projects[0].policy.policy = domain.policy(1).expect("configured seed policy").clone();
                config.core.defaults.rules = domain.rules().clone();
            }
            if let Some(brief) = brief {
                limits.brief = brief;
                // A shipped brief brings its own room: results hold it, forge sections read it.
                limits.journal.result_bytes = limits.journal.result_bytes.max(brief.brief_bytes);
                limits.forge.brief_bytes = limits.forge.brief_bytes.max(brief.read_bytes);
            }
            if let Some(stall) = stall {
                limits.forge.change_policy.stall = stall;
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
        if let Some(documents) = documents {
            limits.documents = paths::document_limits(documents, &limits);
        }
        // Keep the world's broad page bounds and align its message codec with the root.
        let mut store_limits = temper_engine_domain_world::store::widest();
        store_limits.people.words = limits.people.words;
        store_limits.tasks.message_bytes = limits.tasks.message_bytes;
        let store =
            temper_engine_domain_world::fixtures::restored_over(Store::with_limits(&store_limits), bootstrap, &limits);
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
            forge_calls: 0,
            adopted: Vec::new(),
            signed_in: None,
            assigned: Vec::new(),
            answers: Vec::new(),
            reads: Vec::new(),
            protocol: Protocol::default(),
            immediate: VecDeque::new(),
            period: None,
            fail_job_reads: false,
            slow_brief_reads: false,
            slow_branch: None,
            owner_gate,
            held_reads: None,
            merges: 0,
        }
    }

    #[must_use]
    pub fn chat(&self, project: u32, words: &[u8]) -> people::Ask {
        walking::configured_chat(&self.creating, &self.store.rows, self.env.limits.tasks.tree_tasks, project, words)
    }

    pub fn send(&mut self, event: engine::Event) {
        self.immediate.push_back(event);
        self.flush();
    }

    /// Step every input waiting at once: the one sent, then what the
    /// protocol layer answered at once, as the root's door gives them.
    pub fn flush(&mut self) {
        while let Some(event) = self.immediate.pop_front() {
            if let Err(event) = engine::try_step(&mut self.root, &self.env, event) {
                self.events.push_back(event);
            }
            engine::release(&mut self.root, &self.env, &mut self.out);
            self.collect();
            self.root.reclaim();
        }
    }

    /// Lose the process while retaining only the store, external forge and
    /// worker's hosted attempt. Outstanding API terminals belong to the old
    /// process; startup has to rediscover the outbox by its durable key.
    pub fn restart(&mut self, with_read: bool, with_change: bool) {
        assert!(self.store.pending.is_empty(), "the chosen cut resolves store uncertainty first");
        assert!(self.pending.is_empty(), "old API terminals are drained at this cut");
        let mut fresh = if self.owner_gate {
            Self::configured_brief(
                with_read,
                with_change,
                false,
                0,
                paths::Gate { owner: true, ..paths::Gate::default() },
                None,
                Some(self.env.limits.brief),
            )
        } else {
            Self::configured_brief(
                with_read,
                with_change,
                false,
                1000,
                paths::Gate::default(),
                None,
                Some(self.env.limits.brief),
            )
        };
        std::mem::swap(&mut self.root, &mut fresh.root);
        std::mem::swap(&mut self.out, &mut fresh.out);
        self.events.clear();
        self.answers.clear();
        self.adopted.clear();
        self.period = None;
        self.events.push_back(engine::Event::Restart);
        if !self.assigned.is_empty() {
            self.events.push_back(protocol::hello_slots(
                Token::new(7),
                if with_change { 2 } else { 1 },
                self.assigned
                    .iter()
                    .map(|assignment| {
                        protocol::hosting(assignment.task, assignment.attempt, jig_domain_host::Phase::Active)
                    })
                    .collect(),
            ));
        }
    }

    pub fn external(&mut self, user: u64, op: raw::Op) -> raw::Answer {
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
    #[expect(clippy::too_many_lines, reason = "one exhaustive root output table")]
    pub fn collect(&mut self) {
        for _ in 0..self.out.len() {
            let request = self.out.pop().expect("counted root output");
            let Some(request) = self.stored_read(request) else { continue };
            let request = match self.protocol.play(request) {
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
                    if let Some(reads) = &mut self.held_reads
                        && matches!(op, skein_forge::api::Op::Read(_))
                        && self.store.rows.values().any(|row| matches!(row,
                            Record::Forge { row, .. } if matches!(row.as_ref(), forge_top::Stored::Change(row)
                                if matches!(row.change.state, temper_engine_domain_forge_change::State::Queued { .. } | temper_engine_domain_forge_change::State::First { .. }))))
                    {
                        reads.push(engine::Request::Forge { call, repository, op });
                        continue;
                    }
                    if matches!(op, skein_forge::api::Op::Write(skein_forge::api::Write::Merge { .. })) {
                        self.merges += 1;
                    }
                    self.forge_calls += 1;
                    if self.fail_job_reads
                        && matches!(op, skein_forge::api::Op::Read(skein_forge::api::Read::Job { .. }))
                    {
                        self.events.push_back(engine::Event::Forge(Box::new(
                            temper_engine_domain_forge::Event::Client(skein_forge_client::Event::Answered {
                                free: 0,
                                rate: None,
                                call,
                                cost: 0,
                                result: Err(skein_forge::api::Error::Refused),
                            }),
                        )));
                        continue;
                    }
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
                    let slow = (self.slow_brief_reads
                        && matches!(op, skein_forge::api::Op::Read(skein_forge::api::Read::Job { .. })))
                        || matches!((&op, &self.slow_branch),
                            (skein_forge::api::Op::Read(skein_forge::api::Read::Branch { branch }), Some(slow))
                                if branch == slow);
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
                engine::Request::Worker(engine::WorkerRequest::Deliver(delivery))
                | engine::Request::Party(engine::PartyRequest::Deliver(delivery)) => match delivery {
                    Delivery::WebReply { reply: people::Reply::SignedIn { sign_in, .. }, .. } => {
                        self.signed_in = Some(sign_in);
                    }
                    Delivery::Assigned { assignment, .. } => self.assigned.push(assignment),
                    Delivery::Read { reply, answer } => self.reads.push((reply, answer)),
                    // The run has its answer once the hub relays it down; the
                    // world renders an answer as its first call's name, and a
                    // replayed call carries that same text.
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
                            panic!("retained adoption answer")
                        };
                        self.adopted.push(outcome);
                    }
                    _ => {}
                },
                engine::Request::Account(_)
                | engine::Request::StopGroup { .. }
                | engine::Request::SetUpGroup { .. }
                | engine::Request::Connect { .. }
                | engine::Request::Manifest { .. }
                | engine::Request::Reachable { .. }
                | engine::Request::Install { .. }
                | engine::Request::EraseCarried { .. }
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

    pub fn tick(&mut self) {
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
        if self.period.is_none() && self.root.ready() {
            self.period = Some(1);
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

    pub fn until(&mut self, target: Until) {
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
                    match self.store.rows.get(&Key::Core(jig_domain_core::Key::Tasks(tasks::Key::Live(task)))) {
                        Some(Record::Core(jig_domain_core::Record::Tasks(tasks::Stored::Live(row)))) => {
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
            self.store.rows.get(&Key::Core(jig_domain_core::Key::Tasks(tasks::Key::Live(2)))),
            self.store.rows.get(&Key::Core(jig_domain_core::Key::Tasks(tasks::Key::Live(3)))),
            self.store.rows.values().filter(|row| matches!(row, Record::Forge { .. })).collect::<Vec<_>>()
        );
    }

    pub fn adopt(&mut self) {
        self.adopt_with_checks(true, Box::new([]));
    }

    pub fn sign_in(&mut self) {
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
    }

    pub fn adopt_with_checks(&mut self, ci: bool, checks: Box<[u32]>) {
        self.sign_in();
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
                ci,
                checks,
            })),
        }));
        self.until(Until::Adopted);
        assert!(matches!(self.adopted.as_slice(), [forge_top::ApplicationOutcome::Adopted(_)]));
    }
}

#[must_use]
pub fn change_world(
    silent_ci: bool,
    passes: u32,
    approval: Option<engine::Freshness>,
) -> (World, engine::Assignment, engine::Assignment, Box<[u8]>) {
    change_world_with_gate(silent_ci, passes, approval, false)
}

#[must_use]
pub fn change_world_with_gate(
    silent_ci: bool,
    passes: u32,
    approval: Option<engine::Freshness>,
    agent_gate: bool,
) -> (World, engine::Assignment, engine::Assignment, Box<[u8]>) {
    change_world_with_policy(silent_ci, passes, approval, agent_gate, false, false, false)
}

#[expect(clippy::fn_params_excessive_bools, reason = "independent CI, gate and policy settings select test worlds")]
#[must_use]
pub fn change_world_with_policy(
    silent_ci: bool,
    passes: u32,
    approval: Option<engine::Freshness>,
    agent_gate: bool,
    no_ci: bool,
    owner_gate: bool,
    tracked: bool,
) -> (World, engine::Assignment, engine::Assignment, Box<[u8]>) {
    let world = World::configured_policy(
        true,
        true,
        silent_ci,
        passes,
        paths::Gate { approval, agent: agent_gate, owner: owner_gate },
        None,
    );
    change_world_in(world, paths::Change { no_ci, owner_gate, tracked })
}

#[expect(clippy::too_many_lines, reason = "the root change fixture builds both producer and review routes")]
#[must_use]
pub fn change_world_in(
    mut world: World,
    policy: paths::Change,
) -> (World, engine::Assignment, engine::Assignment, Box<[u8]>) {
    let paths::Change { no_ci, owner_gate, tracked } = policy;
    if no_ci {
        world.adopt_with_checks(false, Box::new([1]));
    } else {
        world.adopt();
    }
    if owner_gate {
        let branch = people::Pattern {
            segments: Box::new([
                Box::from(&b"forge"[..]),
                Box::from(&b"forge.example"[..]),
                Box::from(&b"org"[..]),
                Box::from(&b"repo"[..]),
                Box::from(&b"branch"[..]),
            ]),
            last: people::Last::Exact(Box::from(&b"main"[..])),
        };
        world.send(engine::Event::Party(engine::PartyInput::Ask {
            models: Box::new([]),
            reply_to: ReplyTo::new(Token::new(900)),
            sign_in: world.signed_in.expect("owner session"),
            key: [90; 16],
            ask: people::Ask::ChangePolicy {
                project: 1,
                change: people::PolicyChange::Requirements {
                    requirements: Box::new([
                        people::Requirement {
                            connector: 0,
                            kind: 4,
                            pattern: branch.clone(),
                            judge: people::Judge { connector: 0, requirement: 1, parameters: 0x8000_0000 },
                            guard: people::Guard::Guarded,
                            must_be_guarded: true,
                        },
                        people::Requirement {
                            connector: 0,
                            kind: 4,
                            pattern: branch.clone(),
                            judge: people::Judge { connector: 0, requirement: 2, parameters: 0x8000_0001 },
                            guard: people::Guard::Observed { freshness: Duration::ZERO },
                            must_be_guarded: false,
                        },
                        people::Requirement {
                            connector: 0,
                            kind: 4,
                            pattern: branch,
                            judge: people::Judge { connector: 0, requirement: 3, parameters: 0x8000_0002 },
                            guard: people::Guard::Guarded,
                            must_be_guarded: true,
                        },
                    ]),
                },
            },
        }));
        world.until(Until::Ready);
        assert!(
            world.store.rows.contains_key(&Key::Core(jig_domain_core::Key::People(people::Key::Policy(1)))),
            "owner policy committed"
        );
        world.restart(true, true);
        world.until(Until::Ready);
    }
    world.send(protocol::hello_slots(Token::new(7), 2, Box::new([])));
    let ask = if tracked {
        walking::goal_from(
            world.chat(1, b"make the fix"),
            people::TaskExecutor::Agent { charter: 1 },
            jig_core_world::fixtures::Goal { budget: 100, priority: 3, deadline: None },
        )
    } else {
        world.chat(1, &b"make the fix"[..])
    };
    world.send(engine::Event::Party(engine::PartyInput::Ask {
        models: Box::new([]),
        reply_to: ReplyTo::new(Token::new(92)),
        sign_in: world.signed_in.expect("owner session"),
        key: [92; 16],
        ask,
    }));
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
        tools: tasks::Tools(1023),
        grants: [3, 4, 10, 11, 12].map(|kind| tasks::Grant { kind, ..grant.clone() }).into(),
        delegation: tasks::Delegation { kinds: Box::new([tasks::AuthorityExecutor::Charter(1)]), tasks: 4, depth: 1 },
        // Each run's spend must fund its finishing reserve, six with the
        // world's charter (jig a020bf1c): room for five attempts of five.
        budget: tasks::Budget { spend: 40, deadline: None },
        notes: tasks::Scopes(0),
        note_resources: Box::new([]),
        models: tasks::Models::NONE,
    };
    let event = world.protocol.call(
        Token::new(7),
        chat.task,
        chat.attempt,
        93,
        engine::Call {
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
                                resource: temper_engine_domain_forge::tag(
                                    forge_world::REPO.forge,
                                    forge_world::REPO.repository,
                                ),
                            },
                            tasks::Parameter::Bytes { name: 2, value: Box::from(&b"main"[..]) },
                            tasks::Parameter::Bytes { name: 6, value: Box::from(&b"file"[..]) },
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
                    ..temper_engine_domain_world::fixtures::report_delegate(b"fixture", Box::new([]))
                }]),
            },
        },
    );
    world.send(event);
    world.until(Until::Delegated);
    world.until(Until::SecondAssignment);
    let producer = world.assigned[1].clone();
    let change = world
        .answers
        .iter()
        .find_map(|answer| match answer {
            temper_engine_domain::CallAnswer::Delegated(numbers) => numbers.first().map(|made| made.task),
            _ => None,
        })
        .expect("change task number");
    assert!(
        world.store.rows.values().any(|stored| matches!(stored,
            Record::Core(jig_domain_core::Record::Tasks(tasks::Stored::Live(task)))
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

#[derive(Clone, Copy, Default)]
pub struct Gate {
    pub approval: Option<engine::Freshness>,
    pub agent: bool,
    pub owner: bool,
}

#[derive(Clone, Copy, Default)]
pub struct Change {
    pub no_ci: bool,
    pub owner_gate: bool,
    pub tracked: bool,
}

pub fn add_reader(policy: &mut authority::Policy) {
    let mut reader = policy.roles[0].clone();
    reader.number = people::Role::Observer.number();
    reader.authority.grants = Box::new([]);
    reader.requests = authority::Requests(0);
    reader.decides = authority::Proposals(0);
    policy.roles = Box::new([policy.roles[0].clone(), reader]);
}

impl World {
    pub(super) fn stored_read(&mut self, request: engine::Request) -> Option<engine::Request> {
        if let engine::Request::Store(engine::StoreRequest::Read { load, range }) = request {
            let result = self.store.read(range);
            self.immediate.push_back(engine::Event::Store(engine::StoreInput::ReadLoaded { load, result }));
            None
        } else {
            Some(request)
        }
    }
}

pub(super) fn document_limits(
    mut documents: documents::DocumentLimits,
    limits: &engine::Limits,
) -> documents::DocumentLimits {
    // jig-protocol-config's overrides::read derives this after all name overrides.
    documents.segments = limits.authority.segments.max(limits.tasks.hold_segments).max(limits.tasks.authority_segments);
    documents.grants = limits.authority.grants.max(limits.tasks.authority_grants);
    documents.executors = limits.authority.executors.max(limits.tasks.executor_kinds);
    documents.implications = limits.authority.implications;
    documents
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

#[must_use]
pub fn failed_ci_with_delayed_brief() -> (World, u64, u64) {
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
    let event = world.protocol.answer(
        Token::new(7),
        producer.task,
        producer.attempt,
        5,
        tasks::End::Finished {
            result: tasks::TaskResult::Change {
                connector: 0,
                kind: 2,
                resource: temper_engine_domain_forge::tag(forge_world::REPO.forge, forge_world::REPO.repository),
                words: Box::from(&b"pushed"[..]),
            },
            cancel_delegates: false,
        },
        engine::Parts::none(),
    );
    world.send(event);
    (world, chat.task, producer.task)
}
