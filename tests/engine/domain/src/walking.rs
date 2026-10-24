//! One real root and its children, driven by one person, one worker and
//! the existing ordered fake store. Requests become later input events
//! (domain/root.md, section 12).

use crate::protocol::{self, Protocol};
use crate::store::Store;
use crate::walking_referee::{FINAL_SPEND, QUESTION, REPORT, TURNS, WalkingReferee};
use jig_domain_accounts as accounts;
use jig_domain_authority as authority;
use jig_domain_fleet as fleet;
use jig_domain_host as host;
use jig_domain_people as people;
use jig_domain_tasks as tasks;
use skein_lib::{Duration, Env, Queue, ReplyTo, Time, Token, Wall};
use std::{
    collections::{BTreeMap, VecDeque},
    fmt::Write as _,
};
use temper_engine_domain as engine;
use temper_engine_domain::{Delivery, JournalLimits, Record, loads};

/// Tiny counted child limits plus the root's declared synchronous-route room.
#[must_use]
#[expect(clippy::too_many_lines, reason = "one explicit fixture lists the real child's tiny admission limits together")]
pub fn limits() -> engine::Limits {
    let retry = tasks::Retry { retries: 1, base: Duration::from_millis(10), max: Duration::from_secs(1) };
    let tasks = tasks::Limits {
        tasks: 2,
        funders: 4,
        project_tasks: 2,
        project_goals: 2,
        grace: Duration::from_secs(5),
        tree_tasks: 2,
        depth: 1,
        delegates: 1,
        references: 2,
        subscriptions: 2,
        batch: 1,
        dependencies: 1,
        circle: 2,
        holdings: 2,
        hold_kinds: 2,
        resource_reports: 64,
        pools: 2,
        hold_segments: 4,
        hold_bytes: 128,
        hold_waiters: 2,
        hold_wait: Duration::from_secs(10),
        inputs: 1,
        spec_bytes: 64,
        parameters: 1,
        result_bytes: 128,
        run_inbox: 4,
        inbox_messages: 4,
        inbox_bytes: 128,
        message_bytes: 64,
        person_stall: Duration::from_secs(3600),
        proposal_stall: Duration::from_millis(10),
        escalation_stall: Duration::from_secs(3600),
        saved_resources: 2,
        contract_choices: 1,
        charters: 1,
        authority_grants: 1,
        authority_segments: 2,
        authority_bytes: 32,
        executor_kinds: 1,
        retries: tasks::Retries {
            transient: retry,
            permanent: retry,
            run: retry,
            agent: retry,
            lost: retry,
            invalid: retry,
        },
        facts: 2,
        room_refusals: 2,
    };
    let people = people::Limits {
        act_grants: 0,
        batch: 4,
        operations: 2,
        people: 2,
        program_hosts: 2,
        program_host_name_bytes: 32,
        inbox_entries: 2,
        inbox_tasks: 4,
        inbox_recipients: 16,
        sign_ins: 2,
        projects: 2, // The configured project and the deployment roster.
        holdings: 2,
        initial_owners: 1,
        requests: 4,
        pending: 2,
        waiters: 2,
        identity_bytes: 32,
        words: 64,
        amendment_bytes: 512,
        sign_in_lifetime: Duration::from_secs(60),
        request_retention: Duration::from_secs(120),
        facts: 2,
        room_refusals: 2,
    };
    let fleet = fleet::Limits {
        configured_workers: 1,
        worker_name_bytes: 32,
        program_sources: 12,
        source_detail_bytes: 4096,
        workers: 1,
        needs_bytes: 65_536,
        engine_slots: 0,
        slots: 1,
        workstreams: 1,
        attempts: 2,
        grants: 2,
        calls: 1,
        call_name_bytes: 64,
        progress_bytes: 0,
        turns: 2,
        grace: Duration::from_secs(5),
        facts: 2,
        history: 2,
    };
    let deliveries = 20_u32.max(fleet::max_out(&fleet).checked_add(1).expect("fleet burst plus continuation"));
    let mut limits = engine::Limits {
        settings: jig_domain_core::settings::Limits { history: 64, alternatives: 16, permissions: 32 },
        templates: jig_protocol_config::profile(jig_protocol_config::Profile::Small).core.templates,
        documents: jig_domain_core::documents::DocumentLimits {
            roles: jig_protocol_config::profile(jig_protocol_config::Profile::Small).core.documents.roles,
            segments: 9,
            grants: authority_limits().grants.max(tasks.authority_grants),
            executors: authority_limits().executors.max(tasks.executor_kinds),
            implications: authority_limits().implications,
            rows: 4,
            text_bytes: 4_096,
            chips: 4,
            lines: 4,
            versions: jig_domain_core::documents::Versions { minimum: 1, maximum: 1 },
        },
        authority: authority_limits(),
        journal: JournalLimits {
            commits: 3,
            held: deliveries * 3 + 16,
            writes: 0,
            deliveries,
            transcript_bytes: 8192,
            result_bytes: 384,
            run_bytes: 4096,
        },
        loads: loads::Limits {
            read_bytes: 65_536,
            loads: 2,
            rows: 2,
            bytes: 16384,
            reply_bytes: 16384,
            transcript_bytes: 8192,
        },
        tasks,
        people,
        fleet,
        call_records: 2,
        brief: engine::BriefLimits {
            briefs: 2,
            sections: 3,
            parts: 2,
            read_bytes: 256,
            budgets: engine::BriefBudgets {
                task: 128,
                dependencies: 128,
                ci: 128,
                reviews: 128,
                pull: 128,
                attempts: 128,
                plan: 128,
                notes: 64,
            },
            brief_bytes: 384,
            gather: Duration::from_secs(1),
        },
        accounts: accounts::Limits {
            move_deadline: Duration::from_secs(60),
            accounts: 1,
            holder_accounts: 1,
            sources: 3,
            sign_ins: 1,
            credentials: 1,
            program_names: 3,
            program_hosts: 2,
            backoff_base: Duration::from_millis(10),
            backoff_max: Duration::from_secs(1),
            retry: Duration::from_millis(10),
            spent_attention: Duration::from_secs(1),
            facts: 2,
        },
        notes: jig_domain_notes::Limits {
            scopes: 4,
            entries_per_scope: 1,
            pattern_bytes: 64,
            description_bytes: 64,
            body_bytes: 256,
            references: 1,
            load_rows: 1,
            lines: 1,
            recalled: 1,
        },
        forge: temper_engine_forge_world::LIMITS,
    };
    limits.journal.writes = writes(&limits);
    limits
}

