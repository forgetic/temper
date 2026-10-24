//! Unit configuration uses the existing engine world's tiny limits and charter,
//! with no templates. No store, worker or connector driver is added.
use crate as engine;
use crate::{JournalLimits, loads};
use jig_domain_accounts as accounts;
use jig_domain_authority as authority;
use jig_domain_fleet as fleet;
use jig_domain_people as people;
use jig_domain_tasks as tasks;
use skein_lib::{Duration, Env, Time, Wall};
use temper_engine_domain_forge as forge;

#[expect(clippy::too_many_lines, reason = "one fixture lists the real children's tiny admission limits together")]
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
        authority_segments: 1,
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
        batch: 4,
        act_grants: 0,
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
        templates: 2,
        documents: jig_domain_core::documents::DocumentLimits {
            roles: 1,
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
            held: deliveries.checked_mul(3).unwrap().checked_add(16).unwrap(),
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
            accounts: 1,
            move_deadline: Duration::from_secs(60),
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
        forge: FORGE_LIMITS,
    };
    limits.journal.writes = crate::limits::route_bound(&limits).unwrap().checked_add(limits.call_records).unwrap();
    limits
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
    config_with_project_room(seed, 1)
}

pub fn config_with_project_room(seed: u64, projects: u32) -> engine::Config {
    let ceiling = authority_value(1000, Box::new([authority::Executor::Charter(1)]));
    let rules = authority::Rules {
        ceiling: ceiling.clone(),
        period_spend: 1000,
        minimum_run_spend: 1,
        maximum_run_spend: 100,
        implies: authority::Implies::new(Box::new([]), 0).expect("empty implication configuration"),
        requirements: Box::new([]),
    };
    let mut policy_limits = authority_limits();
    policy_limits.projects = projects;
    // Check the same fixture bounds without allocating a discarded live policy table.
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
                templates: Box::new([]),
                policies: skein_lib::Map::with_capacity(authority_limits().projects),
            },
            settings: jig_domain_core::Settings {
                key_bytes: 256,
                redirect_bytes: 256,
                deployment_provider: 1,
                resume_bytes: 256,
                tail_bytes: 2048,
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

const CLIENT: skein_forge_client::Limits = skein_forge_client::Limits {
    pending: 4,
    calls: 2,
    rate: 100,
    reserve: 0,
    window: Duration::from_secs(10),
    op_bytes: 512,
    answer_bytes: 4096,
    rows: 8,
    inbox: 16,
    read_attempts: 3,
    backoff: Duration::from_secs(1),
    backoff_max: Duration::from_secs(4),
    entries: 8,
    write_attempts: 3,
    lifetime: Duration::from_secs(10),
    clock_margin: Duration::from_secs(1),
    resources: 8,
    repositories: 2,
    poll: Duration::from_secs(5),
    poll_max: Duration::from_secs(30),
    hinted: Duration::from_secs(1),
    slow: Duration::from_secs(40),
    settle: Duration::from_secs(30),
    facts: 16,
};
const FORGE_LIMITS: forge::Limits = forge::Limits {
    repositories: 2,
    tasks: 8,
    holds: 8,
    subscriptions: 8,
    entries: 8,
    named_calls: 16,
    changes: 8,
    issues: 8,
    resources_per_task: 4,
    paths_per_subscription: 4,
    name_bytes: 128,
    output: 80,
    facts: 16,
    adoptions: 2,
    applications: 4,
    bindings: 4,
    collaborators: 8,
    landings: 8,
    judge_projects: 2,
    judge_criteria: 8,
    brief_sections: 32,
    brief_bytes: 4096,
    client: CLIENT,
    issue_policy: temper_engine_domain_forge_issues::Limits {
        plan_items: 8,
        milestones: 8,
        title_bytes: 128,
        body_bytes: 512,
        comment_bytes: 256,
        interval: Duration::from_secs(5),
    },
    change_policy: temper_engine_domain_forge_change::Limits {
        repairs: 3,
        resolutions: 3,
        updates: 3,
        stall: Duration::from_secs(30),
        gates: 8,
        clean_heads: 8,
    },
    queue_window: Duration::from_secs(30),
};

// Seven restart steps for one connector, followed by Idle.
const RESTORE_PASSES: u32 = 8;

pub fn root() -> (engine::Domain, Env<engine::Limits>) {
    root_with_config(config(7))
}
pub fn root_with_config(config: engine::Config) -> (engine::Domain, Env<engine::Limits>) {
    root_with_config_and_limits(config, &limits())
}

pub fn root_with_config_and_limits(
    config: engine::Config,
    limits: &engine::Limits,
) -> (engine::Domain, Env<engine::Limits>) {
    // Model a later start: the store supplies the complete project rows and
    // their deployment high-water mark, rather than configuration reseeding.
    let mut projects = skein_lib::List::with_capacity(limits.authority.projects);
    let mut project_mark = 0;
    for project in &config.core.seed_projects {
        project_mark = project_mark.max(u64::from(project.number));
        let value = jig_domain_core::policy_snapshot(
            &project.policy.policy,
            &project.policy.permissions,
            &project.policy.templates,
        )
        .expect("fixture policy snapshot");
        projects
            .push(people::Stored::Policy { project: project.number, name: project.name.clone(), value })
            .expect("bounded fixture projects");
    }
    let mut root = engine::Domain::new(config, limits).unwrap();
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: *limits };
    let mut request = root.core.restart_begin();
    let mut deployment = root.core.counters.deployment();
    deployment.projects = project_mark;
    engine::restart::restore_page_row(
        &mut root,
        &env,
        engine::Record::Core(jig_domain_core::Record::Core(jig_domain_core::CoreRecord::Deployment(deployment))),
    );
    for record in projects.into_boxed() {
        engine::restart::restore_page_row(
            &mut root,
            &env,
            engine::Record::Core(jig_domain_core::Record::People(record)),
        );
    }
    // In production the page's own write-free decision takes the header's
    // commit count while the journal is still idle.
    if let Some(commits) = root.restored_commits.take() {
        root.journal = engine::Journal::from_durable(&crate::limits::root_journal_limits(&env.limits), commits);
    }
    for _ in 0..RESTORE_PASSES {
        match request {
            jig_domain_core::RestartRequest::Step(step) => request = root.core.restart_done(step),
            jig_domain_core::RestartRequest::Idle => break,
            jig_domain_core::RestartRequest::Settings { failure, changed } => {
                panic!("fixture settings restore: {failure:?}, changed {changed}")
            }
            jig_domain_core::RestartRequest::Refused { step } => panic!("fixture restore: {step:?}"),
        }
    }
    assert!(root.ready(), "fixture store restored");
    (root, env)
}
