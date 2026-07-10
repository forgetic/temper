use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use temper_cli_common::{LoadOptions, ScriptedPrompter};
use temper_config::{ExposeSecret, Secret};
use temper_forge::RepositoryId;
use temper_provision::{Provisioned, RoleIdentity};
use temper_workflow::{RawWorkflowSpec, RoleId};

use super::*;
use crate::apply::args::parse_apply_args;
use crate::provisioner::{ApplyPlanOutcome, ApplyPlanRequest, ApplyProvisioner};

struct StubApplyProvisioner {
    seen: Option<ApplyPlanRequest>,
    fail_repo: Option<String>,
}

impl ApplyProvisioner for StubApplyProvisioner {
    fn provision_apply_plan(
        &mut self,
        request: &ApplyPlanRequest,
    ) -> Result<ApplyPlanOutcome, String> {
        self.seen = Some(request.clone());
        let identity = |user: &str| RoleIdentity {
            user: user.to_string(),
            email: format!("{user}@example.invalid"),
            token: format!("token-{user}"),
            password: format!("pw-{user}"),
        };
        let mut provisioned = Vec::new();
        for plan in &request.plans {
            let path = format!("{}/{}", plan.repo.owner, plan.repo.name);
            if self.fail_repo.as_deref() == Some(path.as_str()) {
                return Err(format!("{path}: simulated failure"));
            }
            let mut roles = BTreeMap::new();
            for binding in &plan.roles {
                roles.insert(binding.role.clone(), identity(&binding.user.handle));
            }
            provisioned.push(Provisioned {
                owner: plan.repo.owner.clone(),
                name: plan.repo.name.clone(),
                repository: RepositoryId::new(path),
                roles,
                automation: identity(&plan.automation_login),
            });
        }
        Ok(ApplyPlanOutcome {
            provisioned,
            admin_token: Secret::from("admin-rest-token"),
        })
    }
}

fn write_apply_bundle(dir: &Path, repos: &[&str]) -> (PathBuf, PathBuf) {
    let config_path = dir.join("config.toml");
    let credentials_path = dir.join("credentials.toml");
    let workflow_path = dir.join("workflow.yaml");
    let webhook_secret_path = dir.join("webhook-secret");
    let spec: RawWorkflowSpec =
        serde_json::from_str(temper_reference_delivery::basic_delivery_workflow_json())
            .expect("basic workflow parses");
    let workflow_yaml = serde_yaml::to_string(&spec).expect("workflow renders as YAML");
    std::fs::write(&workflow_path, workflow_yaml).expect("workflow");
    std::fs::write(&webhook_secret_path, "secret").expect("webhook secret");
    let repos = repos
        .iter()
        .map(|repo| format!("\"{repo}\""))
        .collect::<Vec<_>>()
        .join(", ");
    std::fs::write(
            &config_path,
            format!(
                "schema_version = 1\n\n[deployment]\nname = \"local-dev\"\ntopology = \"standalone\"\n\n[workflow]\nfile = \"workflow.yaml\"\n\n[forge]\ntype = \"forgejo\"\nurl = \"http://forge.local:3000\"\nadmin = \"root\"\nci_user = \"bot\"\n\n[engine]\nbind = \"127.0.0.1:38100\"\nrepos = [{repos}]\nroles = [\"architect\", \"engineer\"]\nwebhook_secret_file = \"webhook-secret\"\n"
            ),
        )
        .expect("config");
    std::fs::write(
            &credentials_path,
            "schema_version = 1\n\n[forge.users.root]\npassword = \"admin-pass\"\n\n[agent.providers.deepseek]\ntype = \"api-key\"\nkey = \"sk-key\"\n",
        )
        .expect("credentials");
    (config_path, credentials_path)
}

