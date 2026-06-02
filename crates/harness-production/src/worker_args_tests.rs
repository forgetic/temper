use super::*;

fn env(key: &str) -> Option<String> {
    match key {
        FORGEJO_TOKEN_ENV => Some("secret-token".into()),
        _ => None,
    }
}

#[test]
fn parses_role_worker_and_redacts_token_in_debug() {
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
        env,
    )
    .expect("parses");
    let ParseOutcome::Run(args) = outcome else {
        panic!("expected run")
    };
    assert_eq!(args.owner, "acme");
    assert_eq!(args.name, "service");
    assert_eq!(
        args.repositories,
        vec![RepositoryPath::new("acme", "service")]
    );
    assert!(format!("{:?}", args.forgejo).contains("<redacted>"));
    assert!(!format!("{:?}", args.forgejo).contains("secret-token"));
    assert!(!args.architect_close_produced_issues);
    assert!(!args.allow_synthetic_pr_prep);
    assert!(!args.allow_bookkeeping_only_pr);
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
            "--architect-close-produced-issues",
            "--allow-synthetic-pr-prep",
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
    assert!(args.architect_close_produced_issues);
    assert!(args.allow_synthetic_pr_prep);
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
        "harness-production-repos-{}-{}.txt",
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
