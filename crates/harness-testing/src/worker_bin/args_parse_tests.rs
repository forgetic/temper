//! Parsing unit tests for `super` (the `args_parse` module), split out to keep
//! files within the line budget.

use super::*;

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
fn parses_forgejo_role_with_env_secrets() {
    let args = run_env(
        &[
            "--kind",
            "role",
            "--role",
            "engineer",
            "--user",
            "engineer",
            "--backend",
            "forgejo",
            "--base-url",
            "http://127.0.0.1:3000/",
            "--root",
            "/tmp/unused",
            "--repo",
            "acme/service",
            "--clock",
            "wall",
        ],
        &[
            (FORGEJO_TOKEN_ENV, "tok-engineer"),
            (FORGEJO_USERNAME_ENV, "engineer"),
            (FORGEJO_PASSWORD_ENV, "pw-engineer"),
        ],
    );
    assert_eq!(
        args.backend,
        Backend::Forgejo(ForgejoArgs {
            base_url: "http://127.0.0.1:3000/".to_string(),
            token: "tok-engineer".to_string(),
            username: Some("engineer".to_string()),
            password: Some("pw-engineer".to_string()),
        })
    );
    assert_eq!(args.backend.kind(), BackendKind::Forgejo);
}

#[test]
fn forgejo_token_comes_from_env_not_argv() {
    let error = parse_env(
        &[
            "--kind",
            "role",
            "--role",
            "engineer",
            "--user",
            "engineer",
            "--backend",
            "forgejo",
            "--base-url",
            "http://127.0.0.1:3000",
            "--root",
            "/tmp/unused",
            "--repo",
            "acme/service",
            "--clock",
            "wall",
        ],
        &[],
    )
    .unwrap_err();
    assert!(error.to_string().contains(FORGEJO_TOKEN_ENV));
}

#[test]
fn forgejo_requires_base_url() {
    let error = parse_env(
        &[
            "--kind",
            "role",
            "--role",
            "engineer",
            "--user",
            "engineer",
            "--backend",
            "forgejo",
            "--root",
            "/tmp/unused",
            "--repo",
            "acme/service",
            "--clock",
            "wall",
        ],
        &[(FORGEJO_TOKEN_ENV, "tok")],
    )
    .unwrap_err();
    assert!(error.to_string().contains("--base-url"));
}

#[test]
fn forgejo_rejects_ci_kind() {
    let error = parse_env(
        &[
            "--kind",
            "ci",
            "--backend",
            "forgejo",
            "--base-url",
            "http://127.0.0.1:3000",
            "--root",
            "/tmp/unused",
            "--repo",
            "acme/service",
            "--clock",
            "wall",
        ],
        &[(FORGEJO_TOKEN_ENV, "tok")],
    )
    .unwrap_err();
    assert!(error.to_string().contains("--kind ci"));
    assert!(error.to_string().contains("forgejo"));
}

#[test]
fn forgejo_requires_wall_clock() {
    let error = parse_env(
        &[
            "--kind",
            "role",
            "--role",
            "engineer",
            "--user",
            "engineer",
            "--backend",
            "forgejo",
            "--base-url",
            "http://127.0.0.1:3000",
            "--root",
            "/tmp/unused",
            "--repo",
            "acme/service",
        ],
        &[(FORGEJO_TOKEN_ENV, "tok")],
    )
    .unwrap_err();
    assert!(error.to_string().contains("--clock wall"));
}

#[test]
fn filesystem_ci_kind_still_accepted() {
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
    assert_eq!(args.backend, Backend::Filesystem);
    assert_eq!(
        args.kind,
        WorkerKind::Ci {
            policy: CiPolicyKind::FailThenPass
        }
    );
}

#[test]
fn rejects_bad_backend() {
    let error = parse(argv(&[
        "--kind",
        "provision",
        "--backend",
        "bogus",
        "--root",
        "/tmp/x",
        "--repo",
        "acme/service",
    ]))
    .unwrap_err();
    assert!(error.to_string().contains("--backend"));
}

#[test]
fn base_url_rejected_for_filesystem() {
    let error = parse(argv(&[
        "--kind",
        "provision",
        "--base-url",
        "http://127.0.0.1:3000",
        "--root",
        "/tmp/x",
        "--repo",
        "acme/service",
    ]))
    .unwrap_err();
    assert!(error.to_string().contains("--base-url"));
}

#[test]
fn forgejo_debug_redacts_secrets() {
    let args = ForgejoArgs {
        base_url: "http://127.0.0.1:3000".to_string(),
        token: "super-secret-token".to_string(),
        username: Some("engineer".to_string()),
        password: Some("super-secret-password".to_string()),
    };
    let rendered = format!("{args:?}");
    assert!(!rendered.contains("super-secret-token"));
    assert!(!rendered.contains("super-secret-password"));
    assert!(rendered.contains("<redacted>"));
    assert!(rendered.contains("http://127.0.0.1:3000"));
}
