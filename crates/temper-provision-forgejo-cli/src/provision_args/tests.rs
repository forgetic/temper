use std::path::PathBuf;

use crate::provision::AccessScope;

use super::{ADMIN_TOKEN_ENV, ParseOutcome, ProvisionArgs, WORKFLOW_FILE_ENV, parse_with_env};

#[test]
fn parse_requires_admin_token_from_env() {
    let error = parse_with_env(
        [
            "--base-url",
            "http://127.0.0.1:3000",
            "--owner",
            "acme",
            "--name",
            "service",
            "--out",
            "roles.env",
        ]
        .into_iter()
        .map(String::from),
        |_| None,
    )
    .unwrap_err();
    assert!(error.to_string().contains(ADMIN_TOKEN_ENV));
}

#[test]
fn parse_reads_token_from_env_and_debug_redacts_it() {
    let outcome = parse_with_env(
        [
            "--base-url",
            "http://127.0.0.1:3000",
            "--owner",
            "acme",
            "--name",
            "service",
            "--out",
            "roles.env",
        ]
        .into_iter()
        .map(String::from),
        |key| (key == ADMIN_TOKEN_ENV).then(|| "admin-secret".to_string()),
    )
    .expect("parses");
    let ParseOutcome::Run(args) = outcome else {
        panic!("expected run")
    };
    assert!(args.seed_intake);
    let rendered = format!("{args:?}");
    assert!(!rendered.contains("admin-secret"));
    assert!(rendered.contains("<redacted>"));
}

#[test]
fn parse_allows_disabling_or_customizing_seed_intake() {
    let outcome = parse_with_env(
        [
            "--base-url",
            "http://127.0.0.1:3000",
            "--owner",
            "acme",
            "--name",
            "service",
            "--out",
            "roles.env",
            "--seed-intake",
            "no",
            "--intake-title",
            "Custom intake",
            "--intake-body-file",
            "body.md",
        ]
        .into_iter()
        .map(String::from),
        |key| (key == ADMIN_TOKEN_ENV).then(|| "admin-secret".to_string()),
    )
    .expect("parses");
    let ParseOutcome::Run(args) = outcome else {
        panic!("expected run")
    };
    assert!(!args.seed_intake);
    assert_eq!(args.intake_title.as_deref(), Some("Custom intake"));
    assert_eq!(args.intake_body_file, Some(PathBuf::from("body.md")));
}

#[test]
fn parse_seed_only_keeps_seeding_enabled() {
    let outcome = parse_with_env(
        [
            "--base-url",
            "http://127.0.0.1:3000",
            "--owner",
            "acme",
            "--name",
            "service",
            "--out",
            "roles.env",
            "--seed-only",
        ]
        .into_iter()
        .map(String::from),
        |key| (key == ADMIN_TOKEN_ENV).then(|| "admin-secret".to_string()),
    )
    .expect("parses");
    let ParseOutcome::Run(args) = outcome else {
        panic!("expected run")
    };
    assert!(args.seed_only);
    assert!(args.seed_intake);
}

#[test]
fn parse_rejects_seed_only_with_seed_intake_no() {
    let error = parse_with_env(
        [
            "--base-url",
            "http://127.0.0.1:3000",
            "--owner",
            "acme",
            "--name",
            "service",
            "--out",
            "roles.env",
            "--seed-only",
            "--seed-intake",
            "no",
        ]
        .into_iter()
        .map(String::from),
        |key| (key == ADMIN_TOKEN_ENV).then(|| "admin-secret".to_string()),
    )
    .unwrap_err();
    assert!(error.to_string().contains("--seed-only"));
}

/// Parses a minimal valid invocation with the given extra args appended,
/// returning the resulting `ProvisionArgs`. Panics if parsing yields `Help`.
fn parse_args(extra: &[&str]) -> ProvisionArgs {
    let mut argv = vec![
        "--base-url",
        "http://127.0.0.1:3000",
        "--owner",
        "acme",
        "--name",
        "service",
        "--out",
        "roles.env",
    ];
    argv.extend_from_slice(extra);
    let outcome = parse_with_env(argv.into_iter().map(String::from), |key| {
        (key == ADMIN_TOKEN_ENV).then(|| "admin-secret".to_string())
    })
    .expect("parses");
    match outcome {
        ParseOutcome::Run(args) => args,
        ParseOutcome::Help => panic!("expected run"),
    }
}