fn write_token_apply_bundle(dir: &Path) -> (PathBuf, PathBuf) {
    let config_path = dir.join("config.toml");
    let credentials_path = dir.join("credentials.toml");
    std::fs::write(
            &config_path,
            "schema_version = 1\n\n[forge]\ntype = \"forgejo\"\nurl = \"http://forge.local:3000\"\n\n[engine]\nbind = \"127.0.0.1:38100\"\nrepos = [\"acme/service\"]\nroles = [\"architect\", \"engineer\"]\nforge_token = \"forge-admin\"\nwebhook_secret = \"forge-webhook\"\n",
        )
        .expect("config");
    std::fs::write(
            &credentials_path,
            "schema_version = 1\n\n[secrets]\nforge-admin = \"admin-token-from-secret\"\nforge-webhook = \"webhook-secret-from-secret\"\n",
        )
        .expect("credentials");
    (config_path, credentials_path)
}

fn apply_options(config_path: PathBuf, credentials_path: PathBuf, yes: bool) -> ApplyOptions {
    ApplyOptions {
        options: LoadOptions {
            config: Some(config_path),
            credentials: Some(credentials_path),
        },
        yes,
        ..Default::default()
    }
}

#[test]
fn run_apply_loads_deployment_plan_for_all_repos_and_updates_credentials() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (config_path, credentials_path) =
        write_apply_bundle(dir.path(), &["acme/service", "acme/api"]);
    let opts = apply_options(config_path, credentials_path.clone(), true);
    let mut prompter = ScriptedPrompter::new(Vec::<String>::new());
    let mut provisioner = StubApplyProvisioner {
        seen: None,
        fail_repo: None,
    };

    run_apply(&mut prompter, &mut provisioner, &opts).expect("apply succeeds");

    let seen = provisioner.seen.expect("provisioner called");
    assert_eq!(seen.base_url, "http://forge.local:3000");
    assert_eq!(seen.admin_user.as_deref(), Some("root"));
    assert_eq!(
        seen.admin_password
            .as_ref()
            .map(ExposeSecret::expose_secret),
        Some("admin-pass")
    );
    assert_eq!(seen.plans.len(), 2);
    assert_eq!(seen.plans[0].repo.owner, "acme");
    assert_eq!(seen.plans[0].repo.name, "service");
    assert_eq!(seen.plans[1].repo.name, "api");
    for plan in &seen.plans {
        assert!(plan.webhook.is_some(), "webhook should be planned");
        assert!(!plan.labels.is_empty(), "workflow labels should be planned");
        assert!(
            plan.roles
                .iter()
                .any(|binding| binding.role == RoleId::new("architect")),
            "workflow role metadata should be planned: {:?}",
            plan.roles
        );
        assert!(
            !plan.repository_auto_init,
            "apply must not seed repository content"
        );
    }

    let creds = std::fs::read_to_string(&credentials_path).expect("credentials updated");
    assert!(creds.contains("admin-rest-token"), "{creds}");
    assert!(creds.contains("token-architect"), "{creds}");
    assert!(creds.contains("token-engineer"), "{creds}");
    assert!(creds.contains("token-bot"), "{creds}");
    assert!(
        creds.contains("sk-key"),
        "provider secret preserved: {creds}"
    );
    assert!(
        prompter.notes.iter().any(|note| note == "Apply plan:"),
        "notes: {:?}",
        prompter.notes
    );
    assert!(
        prompter
            .notes
            .iter()
            .any(|note| note.contains("repositories: 2 repo(s)")),
        "notes: {:?}",
        prompter.notes
    );
    assert!(
        prompter
            .notes
            .iter()
            .any(|note| note.contains("deployment: local-dev (standalone)")),
        "notes: {:?}",
        prompter.notes
    );
}

#[test]
fn run_apply_confirmation_can_skip_provisioning_without_mutating_credentials() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (config_path, credentials_path) = write_apply_bundle(dir.path(), &["acme/service"]);
    let before = std::fs::read_to_string(&credentials_path).expect("credentials before");
    let opts = apply_options(config_path, credentials_path.clone(), false);
    let mut prompter = ScriptedPrompter::new(Vec::<String>::new());
    prompter.confirmations.push_back(false);
    let mut provisioner = StubApplyProvisioner {
        seen: None,
        fail_repo: None,
    };

    run_apply(&mut prompter, &mut provisioner, &opts).expect("apply skip succeeds");

    assert!(provisioner.seen.is_none());
    let after = std::fs::read_to_string(&credentials_path).expect("credentials unchanged");
    assert_eq!(after, before);
    assert!(
        prompter.notes.iter().any(|note| note == "Apply plan:"),
        "plan should be shown before confirmation: {:?}",
        prompter.notes
    );
    assert!(
        prompter
            .notes
            .iter()
            .any(|note| note.contains("Skipped forge provisioning")),
        "notes: {:?}",
        prompter.notes
    );
}