/// Root route room for these bounds, including continuations and call records.
#[must_use]
pub fn writes(limits: &engine::Limits) -> u32 {
    tasks::max_out(&limits.tasks) * 8
        + people::max_out(&limits.people) * 4
        + limits.people.pending * 2
        + fleet::max_out(&limits.fleet) * 4
        + temper_engine_domain_forge::max_out(&limits.forge)
        + limits.tasks.tasks
        + 4
        + limits.call_records
}

fn authority_limits() -> authority::Limits {
    authority::Limits {
        projects: 1,
        roles: 1,
        requirements: 1,
        facts: 2,
        grants: 1,
        executors: 1,
        segments: 1,
        segment_bytes: 32,
        implications: 0,
        batch: 1,
        accounts: 2,
        writes: 1,
    }
}

fn authority_value(spend: u64, kinds: Box<[authority::Executor]>) -> authority::Authority {
    authority::Authority {
        tools: authority::Tools(1023),
        models: authority::Models::ALL,
        grants: Box::new([]),
        delegation: authority::Delegation { kinds, tasks: 2, depth: 1 },
        budget: authority::Budget { spend, deadline: None },
        notes: authority::Scopes(0),
        note_resources: Box::new([]),
    }
}

/// Real authority policy and an authenticated owner, rather than allow flags.
#[must_use]
pub fn config(seed: u64) -> engine::Config {
    let ceiling = authority_value(1000, Box::new([authority::Executor::Charter(1)]));
    let rules = authority::Rules {
        ceiling: ceiling.clone(),
        period_spend: 1000,
        minimum_run_spend: 1,
        maximum_run_spend: 100,
        implies: authority::Implies::new(Box::new([]), 0).expect("empty implication configuration"),
        requirements: Box::new([]),
    };
    // Check the same fixture bounds without allocating a discarded live policy table.
    let policy_limits = authority_limits();
    assert!(
        authority::max_out(&policy_limits).is_some()
            && authority::worst_case(&policy_limits).is_some()
            && authority::valid_rules(&rules, &policy_limits),
        "valid authority configuration"
    );
    let policy = authority::Policy {
        projections: ceiling.grants.clone(),
        escalation_role: Some(0),
        ceiling,
        period_spend: 1000,
        roles: Box::new([authority::Role {
            number: 0,
            authority: authority_value(500, Box::new([authority::Executor::Charter(1)])),
            period_spend: 500,
            requests: authority::Requests::ALL,
            decides: authority::Proposals::ALL,
        }]),
        requirements: Box::new([]),
    };
    engine::Config {
        core: jig_domain_core::Config {
            deployment: [31; 16],
            seed,
            // The worlds' workers run apart from the engine (domain/root.md, 2).
            workers_in_process: false,
            worker_names: Box::new([Box::from(b"worker".as_slice())]),
            hub_turns: HUB_TURNS,
            owners: Box::new([people::InitialOwner {
                project: 1,
                identity: people::IdentityKey { provider: 0, subject: 7_u64.to_be_bytes().into() },
            }]),
            grants: Box::new([]),
            defaults: jig_domain_core::settings::Defaults {
                rules,
                charters: Box::new([charter()]),
                templates: crate::fixtures::templates(),
                policies: skein_lib::Map::with_capacity(authority_limits().projects),
            },
            settings: jig_domain_core::Settings {
                key_bytes: 256,
                redirect_bytes: 256,
                deployment_provider: 1,
                resume_bytes: 256,
                tail_bytes: 128,
                tools: jig_domain_core::ToolFamilies::JIG,
                models: models(),
                llm_providers: accounts::Providers {
                    providers: Box::new([accounts::Provider {
                        number: 1,
                        name: Box::from(b"Scripted".as_slice()),
                        signed_in_plan: false,
                        manage_usage: None,
                        models: Box::new([charter().policy.model.name]),
                        offers: accounts::Offers { sign_in: true, key: true, programs: Box::new([]) },
                    }]),
                },
            },
            seed_projects: Box::new([jig_domain_core::SeedProject {
                number: 1,
                name: "Project 1".into(),
                policy: jig_domain_core::ConfiguredPolicy {
                    policy,
                    permissions: permission_roles().remove(&1).expect("project permissions"),
                    templates: Box::new([]),
                },
            }]),
            // The forge is the deployment's one connector.
            connectors: Box::new([0]),
        },
        landing: engine::LandingPolicy { deployment: Box::new([]), projects: skein_lib::Map::with_capacity(1) },
        forge: skein_forge_client::Config {
            namespace: Box::from(&b"root"[..]),
            writers: Box::new([skein_forge_client::Writer { forge: 1, author: 1 }]),
        },
        // The forge's git identity is application credential 1, which the
        // worlds name at start (D10).
        identities: Box::new([temper_engine_domain_forge::GitIdentity { forge: 1, credential: 1 }]),
        forge_charter: 1,
        forges: Box::new([]),
        under_box: false,
        demo: None,
    }
}

