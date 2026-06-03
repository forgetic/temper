use super::*;

fn env(key: &str) -> Option<String> {
    match key {
        FORGEJO_TOKEN_ENV => Some("secret-token".into()),
        _ => None,
    }
}

#[test]
fn rejects_role_worker_without_decision_process() {
    let error = parse_with_env(
        [
            "--backend",
            "forgejo",
            "--base-url",
            "http://127.0.0.1:3000",
            "--repo",
            "acme/service",
            "--kind",
            "role",
            "--role",
            "engineer",
            "--user",
            "engineer",
        ]
        .into_iter()
        .map(String::from),
        env,
    )
    .unwrap_err();
    assert!(error.to_string().contains("role workers require"));
}

#[test]
fn parses_role_decision_process_flags_and_redacts_debug() {
    let outcome = parse_with_env(
        [
            "--backend",
            "forgejo",
            "--base-url",
            "http://127.0.0.1:3000",
            "--repo",
            "acme/service",
            "--kind",
            "role",
            "--role",
            "engineer",
            "--user",
            "engineer",
            "--role-decision-command",
            "/opt/smith-role-decision",
            "--role-decision-arg",
            "--profile",
            "--role-decision-arg",
            "engineer",
            "--role-decision-env",
            "SMITH_MODEL",
            "--role-decision-cwd",
            "/srv/smith",
            "--role-decision-timeout-secs",
            "5",
        ]
        .into_iter()
        .map(String::from),
        env,
    )
    .expect("parses");
    let ParseOutcome::Run(args) = outcome else {
        panic!("expected run")
    };
    let process = args
        .role_decision_process
        .as_ref()
        .expect("role decision process");
    assert_eq!(process.program, PathBuf::from("/opt/smith-role-decision"));
    assert_eq!(process.args, vec!["--profile", "engineer"]);
    assert_eq!(process.env_allowlist, vec!["SMITH_MODEL"]);
    assert_eq!(process.working_dir, Some(PathBuf::from("/srv/smith")));
    assert_eq!(process.timeout, std::time::Duration::from_secs(5));
    assert!(format!("{args:?}").contains("<configured>"));
    assert!(!format!("{args:?}").contains("/opt/smith-role-decision"));
}

#[test]
fn parses_role_decision_process_from_env() {
    let outcome = parse_with_env(
        [
            "--backend",
            "forgejo",
            "--base-url",
            "http://127.0.0.1:3000",
            "--repo",
            "acme/service",
            "--kind",
            "role",
            "--role",
            "engineer",
            "--user",
            "engineer",
        ]
        .into_iter()
        .map(String::from),
        |key| match key {
            FORGEJO_TOKEN_ENV => Some("secret-token".into()),
            ROLE_DECISION_COMMAND_ENV => Some("/opt/smith-role-decision".into()),
            ROLE_DECISION_ARGS_ENV => Some(r#"["--role","engineer"]"#.into()),
            ROLE_DECISION_ENV_ALLOWLIST_ENV => Some("SMITH_TOKEN,SMITH_MODEL".into()),
            ROLE_DECISION_TIMEOUT_ENV => Some("7".into()),
            _ => None,
        },
    )
    .expect("parses");
    let ParseOutcome::Run(args) = outcome else {
        panic!("expected run")
    };
    let process = args
        .role_decision_process
        .as_ref()
        .expect("role decision process");
    assert_eq!(process.args, vec!["--role", "engineer"]);
    assert_eq!(process.env_allowlist, vec!["SMITH_TOKEN", "SMITH_MODEL"]);
    assert_eq!(process.timeout, std::time::Duration::from_secs(7));
}

#[test]
fn parses_production_safety_flags() {
    let outcome = parse_with_env(
        [
            "--backend",
            "forgejo",
            "--base-url",
            "http://127.0.0.1:3000",
            "--repo",
            "acme/service",
            "--kind",
            "role",
            "--role",
            "architect",
            "--user",
            "architect",
            "--role-decision-command",
            "/opt/smith-role-decision",
            "--allow-bookkeeping-only-pr",
        ]
        .into_iter()
        .map(String::from),
        env,
    )
    .expect("parses");
    let ParseOutcome::Run(args) = outcome else {
        panic!("expected run")
    };
    assert!(args.allow_bookkeeping_only_pr);
}

#[test]
fn parses_optional_wake_socket() {
    let outcome = parse_with_env(
        [
            "--backend",
            "forgejo",
            "--base-url",
            "http://127.0.0.1:3000",
            "--repo",
            "acme/service",
            "--kind",
            "mechanical",
            "--wake-socket",
            "run/wake/mechanical.sock",
            "--wake-secret-file",
            "secrets/wake",
        ]
        .into_iter()
        .map(String::from),
        env,
    )
    .expect("parses");
    let ParseOutcome::Run(args) = outcome else {
        panic!("expected run")
    };
    assert_eq!(
        args.wake_socket,
        Some(PathBuf::from("run/wake/mechanical.sock"))
    );
    assert_eq!(args.wake_secret_file, Some(PathBuf::from("secrets/wake")));
}

#[test]
fn parses_multiple_repos_and_deduplicates() {
    let outcome = parse_with_env(
        [
            "--backend",
            "forgejo",
            "--base-url",
            "http://127.0.0.1:3000",
            "--repo",
            "acme/service",
            "--repo",
            "acme/other",
            "--repo",
            "acme/service",
            "--kind",
            "mechanical",
        ]
        .into_iter()
        .map(String::from),
        env,
    )
    .expect("parses");
    let ParseOutcome::Run(args) = outcome else {
        panic!("expected run")
    };
    assert_eq!(
        args.repositories,
        vec![
            RepositoryPath::new("acme", "service"),
            RepositoryPath::new("acme", "other")
        ]
    );
}

#[test]
fn parses_repo_list_file() {
    let path = std::env::temp_dir().join(format!(
        "temper-production-repos-{}-{}.txt",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap()
    ));
    std::fs::write(
        &path,
        "# scan shard\nacme/service\nacme/other # inline comment\n",
    )
    .expect("repo-list writes");
    let outcome = parse_with_env(
        vec![
            "--backend".to_string(),
            "forgejo".to_string(),
            "--base-url".to_string(),
            "http://127.0.0.1:3000".to_string(),
            "--repo-list".to_string(),
            path.display().to_string(),
            "--kind".to_string(),
            "mechanical".to_string(),
        ],
        env,
    )
    .expect("parses");
    let ParseOutcome::Run(args) = outcome else {
        panic!("expected run")
    };
    assert_eq!(args.repositories.len(), 2);
    let _ = std::fs::remove_file(path);
}

#[test]
fn rejects_malformed_repo_names() {
    let error = parse_with_env(
        [
            "--backend",
            "forgejo",
            "--base-url",
            "http://127.0.0.1:3000",
            "--repo",
            "acme/service/extra",
            "--kind",
            "mechanical",
        ]
        .into_iter()
        .map(String::from),
        env,
    )
    .unwrap_err();
    assert!(error.to_string().contains("owner/name"));
}

#[test]
fn rejects_testing_only_backend() {
    let error = parse_with_env(
        ["--backend", "filesystem"].into_iter().map(String::from),
        env,
    )
    .unwrap_err();
    assert!(error.to_string().contains("expected forgejo"));
}

#[test]
fn help_short_circuits_without_env() {
    assert_eq!(
        parse_with_env(["--help".to_string()], |_| None).unwrap(),
        ParseOutcome::Help
    );
}
