//! Parsing unit tests for `super` (the `args_parse` module), split out to keep
//! files within the line budget.

use super::*;

// Backend-selection / Forgejo-secret parsing tests live in a sibling file to
// keep each test file within the line budget; they reuse the helpers below.
#[path = "args_parse_backend_tests.rs"]
mod backend;

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_string()).collect()
}

/// Parses with an empty environment so the suite never reads real secrets.
fn run(parts: &[&str]) -> WorkerArgs {
    run_env(parts, &[])
}

/// Parses with a fixed environment map.
fn run_env(parts: &[&str], env: &[(&str, &str)]) -> WorkerArgs {
    match parse_with_env(argv(parts), env_lookup(env)).expect("args parse") {
        ParseOutcome::Run(args) => *args,
        ParseOutcome::Help => panic!("unexpected help outcome"),
    }
}

fn parse_env(parts: &[&str], env: &[(&str, &str)]) -> Result<ParseOutcome, ArgsError> {
    parse_with_env(argv(parts), env_lookup(env))
}

fn env_lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let owned: Vec<(String, String)> = pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    move |key: &str| {
        owned
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
    }
}

#[test]
fn parses_provision() {
    let args = run(&[
        "--kind",
        "provision",
        "--root",
        "/tmp/x",
        "--repo",
        "acme/service",
    ]);
    assert_eq!(args.kind, WorkerKind::Provision);
    assert_eq!(args.owner, "acme");
    assert_eq!(args.name, "service");
    assert_eq!(args.clock, ClockKind::Deterministic);
    // Filesystem is the default backend so the existing multiprocess test
    // is untouched.
    assert_eq!(args.backend, Backend::Filesystem);
}

#[test]
fn parses_multiple_repositories() {
    let args = run(&[
        "--kind",
        "mechanical",
        "--root",
        "/tmp/x",
        "--repo",
        "acme/service-alpha",
        "--repo",
        "acme/service-beta",
    ]);
    assert_eq!(args.owner, "acme");
    assert_eq!(args.name, "service-alpha");
    assert_eq!(args.repositories.len(), 2);
    assert_eq!(args.repositories[1].name, "service-beta");
}

#[test]
fn parses_role_with_identity() {
    let args = run(&[
        "--kind",
        "role",
        "--role",
        "engineer",
        "--user",
        "engineer",
        "--root",
        "/tmp/x",
        "--repo",
        "acme/service",
        "--poll-ms",
        "10",
        "--clock",
        "wall",
    ]);
    assert_eq!(
        args.kind,
        WorkerKind::Role {
            role: "engineer".into(),
            user: "engineer".into(),
            behavior: RoleBehavior::default(),
        }
    );
    assert_eq!(args.poll_interval, Duration::milliseconds(10));
    assert_eq!(args.clock, ClockKind::Wall);
}

#[test]
fn parses_role_behavior_variants() {
    let args = run(&[
        "--kind",
        "role",
        "--role",
        "reviewer",
        "--user",
        "reviewer",
        "--reviewer",
        "request-changes-then-approve",
        "--architect",
        "closing",
        "--root",
        "/tmp/x",
        "--repo",
        "acme/service",
    ]);
    assert_eq!(
        args.kind,
        WorkerKind::Role {
            role: "reviewer".into(),
            user: "reviewer".into(),
            behavior: RoleBehavior {
                architect: ArchitectKind::Closing,
                reviewer: ReviewerKind::RequestChangesThenApprove,
                ci_sentinel: CiSentinelKind::Present,
            },
        }
    );
}

#[test]
fn parses_deferred_ci_sentinel() {
    let args = run(&[
        "--kind",
        "role",
        "--role",
        "engineer",
        "--user",
        "engineer",
        "--ci-sentinel",
        "deferred",
        "--root",
        "/tmp/x",
        "--repo",
        "acme/service",
    ]);
    let WorkerKind::Role { behavior, .. } = args.kind else {
        panic!("expected a role worker");
    };
    assert_eq!(behavior.ci_sentinel, CiSentinelKind::Deferred);
}

