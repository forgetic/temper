// SPDX-License-Identifier: MPL-2.0

use std::process::ExitCode;
use temper_cli_common::{EX_USAGE, EnvMap, LoadOptions, PathResolver, ScriptedPrompter};
use temper_cli_init::{main_with_options, run_init};
use temper_config::ExposeSecret;

use super::options;
use super::support::RecordingProvisioner;

#[test]
fn single_repository_apply_preserves_init_behavior() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut opts = options(dir.path(), &["acme/service"]);
    opts.apply = true;
    opts.yes = true;
    let mut prompt = ScriptedPrompter::new(Vec::<String>::new());
    let mut provisioner = RecordingProvisioner::default();

    run_init(&mut prompt, &mut provisioner, &opts).expect("single-repo apply");

    assert_eq!(provisioner.calls.len(), 1);
    assert_eq!(provisioner.calls[0].plans.len(), 1);
    assert_eq!(provisioner.calls[0].plans[0].repo.owner, "acme");
    assert_eq!(provisioner.calls[0].plans[0].repo.name, "service");
    let credentials = std::fs::read_to_string(dir.path().join("credentials.toml")).unwrap();
    assert!(credentials.contains("admin-rest-token"), "{credentials}");
    assert!(credentials.contains("token-engineer"), "{credentials}");
    assert!(credentials.contains("token-bot"), "{credentials}");
}

#[test]
fn apply_yes_provisions_two_repositories_in_one_deployment_call() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut opts = options(dir.path(), &["acme/service", "acme/docs"]);
    opts.apply = true;
    opts.yes = true;
    let mut prompt = ScriptedPrompter::new(Vec::<String>::new());
    let mut provisioner = RecordingProvisioner::default();

    run_init(&mut prompt, &mut provisioner, &opts).expect("multi-repo apply");

    assert_eq!(
        provisioner.calls.len(),
        1,
        "one deployment-wide adapter call"
    );
    let request = &provisioner.calls[0];
    assert_eq!(request.plans.len(), 2);
    assert_eq!(request.plans[0].repo.name, "service");
    assert_eq!(request.plans[1].repo.name, "docs");
    assert_eq!(
        request
            .admin_password
            .as_ref()
            .map(ExposeSecret::expose_secret),
        Some("admin-pass")
    );
    for plan in &request.plans {
        assert!(
            plan.seed_commits.is_empty(),
            "init apply must not seed content"
        );
        assert!(
            !plan.repository_auto_init,
            "init apply must not auto-init repos"
        );
    }
    let credentials =
        std::fs::read_to_string(dir.path().join("credentials.toml")).expect("credentials");
    assert!(credentials.contains("admin-rest-token"), "{credentials}");
    assert!(
        credentials.contains("token-architect")
            && credentials.contains("token-engineer")
            && credentials.contains("token-bot"),
        "{credentials}"
    );
    assert!(
        prompt
            .notes
            .iter()
            .any(|note| note.contains("Provisioned 2 repo(s)")),
        "{:?}",
        prompt.notes
    );
    assert!(
        prompt
            .notes
            .iter()
            .any(|note| note.contains("acme/service")),
        "{:?}",
        prompt.notes
    );
    assert!(
        prompt.notes.iter().any(|note| note.contains("acme/docs")),
        "{:?}",
        prompt.notes
    );
}

#[test]
fn interactive_decline_retains_generated_bundle_without_forge_call() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut opts = options(dir.path(), &["acme/service", "acme/docs"]);
    opts.non_interactive = false;
    opts.apply = true;
    opts.overrides.admin_password = None;
    opts.overrides.provider_key = None;
    // Interactive mode still asks for workflow selection before the secrets.
    let mut prompt = ScriptedPrompter::new([
        "".to_string(),
        "admin-pass".to_string(),
        "sk-key".to_string(),
    ]);
    prompt.confirmations.push_back(false);
    let mut provisioner = RecordingProvisioner::default();

    run_init(&mut prompt, &mut provisioner, &opts).expect("decline succeeds");

    assert!(provisioner.calls.is_empty());
    assert!(dir.path().join("config.toml").is_file());
    let credentials = std::fs::read_to_string(dir.path().join("credentials.toml")).unwrap();
    assert!(
        credentials.contains("admin-pass") && credentials.contains("sk-key"),
        "{credentials}"
    );
    assert!(
        !credentials.contains("admin-rest-token") && !credentials.contains("token-engineer"),
        "{credentials}"
    );
    assert!(
        prompt.confirmations.is_empty(),
        "exactly one confirmation consumed"
    );
    assert!(
        prompt
            .notes
            .iter()
            .any(|note| note.contains("repositories: 2 repo(s)")),
        "{:?}",
        prompt.notes
    );
    assert!(
        prompt
            .notes
            .iter()
            .any(|note| note.contains("forge: http://forge.local:3000")),
        "{:?}",
        prompt.notes
    );
}