/// Retained turns per attempt in the workers' hub, as jig's smallest
/// profile sets them (`jig-protocol-config`'s `small.rs`, hub `turns`).
pub const HUB_TURNS: u32 = 8;

/// Project one's forge permissions mapped to its seeded roles.
fn permission_roles() -> skein_lib::Map<u32, Box<[people::PermissionRole]>> {
    let mut projects: skein_lib::Map<u32, Box<[people::PermissionRole]>> = skein_lib::Map::with_capacity(1);
    projects
        .insert(
            1,
            Box::new([
                people::PermissionRole { connector: 0, permission: 3, role: 1 },
                people::PermissionRole { connector: 0, permission: 2, role: 2 },
                people::PermissionRole { connector: 0, permission: 1, role: 3 },
            ]),
        )
        .expect("one configured permission policy");
    projects
}

/// The deployment's configured models: its one charter's.
#[must_use]
pub fn models() -> Box<[engine::Model]> {
    Box::new([charter().policy.model])
}

/// The deployment's one charter: the chats' and the forge's agent work's.
/// Its default spend holds its reserve floor (jig's `domain/authority.md`, 7).
#[must_use]
pub fn charter() -> engine::Charter {
    engine::Charter {
        number: 1,
        policy: engine::RunPolicy {
            instructions: b"Complete the task within its authority.".as_slice().into(),
            waiting: Duration::from_secs(1),
            resume: true,
            turns: 8,
            time: Duration::from_secs(60),
            reserve_turns: 1,
            reserve_time: Duration::from_secs(10),
            model: engine::Model {
                program: None,
                dialect: 1,
                endpoint: 1,
                name: b"scripted".as_slice().into(),
                window: 1024,
                output: 1024,
                input_price: 1,
                cached_price: 1,
                cache_write_price: None,
                output_price: 1,
                price_unit: 1000,
                efforts: Box::new([]),
                effort: None,
            },
            alternatives: Box::new([]),
            inspect: true,
            modify: true,
            shell: true,
            agents: true,
            call_timeout: Duration::from_secs(10),
        },
        hosts: fleet::Kinds::Workers,
        spend: 100,
    }
}

