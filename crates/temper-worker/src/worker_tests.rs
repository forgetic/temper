use super::*;
use crate::worker_role_agent::guard_role_for_manifest;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use temper_coding_workspace::pr_diff_guard::GuardRole;
use temper_forge::{ChangeKind, CreateIssue, CreateRepository, RepositoryId};
use temper_forge_memory::MemoryForge;
use temper_runner::{Agent, AgentError, RoleTools, WorkItem};
use temper_wake::{send_wake, send_wake_with_hint, wait_for_wake_or_poll, WakeWaitOutcome};

fn temp_path(name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "temper-worker-{name}-{}-{}.sock",
        std::process::id(),
        chrono::Utc::now()
            .timestamp_nanos_opt()
            .expect("timestamp has nanos")
    ));
    path
}

fn runtime() -> temper_io_engine::EngineRuntime {
    temper_io_engine::build_runtime().expect("engine runtime builds")
}

#[test]
fn production_role_workers_use_process_decisions_not_in_process_sdk_agents() {
    let worker_source = format!(
        "{}{}",
        include_str!("worker.rs"),
        include_str!("worker_role_agent.rs")
    );
    assert!(worker_source.contains("WorkflowRoleDecisionProcessAgent"));
    assert!(!worker_source.contains("temper_agents"));
    assert!(!worker_source.contains("real_registry"));
    for prompt_constant in [
        "ENGINEER_SYSTEM_PROMPT",
        "ARCHITECT_SYSTEM_PROMPT",
        "REVIEWER_SYSTEM_PROMPT",
        "OWNER_SYSTEM_PROMPT",
        "HUMAN_SYSTEM_PROMPT",
    ] {
        assert!(
            !worker_source.contains(prompt_constant),
            "production worker must not reference {prompt_constant}"
        );
    }
}

#[test]
fn dogfood_reference_engineer_declares_coding_workspace() {
    let compiled = temper_reference_delivery::workflow().compile();
    let engineer = compiled
        .role(&RoleId::new("engineer"))
        .expect("engineer manifest exists");

    assert!(engineer.external_tools.iter().any(|tool| {
        tool.id.as_str() == temper_runner::CODING_WORKSPACE_TOOL_ID
            && tool.description.contains("checked-out repository")
    }));
    assert!(engineer
        .prompt_extension
        .guidance
        .as_deref()
        .is_some_and(|guidance| guidance.contains("real product diff")));
    assert!(engineer.prompt.render().contains("coding_workspace"));
}

#[test]
fn dogfood_runner_config_omits_automation_only_role_binding() {
    let config = temper_reference_delivery::runner_config();
    assert!(config.role_binding(&RoleId::new("engineer")).is_some());
    assert!(config.role_binding(&RoleId::new("mechanical")).is_none());
}

#[test]
fn pr_diff_guard_targets_are_derived_from_role_manifests() {
    let compiled = temper_reference_delivery::workflow().compile();

    let reviewer = guard_role_for_manifest(
        &compiled,
        compiled
            .role(&RoleId::new("reviewer"))
            .expect("reviewer manifest exists"),
    )
    .expect("reviewer gets a guard");
    assert!(matches!(
        reviewer,
        GuardRole::Reviewer {
            ref request_changes,
            ref queues
        } if request_changes.as_str() == "request_changes"
            && queues.iter().any(|queue| queue.as_str() == "pr_needs_review")
    ));

    let owner = compiled
        .role(&RoleId::new("owner"))
        .expect("owner manifest exists");
    assert!(guard_role_for_manifest(&compiled, owner).is_none());

    let engineer = compiled
        .role(&RoleId::new("engineer"))
        .expect("engineer manifest exists");
    assert!(guard_role_for_manifest(&compiled, engineer).is_none());

    let mechanical = compiled
        .role(&RoleId::new("mechanical"))
        .expect("mechanical manifest exists");
    assert!(mechanical.queues.is_empty());
    assert!(guard_role_for_manifest(&compiled, mechanical).is_none());
}

#[test]
fn resolves_multiple_repositories_and_reports_missing_without_secret() {
    let forge = MemoryForge::new();
    let runtime = runtime();
    runtime
        .block_on(forge.create_repository(CreateRepository {
            owner: "acme".into(),
            name: "service".into(),
            default_branch: "main".into(),
            description: None,
        }))
        .expect("repo creates");

    let repositories = runtime
        .block_on(resolve_repositories(
            &forge,
            &[
                RepositoryPath::new("acme", "service"),
                RepositoryPath::new("acme", "missing"),
            ],
        ))
        .unwrap_err();

    let rendered = repositories.to_string();
    assert!(rendered.contains("acme/missing"));
    assert!(rendered.contains("not found or not readable"));
    assert!(!rendered.contains("secret-token"));
}