#[test]
fn run_apply_failure_reports_repo_and_leaves_credentials_unchanged() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (config_path, credentials_path) =
        write_apply_bundle(dir.path(), &["acme/service", "acme/api"]);
    let before = std::fs::read_to_string(&credentials_path).expect("credentials before");
    let opts = apply_options(config_path, credentials_path.clone(), true);
    let mut prompter = ScriptedPrompter::new(Vec::<String>::new());
    let mut provisioner = StubApplyProvisioner {
        seen: None,
        fail_repo: Some("acme/api".to_string()),
    };

    let err = run_apply(&mut prompter, &mut provisioner, &opts)
        .expect_err("repo-specific provisioning failure should surface");

    let message = err.to_string();
    assert!(message.contains("acme/api"), "{message}");
    assert!(message.contains("simulated failure"), "{message}");
    let after = std::fs::read_to_string(&credentials_path).expect("credentials unchanged");
    assert_eq!(after, before);
}

#[test]
fn run_apply_can_skip_local_credential_mutation_after_success() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (config_path, credentials_path) = write_apply_bundle(dir.path(), &["acme/service"]);
    let before = std::fs::read_to_string(&credentials_path).expect("credentials before");
    let opts = ApplyOptions {
        credential_mode: ApplyCredentialMode::SkipLocalCredentials,
        ..apply_options(config_path, credentials_path.clone(), true)
    };
    let mut prompter = ScriptedPrompter::new(Vec::<String>::new());
    let mut provisioner = StubApplyProvisioner {
        seen: None,
        fail_repo: None,
    };

    run_apply(&mut prompter, &mut provisioner, &opts).expect("apply succeeds");

    assert!(provisioner.seen.is_some(), "provisioning still runs");
    let after = std::fs::read_to_string(&credentials_path).expect("credentials unchanged");
    assert_eq!(after, before);
    assert!(
        prompter
            .notes
            .iter()
            .any(|note| note.contains("Local credentials were not modified")),
        "notes: {:?}",
        prompter.notes
    );
}

#[test]
fn programmatic_no_write_mode_uses_ambient_named_credentials() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (config_path, sibling_credentials) = write_token_apply_bundle(dir.path());
    let credentials_dir = dir.path().join("ambient-credentials");
    std::fs::create_dir_all(&credentials_dir).expect("credentials dir");
    std::fs::write(credentials_dir.join("forge-admin"), "ambient-admin-token\n")
        .expect("admin token");
    std::fs::write(
        credentials_dir.join("forge-webhook"),
        "ambient-webhook-secret\n",
    )
    .expect("webhook secret");
    let before = std::fs::read_to_string(&sibling_credentials).expect("sibling before");
    let mut env = temper_config::EnvMap::new();
    env.insert(
        "CREDENTIALS_DIRECTORY",
        credentials_dir.to_string_lossy().into_owned(),
    );
    let opts = ApplyOptions {
        options: LoadOptions {
            config: Some(config_path),
            credentials: None,
        },
        yes: true,
        credential_mode: ApplyCredentialMode::SkipLocalCredentials,
        env,
        ..Default::default()
    };
    let mut prompter = ScriptedPrompter::new(Vec::<String>::new());
    let mut provisioner = StubApplyProvisioner {
        seen: None,
        fail_repo: None,
    };

    run_apply(&mut prompter, &mut provisioner, &opts).expect("ambient no-write apply");

    let seen = provisioner.seen.expect("provisioner called");
    assert_eq!(
        seen.admin_token.as_ref().map(ExposeSecret::expose_secret),
        Some("ambient-admin-token")
    );
    assert_eq!(
        seen.plans[0]
            .webhook
            .as_ref()
            .map(|webhook| webhook.secret.as_str()),
        Some("ambient-webhook-secret")
    );
    assert_eq!(
        std::fs::read_to_string(sibling_credentials).expect("sibling after"),
        before
    );
}