/// Store latency and a lost-completion cut are deterministic world inputs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Settings {
    /// Deterministic child seed, replayed with the same external stimuli.
    pub seed: u64,
    /// Iterations a submitted commit waits before the fake store applies it.
    pub commit_delay: u32,
    /// Iterations a captured page waits before its load terminal arrives.
    pub page_delay: u32,
    /// Lose the first turn's commit completion and restart while the worker retains it.
    pub restart: bool,
    /// World script cuts after the final ended/financial transaction is applied
    /// before its completion or ACK/result release; mutually exclusive with
    /// `restart`. The retained worker answer recovers through the real root
    /// routes; reading the result after the cut is the core's (jig's
    /// `client/domain/contract.md`, 6).
    pub terminal_restart: bool,
    /// Drain observational child facts or leave their tiny queues saturated.
    pub facts: bool,
}

impl Settings {
    /// The required commit-cut restart story with tiny one-row pages.
    #[must_use]
    pub const fn calm(seed: u64) -> Settings {
        Settings { seed, commit_delay: 0, page_delay: 0, restart: true, terminal_restart: false, facts: true }
    }

    /// Script one final-commit process cut and one extra worker terminal resend;
    /// the fake checks unchanged durable rows after the cut.
    #[must_use]
    pub const fn terminal(seed: u64) -> Settings {
        Settings { seed, commit_delay: 0, page_delay: 0, restart: false, terminal_restart: true, facts: true }
    }
}

// The outside worker makes two recovery sends in order: its retained answer,
// then one explicit post-ACK resend. This transport sequence is independent
// of the person's one authenticated result request.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TerminalRecovery {
    NotRequested,
    AwaitingRetainedAck,
    AwaitingReplayAck,
    Complete,
}

/// Complete frozen world state between iterations; only the store and scripts
/// outlive replacement of the root process (domain/root.md, section 12).
#[derive(Debug)]
pub struct World {
    settings: Settings,
    domain: engine::Domain,
    environment: Env<engine::Limits>,
    out: Queue<engine::Request>,
    events: VecDeque<(u32, engine::Event)>,
    /// Durable ordered fake store shared with the journal/load worlds.
    pub store: Store,
    /// Independent expectations derived from person and worker inputs.
    pub referee: WalkingReferee,
    /// Deterministic outside observations, excluding optional diagnostic facts.
    pub trace: Vec<String>,
    /// Exact number of injected process restarts.
    pub restarts: u32,
    /// Real paged store loads made by the root, including restart restoration.
    pub pages: u32,
    iteration: u32,
    commit_wait: u32,
    person_sent: bool,
    session: Option<u64>,
    assignment: Option<engine::Assignment>,
    pending_turn: Option<u32>,
    pending_answer: bool,
    terminal_recovery: TerminalRecovery,
    recovery_clock_offset: u64,
    terminal_snapshot: Option<BTreeMap<temper_engine_domain::Key, Record>>,
    protocol: Protocol,
    period_sent: bool,
}

impl World {
    /// Construct the real children and schedule startup plus an empty worker hello.
    #[must_use]
    pub fn new(settings: Settings) -> World {
        assert!(!(settings.restart && settings.terminal_restart), "one chosen process cut per story");
        let limits = limits();
        let config = config(settings.seed);
        let store = crate::fixtures::restored_store(self::config(settings.seed), &limits);
        let mut world = World {
            domain: engine::Domain::new(config, &limits).expect("valid engine configuration"),
            environment: Env { now: Time::ZERO, wall: Wall::EPOCH, limits },
            out: Queue::with_capacity(engine::max_out(&limits)),
            events: VecDeque::new(),
            store,
            referee: WalkingReferee::default(),
            trace: Vec::new(),
            restarts: 0,
            pages: 0,
            iteration: 0,
            commit_wait: 0,
            person_sent: false,
            session: None,
            assignment: None,
            pending_turn: None,
            pending_answer: false,
            terminal_recovery: TerminalRecovery::NotRequested,
            recovery_clock_offset: 0,
            terminal_snapshot: None,
            protocol: Protocol::default(),
            period_sent: false,
            settings,
        };
        world.queue(engine::Event::Restart, 0);
        world.hello();
        world
    }