#[test]
fn non_interactive_apply_without_yes_fails_before_generation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut opts = options(dir.path(), &["acme/service", "acme/docs"]);
    opts.apply = true;
    let mut provisioner = RecordingProvisioner::default();
    let error = run_init(
        &mut ScriptedPrompter::new(Vec::<String>::new()),
        &mut provisioner,
        &opts,
    )
    .expect_err("confirmation required");
    assert!(error.to_string().contains("requires --yes"), "{error}");
    assert!(provisioner.calls.is_empty());
    assert!(!dir.path().join("config.toml").exists());
}

#[test]
fn non_interactive_apply_without_yes_is_a_cli_usage_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let code = main_with_options(
        vec!["--non-interactive".into(), "--apply".into()],
        &EnvMap::new(),
        &PathResolver::default(),
        LoadOptions {
            config: Some(dir.path().join("config.toml")),
            credentials: Some(dir.path().join("credentials.toml")),
        },
    );
    assert_eq!(code, ExitCode::from(EX_USAGE));
    assert!(!dir.path().join("config.toml").exists());
}

#[test]
fn later_repository_failure_keeps_only_initial_operator_credentials() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut opts = options(dir.path(), &["acme/service", "acme/docs"]);
    opts.apply = true;
    opts.yes = true;
    let mut provisioner = RecordingProvisioner {
        fail_repo: Some("acme/docs".into()),
        ..Default::default()
    };
    let error = run_init(
        &mut ScriptedPrompter::new(Vec::<String>::new()),
        &mut provisioner,
        &opts,
    )
    .expect_err("second repo failure");
    assert!(
        error.to_string().contains("acme/docs") && error.to_string().contains("simulated failure"),
        "{error}"
    );
    assert_eq!(provisioner.calls.len(), 1);
    assert_eq!(provisioner.calls[0].plans.len(), 2);
    let credentials = std::fs::read_to_string(dir.path().join("credentials.toml")).unwrap();
    assert!(
        credentials.contains("admin-pass") && credentials.contains("sk-key"),
        "{credentials}"
    );
    assert!(
        !credentials.contains("admin-rest-token")
            && !credentials.contains("token-architect")
            && !credentials.contains("token-bot"),
        "{credentials}"
    );
}

#[test]
fn existing_repo_compatibility_applies_to_every_selected_repository() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut opts = options(dir.path(), &["acme/service", "acme/docs"]);
    opts.apply = true;
    opts.yes = true;
    opts.existing_repo = true;
    let mut provisioner = RecordingProvisioner::default();
    run_init(
        &mut ScriptedPrompter::new(Vec::<String>::new()),
        &mut provisioner,
        &opts,
    )
    .expect("apply");
    assert!(
        provisioner.calls[0]
            .plans
            .iter()
            .all(|plan| plan.existing_repo)
    );
}

#[test]
fn generated_credentials_path_wins_over_ambient_credentials_directory() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ambient = dir.path().join("ambient");
    std::fs::create_dir_all(&ambient).unwrap();
    std::fs::write(ambient.join("forge-engine-token"), "wrong-token").unwrap();
    let mut opts = options(dir.path(), &["acme/service"]);
    opts.apply = true;
    opts.yes = true;
    opts.env
        .insert("CREDENTIALS_DIRECTORY", ambient.display().to_string());
    let mut provisioner = RecordingProvisioner::default();
    run_init(
        &mut ScriptedPrompter::new(Vec::<String>::new()),
        &mut provisioner,
        &opts,
    )
    .expect("explicit bootstrap credentials");
    assert_eq!(
        provisioner.calls[0]
            .admin_password
            .as_ref()
            .map(ExposeSecret::expose_secret),
        Some("admin-pass")
    );
    let credentials = std::fs::read_to_string(dir.path().join("credentials.toml")).unwrap();
    assert!(credentials.contains("admin-rest-token"), "{credentials}");
}
