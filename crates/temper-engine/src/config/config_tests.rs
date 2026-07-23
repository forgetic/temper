// SPDX-License-Identifier: MPL-2.0

//! Unit tests for [`super`] argument and environment parsing.

use super::*;

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_string()).collect()
}

fn run(args: &[&str]) -> DaemonRunConfig {
    match parse(strings(args)).expect("arguments parse") {
        ParseOutcome::Run(config) => config,
        ParseOutcome::Help => panic!("expected run config"),
    }
}

fn repo(owner: &str, name: &str) -> RepositoryPath {
    RepositoryPath::new(owner, name)
}

fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

fn role(id: &str) -> RoleId {
    RoleId::new(id)
}

#[test]
fn role_tokens_resolve_configured_roles_only() {
    let tokens = role_tokens_from_env(
        vec!["engineer".to_string(), "reviewer".to_string()],
        env(&[
            ("TEMPER_FORGEJO_TOKEN_ENGINEER", "engineer-token"),
            ("TEMPER_FORGEJO_TOKEN_ARCHITECT", "architect-token"),
            ("UNRELATED", "ignored"),
        ]),
    );

    assert_eq!(
        tokens,
        BTreeMap::from([("engineer".to_string(), "engineer-token".to_string())])
    );
}

#[test]
fn role_tokens_map_hyphenated_and_non_alphanumeric_roles_to_env_keys() {
    let tokens = role_tokens_from_env(
        vec![
            "ci-reviewer".to_string(),
            "qa.bot/2".to_string(),
            "agent_007".to_string(),
        ],
        env(&[
            ("TEMPER_FORGEJO_TOKEN_CI_REVIEWER", "ci-token"),
            ("TEMPER_FORGEJO_TOKEN_QA_BOT_2", "qa-token"),
            ("TEMPER_FORGEJO_TOKEN_AGENT_007", "agent-token"),
        ]),
    );

    assert_eq!(
        tokens,
        BTreeMap::from([
            ("agent_007".to_string(), "agent-token".to_string()),
            ("ci-reviewer".to_string(), "ci-token".to_string()),
            ("qa.bot/2".to_string(), "qa-token".to_string()),
        ])
    );
}

#[test]
fn role_tokens_treat_trimmed_empty_values_as_absent() {
    let tokens = role_tokens_from_env(
        vec!["engineer".to_string(), "reviewer".to_string()],
        env(&[
            ("TEMPER_FORGEJO_TOKEN_ENGINEER", "  "),
            ("TEMPER_FORGEJO_TOKEN_REVIEWER", "reviewer-token"),
        ]),
    );

    assert_eq!(
        tokens,
        BTreeMap::from([("reviewer".to_string(), "reviewer-token".to_string())])
    );
}

#[test]
fn role_tokens_deduplicate_duplicate_roles() {
    let tokens = role_tokens_from_env(
        vec![
            "engineer".to_string(),
            "engineer".to_string(),
            "reviewer".to_string(),
        ],
        env(&[
            ("TEMPER_FORGEJO_TOKEN_ENGINEER", "engineer-token"),
            ("TEMPER_FORGEJO_TOKEN_REVIEWER", "reviewer-token"),
        ]),
    );

    assert_eq!(tokens.len(), 2);
    assert_eq!(
        tokens.get("engineer").map(String::as_str),
        Some("engineer-token")
    );
    assert_eq!(
        tokens.get("reviewer").map(String::as_str),
        Some("reviewer-token")
    );
}

#[test]
fn defaults_apply_when_only_repo_and_role_given() {
    let config = run(&["--repo", "acme/service", "--role", "engineer"]);

    assert_eq!(config.bind, "127.0.0.1:8080".parse().unwrap());
    assert_eq!(config.repos, vec![repo("acme", "service")]);
    assert_eq!(config.roles, vec![role("engineer")]);
    assert_eq!(config.poll_cadence, Duration::from_secs(300));
    assert_eq!(config.ci_poll_cadence, Some(Duration::from_secs(60)));
    assert_eq!(config.ci_missing_grace, Duration::from_secs(300));
    // The mechanical backstop is on by default; webhooks are the primary
    // reaction path and this is the level-triggered safety net.
    assert_eq!(config.mechanical_cadence, Some(Duration::from_secs(120)));
    assert_eq!(config.lease_ttl, Duration::from_secs(300));
    assert_eq!(config.daemon_id, "temper-daemon-1");
    assert_eq!(config.workflow_file, None);
    assert_eq!(config.webhook_secret_file, None);
}

#[test]
fn repeated_repo_and_role_accumulate_and_dedup() {
    let config = run(&[
        "--repo",
        "a/b",
        "--repo",
        "a/b",
        "--repo",
        "c/d",
        "--role",
        "engineer",
        "--role",
        "architect",
        "--role",
        "engineer",
    ]);

    assert_eq!(config.repos, vec![repo("a", "b"), repo("c", "d")]);
    assert_eq!(config.roles, vec![role("engineer"), role("architect")]);
}

#[test]
fn missing_repo_is_rejected() {
    let error = parse(strings(&["--role", "engineer"])).unwrap_err();

    assert!(error.contains("--repo"));
}

#[test]
fn missing_role_is_rejected() {
    let error = parse(strings(&["--repo", "a/b"])).unwrap_err();

    assert!(error.contains("--role"));
}

#[test]
fn malformed_repo_is_rejected() {
    for raw in ["nope", "a/", "/b", "a/b/c"] {
        let error = parse(strings(&["--repo", raw, "--role", "engineer"])).unwrap_err();
        assert!(error.contains("--repo"), "error for {raw:?}: {error}");
    }
}