#[test]
fn known_hints_drop_unknown_repos_so_wake_becomes_broad_scan() {
    let repositories = RepositorySet::new(vec![RepositoryTarget::new(
        temper_forge::RepositoryId::new("repo-1"),
        RepositoryPath::new("acme", "service"),
    )]);
    let known = ChangeHint::repo(RepositoryPath::new("acme", "service"), ChangeKind::Issue);
    let unknown = ChangeHint::repo(RepositoryPath::new("acme", "other"), ChangeKind::Issue);

    assert_eq!(
        known_hints_for(&repositories, std::slice::from_ref(&known), "test"),
        vec![known]
    );
    assert!(known_hints_for(&repositories, &[unknown], "test").is_empty());
}

struct RecordingAgent {
    seen: Arc<Mutex<Vec<RepositoryId>>>,
}

#[async_trait::async_trait]
impl Agent<MemoryForge> for RecordingAgent {
    async fn service(
        &self,
        _item: &WorkItem,
        tools: &RoleTools<'_, MemoryForge>,
    ) -> Result<bool, AgentError> {
        self.seen
            .lock()
            .expect("recording mutex")
            .push(tools.repo().clone());
        Ok(false)
    }
}

#[test]
fn production_role_wake_with_known_hint_scans_only_that_repo() {
    let forge = MemoryForge::new();
    let runtime = runtime();
    let repo_a = runtime
        .block_on(forge.create_repository(CreateRepository {
            owner: "acme".into(),
            name: "alpha".into(),
            default_branch: "main".into(),
            description: None,
        }))
        .expect("repo a creates");
    let repo_b = runtime
        .block_on(forge.create_repository(CreateRepository {
            owner: "acme".into(),
            name: "bravo".into(),
            default_branch: "main".into(),
            description: None,
        }))
        .expect("repo b creates");
    for repo in [&repo_a.id, &repo_b.id] {
        runtime
            .block_on(forge.create_issue(
                repo,
                CreateIssue {
                    title: "ready work".into(),
                    body: String::new(),
                    labels: vec!["code".into(), "ready".into()],
                    assignees: Vec::new(),
                },
            ))
            .expect("issue creates");
    }
    let workflow = temper_reference_delivery::workflow();
    let compiled = workflow.compile();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let worker = MultiRepoRoleWorker::new(
        &workflow,
        &compiled,
        &forge,
        RepositorySet::new(vec![
            RepositoryTarget::new(
                repo_a.id.clone(),
                RepositoryPath::new(repo_a.owner.clone(), repo_a.name.clone()),
            ),
            RepositoryTarget::new(
                repo_b.id.clone(),
                RepositoryPath::new(repo_b.owner.clone(), repo_b.name.clone()),
            ),
        ]),
        RoleId::new("engineer"),
        Arc::new(RecordingAgent {
            seen: Arc::clone(&seen),
        }),
        temper_workflow::ExecutionContext::new(),
    );
    let hint = ChangeHint::repo(RepositoryPath::new("acme", "bravo"), ChangeKind::Issue);

    let report = runtime
        .block_on(worker.tick_for_reason(
            chrono::Utc::now(),
            TickReason::Wake,
            &[hint],
            "tick/test/wake/1",
        ))
        .expect("wake tick succeeds");

    assert_eq!(report.scanned_repository_count, 1);
    assert_eq!(
        report.scanned_repository_paths,
        vec!["acme/bravo".to_string()]
    );
    assert_eq!(*seen.lock().unwrap(), vec![repo_b.id.clone()]);

    seen.lock().unwrap().clear();
    let unknown = ChangeHint::repo(RepositoryPath::new("acme", "charlie"), ChangeKind::Issue);
    let report = runtime
        .block_on(worker.tick_for_reason(
            chrono::Utc::now(),
            TickReason::Wake,
            &[unknown],
            "tick/test/wake/2",
        ))
        .expect("unknown hint falls back to broad scan");
    assert_eq!(report.scanned_repository_count, 2);
    assert_eq!(
        report.scanned_repository_paths,
        vec!["acme/alpha".to_string(), "acme/bravo".to_string()]
    );
    assert_eq!(
        *seen.lock().unwrap(),
        vec![repo_a.id.clone(), repo_b.id.clone()]
    );
}

#[test]
fn production_tick_id_is_stable_for_log_correlation() {
    assert_eq!(
        production_tick_id("multi-role:engineer", TickReason::Wake, 3),
        "tick/multi-role:engineer/wake/3"
    );
}

