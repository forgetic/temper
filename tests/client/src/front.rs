//! Real engine root on its direct store driver, with jig's protocol correlation.
pub mod account_acts;
use crate::app::Event;
use crate::{app::Temper, fixtures, translate};
use jig_client_world::{
    harness::{ClientId, Front, FrontFault, Observed},
    joint::{
        packets,
        transport::{Transport, Watched},
    },
};
use skein_lib::{Duration, RequestEnvelope, Time, Token, Wall};
use smith_domain_session::llm::Block;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use temper_client_domain::{self as client, Event as RootEvent, Request};
use temper_engine_domain as engine;
use temper_engine_domain_world::{direct::Driver, store::Store, walking};

/// The common client Front backed by temper's own engine root.
#[expect(clippy::struct_excessive_bools, reason = "independent fixture switches keep existing worlds unchanged")]
pub struct Joint {
    pub engine: Driver,
    pub sign_ins: u32,
    pub leaves: Vec<Box<[u8]>>,
    pub refuse_return: bool,
    set_ups: Vec<(u16, Token)>,
    pub admin_connects: Vec<(u16, Token)>,
    pub demo: Option<temper_client_protocol_wire::v1::DemoAnswer>,
    pub demo_starts: Vec<skein_lib::RequestKey>,
    pub demo_removals: Vec<skein_lib::RequestKey>,
    pub demo_reads: usize,
    pub reachable_reads: Vec<(u16, Option<u64>, u32)>,
    pub reachable_empty: bool,
    pub installs: Vec<(u16, Token)>,
    pub demo_outcomes: VecDeque<temper_client_protocol_wire::v1::Outcome>,
    pub answer_runs: bool,
    pub reply_runs: bool,
    pub runs_answered: u32,
    reply_pieces: Option<&'static [&'static [u8]]>,
    held_reply: Option<HeldReply>,
    pub park_runs: bool,
    settles_with_a_deadline_armed: bool,
    keyed_fault: Option<RequestEnvelope>,
    reads: usize,
    read_loads: BTreeSet<Token>,
    transport: Transport,
    limits: client::Limits,
    client_limits: BTreeMap<ClientId, client::Limits>,
    next_limits: Option<client::Limits>,
    callbacks: Vec<(ClientId, Event)>,
    parked: BTreeMap<ClientId, Vec<Request>>,
    offline_until: Option<Time>,
    due: Option<Time>,
    observed: Vec<Observed>,
    lose_answers: u32,
    lost_answer: Option<(ClientId, Token)>,
    hosting: BTreeMap<u64, u64>,
    seed: u64,
    now: Time,
    last_commit: u64,
    sign_outs: BTreeMap<ClientId, VecDeque<Option<u64>>>,
    truth: jig_client_world::joint::truth::Truth,
}
struct HeldReply {
    task: u64,
    attempt: u64,
    read: Option<u64>,
    pieces: &'static [&'static [u8]],
    next: usize,
    words: Vec<u8>,
}
fn engine_limits() -> engine::Limits {
    let mut limits = temper_engine_domain_world::fixtures::forge_limits();
    // The world's own bound, not production's: automatic Seen takes a key per arrival.
    limits.people.requests = 5;
    // A goal's driver plus two clients, as a fresh client at an address beside the person's.
    limits.people.sign_ins = 3;
    let tools = temper_client_tools::limits::client().client.domain;
    // Frame project allocations must also fit the smaller tools project bound.
    limits.documents.lines = limits.documents.lines.min(tools.projects);
    limits
}
/// The first-run stories need the seeded project and one made at runtime.
/// Use the real `MakeProject` path, with a configured policy and room for its owner.
pub fn first_run_projects(config: &mut engine::Config, limits: &mut engine::Limits) {
    limits.authority.projects = 2;
    limits.people.projects = 3; // The deployment roster and two projects.
    limits.people.holdings = 3;
    let seed = &config.core.seed_projects[0].policy;
    let policy = jig_domain_core::ConfiguredPolicy {
        policy: seed.policy.clone(),
        // The seed's permission probes name unoffered roles; a new project's
        // owner needs no such bindings. Settings check all policy references.
        permissions: seed
            .permissions
            .iter()
            .filter(|permission| seed.policy.roles.iter().any(|role| role.number == permission.role))
            .copied()
            .collect(),
        templates: seed.templates.clone(),
    };
    config.core.defaults.policies.insert(1, policy).expect("first-run policy");
}
impl Joint {
    /// The GitHub first run: real project/adoption engine, fixed reachable page.
    #[must_use]
    pub fn github_fixture(seed: u64) -> (Self, crate::app::Root) {
        Self::github_fixture_with(seed, true, false)
    }
    /// GitHub at the setup boundary: optionally uninstalled, or with no repositories.
    #[must_use]
    pub fn github_fixture_with(seed: u64, installed: bool, empty: bool) -> (Self, crate::app::Root) {
        use temper_engine_domain_forge as forge;
        let limits = fixtures::domain_limits();
        let [(bootstrap, engine_limits), (config, _)] = std::array::from_fn(|_| {
            let mut config = walking::config(seed);
            let mut engine_limits = engine_limits();
            first_run_projects(&mut config, &mut engine_limits);
            config.core.connectors = Box::new([fixtures::FORGE]);
            config.forges = Box::new([forge::Configured {
                admin_connect: false,
                bot: Box::new([]),
                owned: None,
                number: 1,
                kind: forge::Kind::GitHub,
                mode: forge::Mode::Connected,
                web: Some(b"https://github.test".as_slice().into()),
                read_only: false,
                asked: false,
            }]);
            (config, engine_limits)
        });
        let mut front = Self::with_engine(seed, &limits, Store::new(), [bootstrap, config], &engine_limits, 1);
        front.reachable_empty = empty;
        front.engine.forge = Some(temper_engine_domain_world::forge_stand::ForgeStand::with(&[
            (b"acme", b"shop", 7),
            (b"acme", b"second", 8),
        ]));
        front.engine.send(engine::Event::Forge(Box::new(forge::Event::Connection {
            forge: 1,
            state: forge::Connection::Connected { id: 1, login: b"app[bot]".as_slice().into() },
        })));
        if installed {
            front.engine.send(engine::Event::Forge(Box::new(forge::Event::Installation {
                forge: 1,
                installation: 1,
                state: Some(forge::Installation {
                    account: Some((1, b"acme".as_slice().into())),
                    stray: false,
                    suspended: false,
                }),
            })));
        }
        front.engine.settle();
        let who = ClientId(0);
        front.bind(who, fixtures::PARTY);
        let mut root = crate::app::Root::Person(Box::new(client::unlinked(&limits, seed)));
        account_acts::event(
            &mut front,
            &mut root,
            <Temper as jig_client_world::harness::App>::start(
                client::Address::ConnectForge { forge: Some(client::Choice::Forge(1)) },
                Vec::new(),
                0,
            ),
        );
        (front, root)
    }
    /// The next chat run's reply, written in these pieces: the first is sent at the
    /// run's assignment and each later part at one `release_reply` (testing.md 6.3).
    pub fn reply_in_pieces(&mut self, pieces: &'static [&'static [u8]]) {
        assert!(self.reply_pieces.is_none() && self.held_reply.is_none(), "one scripted reply at a time");
        assert!(!pieces.is_empty() && pieces.iter().all(|piece| !piece.is_empty()), "a reply has nonempty pieces");
        self.reply_pieces = Some(pieces);
    }

    /// Send the held reply's next part: its next piece, or, once every piece is sent,
    /// the committed turn with the whole text and the run's answer. False when no reply
    /// is held.
    pub fn release_reply(&mut self) -> bool {
        let Some(mut reply) = self.held_reply.take() else { return false };
        if let Some(bytes) = reply.pieces.get(reply.next) {
            self.engine.send(temper_engine_domain_world::protocol::up(
                Token::new(7),
                jig_domain_host::LinkUp::Piece {
                    run: Token::new(reply.task),
                    attempt: Token::new(reply.attempt),
                    piece: jig_domain_host::Piece {
                        turn: 1,
                        retry: 0,
                        kind: jig_domain_host::LiveKind::Text,
                        offset: u64::try_from(reply.words.len()).expect("scripted reply length fits"),
                        bytes: (*bytes).into(),
                    },
                },
                engine::Parts::none(),
            ));
            reply.words.extend_from_slice(bytes);
            reply.next += 1;
            self.held_reply = Some(reply);
        } else {
            self.finish_reply(reply.task, reply.attempt, reply.read, Some(&reply.words));
        }
        self.settle();
        true
    }

    fn finish_reply(&mut self, task: u64, attempt: u64, read: Option<u64>, words: Option<&[u8]>) {
        self.runs_answered += 1;
        if let Some(words) = words {
            let mut turn = temper_worker_domain_world::protocol::session_turn(1, 1, words.len(), true);
            turn.messages[0].content = Box::new([Block::Text { text: words.into(), replay: None }]);
            let body = smith_protocol_channel::encode_turn(
                &turn,
                &smith_transcript::CEILINGS,
                &temper_worker_domain_world::protocol::endpoints(),
            )
            .expect("scripted reply encodes");
            self.engine.send(temper_engine_domain_world::protocol::turn(
                Token::new(7),
                task,
                attempt,
                1,
                1,
                read,
                &body,
            ));
            self.engine.settle();
        }
        self.engine.answer(
            Token::new(7),
            task,
            attempt,
            u64::from(words.is_some()),
            jig_domain_tasks::End::Finished {
                result: jig_domain_tasks::TaskResult::Report { words: b"Answered by the run".as_slice().into() },
                cancel_delegates: false,
            },
            engine::Parts::none(),
        );
    }

    #[must_use]
    pub fn set_ups(&self) -> &[(u16, Token)] {
        &self.set_ups
    }
    #[must_use]
    pub fn answers_to_lose(&self) -> u32 {
        self.lose_answers
    }
    #[must_use]
    pub fn engine_adopted(&self, forge: u16, repository: u32) -> bool {
        use temper_engine_domain_forge::{Key, Role, Stored};
        let key = engine::Key::Forge(Key::Repository(skein_forge::api::Repository { forge, repository }));
        let Some(engine::Record::Forge { row }) = self.engine.store.rows.get(&key) else { return false };
        matches!(row.as_ref(), Stored::Repository(value) if value.role == Role::Owned)
    }
    #[must_use]
    pub fn applications_admitted(&self) -> usize {
        self.engine
            .store
            .rows
            .values()
            .filter(|record| {
                matches!(record,
                    engine::Record::Core(jig_domain_core::Record::People(jig_domain_people::Stored::Answer { ask, .. }))
                    if matches!(ask.as_ref(), jig_domain_people::Ask::Application { .. })
                )
            })
            .count()
    }
    #[must_use]
    pub fn new(seed: u64, limits: client::Limits) -> Joint {
        Self::with_engine(
            seed,
            &limits,
            Store::new(),
            [walking::config(seed), walking::config(seed)],
            &engine_limits(),
            1,
        )
    }
    /// Chats end only when their requester closes them.
    #[must_use]
    pub fn open_chats(seed: u64, limits: client::Limits) -> Joint {
        Self::open_chats_with_program(seed, limits, false)
    }
    /// This host can only run the chat's model through a program.
    #[must_use]
    pub fn open_chats_on_a_program(seed: u64, limits: client::Limits) -> Joint {
        Self::open_chats_with_program(seed, limits, true)
    }
    fn open_chats_with_program(seed: u64, limits: client::Limits, program: bool) -> Joint {
        let configs = std::array::from_fn(|_| {
            let mut config = walking::config(seed);
            config.core.defaults.templates =
                temper_engine_domain_world::fixtures::templates_ending(jig_domain_tasks::EndedBy::Requester);
            config
        });
        Self::with_engine_on_program(seed, &limits, Store::new(), configs, &engine_limits(), 1, program)
    }
    /// The small profile carries a whole turn while an activity page can size it.
    #[must_use]
    pub fn with_large_turns(seed: u64, limits: &client::Limits) -> Self {
        let profile = temper_service::limits::profile(temper_service::limits::Profile::Small).root;
        Self::with_turn_bytes(seed, limits, profile.journal.transcript_bytes, profile.loads.read_bytes)
    }
    /// A committed turn past jig small's cap, with a configured whole-read allowance.
    #[must_use]
    pub fn with_long_turns(seed: u64, limits: &client::Limits) -> Self {
        Self::with_turn_bytes(seed, limits, 131_072, 262_144)
    }
    fn with_turn_bytes(seed: u64, limits: &client::Limits, bytes: u32, read_bytes: u64) -> Self {
        let mut engine_limits = engine_limits();
        engine_limits.journal.transcript_bytes = bytes;
        engine_limits.loads.bytes = bytes;
        engine_limits.loads.reply_bytes = bytes;
        engine_limits.loads.transcript_bytes = bytes;
        engine_limits.loads.read_bytes = read_bytes;
        Self::with_engine(
            seed,
            limits,
            Store::with_transcript_bytes(bytes),
            [walking::config(seed), walking::config(seed)],
            &engine_limits,
            1,
        )
    }
    #[must_use]
    pub fn with_goal_proposals(seed: u64, limits: client::Limits) -> Self {
        let [(bootstrap, engine_limits), (config, _)] = std::array::from_fn(|_| {
            let mut config = walking::config(seed);
            let rules = config.core.defaults.rules.clone();
            let mut policy = config.core.seed_projects[0].policy.policy.clone();
            policy.roles[0].authority.budget.spend = 150;
            let mut domain =
                jig_domain_authority::Domain::new(rules, temper_engine_domain_world::walking::limits().authority)
                    .expect("goal authority");
            let mut out = skein_lib::Queue::with_capacity(jig_domain_authority::POLICY_MAX_OUT);
            jig_domain_authority::step(
                &mut domain,
                jig_domain_authority::Event::Policy { project: 1, policy },
                &mut out,
            );
            assert_eq!(out.pop(), Some(jig_domain_authority::PolicyFact::Added { project: 1 }));
            config.core.seed_projects[0].policy.policy = domain.policy(1).expect("configured seed policy").clone();
            config.core.defaults.rules = domain.rules().clone();
            let mut engine_limits = engine_limits();
            engine_limits.fleet.slots = 2;
            engine_limits.fleet.attempts = 4;
            engine_limits.journal.writes = 4096;
            engine_limits.journal.deliveries = 64;
            engine_limits.journal.held = 192;
            (config, engine_limits)
        });
        Self::with_engine(seed, &limits, Store::new(), [bootstrap, config], &engine_limits, 2)
    }
    pub(crate) fn with_questions(seed: u64, limits: &client::Limits) -> Self {
        use jig_domain_authority as authority;
        let [(bootstrap, engine_limits), (config, _)] = std::array::from_fn(|_| {
            let mut config = walking::config(seed);
            let mut rules = config.core.defaults.rules.clone();
            rules.ceiling.delegation.depth = 2;
            let mut policy = config.core.seed_projects[0].policy.policy.clone();
            policy.ceiling.delegation.depth = 2;
            policy.roles[0].authority.delegation.depth = 2;
            let mut domain = authority::Domain::new(rules, temper_engine_domain_world::walking::limits().authority)
                .expect("question authority");
            let mut out = skein_lib::Queue::with_capacity(authority::POLICY_MAX_OUT);
            authority::step(
                &mut domain,
                authority::Event::Policy { project: fixtures::QUESTION_PROJECT, policy },
                &mut out,
            );
            assert_eq!(out.pop(), Some(authority::PolicyFact::Added { project: fixtures::QUESTION_PROJECT }));
            config.core.seed_projects[0].policy.policy = domain.policy(1).expect("configured seed policy").clone();
            config.core.defaults.rules = domain.rules().clone();
            let mut engine_limits = engine_limits();
            engine_limits.tasks.inbox_bytes = 512;
            engine_limits.fleet.slots = 2;
            engine_limits.fleet.attempts = 4;
            engine_limits.journal.writes = 4096;
            engine_limits.journal.deliveries = 64;
            engine_limits.journal.held = 192;
            (config, engine_limits)
        });
        Self::with_engine(seed, limits, Store::new(), [bootstrap, config], &engine_limits, 2)
    }
    /// A goal and its delegated authority fit, with bounds chosen by the story.
    pub(crate) fn with_delegates(seed: u64, limits: &client::Limits, delegates: u32) -> Self {
        use jig_domain_authority as authority;
        let tasks = delegates.checked_add(1).expect("goal plus its delegates");
        let [(bootstrap, engine_limits), (config, _)] = std::array::from_fn(|_| {
            let mut config = walking::config(seed);
            let mut rules = config.core.defaults.rules.clone();
            rules.ceiling.delegation.tasks = tasks;
            rules.ceiling.delegation.depth = 2;
            let mut policy = config.core.seed_projects[0].policy.policy.clone();
            policy.ceiling.delegation.tasks = tasks;
            policy.ceiling.delegation.depth = 2;
            policy.roles[0].authority.delegation.tasks = tasks;
            policy.roles[0].authority.delegation.depth = 2;
            let mut authority_limits = temper_engine_domain_world::walking::limits().authority;
            authority_limits.batch = delegates;
            let mut domain = authority::Domain::new(rules, authority_limits).expect("delegate authority");
            let mut out = skein_lib::Queue::with_capacity(authority::POLICY_MAX_OUT);
            authority::step(
                &mut domain,
                authority::Event::Policy { project: fixtures::QUESTION_PROJECT, policy },
                &mut out,
            );
            assert_eq!(out.pop(), Some(authority::PolicyFact::Added { project: fixtures::QUESTION_PROJECT }));
            config.core.seed_projects[0].policy.policy = domain.policy(1).expect("configured seed policy").clone();
            config.core.defaults.rules = domain.rules().clone();
            let mut engine_limits = engine_limits();
            engine_limits.tasks.tasks = tasks;
            engine_limits.tasks.project_tasks = tasks;
            engine_limits.tasks.tree_tasks = tasks;
            engine_limits.tasks.delegates = delegates;
            engine_limits.tasks.batch = delegates;
            engine_limits.authority = authority_limits;
            // The core prices two inbox projections for each live task.
            engine_limits.people.inbox_tasks = tasks * 2;
            engine_limits.tasks.inbox_bytes = 512;
            engine_limits.fleet.slots = 2;
            engine_limits.fleet.attempts = 4;
            engine_limits.journal.writes = 4096;
            engine_limits.journal.deliveries = 64;
            engine_limits.journal.held = 192;
            (config, engine_limits)
        });
        Self::with_engine(seed, limits, Store::new(), [bootstrap, config], &engine_limits, 2)
    }
    /// A real forge procedure is admitted by this scenario's policy.
    #[must_use]
    pub(crate) fn with_procedures(seed: u64, limits: &client::Limits) -> Self {
        const SLOTS: u32 = 1;
        let [(bootstrap, engine_limits), (config, _)] = std::array::from_fn(|_| Self::procedure_config(seed));
        Self::with_engine(seed, limits, Store::new(), [bootstrap, config], &engine_limits, SLOTS)
    }
    fn procedure_config(seed: u64) -> (engine::Config, engine::Limits) {
        use jig_domain_authority as a;
        const PROJECT: u32 = 1;
        const PROCEDURE: u32 = 2;
        const EXECUTORS: u32 = 1;
        const POLICY_OUT: u32 = a::POLICY_MAX_OUT;
        let mut config = walking::config(seed);
        config.core.connectors = Box::new([1]);
        for role in &mut config.core.seed_projects[0].policy.permissions {
            role.connector = 1;
        }
        let mut rules = config.core.defaults.rules.clone();
        let mut policy = config.core.seed_projects[0].policy.policy.clone();
        let kinds: Box<[a::Executor]> = Box::new([a::Executor::Procedure(PROCEDURE)]);
        rules.ceiling.delegation.kinds.clone_from(&kinds);
        policy.ceiling.delegation.kinds.clone_from(&kinds);
        policy.roles[0].authority.delegation.kinds = kinds;
        let mut authority_limits = temper_engine_domain_world::walking::limits().authority;
        authority_limits.executors = EXECUTORS;
        let mut domain = a::Domain::new(rules, authority_limits).expect("procedure policy limits");
        let mut out = skein_lib::Queue::with_capacity(POLICY_OUT);
        a::step(&mut domain, a::Event::Policy { project: PROJECT, policy }, &mut out);
        assert_eq!(out.pop(), Some(a::PolicyFact::Added { project: PROJECT }));
        config.core.seed_projects[0].policy.policy = domain.policy(1).expect("configured seed policy").clone();
        config.core.defaults.rules = domain.rules().clone();
        let mut engine_limits = engine_limits();
        engine_limits.authority = authority_limits;
        (config, engine_limits)
    }
    /// The last attempt assigned to the joint's scripted worker, if any.
    #[must_use]
    pub fn hosted_attempt(&self, task: u64) -> Option<u64> {
        self.hosting.get(&task).copied()
    }

    /// Whether the client's change panel still has its application route.
    #[must_use]
    pub fn watching_change(&self, client: ClientId, task: u64) -> bool {
        use jig_client_test_domain::family::Kind;
        use jig_client_world::joint::transport::Watched;
        self.transport.watches().into_iter().any(|watcher| {
            let route = self.transport.watch(watcher).expect("live watch");
            route.client == client
                && matches!(route.subject, Watched::Application { subject, .. } if subject.kind == Kind::Panel(task))
                && self.transport.application_watch_token(client, route.native) == Some(watcher)
        })
    }

    /// client/README.md, 8 and 10: a pushed change lands through CI and an agent gate.
    #[must_use]
    pub fn with_change_landing(seed: u64, limits: &client::Limits) -> Self {
        const SLOTS: u32 = 1;
        use jig_domain_authority as a;
        let [(bootstrap, engine_limits), (config, _)] = std::array::from_fn(|_| {
            let (mut config, mut limits) = Self::procedure_config(seed);
            let connector = config.core.connectors[0];
            // Same agent gate and rule as forge_routes.rs's configured_policy
            // (domain/forge.md, 8.3 and 8.7), on the client world's repository.
            let landing: Box<[engine::LandingRule]> = Box::new([engine::LandingRule {
                connector,
                kind: 4,
                enforce: true,
                pattern: a::Pattern {
                    segments: [b"forge".as_slice(), fixtures::FORGE_HOST, b"acme", b"shop", b"branch"]
                        .map(Box::from)
                        .into(),
                    last: a::Last::Exact(b"main".as_slice().into()),
                },
                ci: true,
                up_to_date: true,
                gates: Box::new([engine::Gate { number: 1, blocking: true, freshness: engine::Freshness::Exact }]),
                approvals: Box::new([]),
            }]);
            config.landing.deployment.clone_from(&landing);
            assert!(config.landing.projects.insert(1, landing).is_ok());
            let mut rules = config.core.defaults.rules.clone();
            let mut policy = config.core.seed_projects[0].policy.policy.clone();
            rules.ceiling.delegation.kinds = Box::new([a::Executor::Charter(1), a::Executor::Procedure(2)]);
            rules.ceiling.delegation.depth = 2;
            // This pushed-branch path needs read, push, open, land and its goal's issue.
            let grants: Box<[a::Grant]> = [1, 2, 3, 4, 8]
                .map(|kind| a::Grant {
                    connector,
                    kind,
                    pattern: a::Pattern {
                        segments: [b"forge".as_slice(), fixtures::FORGE_HOST, b"acme"].map(Box::from).into(),
                        last: a::Last::Open(b"shop".as_slice().into()),
                    },
                })
                .into();
            // As in configured_policy, goal projections can write their issue and comments.
            let projections = [7, 13].map(|kind| a::Grant { kind, ..grants[0].clone() });
            rules.ceiling.grants = grants.iter().cloned().chain(projections).collect();
            policy.ceiling.grants.clone_from(&rules.ceiling.grants);
            policy.projections.clone_from(&rules.ceiling.grants);
            policy.roles[0].authority.grants = grants;
            policy.ceiling.delegation = rules.ceiling.delegation.clone();
            policy.roles[0].authority.delegation = rules.ceiling.delegation.clone();
            let mut maintainer = policy.roles[0].clone();
            maintainer.number = 1;
            let mut member = maintainer.clone();
            member.number = 2;
            policy.roles = Box::new([policy.roles[0].clone(), maintainer, member]);
            limits.authority.roles = 3;
            limits.authority.requirements = 8;
            limits.authority.facts = 8;
            limits.authority.executors = 2;
            limits.authority.grants = 7;
            limits.authority.segments = 8;
            limits.authority.segment_bytes = 64;
            limits.documents.grants = limits.authority.grants;
            limits.documents.executors = limits.authority.executors;
            // Task offers name all three roles; this joint serves the person's page.
            limits.documents.lines = 3;
            limits.tasks.tasks = 3;
            limits.tasks.project_tasks = 3;
            limits.tasks.tree_tasks = 3;
            limits.tasks.depth = 2;
            limits.tasks.parameters = 5;
            // A gate's brief spec includes its branch and hexadecimal head.
            limits.tasks.spec_bytes = 128;
            limits.tasks.contract_choices = 2;
            limits.tasks.executor_kinds = 2;
            limits.tasks.authority_grants = 5;
            limits.tasks.authority_segments = 9;
            limits.tasks.authority_bytes = 512;
            limits.tasks.inbox_bytes = 1024;
            limits.people.inbox_tasks = 6;
            // configured_policy uses jig's Small profile's creation-request bound.
            limits.people.amendment_bytes = 8_192;
            // The core prices deployment, project, task and every grant's note scope.
            limits.notes.scopes = 3 + limits.authority.grants;
            limits.journal.writes = 8192;
            limits.journal.held = 1024;
            limits.journal.deliveries = 256;
            limits.journal.result_bytes = 4096;
            limits.journal.run_bytes = 8192;
            limits.brief.sections = 5;
            limits.brief.brief_bytes = 768;
            limits.brief.read_bytes = 512;
            limits.brief.budgets.pull = 384;
            limits.brief.budgets.ci = 384;
            let mut domain = a::Domain::new(rules, limits.authority).expect("landing authority");
            let mut out = skein_lib::Queue::with_capacity(a::POLICY_MAX_OUT);
            a::step(&mut domain, a::Event::Policy { project: 1, policy }, &mut out);
            assert_eq!(out.pop(), Some(a::PolicyFact::Added { project: 1 }));
            config.core.seed_projects[0].policy.policy = domain.policy(1).expect("configured seed policy").clone();
            config.core.defaults.rules = domain.rules().clone();
            (config, limits)
        });
        Self::with_engine(seed, limits, Store::new(), [bootstrap, config], &engine_limits, SLOTS)
    }
    fn with_engine(
        seed: u64,
        limits: &client::Limits,
        store: Store,
        configs: [engine::Config; 2],
        engine_limits: &engine::Limits,
        slots: u32,
    ) -> Self {
        Self::with_engine_on_program(seed, limits, store, configs, engine_limits, slots, false)
    }
    fn with_engine_on_program(
        seed: u64,
        limits: &client::Limits,
        store: Store,
        configs: [engine::Config; 2],
        engine_limits: &engine::Limits,
        slots: u32,
        program: bool,
    ) -> Self {
        let mut transport = Transport::new(jig_client::contract::VersionRange {
            first: jig_client_protocol_wire::VERSION,
            last: jig_client_protocol_wire::VERSION,
        });
        transport.models.clone_from(&configs[1].core.settings.models);
        transport.providers = configs[1].core.settings.llm_providers.clone();
        let worker_models = configs[1]
            .core
            .settings
            .models
            .iter()
            .map(|model| jig_domain_host::ModelBound {
                name: model.name.clone(),
                window: model.window,
                output: model.output,
            })
            .collect();
        let mut hello =
            temper_engine_domain_world::protocol::hello_slots(fixtures::QUESTION_WORKER, slots, Box::new([]));
        let engine::Event::Worker(engine::WorkerInput::Link { link: engine::Link::Hello { hello: value, .. }, .. }) =
            &mut hello
        else {
            unreachable!("worker hello")
        };
        value.bounds.models = worker_models;
        if program {
            value.bounds.program_endpoints = Box::new([walking::charter().policy.model.endpoint]);
        }
        let mut engine = Driver::with_repositories(store, configs, engine_limits, fixtures::REPOSITORIES);
        engine.send(hello);
        engine.settle();
        let last_commit = engine.store.header().commits;
        let truth = jig_client_world::joint::truth::Truth::after(core_rows(&engine));
        engine.delivered.clear();
        engine.immediate.clear();
        Joint {
            engine,
            sign_ins: 0,
            leaves: Vec::new(),
            refuse_return: false,
            set_ups: Vec::new(),
            admin_connects: Vec::new(),
            demo: None,
            demo_starts: Vec::new(),
            demo_removals: Vec::new(),
            demo_reads: 0,
            reachable_reads: Vec::new(),
            reachable_empty: false,
            installs: Vec::new(),
            demo_outcomes: VecDeque::new(),
            answer_runs: false,
            reply_runs: false,
            runs_answered: 0,
            reply_pieces: None,
            held_reply: None,
            park_runs: false,
            settles_with_a_deadline_armed: program,
            keyed_fault: None,
            reads: 0,
            read_loads: BTreeSet::new(),
            transport,
            limits: *limits,
            client_limits: BTreeMap::new(),
            next_limits: None,
            callbacks: Vec::new(),
            parked: BTreeMap::new(),
            offline_until: None,
            due: None,
            observed: vec![Observed::Reachable { live: true }],
            lose_answers: 0,
            lost_answer: None,
            hosting: BTreeMap::new(),
            seed,
            now: Time::ZERO,
            last_commit,
            truth,
            sign_outs: BTreeMap::new(),
        }
    }
    pub fn next_client_limits(&mut self, limits: client::Limits) {
        self.next_limits = Some(limits);
    }
    fn hello(&mut self, who: ClientId) {
        self.callbacks.push((
            who,
            translate::callback(jig_client_test_domain::Event::Client {
                event: jig_client::Event::Up { up: self.transport.hello() },
            })
            .into(),
        ));
        self.callbacks.push((
            who,
            RootEvent::Client {
                event: jig_client::Event::Up {
                    up: jig_client::Up::FamilyServed {
                        family: fixtures::FORGE,
                        versions: Some(jig_client::contract::VersionRange {
                            first: temper_client_protocol_wire::CLIENT_VERSION,
                            last: temper_client_protocol_wire::CLIENT_VERSION,
                        }),
                    },
                },
            }
            .into(),
        ));
    }
    fn advance(&mut self, now: Time) {
        self.now = now;
        self.engine.env.now = now;
        self.engine.env.wall = Wall::from_nanos(now.as_nanos());
        if let Some(deadline) = self.engine.root.next_deadline(&self.engine.env)
            && deadline <= now
        {
            engine::fire(&mut self.engine.root, &self.engine.env);
        }
    }
    fn send(&mut self, mut event: engine::Event) {
        // The deployment front serves a window within the engine's configured page bound.
        if let engine::Event::Party(engine::PartyInput::Watch { window, .. }) = &mut event {
            window.rows = window.rows.min(self.engine.env.limits.documents.rows);
        }
        if matches!(event, engine::Event::Party(engine::PartyInput::SignedIn { .. })) {
            self.sign_ins += 1;
        }
        self.engine.send(event);
    }
    fn reachable(&mut self, who: ClientId, request: &Request) -> bool {
        use temper_client_protocol_wire::{self as wire, v1 as w};
        let Request::Client {
            request:
                jig_client::Request::Down {
                    down: jig_client::Down::Send { attempt, request: jig_client::Ask::Family { record, .. }, .. },
                },
        } = request
        else {
            return false;
        };
        if record.family != wire::FORGE {
            return false;
        }
        let Some(request) = wire::decode_requesting(&record.bytes, &w::CEILINGS) else { return false };
        let w::Request::Reachable(read) = request.request() else { return false };
        self.reachable_reads.push((read.forge(), *read.installation(), read.page()));
        let mut rows = skein_lib::List::with_capacity(2);
        for (id, name) in [(7, b"shop".as_slice()), (8, b"second".as_slice())] {
            if self.reachable_empty {
                break;
            }
            rows.push(
                w::Reached::new(
                    &w::CEILINGS,
                    w::ReachedParts {
                        id,
                        owner: w::Account::new(
                            &w::CEILINGS,
                            w::AccountParts { id: 1, login: b"acme".as_slice().into() },
                        )
                        .expect("fixed owner"),
                        name: name.into(),
                        private: id == 8,
                    },
                )
                .expect("fixed row"),
            )
            .expect("fixed page");
        }
        let outcome =
            w::Outcome::Page(w::Page::new(&w::CEILINGS, w::PageParts { rows, more: false }).expect("fixed page"));
        self.callbacks.push((
            who,
            RootEvent::Client {
                event: jig_client::Event::Up {
                    up: jig_client::Up::Answer {
                        attempt: *attempt,
                        envelope: RequestEnvelope::Final,
                        why: None,
                        answer: Some(jig_client::Answered::Family(jig_client::Family {
                            family: wire::FORGE,
                            version: wire::CLIENT_VERSION,
                            bytes: wire::answer(
                                &w::Answer::new(&w::CEILINGS, w::AnswerParts { outcome }).expect("fixed answer"),
                            ),
                        })),
                    },
                },
            }
            .into(),
        ));
        true
    }
    fn start_demo(&mut self, who: ClientId, request: &Request) -> bool {
        // StartDemo is scripted here until the parallel engine task lands.
        if let Request::Client {
            request:
                jig_client::Request::Down {
                    down: jig_client::Down::Send { attempt, key, request: jig_client::Ask::Family { record, .. }, .. },
                },
        } = request
        {
            use temper_client_protocol_wire::{self as wire, v1 as w};
            if record.family == wire::FORGE
                && wire::decode_requesting(&record.bytes, &w::CEILINGS)
                    .is_some_and(|request| *request.request() == w::Request::StartDemo)
            {
                self.demo_starts.push(*key);
                let outcome = self.demo_outcomes.pop_front().unwrap_or(w::Outcome::Running);
                if outcome == w::Outcome::Running
                    && let Some(demo) = self.demo.take()
                {
                    let mut parts = demo.into_parts();
                    parts.state = w::DemoState::Running;
                    self.demo = Some(w::DemoAnswer::new(&w::CEILINGS, parts).expect("demo fixture"));
                }
                self.callbacks.push((
                    who,
                    RootEvent::Client {
                        event: jig_client::Event::Up {
                            up: jig_client::Up::Answer {
                                attempt: *attempt,
                                envelope: RequestEnvelope::Final,
                                why: None,
                                answer: Some(jig_client::Answered::Family(jig_client::Family {
                                    family: wire::FORGE,
                                    version: wire::CLIENT_VERSION,
                                    bytes: wire::answer(
                                        &w::Answer::new(&w::CEILINGS, w::AnswerParts { outcome })
                                            .expect("scripted start answer"),
                                    ),
                                })),
                            },
                        },
                    }
                    .into(),
                ));
                return true;
            }
        }
        false
    }
    fn refused_return(&mut self, who: ClientId, request: &Request) -> bool {
        if self.refuse_return
            && let Request::Client {
                request:
                    jig_client::Request::Down {
                        down: jig_client::Down::Send { attempt, request: jig_client::Ask::Family { record, .. }, .. },
                    },
            } = request
            && let Some(decoded) = temper_client_protocol_wire::decode_requesting(
                &record.bytes,
                &temper_client_protocol_wire::v1::CEILINGS,
            )
            && matches!(decoded.request(), temper_client_protocol_wire::v1::Request::Return(_))
        {
            self.callbacks.push((
                who,
                RootEvent::Client {
                    event: jig_client::Event::Up {
                        up: jig_client::Up::Answer {
                            attempt: *attempt,
                            envelope: RequestEnvelope::Final,
                            why: None,
                            answer: Some(jig_client::Answered::Jig(jig_client::contract::Answer::Refused {
                                refusal: jig_client::contract::Refusal::Unknown,
                                at: None,
                            })),
                        },
                    },
                }
                .into(),
            ));
            return true;
        }
        false
    }
    fn request(&mut self, who: ClientId, mut request: Request) {
        if self.refused_return(who, &request) {
            return;
        }
        if self.offline_until.is_some() || self.transport.sign_in(who).is_none() {
            if let Request::Client {
                request: jig_client::Request::Down { down: jig_client::Down::Send { attempt, value: Some(_), .. } },
            } = &request
            {
                self.callbacks.push((
                    who,
                    RootEvent::Client {
                        event: jig_client::Event::Up {
                            up: jig_client::Up::Answer {
                                attempt: *attempt,
                                envelope: RequestEnvelope::Again,
                                answer: None,
                                why: None,
                            },
                        },
                    }
                    .into(),
                ));
                return;
            }
            self.parked.entry(who).or_default().push(request);
            return;
        }
        if let Request::Client { request: jig_client::Request::Down { down: jig_client::Down::Send { attempt, .. } } } =
            &request
            && let Some(envelope) = self.keyed_fault.take()
        {
            if envelope == RequestEnvelope::SignedOut {
                self.transport.signed_out(who);
                self.sign_outs.entry(who).or_default().push_back(None);
                self.callbacks.push((
                    who,
                    RootEvent::Client { event: jig_client::Event::Up { up: jig_client::Up::SignedOut } }.into(),
                ));
            }
            self.callbacks.push((
                who,
                RootEvent::Client {
                    event: jig_client::Event::Up {
                        up: jig_client::Up::Answer { attempt: *attempt, envelope, answer: None, why: None },
                    },
                }
                .into(),
            ));
            return;
        }
        if let Request::Client {
            request:
                jig_client::Request::Down { down: jig_client::Down::Send { key, request: jig_client::Ask::Jig(ask), .. } },
        } = &request
            && matches!(
                ask.as_ref(),
                jig_client::contract::Ask::AccountChange {
                    holder: jig_client::contract::AccountHolder::Deployment,
                    change: jig_client::contract::AccountChange::Remove { .. },
                }
            )
        {
            self.demo_removals.push(*key);
        }
        if self.reachable(who, &request) || self.start_demo(who, &request) {
            return;
        }
        if let Request::Client {
            request:
                jig_client::Request::Down {
                    down: jig_client::Down::Read { read, what: jig_client::Readable::Family { record, .. } },
                },
        } = &request
        {
            // This front supplies the demo read independently of the engine.
            self.demo_reads += 1;
            self.callbacks.push((
                who,
                RootEvent::Client {
                    event: jig_client::Event::Up {
                        up: jig_client::Up::ReadDone {
                            read: *read,
                            result: jig_client::Readout::Family {
                                family: record.family,
                                record: self.demo.as_ref().map(temper_client_protocol_wire::demo_answer),
                            },
                        },
                    },
                }
                .into(),
            ));
            return;
        }
        let (events, refusal) =
            translate::request(&mut self.transport, who, &mut request, &self.client_limits[&who].client.domain);
        if let Some(packet) = refusal {
            self.incoming(packet);
        }
        for event in events {
            self.send(event);
        }
    }
    fn demo_forges(&self, event: &mut RootEvent) {
        use temper_client_protocol_wire::{self as wire, v1 as w};
        let Some(demo) = &self.demo else { return };
        let RootEvent::Client { event: jig_client::Event::Up { up: jig_client::Up::FamilyDelivered { record, .. } } } =
            event
        else {
            return;
        };
        let Some(rows) = wire::decode_forges(&record.bytes, &w::CEILINGS) else { return };
        let mut forges = skein_lib::List::with_capacity(wire::FORGES);
        for row in rows.into_parts().forges.into_boxed() {
            forges.push(row).expect("scripted deployment row");
        }
        let connection = if *demo.state() == w::DemoState::Running {
            w::Connection::Connected(
                w::Connected::new(&w::CEILINGS, w::ConnectedParts { id: 2, login: b"playground".as_slice().into() })
                    .expect("fake connection"),
            )
        } else {
            w::Connection::NotConnected
        };
        forges
            .push(
                w::ForgeRow::new(
                    &w::CEILINGS,
                    w::ForgeRowParts {
                        number: demo.forge(),
                        kind: w::Kind::Forgejo,
                        mode: w::Mode::Fake,
                        connection,
                        offer: w::Offer::NotOffered,
                        first: None,
                        installations: skein_lib::List::with_capacity(0),
                        ended: None,
                        app: None,
                        web: None,
                        repositories: skein_lib::List::with_capacity(0),
                        owned: None,
                        admin: false,
                        ended_stop: None,
                    },
                )
                .expect("scripted demo forge"),
            )
            .expect("demo row fits");
        record.bytes = wire::forges(&w::Forges::new(&w::CEILINGS, w::ForgesParts { forges }).expect("scripted forges"));
    }
    fn callback(&mut self, client: ClientId, mut event: RootEvent) {
        self.demo_forges(&mut event);
        if let RootEvent::Client {
            event: jig_client::Event::Up { up: jig_client::Up::Answer { attempt, envelope, answer, why } },
        } = &mut event
            && self.lose_answers != 0
        {
            self.lost_answer = Some((client, *attempt));
            self.lose_answers -= 1;
            *envelope = RequestEnvelope::Lost;
            *answer = None;
            *why = None;
        }
        self.callbacks.push((client, event.into()));
    }
    fn incoming(&mut self, packet: packets::Incoming) {
        match packet {
            packets::Incoming::Authenticated { client, .. } => {
                for request in self.parked.remove(&client).unwrap_or_default() {
                    self.request(client, request);
                }
            }
            packets::Incoming::Callback { client, event } => {
                self.callback(client, translate::callback(event));
            }
            packets::Incoming::SignedOut { client, sign_in } => {
                self.observed.push(Observed::SignInEnded { client });
                self.sign_outs.entry(client).or_default().push_back(Some(sign_in));
                self.callbacks.push((
                    client,
                    Event::Root(RootEvent::Client { event: jig_client::Event::Up { up: jig_client::Up::SignedOut } }),
                ));
            }
            packets::Incoming::SignInAgain { why, .. } => panic!("the deployment owner signs in again: {why:?}"),
            packets::Incoming::SignInFailed { failure, .. } => panic!("the deployment owner signs in: {failure:?}"),
        }
    }
    fn packet(&mut self, request: engine::Request) {
        match translate::forge::incoming(&mut self.transport, request, &self.limits.client.domain) {
            Ok(Some((client, event))) => {
                self.callback(client, event);
            }
            Ok(None) => {}
            Err(request) => {
                let mut who = None;
                if let engine::Request::Party(engine::PartyRequest::Deliver(
                    engine::Delivery::WebReply { to, .. } | engine::Delivery::Read { reply: to, .. },
                )) = request.as_ref()
                {
                    who = self
                        .transport
                        .reply_tokens()
                        .into_iter()
                        .find(|token| *to == skein_lib::ReplyTo::new(*token))
                        .and_then(|token| self.transport.reply(token))
                        .map(|reply| match reply {
                            jig_client_world::joint::transport::Reply::SignIn { client }
                            | jig_client_world::joint::transport::Reply::SignOut { client, .. }
                            | jig_client_world::joint::transport::Reply::Keyed { client, .. }
                            | jig_client_world::joint::transport::Reply::Read { client, .. }
                            | jig_client_world::joint::transport::Reply::ApplicationRead { client, .. } => *client,
                        });
                }
                if let engine::Request::Party(
                    engine::PartyRequest::Deliver(
                        engine::Delivery::Document { watcher, .. }
                        | engine::Delivery::WatchEnded { watcher, .. }
                        | engine::Delivery::Panel { watcher, .. },
                    )
                    | engine::PartyRequest::WatchRefused { watcher, .. }
                    | engine::PartyRequest::RunText { watcher, .. },
                ) = request.as_ref()
                {
                    who = self.transport.watch(*watcher).map(|watch| watch.client);
                }
                let limits = who.map_or(self.limits, |who| self.client_limits[&who]);
                let codec =
                    if limits == self.limits { fixtures::codec() } else { fixtures::codec_for(&limits.client.domain) };
                if let Some(packet) = translate::incoming(&mut self.transport, *request, &limits.client.domain, &codec)
                {
                    self.incoming(packet);
                }
            }
        }
    }
    fn settle(&mut self) {
        for step in 0..200 {
            for event in &mut self.engine.events {
                if let engine::Event::Store(engine::StoreInput::ReadLoaded { load, result }) = event
                    && self.read_loads.insert(*load)
                    && let Some(range) = self.engine.protocol.reads.get(self.reads)
                {
                    *result = fixtures::read(&self.engine.store, range);
                    self.reads += 1;
                }
            }
            self.engine.advance(true);
            // The refused Open chat keeps the fleet's placement deadline armed.
            if (self.engine.root.quiescent() || (self.settles_with_a_deadline_armed && step == 199))
                && self.engine.events.is_empty()
                && self.engine.store.pending.is_empty()
                && self.engine.forge.as_ref().is_none_or(temper_engine_domain_world::forge_stand::ForgeStand::quiescent)
            {
                return;
            }
        }
        panic!(
            "joint engine did not settle: quiescent={}; returned={}; events={:?}; pending={:?}",
            self.engine.root.quiescent(),
            self.engine.returned,
            self.engine.events,
            self.engine.store.pending
        );
    }
    fn drain(&mut self) {
        self.settle();
        let commit = self.engine.store.header().commits;
        if commit != self.last_commit {
            self.observed.extend(self.truth.applied(jig_core_world::store_peer::DurableSnapshot {
                commit,
                at: self.now,
                rows: core_rows(&self.engine),
            }));
            self.last_commit = commit;
        }
        let deliveries = std::mem::take(&mut self.engine.delivered);
        for delivery in deliveries {
            if let engine::Delivery::Link { channel, down: jig_domain_host::LinkDown::Cancel { run, attempt } } =
                &delivery
            {
                self.hosting.remove(&run.raw());
                self.engine.answer(
                    *channel,
                    run.raw(),
                    attempt.raw(),
                    0,
                    jig_domain_tasks::End::Parked,
                    engine::Parts::none(),
                );
            }
            if self.park_runs
                && let engine::Delivery::Assigned { assignment, .. } = &delivery
            {
                if let Some(word) = assignment.messages.last() {
                    self.engine.send(temper_engine_domain_world::protocol::turn(
                        Token::new(7),
                        assignment.task,
                        assignment.attempt,
                        1,
                        0,
                        Some(word.number),
                        b"read messages",
                    ));
                    self.engine.settle();
                }
                self.engine.answer(
                    Token::new(7),
                    assignment.task,
                    assignment.attempt,
                    0,
                    jig_domain_tasks::End::Parked,
                    engine::Parts::none(),
                );
            }
            if let engine::Delivery::Assigned { assignment, .. } = &delivery {
                let read = assignment.messages.last().map(|word| word.number);
                if let Some(pieces) = self.reply_pieces.take() {
                    self.held_reply = Some(HeldReply {
                        task: assignment.task,
                        attempt: assignment.attempt,
                        read,
                        pieces,
                        next: 0,
                        words: Vec::new(),
                    });
                    assert!(self.release_reply(), "the assignment holds its first piece");
                } else if self.answer_runs {
                    let words = self.reply_runs.then(|| format!("Answered, run {}", self.runs_answered + 1));
                    self.finish_reply(assignment.task, assignment.attempt, read, words.as_deref().map(str::as_bytes));
                } else {
                    self.hosting.insert(assignment.task, assignment.attempt);
                }
            }
            self.packet(engine::Request::Party(engine::PartyRequest::Deliver(delivery)));
        }
        self.immediate();
    }
    fn immediate(&mut self) {
        let immediate = std::mem::take(&mut self.engine.immediate);
        for request in immediate {
            if let engine::Request::Install { forge, request, .. } = &request {
                self.installs.push((*forge, *request));
            }
            if let engine::Request::Connect {
                forge,
                request: attempt,
                carried,
                how: temper_engine_domain_forge::How::ForgejoAdmin { .. },
            } = &request
            {
                let value = self.transport.carried().get(&carried.number).expect("admin carrier");
                assert!(value.expose().starts_with(&[1, 1, 1, 0]), "item-1 bundle");
                self.admin_connects.push((*forge, *attempt));
                self.engine.send(engine::Event::Forge(Box::new(temper_engine_domain_forge::Event::ConnectAnswered {
                    forge: *forge,
                    request: *attempt,
                    answer: temper_engine_domain_forge::ConnectAnswer::Connecting,
                })));
            }
            if let engine::Request::EraseCarried { number } = &request {
                self.transport.unused(*number);
            }

            if let engine::Request::SetUpGroup { forge, request } | engine::Request::SetUp { forge, request, .. } =
                &request
            {
                self.set_ups.push((*forge, *request));
            }
            self.packet(request);
        }
    }
    fn detach(&mut self, who: ClientId) {
        self.sign_outs.remove(&who);
        self.callbacks.retain(|(client, _)| *client != who);
        self.parked.remove(&who);
        for watcher in self.transport.detach(who) {
            self.send(translate::party(jig_domain_core::Event::Unwatch { watcher }));
        }
    }
    fn drop_watches(&mut self) {
        for watcher in self.transport.watches() {
            let packet = packets::dropped(&mut self.transport, watcher).expect("original protocol watch");
            self.send(translate::party(jig_domain_core::Event::Unwatch { watcher }));
            self.incoming(packet);
        }
    }
}
impl Front<Temper> for Joint {
    fn bind(&mut self, who: ClientId, party: u64) {
        self.client_limits.insert(who, self.next_limits.take().unwrap_or(self.limits));
        assert_eq!(party, fixtures::PARTY, "the configured deployment owner");
        self.transport.bind(
            who,
            party,
            jig_domain_people::Identity {
                key: jig_domain_people::IdentityKey { provider: 0, subject: 7_u64.to_be_bytes().into() },
                login: b"person".as_slice().into(),
                name: b"Person".as_slice().into(),
            },
        );
        self.callbacks.push((
            who,
            Event::Root(translate::callback(jig_client_test_domain::Event::Client {
                event: jig_client::Event::Up { up: self.transport.hello() },
            })),
        ));
        let event = translate::party(self.transport.start_sign_in(who));
        self.send(event);
    }
    fn wire(&mut self, who: ClientId, now: Time, _wall: Wall, request: Request) {
        self.advance(now);
        self.request(who, request);
    }
    fn poll(&mut self, now: Time, _wall: Wall) -> Vec<(ClientId, Event)> {
        self.advance(now);
        if let Some(until) = self.offline_until {
            if now < until {
                return std::mem::take(&mut self.callbacks);
            }
            self.offline_until = None;
            self.observed.push(Observed::Reachable { live: true });
            for who in self.transport.clients() {
                self.hello(who);
                if self.transport.sign_in(who).is_none() {
                    self.sign_outs.entry(who).or_default().push_back(None);
                    self.callbacks.push((
                        who,
                        Event::Root(RootEvent::Client {
                            event: jig_client::Event::Up { up: jig_client::Up::SignedOut },
                        }),
                    ));
                }
            }
            for (who, requests) in std::mem::take(&mut self.parked) {
                for request in requests {
                    self.request(who, request);
                }
            }
        }
        self.drain();
        if self.due.is_some_and(|due| now < due) {
            return Vec::new();
        }
        self.due = None;
        std::mem::take(&mut self.callbacks)
    }
    fn next_due(&self) -> Option<Time> {
        self.offline_until.or(self.due).or_else(|| Some(self.now.saturating_add(Duration::from_millis(100))))
    }
    fn observed(&mut self) -> Vec<Observed> {
        std::mem::take(&mut self.observed)
    }
    fn fault(&mut self, now: Time, fault: FrontFault) {
        self.advance(now);
        match fault {
            FrontFault::Restart { offline } => {
                let limits = self.engine.env.limits;
                let hosting = self
                    .hosting
                    .iter()
                    .map(|(task, attempt)| {
                        temper_engine_domain_world::protocol::hosting(*task, *attempt, jig_domain_host::Phase::Active)
                    })
                    .collect();
                let store = std::mem::replace(&mut self.engine.store, Store::new());
                let config = walking::config(self.seed);
                // The restarted engine runs the walking settings: judge asks by the same ones, as jig's kit does.
                self.transport.models.clone_from(&config.core.settings.models);
                self.transport.providers = config.core.settings.llm_providers.clone();
                self.engine = Driver::with_repositories(
                    store,
                    [walking::config(self.seed), config],
                    &limits,
                    fixtures::REPOSITORIES,
                );
                self.engine.send(temper_engine_domain_world::protocol::hello_slots(
                    fixtures::QUESTION_WORKER,
                    limits.fleet.slots,
                    hosting,
                ));
                self.engine.settle();
                self.reads = 0;
                self.read_loads.clear();
                self.offline_until = Some(now.saturating_add(offline));
                self.observed.push(Observed::Reachable { live: false });
                self.drop_watches();
            }
            FrontFault::Slow { span } => self.due = Some(now.saturating_add(span)),
            // The engine receives the request but its answer is lost on a live link.
            // Report Lost immediately: this front has no exchange deadline to wait for
            // (jig's client/domain/client.md, 16); jig-client owns the retry backoff.
            FrontFault::NoAnswer => self.lose_answers = self.lose_answers.checked_add(1).expect("finite lost answers"),
            FrontFault::DropWatch => self.drop_watches(),
            FrontFault::Busy => self.keyed_fault = Some(RequestEnvelope::Again),
            FrontFault::SignedOut => self.keyed_fault = Some(RequestEnvelope::SignedOut),
            FrontFault::NotReady | FrontFault::Versions | FrontFault::ObjectGone => {
                panic!("not a temper joint story fault: {fault:?}")
            }
        }
    }
    fn reload(&mut self, who: ClientId, now: Time) {
        self.advance(now);
        self.detach(who);
        // A terminal restart has no retained address, unlike a browser reload.
        self.callbacks.push((who, RootEvent::Went { place: client::Place::Address(fixtures::inbox()) }.into()));
        if self.offline_until.is_none() {
            if self.transport.sign_in(who).is_some() {
                self.callbacks.push((
                    who,
                    RootEvent::Client { event: jig_client::Event::Up { up: jig_client::Up::SignedIn } }.into(),
                ));
            }
            self.hello(who);
            if self.transport.sign_in(who).is_none() {
                self.sign_outs.entry(who).or_default().push_back(None);
                self.callbacks.push((
                    who,
                    Event::Root(RootEvent::Client { event: jig_client::Event::Up { up: jig_client::Up::SignedOut } }),
                ));
            }
        }
    }
    fn received(&mut self, who: ClientId, event: &Event) {
        if let Event::Root(RootEvent::Client {
            event:
                jig_client::Event::Up {
                    up: jig_client::Up::Watched { watch, event: jig_client::contract::WatchEvent::Delivered(delivery) },
                },
        }) = event
        {
            let acknowledgement = if let jig_client::contract::Delivery::Run(text) = delivery {
                let route = self.transport.watch_token(who, *watch).and_then(|watcher| self.transport.watch(watcher));
                if let Some(Watched::Jig(jig_client::contract::Subject::Run { task, .. })) =
                    route.map(|route| &route.subject)
                {
                    let task = *task;
                    self.transport
                        .continued(
                            who,
                            *watch,
                            jig_client::contract::Subject::Run { task, turn: Some(text.turn), offset: text.through },
                        )
                        .expect("delivered stream belongs to its run watch")
                } else {
                    None
                }
            } else {
                self.transport.taken(who, *watch)
            };
            if let Some(event) = acknowledgement {
                self.send(translate::party(event));
            }
        }
        if let Event::Root(RootEvent::Client {
            event: jig_client::Event::Up { up: jig_client::Up::FamilyDelivered { watch, .. } },
        }) = event
            && let Some(event) = self.transport.application_taken(who, *watch)
        {
            self.send(translate::party(event));
        }
    }
    fn sign_in(&mut self, who: ClientId, now: Time, _wall: Wall) {
        self.advance(now);
        self.callbacks.retain(|(client, event)| {
            *client != who
                || !matches!(
                    event,
                    Event::Root(RootEvent::Client { event: jig_client::Event::Up { up: jig_client::Up::SignedOut } })
                )
        });
        self.sign_outs.remove(&who);
        self.callbacks.push((
            who,
            Event::Root(translate::callback(jig_client_test_domain::Event::Client {
                event: jig_client::Event::Up { up: self.transport.hello() },
            })),
        ));
        let event = translate::party(self.transport.start_sign_in(who));
        self.send(event);
    }
    fn delivers(&mut self, client: ClientId, event: &Event) -> bool {
        if !matches!(
            event,
            Event::Root(RootEvent::Client { event: jig_client::Event::Up { up: jig_client::Up::SignedOut } })
        ) {
            return true;
        }
        let sign_in = self
            .sign_outs
            .get_mut(&client)
            .and_then(VecDeque::pop_front)
            .expect("sign-out callback retains its exchange");
        self.transport.sign_in(client).is_none_or(|current| Some(current) == sign_in)
    }
    fn sign_out(&mut self, who: ClientId, now: Time, _wall: Wall) {
        self.advance(now);
        if self.offline_until.is_some() {
            self.callbacks.push((who, Event::Root(RootEvent::Client { event: jig_client::Event::SignOutLost })));
            return;
        }
        let Some(sign_in) = self.transport.sign_in(who) else { return };
        let reply_to =
            self.transport.reply_to(jig_client_world::joint::transport::Reply::SignOut { client: who, sign_in });
        self.send(translate::party(jig_domain_core::Event::People(jig_domain_people::Event::SignOut {
            reply_to,
            sign_in,
        })));
    }
}

// The kit's truth reads the unchanged core family of the externally applied store.
// Temper's other durable families are not jig client-referee facts.
fn core_rows(engine: &Driver) -> BTreeMap<jig_test_domain::Key, jig_test_domain::Record> {
    engine
        .store
        .rows
        .iter()
        .filter_map(|(key, record)| {
            if let (engine::Key::Core(key), engine::Record::Core(record)) = (key, record) {
                Some((jig_test_domain::Key::Core(key.clone()), jig_test_domain::Record::Core(record.clone())))
            } else {
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;

mod accounts;