#[test]
fn ambient_credentials_are_rejected_before_provisioning_in_update_mode() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (config_path, _sibling_credentials) = write_token_apply_bundle(dir.path());
    let credentials_dir = dir.path().join("ambient-credentials");
    std::fs::create_dir_all(&credentials_dir).expect("credentials dir");
    std::fs::write(credentials_dir.join("forge-admin"), "ambient-admin-token\n")
        .expect("admin token");
    std::fs::write(
        credentials_dir.join("forge-webhook"),
        "ambient-webhook-secret\n",
    )
    .expect("webhook secret");
    let mut env = temper_config::EnvMap::new();
    env.insert(
        "CREDENTIALS_DIRECTORY",
        credentials_dir.to_string_lossy().into_owned(),
    );
    let opts = ApplyOptions {
        options: LoadOptions {
            config: Some(config_path),
            credentials: None,
        },
        yes: true,
        env,
        ..Default::default()
    };
    let mut prompter = ScriptedPrompter::new(Vec::<String>::new());
    let mut provisioner = StubApplyProvisioner {
        seen: None,
        fail_repo: None,
    };

    let error = run_apply(&mut prompter, &mut provisioner, &opts)
        .expect_err("ambient update target must be rejected");

    assert!(provisioner.seen.is_none(), "must reject before mutation");
    let message = error.to_string();
    assert!(message.contains("CREDENTIALS_DIRECTORY"), "{message}");
    assert!(message.contains("--secrets"), "{message}");
}

#[test]
fn explicit_credentials_directory_updates_toml_and_preserves_named_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (config_path, source_credentials) = write_apply_bundle(dir.path(), &["acme/service"]);
    let credentials_dir = dir.path().join("explicit-credentials");
    std::fs::create_dir_all(&credentials_dir).expect("credentials dir");
    let credentials_path = credentials_dir.join("credentials.toml");
    std::fs::copy(&source_credentials, &credentials_path).expect("copy credentials");
    let named_path = credentials_dir.join("provider-mounted-secret");
    std::fs::write(&named_path, "leave-this-file-alone\n").expect("named file");
    let opts = apply_options(config_path, credentials_dir, true);
    let mut prompter = ScriptedPrompter::new(Vec::<String>::new());
    let mut provisioner = StubApplyProvisioner {
        seen: None,
        fail_repo: None,
    };

    run_apply(&mut prompter, &mut provisioner, &opts).expect("directory apply");

    let credentials = std::fs::read_to_string(&credentials_path).expect("updated credentials");
    assert!(credentials.contains("admin-rest-token"), "{credentials}");
    assert!(credentials.contains("token-engineer"), "{credentials}");
    assert!(credentials.contains("sk-key"), "{credentials}");
    assert_eq!(
        std::fs::read_to_string(named_path).expect("named file preserved"),
        "leave-this-file-alone\n"
    );
}

#[test]
fn explicit_named_only_directory_creates_durable_credentials_toml() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (config_path, _sibling_credentials) = write_token_apply_bundle(dir.path());
    let credentials_dir = dir.path().join("explicit-named-credentials");
    std::fs::create_dir_all(&credentials_dir).expect("credentials dir");
    let admin_path = credentials_dir.join("forge-admin");
    let webhook_path = credentials_dir.join("forge-webhook");
    std::fs::write(&admin_path, "explicit-admin-token\n").expect("admin token");
    std::fs::write(&webhook_path, "explicit-webhook-secret\n").expect("webhook secret");
    let opts = apply_options(config_path, credentials_dir.clone(), true);
    let mut prompter = ScriptedPrompter::new(Vec::<String>::new());
    let mut provisioner = StubApplyProvisioner {
        seen: None,
        fail_repo: None,
    };

    run_apply(&mut prompter, &mut provisioner, &opts).expect("named directory apply");

    let credentials_path = credentials_dir.join("credentials.toml");
    let credentials = std::fs::read_to_string(credentials_path).expect("created credentials");
    assert!(credentials.contains("schema_version = 1"), "{credentials}");
    assert!(credentials.contains("admin-rest-token"), "{credentials}");
    assert!(credentials.contains("token-engineer"), "{credentials}");
    assert_eq!(
        std::fs::read_to_string(admin_path).expect("admin named file"),
        "explicit-admin-token\n"
    );
    assert_eq!(
        std::fs::read_to_string(webhook_path).expect("webhook named file"),
        "explicit-webhook-secret\n"
    );
}