#[test]
fn bind_and_cadence_and_ttl_and_secret_and_workflow_parse() {
    let config = run(&[
        "--repo",
        "acme/service",
        "--role",
        "engineer",
        "--bind",
        "127.0.0.1:1",
        "--bind",
        "0.0.0.0:9000",
        "--poll-cadence-secs",
        "1",
        "--poll-cadence-secs",
        "60",
        "--ci-poll-cadence-secs",
        "5",
        "--ci-poll-cadence-secs",
        "17",
        "--ci-missing-grace-secs",
        "11",
        "--ci-missing-grace-secs",
        "47",
        "--mechanical-cadence-secs",
        "7",
        "--mechanical-cadence-secs",
        "120",
        "--lease-ttl-secs",
        "2",
        "--lease-ttl-secs",
        "900",
        "--webhook-secret-file",
        "old-secret.txt",
        "--webhook-secret-file",
        "secret.txt",
        "--workflow",
        "old-workflow.json",
        "--workflow",
        "workflow.json",
        "--daemon-id",
        "old-daemon",
        "--daemon-id",
        " daemon-a ",
    ]);

    assert_eq!(config.bind, "0.0.0.0:9000".parse().unwrap());
    assert_eq!(config.poll_cadence, Duration::from_secs(60));
    assert_eq!(config.ci_poll_cadence, Some(Duration::from_secs(17)));
    assert_eq!(config.ci_missing_grace, Duration::from_secs(47));
    assert_eq!(config.mechanical_cadence, Some(Duration::from_secs(120)));
    assert_eq!(config.lease_ttl, Duration::from_secs(900));
    assert_eq!(
        config.webhook_secret_file,
        Some(PathBuf::from("secret.txt"))
    );
    assert_eq!(config.workflow_file, Some(PathBuf::from("workflow.json")));
    assert_eq!(config.daemon_id, "daemon-a");
}

#[test]
fn invalid_bind_is_rejected() {
    let error = parse(strings(&[
        "--repo",
        "a/b",
        "--role",
        "engineer",
        "--bind",
        "not-an-address",
    ]))
    .unwrap_err();

    assert!(error.contains("--bind"));
}

#[test]
fn invalid_cadence_is_rejected() {
    for raw in ["nope", "0"] {
        let error = parse(strings(&[
            "--repo",
            "a/b",
            "--role",
            "engineer",
            "--poll-cadence-secs",
            raw,
        ]))
        .unwrap_err();
        assert!(
            error.contains("--poll-cadence-secs"),
            "error for {raw:?}: {error}"
        );
    }
}

#[test]
fn invalid_ci_poll_cadence_is_rejected() {
    let error = parse(strings(&[
        "--repo",
        "a/b",
        "--role",
        "engineer",
        "--ci-poll-cadence-secs",
        "nope",
    ]))
    .unwrap_err();
    assert!(error.contains("--ci-poll-cadence-secs"), "error: {error}");
}

#[test]
fn invalid_or_zero_missing_ci_grace_is_rejected() {
    for raw in ["nope", "0"] {
        let error = parse(strings(&[
            "--repo",
            "a/b",
            "--role",
            "engineer",
            "--ci-missing-grace-secs",
            raw,
        ]))
        .unwrap_err();
        assert!(
            error.contains("--ci-missing-grace-secs"),
            "error for {raw:?}: {error}"
        );
    }
}

#[test]
fn zero_ci_poll_cadence_disables_the_dedicated_backstop() {
    let config = run(&[
        "--repo",
        "a/b",
        "--role",
        "engineer",
        "--ci-poll-cadence-secs",
        "0",
    ]);
    assert_eq!(config.ci_poll_cadence, None);
    assert_eq!(config.ci_missing_grace, Duration::from_secs(300));
    assert_eq!(config.poll_cadence, Duration::from_secs(300));
    assert_eq!(config.mechanical_cadence, Some(Duration::from_secs(120)));
}

#[test]
fn invalid_mechanical_cadence_is_rejected() {
    let error = parse(strings(&[
        "--repo",
        "a/b",
        "--role",
        "engineer",
        "--mechanical-cadence-secs",
        "nope",
    ]))
    .unwrap_err();
    assert!(
        error.contains("--mechanical-cadence-secs"),
        "error: {error}"
    );
}

#[test]
fn zero_mechanical_cadence_disables_the_backstop() {
    // `0` is the explicit opt-out, not an error: the mechanical worker is not
    // spawned.
    let config = run(&[
        "--repo",
        "a/b",
        "--role",
        "engineer",
        "--mechanical-cadence-secs",
        "0",
    ]);
    assert_eq!(config.mechanical_cadence, None);
}

#[test]
fn invalid_ttl_is_rejected() {
    for raw in ["nope", "0"] {
        let error = parse(strings(&[
            "--repo",
            "a/b",
            "--role",
            "engineer",
            "--lease-ttl-secs",
            raw,
        ]))
        .unwrap_err();
        assert!(
            error.contains("--lease-ttl-secs"),
            "error for {raw:?}: {error}"
        );
    }
}

#[test]
fn unknown_flag_is_rejected() {
    let error = parse(strings(&[
        "--repo",
        "a/b",
        "--role",
        "engineer",
        "--mystery",
    ]))
    .unwrap_err();

    assert!(error.contains("--mystery"));
}

#[test]
fn missing_flag_value_is_rejected_without_consuming_next_flag() {
    let error = parse(strings(&["--repo", "a/b", "--role", "--bind", "bad"])).unwrap_err();

    assert!(error.contains("--role"));
}

#[test]
fn help_flag_returns_help() {
    for flag in ["--help", "-h"] {
        assert_eq!(parse(strings(&[flag])), Ok(ParseOutcome::Help));
    }
}