#[test]
fn rejects_unknown_ci_sentinel() {
    let error = parse_env(
        &[
            "--kind",
            "role",
            "--role",
            "engineer",
            "--user",
            "engineer",
            "--ci-sentinel",
            "sometimes",
            "--root",
            "/tmp/x",
            "--repo",
            "acme/service",
        ],
        &[],
    )
    .expect_err("an unknown --ci-sentinel value must be rejected");
    assert!(error.to_string().contains("--ci-sentinel"));
}

#[test]
fn parses_ci_policy() {
    let args = run(&[
        "--kind",
        "ci",
        "--ci",
        "fail-then-pass",
        "--root",
        "/tmp/x",
        "--repo",
        "acme/service",
    ]);
    assert_eq!(
        args.kind,
        WorkerKind::Ci {
            policy: CiPolicyKind::FailThenPass
        }
    );
}

#[test]
fn rejects_bad_reviewer() {
    let error = parse(argv(&[
        "--kind",
        "role",
        "--role",
        "reviewer",
        "--user",
        "reviewer",
        "--reviewer",
        "bogus",
        "--root",
        "/tmp/x",
        "--repo",
        "acme/service",
    ]))
    .unwrap_err();
    assert!(error.to_string().contains("--reviewer"));
}

#[test]
fn help_short_circuits() {
    assert_eq!(parse(argv(&["--help"])), Ok(ParseOutcome::Help));
}

#[test]
fn role_requires_identity() {
    let error = parse(argv(&[
        "--kind",
        "role",
        "--root",
        "/tmp/x",
        "--repo",
        "acme/service",
    ]))
    .unwrap_err();
    assert!(error.to_string().contains("--role"));
}

#[test]
fn rejects_bad_repo() {
    let error = parse(argv(&[
        "--kind",
        "provision",
        "--root",
        "/tmp/x",
        "--repo",
        "no-slash",
    ]))
    .unwrap_err();
    assert!(error.to_string().contains("owner/name"));
}

#[test]
fn rejects_unknown_flag() {
    let error = parse(argv(&["--kind", "provision", "--bogus", "x"])).unwrap_err();
    assert!(error.to_string().contains("unrecognized argument"));
}

#[test]
fn agents_defaults_to_fake() {
    let args = run(&[
        "--kind",
        "role",
        "--role",
        "engineer",
        "--user",
        "engineer",
        "--root",
        "/tmp/x",
        "--repo",
        "acme/service",
    ]);
    assert_eq!(args.agents, AgentsKind::Fake);
}

#[test]
fn rejects_agents_real_after_smith_split() {
    let error = parse(argv(&[
        "--kind",
        "role",
        "--role",
        "engineer",
        "--user",
        "engineer",
        "--agents",
        "real",
        "--root",
        "/tmp/x",
        "--repo",
        "acme/service",
    ]))
    .unwrap_err();
    assert!(error.to_string().contains("moved out of Temper"));
}

#[test]
fn rejects_provider_auth_flags_after_smith_split() {
    let error = parse(argv(&[
        "--kind",
        "role",
        "--role",
        "engineer",
        "--user",
        "engineer",
        "--auth",
        "chatgpt-oauth",
        "--root",
        "/tmp/x",
        "--repo",
        "acme/service",
    ]))
    .unwrap_err();
    assert!(error.to_string().contains("unrecognized argument '--auth'"));
}

#[test]
fn parses_idle_poll_backoff_cap() {
    let args = run(&[
        "--kind",
        "mechanical",
        "--root",
        "/tmp/x",
        "--repo",
        "acme/service",
    ]);
    assert_eq!(
        args.idle_poll_max_interval,
        Duration::milliseconds(DEFAULT_IDLE_POLL_MAX_MS)
    );

    let args = run(&[
        "--kind",
        "mechanical",
        "--root",
        "/tmp/x",
        "--repo",
        "acme/service",
        "--idle-poll-max-ms",
        "2500",
    ]);
    assert_eq!(args.idle_poll_max_interval, Duration::milliseconds(2500));
}