    fn queue(&mut self, event: engine::Event, delay: u32) {
        self.events.push_back((self.iteration + delay + 1, event));
    }

    fn hello(&mut self) {
        let hosting: Box<[host::Hosting]> = match self.assignment.as_ref() {
            Some(assignment) => Box::new([protocol::hosting(
                assignment.task,
                assignment.attempt,
                if self.pending_answer { host::Phase::Answered } else { host::Phase::Active },
            )]),
            None => Box::new([]),
        };
        self.queue(protocol::hello(Token::new(7), hosting), 0);
    }

    fn turn(&mut self, number: u32) {
        let assignment = self.assignment.as_ref().expect("script has its real assignment");
        let (_, cumulative, transcript) = TURNS.iter().find(|(turn, _, _)| *turn == number).expect("scripted turn");
        let event =
            protocol::turn(Token::new(7), assignment.task, assignment.attempt, number, *cumulative, None, transcript);
        self.pending_turn = Some(number);
        self.queue(event, 0);
    }

    fn restart(&mut self, terminal: bool) {
        assert!(self.store.pending.is_empty(), "cut has no later submitted commit");
        self.domain = engine::Domain::new(config(self.settings.seed), &self.environment.limits)
            .expect("valid engine configuration");
        self.events.clear();
        self.restarts += 1;
        self.trace.push(
            if terminal {
                "restart after durable terminal/result, before completion"
            } else {
                "restart after durable turn, before completion"
            }
            .into(),
        );
        self.queue(engine::Event::Restart, 0);
        self.hello();
    }