#[test]
fn authenticated_wake_interrupts_long_wait() {
    let socket = temp_path("authenticated");
    let runtime = runtime();
    let mut listener = WakeListener::bind(WakeConfig {
        socket: socket.clone(),
        secret: Some("wake-secret".into()),
    })
    .expect("listener binds");
    let stop = StopSignal::new(None, None);
    let sender = thread::spawn(move || {
        thread::sleep(StdDuration::from_millis(50));
        send_wake(&socket, Some("wake-secret")).expect("wake sends");
    });
    let start = Instant::now();

    let outcome = runtime
        .block_on(wait_for_wake_or_poll(
            &temper_io_engine::Cx::for_testing(),
            || stop.should_stop(),
            StdDuration::from_secs(60),
            Some(&mut listener),
        ))
        .expect("wait succeeds");
    sender.join().expect("sender joins");

    assert_eq!(outcome, WakeWaitOutcome::Wake(Vec::new()));
    assert!(
        start.elapsed() < StdDuration::from_secs(1),
        "authenticated wake should beat the long poll interval"
    );
}

#[test]
fn wake_payload_carries_repository_hint_to_waiter() {
    let socket = temp_path("hinted");
    let runtime = runtime();
    let mut listener = WakeListener::bind(WakeConfig {
        socket: socket.clone(),
        secret: Some("wake-secret".into()),
    })
    .expect("listener binds");
    let stop = StopSignal::new(None, None);
    let hint = ChangeHint::repo(RepositoryPath::new("acme", "service"), ChangeKind::Issue);
    let hint_for_thread = hint.clone();
    let sender = thread::spawn(move || {
        thread::sleep(StdDuration::from_millis(50));
        send_wake_with_hint(&socket, Some("wake-secret"), &hint_for_thread).expect("wake sends");
    });

    let outcome = runtime
        .block_on(wait_for_wake_or_poll(
            &temper_io_engine::Cx::for_testing(),
            || stop.should_stop(),
            StdDuration::from_secs(60),
            Some(&mut listener),
        ))
        .expect("wait succeeds");
    sender.join().expect("sender joins");

    assert_eq!(outcome, WakeWaitOutcome::Wake(vec![hint]));
}

#[test]
fn broad_wake_in_coalesced_batch_forces_broad_wait_outcome() {
    let socket = temp_path("burst");
    let runtime = runtime();
    let mut listener = WakeListener::bind(WakeConfig {
        socket: socket.clone(),
        secret: Some("wake-secret".into()),
    })
    .expect("listener binds");
    let stop = StopSignal::new(None, None);
    let issue_hint = ChangeHint::repo(RepositoryPath::new("acme", "service"), ChangeKind::Issue);
    let pr_hint = ChangeHint::repo(
        RepositoryPath::new("acme", "service-canary"),
        ChangeKind::PullRequest,
    );
    send_wake_with_hint(&socket, Some("wake-secret"), &issue_hint).expect("first wake sends");
    send_wake(&socket, Some("wake-secret")).expect("broad wake sends");
    send_wake_with_hint(&socket, Some("wake-secret"), &pr_hint).expect("second hinted wake sends");

    let outcome = runtime
        .block_on(wait_for_wake_or_poll(
            &temper_io_engine::Cx::for_testing(),
            || stop.should_stop(),
            StdDuration::from_secs(60),
            Some(&mut listener),
        ))
        .expect("wait succeeds");

    assert_eq!(outcome, WakeWaitOutcome::Wake(Vec::new()));
}

#[test]
fn unauthorized_wake_is_ignored_until_stop_or_poll() {
    let socket = temp_path("unauthorized");
    let stop_file = temp_path("stop").with_extension("stop");
    let runtime = runtime();
    let mut listener = WakeListener::bind(WakeConfig {
        socket: socket.clone(),
        secret: Some("wake-secret".into()),
    })
    .expect("listener binds");
    let stop = StopSignal::new(Some(stop_file.clone()), None);
    let stop_file_for_thread = stop_file.clone();
    let sender = thread::spawn(move || {
        thread::sleep(StdDuration::from_millis(50));
        send_wake(&socket, Some("wrong-secret")).expect("unauthorized wake sends");
        thread::sleep(StdDuration::from_millis(150));
        std::fs::write(&stop_file_for_thread, b"stop").expect("stop file writes");
    });
    let start = Instant::now();

    let outcome = runtime
        .block_on(wait_for_wake_or_poll(
            &temper_io_engine::Cx::for_testing(),
            || stop.should_stop(),
            StdDuration::from_secs(60),
            Some(&mut listener),
        ))
        .expect("wait succeeds");
    sender.join().expect("sender joins");

    assert_eq!(outcome, WakeWaitOutcome::Stop);
    assert!(
        start.elapsed() >= StdDuration::from_millis(150),
        "unauthorized wake must not end the wait"
    );
    assert!(
        start.elapsed() < StdDuration::from_secs(2),
        "stop backstop should end the test before the poll interval"
    );
    let _ = std::fs::remove_file(stop_file);
}
