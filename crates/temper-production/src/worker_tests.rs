use super::*;
use crate::pr_diff_guard::GuardRole;
use crate::wake::{send_wake, send_wake_with_hint};
use crate::worker_role_agent::guard_role_for_manifest;
use std::path::PathBuf;
use std::thread;
use temper_forge::{ChangeKind, CreateRepository};
use temper_forge_memory::MemoryForge;

fn temp_path(name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "temper-production-worker-{name}-{}-{}.sock",
        std::process::id(),
        chrono::Utc::now()
            .timestamp_nanos_opt()
            .expect("timestamp has nanos")
    ));
    path
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime builds")
}

#[test]
fn production_real_registry_uses_compiled_manifests_not_prompt_constants() {
    let worker_source = format!(
        "{}{}",
        include_str!("worker.rs"),
        include_str!("worker_role_agent.rs")
    );
    assert!(worker_source.contains("real_registry_from_compiled"));
    assert!(!worker_source.contains("real_registry_with("));
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
    let compiled = crate::workflow().compile();
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
fn pr_diff_guard_targets_are_derived_from_role_manifests() {
    let compiled = crate::workflow().compile();

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

    let owner = guard_role_for_manifest(
        &compiled,
        compiled
            .role(&RoleId::new("owner"))
            .expect("owner manifest exists"),
    )
    .expect("owner gets a guard");
    assert!(matches!(
        owner,
        GuardRole::Owner { ref queues }
            if queues.iter().any(|queue| queue.as_str() == "merge_ready")
                && !queues.iter().any(|queue| queue.as_str() == "owner_alignment")
    ));

    let engineer = compiled
        .role(&RoleId::new("engineer"))
        .expect("engineer manifest exists");
    assert!(guard_role_for_manifest(&compiled, engineer).is_none());
}

#[test]
fn resolves_multiple_repositories_and_reports_missing_without_secret() {
    let forge = MemoryForge::new();
    let runtime = runtime();
    let _guard = runtime.enter();
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
        known_hints_for(&repositories, std::slice::from_ref(&known)),
        vec![known]
    );
    assert!(known_hints_for(&repositories, &[unknown]).is_empty());
}

#[test]
fn authenticated_wake_interrupts_long_wait() {
    let socket = temp_path("authenticated");
    let runtime = runtime();
    let _guard = runtime.enter();
    let listener = WakeListener::bind(WakeConfig {
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
        .block_on(wait_for_next_tick(
            &stop,
            StdDuration::from_secs(60),
            Some(&listener),
        ))
        .expect("wait succeeds");
    sender.join().expect("sender joins");

    assert_eq!(outcome, WaitOutcome::Wake(Vec::new()));
    assert!(
        start.elapsed() < StdDuration::from_secs(1),
        "authenticated wake should beat the long poll interval"
    );
}

#[test]
fn wake_payload_carries_repository_hint_to_waiter() {
    let socket = temp_path("hinted");
    let runtime = runtime();
    let _guard = runtime.enter();
    let listener = WakeListener::bind(WakeConfig {
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
        .block_on(wait_for_next_tick(
            &stop,
            StdDuration::from_secs(60),
            Some(&listener),
        ))
        .expect("wait succeeds");
    sender.join().expect("sender joins");

    assert_eq!(outcome, WaitOutcome::Wake(vec![hint]));
}

#[test]
fn burst_wakes_are_coalesced_into_one_wait_outcome() {
    let socket = temp_path("burst");
    let runtime = runtime();
    let _guard = runtime.enter();
    let listener = WakeListener::bind(WakeConfig {
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
        .block_on(wait_for_next_tick(
            &stop,
            StdDuration::from_secs(60),
            Some(&listener),
        ))
        .expect("wait succeeds");

    assert_eq!(outcome, WaitOutcome::Wake(vec![issue_hint, pr_hint]));
}

#[test]
fn unauthorized_wake_is_ignored_until_stop_or_poll() {
    let socket = temp_path("unauthorized");
    let stop_file = temp_path("stop").with_extension("stop");
    let runtime = runtime();
    let _guard = runtime.enter();
    let listener = WakeListener::bind(WakeConfig {
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
        .block_on(wait_for_next_tick(
            &stop,
            StdDuration::from_secs(60),
            Some(&listener),
        ))
        .expect("wait succeeds");
    sender.join().expect("sender joins");

    assert_eq!(outcome, WaitOutcome::Stop);
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
