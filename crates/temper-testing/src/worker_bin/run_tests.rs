use super::*;
use crate::worker_bin::args::{AgentsKind, WorkerArgs};
use crate::{runner_config, workflow};
use temper_runner::Worker;

fn temp_root(suite: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "temper-testing-worker-{suite}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    path
}

fn base_args(root: PathBuf, kind: WorkerKind) -> WorkerArgs {
    WorkerArgs {
        kind,
        backend: Backend::Filesystem,
        root,
        owner: "acme".into(),
        name: "service".into(),
        repositories: vec![RepositoryPath::new("acme", "service")],
        poll_interval: Duration::milliseconds(1),
        wake_debounce: temper_wake::DEFAULT_WAKE_DEBOUNCE,
        idle_poll_max_interval: Duration::milliseconds(60_000),
        audit_interval: Some(Duration::milliseconds(600_000)),
        stop_file: None,
        run_secs: Some(0),
        clock: ClockKind::Deterministic,
        agents: AgentsKind::Fake,
        wake_socket: None,
        wake_secret_file: None,
        workflow_file: None,
    }
}

#[test]
fn each_kind_wires_up_against_a_temp_root() {
    let root = temp_root("wireup");
    run(&base_args(root.clone(), WorkerKind::Provision)).expect("provision succeeds");

    let workflow = workflow();
    let compiled = workflow.compile();
    let config = runner_config();
    let registry = registry_for(RoleBehavior::default());
    for role in compiled.roles() {
        if registry.get(&role.id).is_none() {
            continue;
        }
        let user = resolve_role_user(&config, &role.id, role.id.as_str());
        let forge = FilesystemForge::new(&root).as_user(user);
        let repo = block_on(resolve_repository(&forge, "acme", "service")).expect("repo");
        let agent = registry.get(&role.id).expect("agent").clone();
        let worker = RoleWorker::new(
            &workflow,
            &compiled,
            &forge as &dyn Forge,
            &repo,
            role.id.clone(),
            agent,
            config.execution_context(&role.id),
        );
        let clock = ManualClock::with_tick_step(epoch(), Duration::seconds(1));
        let poll = PollLoop::with_clock(&worker, Duration::milliseconds(1), clock);
        block_on(poll.run_bounded(2)).expect("role worker ticks");
    }

    let forge = FilesystemForge::new(&root);
    let repo = block_on(resolve_repository(&forge, "acme", "service")).expect("repo");
    let journal = InMemoryJournal::new();
    let worker = MechanicalWorker::new(
        &workflow,
        &forge as &dyn Forge,
        &repo,
        &journal,
        LeasePolicy::new(config.lease_ttl),
    );
    assert_eq!(worker.name(), "mechanical");
    let clock = ManualClock::with_tick_step(epoch(), Duration::seconds(1));
    let poll = PollLoop::with_clock(&worker, Duration::milliseconds(1), clock);
    block_on(poll.run_bounded(2)).expect("mechanical worker ticks");

    for policy in [
        CiPolicyKind::Pass,
        CiPolicyKind::FailThenPass,
        CiPolicyKind::FixedFail,
    ] {
        let report = run(&base_args(root.clone(), WorkerKind::Ci { policy })).expect("ci runs");
        assert_eq!(report.workers.len(), 1);
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn run_role_reports_unknown_role() {
    let root = temp_root("unknown-role");
    run(&base_args(root.clone(), WorkerKind::Provision)).expect("provision");
    let error = run(&base_args(
        root.clone(),
        WorkerKind::Role {
            role: "ghost".into(),
            user: "ghost".into(),
            behavior: RoleBehavior::default(),
        },
    ))
    .unwrap_err();
    assert!(matches!(error, RunError::UnknownRole { .. }));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn resolve_repository_reports_missing() {
    let root = temp_root("missing-repo");
    let forge = FilesystemForge::new(&root);
    let error = block_on(resolve_repository(&forge, "acme", "service")).unwrap_err();
    assert!(matches!(error, RunError::RepositoryMissing { .. }));
    let _ = std::fs::remove_dir_all(&root);
}
