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
fn parses_agents_real() {
    let args = run(&[
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
    ]);
    assert_eq!(args.agents, AgentsKind::Real);
}

#[test]
fn auth_defaults_to_chatgpt_oauth() {
    // The worker is a test/dev surface, so it defaults to the flat-rate ChatGPT
    // subscription rather than pay-per-token DeepSeek (the cost policy).
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
    assert_eq!(args.auth, AgentsAuthKind::ChatGptOAuth);
    assert_eq!(args.codex_model, None);
    assert_eq!(args.auth_file, None);
}

#[test]
fn parses_auth_codex_model_and_auth_file() {
    let args = run(&[
        "--kind",
        "role",
        "--role",
        "engineer",
        "--user",
        "engineer",
        "--auth",
        "deepseek",
        "--codex-model",
        "gpt-5.9-codex",
        "--auth-file",
        "/tmp/auth.json",
        "--root",
        "/tmp/x",
        "--repo",
        "acme/service",
    ]);
    assert_eq!(args.auth, AgentsAuthKind::DeepSeek);
    assert_eq!(args.codex_model.as_deref(), Some("gpt-5.9-codex"));
    assert_eq!(args.auth_file, Some(PathBuf::from("/tmp/auth.json")));
}

#[test]
fn auth_env_bridges_config_file() {
    // The launch script sources a config file into HARNESS_AGENTS_AUTH; absent a
    // CLI flag, that env value selects the mode.
    let args = run_env(
        &[
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
        ],
        &[(AGENTS_AUTH_ENV, "deepseek")],
    );
    assert_eq!(args.auth, AgentsAuthKind::DeepSeek);
}

#[test]
fn auth_cli_overrides_env() {
    // Precedence: CLI > config/env > default.
    let args = run_env(
        &[
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
        ],
        &[(AGENTS_AUTH_ENV, "deepseek")],
    );
    assert_eq!(args.auth, AgentsAuthKind::ChatGptOAuth);
}

#[test]
fn rejects_bad_auth() {
    let error = parse(argv(&[
        "--kind",
        "role",
        "--role",
        "engineer",
        "--user",
        "engineer",
        "--auth",
        "bogus",
        "--root",
        "/tmp/x",
        "--repo",
        "acme/service",
    ]))
    .unwrap_err();
    assert!(error.to_string().contains("--auth"));
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