    /// Drive one up/ready/timer/reclaim iteration and independent fake effects.
    #[expect(clippy::too_many_lines, reason = "one scripted iteration includes each journal release pass")]
    pub fn iterate(&mut self) {
        self.iteration += 1;
        self.environment.now = Time::from_nanos(u64::from(self.iteration) * 1_000_000 + self.recovery_clock_offset);
        self.environment.wall = Wall::from_nanos(u64::from(self.iteration) * 1_000_000 + self.recovery_clock_offset);
        crate::poll(&mut self.domain, &self.environment, &mut self.out);
        self.observe();
        if self.events.front().is_some_and(|(due, _)| *due <= self.iteration) {
            let (_, event) = self.events.pop_front().expect("due input");
            self.trace.push(format!("input {event:?}"));
            engine::step(&mut self.domain, &self.environment, event);
            engine::release(&mut self.domain, &self.environment, &mut self.out);
            self.observe();
        }
        engine::fire(&mut self.domain, &self.environment);
        engine::release(&mut self.domain, &self.environment, &mut self.out);
        self.observe();
        if self.settings.facts {
            self.domain.drain_facts();
        }
        self.domain.reclaim();
        if !self.store.pending.is_empty() {
            if self.commit_wait > 0 {
                self.commit_wait -= 1;
            } else {
                let first_turn = self.store.pending.front().expect("pending commit").1.iter().any(
                    |write| matches!(write, temper_engine_domain::Write::Save(Record::Core(jig_domain_core::Record::Core(jig_domain_core::CoreRecord::Turn(turn)))) if turn.turn == 1),
                );
                let terminal = self.store.pending.front().expect("pending commit").1.iter().any(|write| {
                    matches!(
                        write,
                        temper_engine_domain::Write::Save(Record::Core(jig_domain_core::Record::Tasks(
                            tasks::Stored::Ended(_)
                        )))
                    )
                });
                let number = self.store.apply();
                self.trace.push(format!("applied {number}"));
                self.commit_wait = self.settings.commit_delay;
                if self.settings.restart && self.restarts == 0 && first_turn {
                    self.restart(false);
                } else if self.settings.terminal_restart && self.restarts == 0 && terminal {
                    self.referee.terminal_cut(&self.store.rows).expect("durable terminal before any ACK/result");
                    self.terminal_snapshot = Some(self.store.rows.clone());
                    self.restart(true);
                } else {
                    self.queue(engine::Event::Store(engine::StoreInput::Committed { number, fill: 0 }), 0);
                }
            }
        }
        if let Some(snapshot) = &self.terminal_snapshot {
            for (key, row) in snapshot {
                if !matches!(
                    key,
                    temper_engine_domain::Key::Core(jig_domain_core::Key::Core(jig_domain_core::CoreKey::Deployment))
                ) {
                    assert_eq!(
                        self.store.rows.get(key),
                        Some(row),
                        "terminal recovery preserves committed task and charge"
                    );
                }
            }
        }
        // The deployment's first period, at its first start (jig's
        // `protocol/configuration.md`, 5; `testing/jig-test-service/src/engine.rs`).
        if self.domain.ready() && !self.period_sent {
            self.period_sent = true;
            self.queue(
                engine::Event::Party(engine::PartyInput::Period { project: 1, period: 1, budget: 1000, next: None }),
                0,
            );
        }
        if self.domain.ready() && self.period_sent && !self.person_sent {
            self.person_sent = true;
            self.queue(
                engine::Event::Party(engine::PartyInput::SignedIn {
                    held: None,
                    reply_to: ReplyTo::new(Token::new(101)),
                    identity: people::Identity {
                        key: people::IdentityKey { provider: 0, subject: 7_u64.to_be_bytes().into() },
                        login: b"person".as_slice().into(),
                        name: b"Person".as_slice().into(),
                    },
                }),
                0,
            );
        }
        if self.restarts != 0
            && self.domain.ready()
            && self.pending_turn == Some(1)
            && !self.events.iter().any(|(_, event)| {
                matches!(
                    event,
                    engine::Event::Worker(engine::WorkerInput::Link {
                        link: engine::Link::Up { up: host::LinkUp::Turn { .. }, .. },
                        ..
                    })
                )
            })
        {
            self.turn(1);
        }
        if self.settings.terminal_restart
            && self.restarts == 1
            && self.domain.ready()
            && self.terminal_recovery == TerminalRecovery::NotRequested
        {
            self.terminal_recovery = TerminalRecovery::AwaitingRetainedAck;
            self.answer();
        }
        if self.terminal_recovery != TerminalRecovery::NotRequested
            && self.recovery_clock_offset == 0
            && !self.events.iter().any(|(_, event)| {
                matches!(
                    event,
                    engine::Event::Worker(engine::WorkerInput::Link {
                        link: engine::Link::Up { up: host::LinkUp::Answer { .. }, .. },
                        ..
                    })
                )
            })
        {
            // The retained answer has reached the loaded fleet. Advance the
            // external clock past its five-second grace, staying within the
            // person's sixty-second session and account validity.
            self.recovery_clock_offset = 6_000_000_000;
        }
    }

    fn answer(&mut self) {
        let assignment = self.assignment.as_ref().expect("worker retains terminal claim");
        let (task, attempt) = (assignment.task, assignment.attempt);
        self.pending_answer = true;
        let end = tasks::End::Finished {
            result: tasks::TaskResult::Report { words: REPORT.into() },
            cancel_delegates: false,
        };
        let event = self.protocol.answer(Token::new(7), task, attempt, FINAL_SPEND, end, engine::Parts::none());
        self.queue(event, 0);
    }

    fn observe(&mut self) {
        assert!(self.out.len() <= engine::max_out(&self.environment.limits));
        while let Some(request) = self.out.pop() {
            self.trace.push(format!("output {request:?}"));
            let request = match self.protocol.play(request) {
                Ok(events) => {
                    for event in events {
                        self.queue(event, 0);
                    }
                    continue;
                }
                Err(request) => *request,
            };
            match request {
                engine::Request::ToChild(released) => self.queue(engine::Event::Released(released), 0),
                engine::Request::Store(engine::StoreRequest::Commit { number, writes }) => {
                    assert!(self.terminal_snapshot.is_none(), "terminal recovery commits nothing: {writes:?}");
                    self.referee.commit(&writes).expect("independent transaction referee");
                    if self.store.pending.is_empty() {
                        self.commit_wait = self.settings.commit_delay;
                    }
                    self.store.pending.push_back((number, writes));
                }
                engine::Request::Store(engine::StoreRequest::Load { owner, range, after, most, .. }) => {
                    self.pages += 1;
                    let (rows, next) = self.store.page(&range, after.as_ref(), most);
                    self.queue(
                        engine::Event::Store(engine::StoreInput::Loaded { owner, rows, next, texts: None }),
                        self.settings.page_delay,
                    );
                }
                engine::Request::Worker(engine::WorkerRequest::Deliver(delivery))
                | engine::Request::Party(engine::PartyRequest::Deliver(delivery)) => self.delivery(delivery),
                engine::Request::Stop => panic!("walking story stopped: {:?}", self.trace),
                other @ (engine::Request::Worker(_)
                | engine::Request::Party(_)
                | engine::Request::Store(_)
                | engine::Request::Account(_)
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
                | engine::Request::Busy(_)
                | engine::Request::Drained
                | engine::Request::Forge { .. }) => panic!("unexpected walking output {other:?}"),
            }
        }
    }