#[test]
fn parses_audit_interval_and_allows_disabling_audit() {
    let args = run(&[
        "--kind",
        "mechanical",
        "--root",
        "/tmp/x",
        "--repo",
        "acme/service",
        "--audit-ms",
        "2500",
    ]);
    assert_eq!(args.audit_interval, Some(Duration::milliseconds(2500)));

    let args = run(&[
        "--kind",
        "mechanical",
        "--root",
        "/tmp/x",
        "--repo",
        "acme/service",
        "--audit-ms",
        "0",
    ]);
    assert_eq!(args.audit_interval, None);
}

#[test]
fn parses_wake_socket_options() {
    let args = run(&[
        "--kind",
        "mechanical",
        "--root",
        "/tmp/x",
        "--repo",
        "acme/service",
        "--wake-socket",
        "/tmp/worker.sock",
        "--wake-secret-file",
        "/tmp/wake-secret",
    ]);
    assert_eq!(args.wake_socket, Some(PathBuf::from("/tmp/worker.sock")));
    assert_eq!(
        args.wake_secret_file,
        Some(PathBuf::from("/tmp/wake-secret"))
    );
}

#[test]
fn rejects_bad_agents() {
    let error = parse(argv(&[
        "--kind",
        "role",
        "--role",
        "engineer",
        "--user",
        "engineer",
        "--agents",
        "bogus",
        "--root",
        "/tmp/x",
        "--repo",
        "acme/service",
    ]))
    .unwrap_err();
    assert!(error.to_string().contains("--agents"));
}

#[test]
fn workflow_defaults_to_none() {
    // With neither the flag nor the env var, the worker uses the bundled
    // reference-delivery default (`None`), preserving today's behavior.
    let args = run(&[
        "--kind",
        "provision",
        "--root",
        "/tmp/x",
        "--repo",
        "acme/service",
    ]);
    assert_eq!(args.workflow_file, None);
}

#[test]
fn parses_workflow_flag() {
    let args = run(&[
        "--kind",
        "provision",
        "--root",
        "/tmp/x",
        "--repo",
        "acme/service",
        "--workflow",
        "/tmp/basic-delivery.json",
    ]);
    assert_eq!(
        args.workflow_file,
        Some(PathBuf::from("/tmp/basic-delivery.json"))
    );
}

#[test]
fn workflow_falls_back_to_env() {
    // No flag: the env var supplies the workflow path.
    let args = run_env(
        &[
            "--kind",
            "provision",
            "--root",
            "/tmp/x",
            "--repo",
            "acme/service",
        ],
        &[(WORKFLOW_FILE_ENV, "/env/workflow.json")],
    );
    assert_eq!(
        args.workflow_file,
        Some(PathBuf::from("/env/workflow.json"))
    );
}

#[test]
fn workflow_flag_beats_env() {
    // Both set: the flag wins, mirroring the production worker.
    let args = run_env(
        &[
            "--kind",
            "provision",
            "--root",
            "/tmp/x",
            "--repo",
            "acme/service",
            "--workflow",
            "/flag/workflow.json",
        ],
        &[(WORKFLOW_FILE_ENV, "/env/workflow.json")],
    );
    assert_eq!(
        args.workflow_file,
        Some(PathBuf::from("/flag/workflow.json"))
    );
}

#[test]
fn empty_workflow_env_is_ignored() {
    // A blank env value is treated as unset, leaving the bundled default.
    let args = run_env(
        &[
            "--kind",
            "provision",
            "--root",
            "/tmp/x",
            "--repo",
            "acme/service",
        ],
        &[(WORKFLOW_FILE_ENV, "   ")],
    );
    assert_eq!(args.workflow_file, None);
}