#[test]
fn run_apply_uses_configured_admin_token_and_updates_selected_credentials() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (config_path, credentials_path) = write_token_apply_bundle(dir.path());
    let opts = apply_options(config_path, credentials_path.clone(), true);
    let mut prompter = ScriptedPrompter::new(Vec::<String>::new());
    let mut provisioner = StubApplyProvisioner {
        seen: None,
        fail_repo: None,
    };

    run_apply(&mut prompter, &mut provisioner, &opts).expect("apply succeeds");

    let seen = provisioner.seen.expect("provisioner called");
    assert_eq!(
        seen.admin_token.as_ref().map(ExposeSecret::expose_secret),
        Some("admin-token-from-secret")
    );
    assert_eq!(seen.admin_user, None);
    assert!(seen.admin_password.is_none());
    let webhook = seen.plans[0].webhook.as_ref().expect("webhook planned");
    assert_eq!(webhook.secret, "webhook-secret-from-secret");
    let after = std::fs::read_to_string(&credentials_path).expect("credentials updated");
    assert!(after.contains("admin-token-from-secret"), "{after}");
    assert!(after.contains("admin-rest-token"), "{after}");
    assert!(after.contains("token-engineer"), "{after}");
    assert!(
        prompter
            .notes
            .iter()
            .any(|note| note.contains("Updated") && note.contains("credentials.toml")),
        "notes: {:?}",
        prompter.notes
    );
}

#[test]
fn run_apply_loads_target_init_bundle_and_mints_named_forge_secret() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = dir.path().join("config.toml");
    let credentials_path = dir.path().join("credentials.toml");
    let workflow_path = dir.path().join("workflow.yaml");
    let workflow_spec: RawWorkflowSpec =
        serde_json::from_str(temper_reference_delivery::basic_delivery_workflow_json())
            .expect("basic workflow parses");
    let workflow_yaml = serde_yaml::to_string(&workflow_spec).expect("workflow yaml");
    std::fs::write(&workflow_path, workflow_yaml).expect("workflow");
    std::fs::write(
        &config_path,
        "schema_version = 1\n\n[deployment]\ntopology = \"standalone\"\n\n[workflow]\nfile = \"workflow.yaml\"\n\n[paths]\nworkspace_dir = \"workspace\"\n\n[forge]\ntype = \"forgejo\"\nurl = \"http://forge.local:3000\"\nadmin = \"root\"\nci_user = \"bot\"\n\n[engine]\nbind = \"127.0.0.1:38100\"\nworkflow = \"workflow.yaml\"\nrepos = [\"acme/service\"]\nroles = [\"architect\", \"engineer\"]\nforge_token = \"forge-engine-token\"\nwebhook_secret = \"webhook-secret\"\nwebhook_secret_file = \"webhook-secret\"\n\n[worker]\nworkspace = \"workspace\"\n\n[[worker.pools]]\nname = \"local\"\nroles = [\"architect\", \"engineer\"]\nrepos = [\"acme/service\"]\nmax_concurrent_jobs = 1\nworker_token = \"worker-local-token\"\n",
    )
    .expect("config");
    std::fs::write(
        &credentials_path,
        "schema_version = 1\n\n[forge.users.root]\npassword = \"admin-pass\"\n\n[secrets.webhook-secret]\nkind = \"webhook-secret\"\nsecret = \"secret\"\n\n[secrets.worker-local-token]\nkind = \"worker-token\"\ntoken = \"worker-secret\"\n",
    )
    .expect("credentials");
    let opts = apply_options(config_path, credentials_path.clone(), true);
    let mut prompter = ScriptedPrompter::new(Vec::<String>::new());
    let mut provisioner = StubApplyProvisioner {
        seen: None,
        fail_repo: None,
    };

    run_apply(&mut prompter, &mut provisioner, &opts).expect("target apply succeeds");

    let seen = provisioner.seen.expect("provisioner called");
    assert_eq!(seen.admin_user.as_deref(), Some("root"));
    assert_eq!(
        seen.admin_password
            .as_ref()
            .map(ExposeSecret::expose_secret),
        Some("admin-pass")
    );
    assert!(seen.admin_token.is_none());
    assert_eq!(seen.plans.len(), 1);
    assert_eq!(seen.plans[0].repo.owner, "acme");
    assert_eq!(seen.plans[0].repo.name, "service");
    let webhook = seen.plans[0].webhook.as_ref().expect("webhook planned");
    assert_eq!(webhook.secret, "secret");
    let creds = std::fs::read_to_string(&credentials_path).expect("credentials updated");
    assert!(creds.contains("[secrets.forge-engine-token]"), "{creds}");
    assert!(creds.contains("token = \"admin-rest-token\""), "{creds}");
}