    fn check_assignment(&self, channel: Token, assignment: &engine::Assignment) {
        assert_eq!(channel, Token::new(7));
        assert_eq!(assignment.run.placement, fleet::PlacementConstraint { channel, kind: fleet::HostKind::Worker });
        let Some(engine::Record::Core(jig_domain_core::Record::Core(jig_domain_core::CoreRecord::RunProof(proof)))) =
            self.store.rows.get(&engine::Key::Core(jig_domain_core::Key::Core(jig_domain_core::CoreKey::RunProof(
                assignment.task,
            ))))
        else {
            panic!("assignment follows its durable proof")
        };
        assert_eq!(assignment.run.accounts, proof.accounts);
        assert_eq!(proof.placement, Some(channel));
        assert_eq!(assignment.run.sources.as_ref(), &[Some(assignment.grants[0].account)]);
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one exhaustive outside delivery adapter drives the finite authenticated script"
    )]
    fn delivery(&mut self, delivery: Delivery) {
        match delivery {
            Delivery::SourceSignedOut { .. } => panic!("this story issues no source sign-out"),
            Delivery::WebReply { reply: people::Reply::SignedIn { person, sign_in, .. }, .. } => {
                self.referee.signed_in(&self.store.rows, person, sign_in).expect("durable sign-in");
                self.session = Some(sign_in);
                self.queue(
                    engine::Event::Party(engine::PartyInput::Ask {
                        models: Box::new([]),
                        reply_to: ReplyTo::new(Token::new(102)),
                        sign_in,
                        key: [5; 16],
                        ask: crate::walking::chat(1, QUESTION),
                    }),
                    0,
                );
            }
            Delivery::WebReply {
                reply: people::Reply::Outcome(people::Outcome::Created { first: task, last, given }),
                ..
            } => {
                assert_eq!(task, last, "one template task");
                assert_eq!(given, Some(people::Given { task, amount: 500 }), "template funding receipt");
                self.referee.started(&self.store.rows, task).expect("atomic chat task and answer");
            }
            Delivery::Assigned { channel, assignment } => {
                self.check_assignment(channel, &assignment);
                self.referee.assigned(&self.store.rows, &assignment).expect("committed claim and real brief");
                self.assignment = Some(assignment);
                self.turn(1);
            }
            Delivery::Link { channel, down: host::LinkDown::TurnKept { run, attempt, turn } } => {
                assert_eq!(channel, Token::new(7));
                self.referee
                    .turn_ack(&self.store.rows, run.raw(), attempt.raw(), turn)
                    .expect("durable atomic turn ACK");
                if self.pending_turn != Some(turn) {
                    return;
                }
                self.pending_turn = None;
                if turn == 1 {
                    self.turn(2);
                } else {
                    self.answer();
                }
            }
            Delivery::Link { channel, down: host::LinkDown::Acknowledge { run, attempt } } => {
                assert_eq!(channel, Token::new(7));
                let (task, attempt) = (run.raw(), attempt.raw());
                self.pending_answer = false;
                match self.terminal_recovery {
                    TerminalRecovery::AwaitingReplayAck => {
                        self.referee
                            .replayed_answer_ack(&self.store.rows, task, attempt)
                            .expect("fenced duplicate answer receives ACK without another terminal");
                        self.terminal_recovery = TerminalRecovery::Complete;
                    }
                    TerminalRecovery::NotRequested | TerminalRecovery::AwaitingRetainedAck => {
                        self.referee
                            .answer_ack(&self.store.rows, task, attempt)
                            .expect("priced terminal durable before ACK");
                        if self.settings.terminal_restart {
                            self.terminal_recovery = TerminalRecovery::AwaitingReplayAck;
                            self.answer();
                        }
                    }
                    TerminalRecovery::Complete => panic!("ACK after both scripted sends completed"),
                }
            }
            Delivery::Link { down: host::LinkDown::Grant { .. }, .. } => {}
            Delivery::TurnBusy { turn, .. } => self.turn(turn),
            Delivery::Result { person, task, words } => self
                .referee
                .result(&self.store.rows, person, task, &words)
                .expect("committed result reaches person once"),
            Delivery::DemoActed { .. }
            | Delivery::DemoNote { .. }
            | Delivery::Host(_)
            | Delivery::Link { .. }
            | Delivery::ForgeCall { .. }
            | Delivery::Bound { .. }
            | Delivery::Procedure { .. }
            | Delivery::Application { .. }
            | Delivery::Forges { .. }
            | Delivery::Panel { .. }
            | Delivery::WebReply { .. }
            | Delivery::CarriedUnused { .. }
            | Delivery::Read { .. }
            | Delivery::ReadLoad { .. }
            | Delivery::Document { .. }
            | Delivery::WatchEnded { .. }
            | Delivery::Drained
            | Delivery::Refuse { .. } => {
                panic!("unexpected walking delivery {delivery:?}")
            }
            Delivery::Fleet(_)
            | Delivery::ProjectMade { .. }
            | Delivery::ForgeCommitted { .. }
            | Delivery::ReadEscalationDecision { .. }
            | Delivery::Relay { .. }
            | Delivery::Load { .. } => {
                panic!("internal handoff leaked to world")
            }
        }
    }

    /// Settle every obligation within a deterministic iteration bound.
    pub fn run(&mut self) {
        for _ in 0..600 {
            if self.settled() {
                return;
            }
            self.iterate();
        }
        panic!("walking story did not settle: {:?}", self.trace);
    }

    fn settled(&self) -> bool {
        (if self.settings.terminal_restart { self.referee.done_after_cut() } else { self.referee.done() })
            && (!self.settings.terminal_restart
                || (self.terminal_recovery == TerminalRecovery::Complete && self.referee.terminal_replay_done()))
            && !self.pending_answer
            && self.events.is_empty()
            && self.store.pending.is_empty()
            && self.out.is_empty()
            && self.domain.quiescent()
    }
}