#[test]
fn parse_defaults_existing_repo_and_access_to_back_compat() {
    // No flags ⇒ throwaway behavior: create the repo (existing_repo=false)
    // and join the Owners team (AccessScope::OrgOwners).
    let args = parse_args(&[]);
    assert!(!args.existing_repo);
    assert_eq!(args.access, AccessScope::OrgOwners);
}

#[test]
fn parse_existing_repo_flag() {
    let args = parse_args(&["--existing-repo"]);
    assert!(args.existing_repo);
    // `--existing-repo` does not change the access default.
    assert_eq!(args.access, AccessScope::OrgOwners);
}

#[test]
fn parse_access_org_owners_and_repo_collaborator() {
    assert_eq!(
        parse_args(&["--access", "org-owners"]).access,
        AccessScope::OrgOwners
    );
    assert_eq!(
        parse_args(&["--access", "repo-collaborator"]).access,
        AccessScope::RepoCollaborator
    );
}

#[test]
fn parse_rejects_unknown_access_value() {
    let error = parse_with_env(
        [
            "--base-url",
            "http://127.0.0.1:3000",
            "--owner",
            "acme",
            "--name",
            "service",
            "--out",
            "roles.env",
            "--access",
            "owner",
        ]
        .into_iter()
        .map(String::from),
        |key| (key == ADMIN_TOKEN_ENV).then(|| "admin-secret".to_string()),
    )
    .unwrap_err();
    let message = error.to_string();
    assert!(message.contains("--access"), "names the flag: {message}");
    assert!(
        message.contains("org-owners") && message.contains("repo-collaborator"),
        "lists both options: {message}"
    );
}

#[test]
fn parse_existing_repo_does_not_conflict_with_seed_flags() {
    // `--existing-repo` and `--access` compose with the seed knobs; the
    // intended Smith caller pairs `--existing-repo --access repo-collaborator`
    // with `--seed-intake no`.
    let args = parse_args(&[
        "--existing-repo",
        "--access",
        "repo-collaborator",
        "--seed-intake",
        "no",
    ]);
    assert!(args.existing_repo);
    assert_eq!(args.access, AccessScope::RepoCollaborator);
    assert!(!args.seed_intake);
}

#[test]
fn parse_redacts_token_but_shows_new_flags_in_debug() {
    let args = parse_args(&["--existing-repo", "--access", "repo-collaborator"]);
    let rendered = format!("{args:?}");
    assert!(!rendered.contains("admin-secret"));
    assert!(rendered.contains("existing_repo: true"));
    assert!(rendered.contains("RepoCollaborator"));
}

#[test]
fn parse_defaults_workflow_file_to_none() {
    let outcome = parse_with_env(
        [
            "--base-url",
            "http://127.0.0.1:3000",
            "--owner",
            "acme",
            "--name",
            "service",
            "--out",
            "roles.env",
        ]
        .into_iter()
        .map(String::from),
        |key| (key == ADMIN_TOKEN_ENV).then(|| "admin-secret".to_string()),
    )
    .expect("parses");
    let ParseOutcome::Run(args) = outcome else {
        panic!("expected run")
    };
    assert_eq!(args.workflow_file, None);
}

#[test]
fn parse_accepts_workflow_flag_with_env_fallback_and_precedence() {
    // Flag override.
    let ParseOutcome::Run(args) = parse_with_env(
        [
            "--base-url",
            "http://127.0.0.1:3000",
            "--owner",
            "acme",
            "--name",
            "service",
            "--out",
            "roles.env",
            "--workflow",
            "from-flag.json",
        ]
        .into_iter()
        .map(String::from),
        |key| match key {
            ADMIN_TOKEN_ENV => Some("admin-secret".to_string()),
            WORKFLOW_FILE_ENV => Some("from-env.json".to_string()),
            _ => None,
        },
    )
    .expect("parses") else {
        panic!("expected run")
    };
    assert_eq!(args.workflow_file, Some(PathBuf::from("from-flag.json")));

    // Env fallback when the flag is absent.
    let ParseOutcome::Run(args) = parse_with_env(
        [
            "--base-url",
            "http://127.0.0.1:3000",
            "--owner",
            "acme",
            "--name",
            "service",
            "--out",
            "roles.env",
        ]
        .into_iter()
        .map(String::from),
        |key| match key {
            ADMIN_TOKEN_ENV => Some("admin-secret".to_string()),
            WORKFLOW_FILE_ENV => Some("from-env.json".to_string()),
            _ => None,
        },
    )
    .expect("parses") else {
        panic!("expected run")
    };
    assert_eq!(args.workflow_file, Some(PathBuf::from("from-env.json")));
}