#[test]
fn run_apply_reports_workflow_static_validation_before_provisioning() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = dir.path().join("config.toml");
    let credentials_path = dir.path().join("credentials.toml");
    let workflow_path = dir.path().join("invalid-workflow.json");
    std::fs::write(
        &workflow_path,
        r#"{
            "name": "invalid",
            "roles": [{"id": "engineer", "queues": ["missing_queue"]}]
        }"#,
    )
    .expect("workflow");
    std::fs::write(
        &config_path,
        "schema_version = 1\n\n[workflow]\nfile = \"invalid-workflow.json\"\n\n[forge]\ntype = \"forgejo\"\nurl = \"http://forge.local:3000\"\nadmin = \"root\"\n\n[engine]\nbind = \"127.0.0.1:38100\"\nrepos = [\"acme/service\"]\nroles = [\"engineer\"]\nwebhook_secret_file = \"webhook-secret\"\n",
    )
    .expect("config");
    std::fs::write(
        &credentials_path,
        "schema_version = 1\n\n[forge.users.root]\npassword = \"admin-pass\"\n",
    )
    .expect("credentials");
    let opts = apply_options(config_path, credentials_path, true);
    let mut prompter = ScriptedPrompter::new(Vec::<String>::new());
    let mut provisioner = StubApplyProvisioner {
        seen: None,
        fail_repo: None,
    };

    let err = run_apply(&mut prompter, &mut provisioner, &opts)
        .expect_err("invalid workflow should stop apply before provisioning");

    assert!(provisioner.seen.is_none());
    let message = err.to_string();
    assert!(message.contains("invalid-workflow.json"), "{message}");
    assert!(message.contains("missing_queue"), "{message}");
}

#[test]
fn parse_accepts_yes_existing_repo_and_global_options() {
    let parsed = parse_apply_args(
        vec!["--yes".to_string(), "--existing-repo".to_string()],
        LoadOptions {
            config: Some("bundle".into()),
            credentials: None,
        },
    )
    .expect("parse");

    assert_eq!(parsed.options.config, Some("bundle".into()));
    assert!(parsed.yes);
    assert!(parsed.existing_repo);
}

#[test]
fn run_apply_existing_repo_compat_applies_to_every_repo() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (config_path, credentials_path) =
        write_apply_bundle(dir.path(), &["acme/service", "acme/api"]);
    let opts = ApplyOptions {
        existing_repo: true,
        ..apply_options(config_path, credentials_path, true)
    };
    let mut prompter = ScriptedPrompter::new(Vec::<String>::new());
    let mut provisioner = StubApplyProvisioner {
        seen: None,
        fail_repo: None,
    };

    run_apply(&mut prompter, &mut provisioner, &opts).expect("apply succeeds");

    let seen = provisioner.seen.expect("provisioner called");
    assert!(seen.plans.iter().all(|plan| plan.existing_repo));
    assert!(
        prompter
            .notes
            .iter()
            .any(|note| note.contains("--existing-repo compatibility")),
        "notes: {:?}",
        prompter.notes
    );
}

#[test]
fn parse_rejects_local_config_flag() {
    let err = parse_apply_args(
        vec!["--config".to_string(), "bundle".to_string()],
        LoadOptions::default(),
    )
    .expect_err("--config is global-only");

    assert!(err.contains("--config"), "{err}");
}
