use temper_forge::{Repository, RepositoryId};

use super::*;

struct RecordingInspector {
    inspection: ForgeInspection,
    calls: usize,
}

impl DeploymentInspector for RecordingInspector {
    fn inspect(&mut self, _bundle: &DeploymentBundle) -> Result<ForgeInspection, String> {
        self.calls += 1;
        Ok(self.inspection.clone())
    }
}

#[test]
fn plan_json_redacts_secret_values_and_uses_inspector_snapshot() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = write_bundle(dir.path(), true);
    let opts = PlanOptions {
        options: LoadOptions {
            config: Some(bundle.join("config.toml")),
            credentials: Some(bundle.join("credentials.toml")),
        },
        ..Default::default()
    };
    let bundle = load_deployment(&opts.options, &opts.env, &opts.paths, opts.existing_repo)
        .expect("bundle loads");
    let mut inspector = RecordingInspector {
        inspection: ForgeInspection {
            inspected: true,
            repository: Some(repository()),
            labels: vec!["queued".to_string()],
            webhooks: vec![WebhookStatus {
                url: bundle.webhook.as_ref().expect("webhook").url.clone(),
                events: temper_forge::WebhookEvents::All,
            }],
            users: desired_users(&bundle)
                .into_iter()
                .map(|user| (user, true))
                .collect(),
            ..ForgeInspection::default()
        },
        calls: 0,
    };

    let report = build_report(&bundle, &mut inspector).expect("report builds");
    let json = serde_json::to_string(&report).expect("json");

    assert_eq!(inspector.calls, 1);
    assert_eq!(report.result, "ok");
    assert!(json.contains("\"secret\":\"<redacted>\""), "{json}");
    assert!(!json.contains("admin-pass"), "{json}");
    assert!(!json.contains("webhook-secret-value"), "{json}");
    assert!(json.contains("\"repository\""), "{json}");
    assert!(json.contains("\"metadata\""), "{json}");
}

#[test]
fn existing_repo_missing_is_an_error_without_mutating() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = write_bundle(dir.path(), false);
    let opts = PlanOptions {
        options: LoadOptions {
            config: Some(bundle.join("config.toml")),
            credentials: Some(bundle.join("credentials.toml")),
        },
        existing_repo: true,
        ..Default::default()
    };
    let bundle = load_deployment(&opts.options, &opts.env, &opts.paths, opts.existing_repo)
        .expect("bundle loads");
    let mut inspector = RecordingInspector {
        inspection: ForgeInspection {
            inspected: true,
            repository: None,
            ..ForgeInspection::default()
        },
        calls: 0,
    };

    let report = build_report(&bundle, &mut inspector).expect("report builds");

    assert_eq!(inspector.calls, 1);
    assert!(report.has_error_findings());
    assert_eq!(report.repository.action, "require_existing");
}

fn write_bundle(root: &std::path::Path, with_admin_token: bool) -> std::path::PathBuf {
    let bundle = root.join("bundle");
    std::fs::create_dir_all(&bundle).expect("bundle");
    std::fs::write(bundle.join("webhook-secret"), "webhook-secret-value").expect("webhook");
    std::fs::write(
        bundle.join("config.toml"),
        "schema_version = 1\n\
         [forge]\n\
         url = \"http://forge.local:3000\"\n\
         admin = \"root\"\n\
         ci_user = \"bot\"\n\
         [engine]\n\
         bind = \"127.0.0.1:38100\"\n\
         repos = [\"acme/service\"]\n\
         roles = [\"architect\", \"engineer\"]\n\
         webhook_secret_file = \"webhook-secret\"\n",
    )
    .expect("config");
    let token = if with_admin_token {
        "token = \"admin-token\"\n"
    } else {
        ""
    };
    std::fs::write(
        bundle.join("credentials.toml"),
        format!(
            "schema_version = 1\n\
             [forge.users.root]\n\
             password = \"admin-pass\"\n\
             {token}\n\
             [agent.providers.deepseek]\n\
             type = \"api-key\"\n\
             key = \"provider-key\"\n"
        ),
    )
    .expect("credentials");
    bundle
}

fn repository() -> Repository {
    Repository {
        id: RepositoryId::new("repo-1"),
        owner: "acme".to_string(),
        name: "service".to_string(),
        default_branch: "main".to_string(),
        description: None,
        created_at: chrono::DateTime::from_timestamp(0, 0).expect("timestamp"),
        updated_at: chrono::DateTime::from_timestamp(0, 0).expect("timestamp"),
    }
}