/// Replay every frozen root, child, store and pending script state in lockstep,
/// including diagnostic queues with identical observation settings
/// (domain/root.md, section 12; testing-strategy.md, section 6).
#[must_use]
pub fn run_replayed(settings: Settings) -> World {
    let mut first = World::new(settings);
    let mut replay = World::new(settings);
    let (mut first_state, mut replay_state) = (String::new(), String::new());
    for _ in 0..600 {
        if first.settled() {
            assert!(replay.settled());
            return first;
        }
        first.iterate();
        replay.iterate();
        // Compare the accumulated trace directly: formatting its strings anew
        // at every iteration only repeats an exact comparison of the same data.
        let first_trace = std::mem::take(&mut first.trace);
        let replay_trace = std::mem::take(&mut replay.trace);
        assert_eq!(first_trace, replay_trace, "trace at iteration {}", first.iteration);
        first_state.clear();
        replay_state.clear();
        write!(first_state, "{first:?}").expect("write to string");
        write!(replay_state, "{replay:?}").expect("write to string");
        assert_eq!(first_state, replay_state, "complete frozen state at iteration {}", first.iteration);
        first.trace = first_trace;
        replay.trace = replay_trace;
    }
    panic!("walking replay did not settle");
}

#[cfg(test)]
mod tests {
    use super::{Settings, World};
    use crate::walking_referee::REPORT;
    use skein_lib::Queue;
    use temper_engine_domain::{self as engine, Delivery, PartyRequest, Request};

    #[test]
    #[should_panic(expected = "committed result reaches person once: \"person received result twice\"")]
    fn a_queued_late_result_prevents_settlement_and_reaches_the_referee() {
        let mut world = World::new(Settings::calm(66));
        world.run();
        let person = world.referee.person().expect("story's person");
        let task = world.assignment.as_ref().expect("script retained assignment").task;
        world.out = Queue::with_capacity(engine::max_out(&world.environment.limits) + 1);
        world.out.push(Request::Party(PartyRequest::Deliver(Delivery::Result { person, task, words: REPORT.into() })));
        assert!(!world.settled(), "queued outward work cannot be hidden by completed story counters");
        world.run();
    }
}

mod creating;
pub use creating::{chat, configured_chat, goal, goal_from};
