use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use temper_cli_common::LoadOptions;
use temper_config::{NoEnv, ProviderCredential, ProviderKind, lint, load_with_env};

use super::{ADMIN_USER, DUMMY_DEEPSEEK_KEY, REPO_NAME, REPO_OWNER};

#[allow(clippy::too_many_arguments)]
pub(super) fn assert_local_artifacts(
    base_url: &str,
    config_path: &Path,
    credentials_path: &Path,
    workflow_path: &Path,
    webhook_secret_path: &Path,
    bind_port: u16,
    workspace_root: &Path,
) {
    let workflow = std::fs::read_to_string(workflow_path).expect("workflow.json written");
    assert_eq!(
        workflow,
        temper_reference_delivery::basic_delivery_workflow_json(),
        "workflow.json must be the embedded basic-delivery bytes verbatim"
    );
    let validated = temper_reference_delivery::basic_delivery_workflow();

    assert_mode_600(webhook_secret_path);
    let secret = std::fs::read_to_string(webhook_secret_path).expect("webhook-secret written");
    assert!(
        !secret.trim().is_empty(),
        "webhook secret must be non-empty"
    );

    assert_mode_600(credentials_path);
    let creds_text = std::fs::read_to_string(credentials_path).expect("credentials.toml written");
    assert!(
        creds_text.contains(DUMMY_DEEPSEEK_KEY),
        "credentials must carry the DeepSeek key"
    );
    assert!(
        creds_text.contains("api-key"),
        "DeepSeek key must be stored as an api-key"
    );
    assert!(
        creds_text.contains(ADMIN_USER),
        "credentials must carry the admin identity"
    );

    let resolved = load_isolated(config_path, credentials_path);
    assert_eq!(
        resolved.forge.url.as_deref(),
        Some(base_url.trim_end_matches('/')),
        "resolved forge URL must match the live fixture"
    );
    assert!(
        resolved.forge.admin_token.is_some(),
        "resolved deployment must have an admin token from credentials"
    );
    assert!(
        resolved.forge.web_ui.is_some(),
        "resolved deployment must have CI web-UI credentials (bot)"
    );

    let repos: Vec<String> = resolved.engine.repos.iter().map(|r| r.display()).collect();
    assert_eq!(
        repos,
        vec![format!("{REPO_OWNER}/{REPO_NAME}")],
        "resolved repos must be the default reference repo"
    );
    let mut roles = resolved.engine.roles.clone();
    roles.sort();
    let mut expected_roles: Vec<String> = validated
        .roles()
        .iter()
        .filter(|role| !role.queues.is_empty())
        .map(|role| role.id.as_str().to_string())
        .collect();
    expected_roles.sort();
    assert_eq!(
        roles, expected_roles,
        "resolved roles must be the workflow's queue-subscribing roles"
    );
    for role in &roles {
        assert!(
            resolved.forge.role_identities.contains_key(role),
            "role `{role}` must have a resolved git identity from credentials"
        );
    }

    assert_eq!(
        resolved.engine.bind.port(),
        bind_port,
        "engine bind must use the scripted webhook port"
    );
    assert_eq!(
        resolved.worker.workspace_root.as_path(),
        workspace_root,
        "worker workspace must be the temp dir we passed via InitOptions"
    );
    assert_eq!(
        resolved.engine.webhook_secret_file.as_deref(),
        Some(webhook_secret_path),
        "engine webhook_secret_file must point at the generated secret"
    );

    assert_eq!(resolved.agent.provider.kind, ProviderKind::DeepSeek);
    assert!(
        matches!(
            resolved.agent.provider.credential,
            ProviderCredential::ApiKey(_)
        ),
        "provider credential must be an api-key, got {:?}",
        resolved.agent.provider.credential
    );

    let findings = lint(&resolved);
    let errors: Vec<&str> = findings
        .iter()
        .filter(|f| f.error)
        .map(|f| f.message.as_str())
        .collect();
    assert!(
        errors.is_empty(),
        "lint must report no errors for the init-produced deployment, got: {errors:?}"
    );
}

fn load_isolated(config_path: &Path, credentials_path: &Path) -> temper_config::Resolved {
    load_with_env(
        &LoadOptions {
            config: Some(config_path.to_path_buf()),
            credentials: Some(credentials_path.to_path_buf()),
        },
        &NoEnv,
    )
    .expect("init-produced config + credentials load and resolve")
    .0
}

fn assert_mode_600(path: &Path) {
    let mode = std::fs::metadata(path)
        .unwrap_or_else(|e| panic!("stat {}: {e}", path.display()))
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "{} must be 0600, got {mode:o}", path.display());
}
