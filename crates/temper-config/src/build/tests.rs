// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeMap;

use secrecy::ExposeSecret;

use super::*;
use crate::{NoEnv, ProviderKind, resolve};

fn sample_config_inputs() -> ConfigInputs {
    ConfigInputs {
        forge_url: Some("http://localhost:3000".to_string()),
        forge_kind: Some("forgejo".to_string()),
        repos: vec!["acme/widgets".to_string(), "acme/docs".to_string()],
        roles: vec!["engineer".to_string(), "architect".to_string()],
        workflow_path: Some("/wf.json".to_string()),
        webhook_addr: Some("0.0.0.0:4000".to_string()),
        admin_user: Some("agent".to_string()),
        provider: Some("anthropic".to_string()),
        provider_url: None,
        workspace: Some("~/.local/state/temper/workspace".to_string()),
    }
}

fn sample_credential_inputs() -> CredentialInputs {
    let mut forge_users = BTreeMap::new();
    forge_users.insert(
        "agent".to_string(),
        forge_user(
            None,
            None,
            Some("agent-pw".to_string()),
            Some("agent-tok".to_string()),
        ),
    );
    forge_users.insert(
        "engineer".to_string(),
        forge_user(
            None,
            Some("eng@example.test".to_string()),
            None,
            Some("eng-tok".to_string()),
        ),
    );
    forge_users.insert(
        "bot".to_string(),
        forge_user(
            None,
            None,
            Some("bot-pw".to_string()),
            Some("bot-tok".to_string()),
        ),
    );
    CredentialInputs {
        forge_users,
        provider_key: Some(ProviderKeyInput {
            provider: "anthropic".to_string(),
            secret: ProviderSecretInput::OAuth {
                access: "secret-access".to_string(),
                refresh: Some("secret-refresh".to_string()),
                expires: 1781371005373,
            },
        }),
    }
}

#[test]
fn build_config_populates_only_provided_fields() {
    let config = build_config(&sample_config_inputs());
    assert_eq!(config.schema_version, SCHEMA_VERSION);
    assert_eq!(config.forge.url.as_deref(), Some("http://localhost:3000"));
    assert_eq!(config.forge.admin.as_deref(), Some("agent"));
    assert_eq!(config.engine.repos.as_ref().unwrap().len(), 2);
    assert_eq!(config.agent.provider.as_deref(), Some("anthropic"));
    // Unset optionals stay None.
    assert!(config.engine.daemon_id.is_none());
    assert!(config.engine.ci_poll_cadence_secs.is_none());
    assert!(config.engine.ci_missing_grace_secs.is_none());
    assert!(config.worker.worker_id.is_none());
}

#[test]
fn build_config_omits_empty_repos_and_roles() {
    let config = build_config(&ConfigInputs {
        forge_url: Some("http://localhost:3000".to_string()),
        ..ConfigInputs::default()
    });
    assert!(config.engine.repos.is_none());
    assert!(config.engine.roles.is_none());
}

#[test]
fn serialized_config_omits_unset_optionals() {
    let config = build_config(&ConfigInputs {
        forge_url: Some("http://localhost:3000".to_string()),
        ..ConfigInputs::default()
    });
    let text = toml::to_string(&config).expect("serializes");
    // No empty-string noise for unset fields.
    assert!(!text.contains("admin ="), "unexpected admin key: {text}");
    assert!(!text.contains("= \"\""), "empty-string noise: {text}");
    // The empty [engine]/[worker]/[agent] sections are skipped entirely.
    assert!(
        !text.contains("[engine]"),
        "engine emitted while empty: {text}"
    );
    assert!(
        !text.contains("[worker]"),
        "worker emitted while empty: {text}"
    );
    // schema_version is always present.
    assert!(
        text.contains("schema_version = 1"),
        "version missing: {text}"
    );
}

#[test]
fn build_credentials_stores_oauth_secret() {
    let creds = build_credentials(&sample_credential_inputs());
    assert_eq!(creds.schema_version, SCHEMA_VERSION);
    let anthropic = creds.agent.providers.get("anthropic").expect("provider");
    assert_eq!(anthropic.kind.as_deref(), Some("oauth"));
    assert_eq!(anthropic.access.as_deref(), Some("secret-access"));
    assert_eq!(creds.forge.users.len(), 3);
}

#[test]
fn build_credentials_api_key_shape() {
    let creds = build_credentials(&CredentialInputs {
        forge_users: BTreeMap::new(),
        provider_key: Some(ProviderKeyInput {
            provider: "deepseek".to_string(),
            secret: ProviderSecretInput::ApiKey("sk-secret".to_string()),
        }),
    });
    let deepseek = creds.agent.providers.get("deepseek").expect("provider");
    assert_eq!(deepseek.kind.as_deref(), Some("api-key"));
    assert_eq!(deepseek.key.as_deref(), Some("sk-secret"));
    assert!(deepseek.access.is_none());
}

#[test]
fn build_credentials_can_omit_provider_credentials() {
    let creds = build_credentials(&CredentialInputs {
        forge_users: BTreeMap::new(),
        provider_key: None,
    });

    assert!(creds.agent.providers.is_empty());
}

#[test]
fn build_credentials_auth_file_shape() {
    let creds = build_credentials(&CredentialInputs {
        forge_users: BTreeMap::new(),
        provider_key: Some(ProviderKeyInput {
            provider: "chatgpt".to_string(),
            secret: ProviderSecretInput::AuthFile("/home/agent/auth.json".to_string()),
        }),
    });
    let chatgpt = creds.agent.providers.get("chatgpt").expect("provider");
    assert_eq!(chatgpt.auth_file.as_deref(), Some("/home/agent/auth.json"));
    assert!(chatgpt.kind.is_none());
}

#[test]
fn forge_users_from_provisioned_drops_redundant_user_key() {
    let mut provisioned = BTreeMap::new();
    provisioned.insert(
        "engineer".to_string(),
        ProvisionedForgeUser {
            // user == key → dropped (the key is the default).
            user: Some("engineer".to_string()),
            email: Some("eng@noreply.localhost".to_string()),
            token: Some("tok".to_string()),
        },
    );
    provisioned.insert(
        "bot".to_string(),
        ProvisionedForgeUser {
            // user != key → kept verbatim.
            user: Some("automation-bot".to_string()),
            email: None,
            token: Some("bot-tok".to_string()),
        },
    );
    let users = forge_users_from_provisioned(&provisioned);
    assert!(
        users["engineer"].user.is_none(),
        "redundant key not dropped"
    );
    assert_eq!(users["bot"].user.as_deref(), Some("automation-bot"));
    assert_eq!(users["engineer"].token.as_deref(), Some("tok"));
}

#[test]
fn config_round_trips_through_write_and_load() {
    let dir = tempdir();
    let path = dir.join("config.toml");
    let config = build_config(&sample_config_inputs());
    write_config(&config, &path, false).expect("writes");

    let loaded = Config::load(&path).expect("loads back");
    let resolved = resolve(&loaded, &Credentials::default(), &NoEnv).expect("resolves");
    assert_eq!(resolved.forge.url.as_deref(), Some("http://localhost:3000"));
    assert_eq!(resolved.engine.bind.to_string(), "0.0.0.0:4000");
    assert_eq!(resolved.engine.repos.len(), 2);
    assert_eq!(resolved.agent.provider.kind, ProviderKind::Anthropic);
    // The `~`-prefixed workspace expands against HOME at resolve time.
    let env: BTreeMap<String, String> =
        BTreeMap::from([("HOME".to_string(), "/home/tester".to_string())]);
    let resolved = resolve(&loaded, &Credentials::default(), &env).expect("resolves");
    assert_eq!(
        resolved.worker.workspace_root,
        std::path::Path::new("/home/tester/.local/state/temper/workspace")
    );
}

#[test]
fn credentials_round_trip_and_are_0600() {
    let dir = tempdir();
    let path = dir.join("credentials.toml");
    let creds = build_credentials(&sample_credential_inputs());
    write_credentials(&creds, &path, false).expect("writes");

    let loaded = Credentials::load(&path).expect("loads back");
    assert_eq!(loaded.forge.users.len(), 3);
    // The guarded write path emits the REAL secret to disk (schema fields are
    // raw `Option<String>`), so the on-disk TOML carries the verbatim token …
    let on_disk = std::fs::read_to_string(&path).expect("reads back");
    assert!(
        on_disk.contains("agent-tok"),
        "credentials file must store the real admin token"
    );
    assert!(
        on_disk.contains("secret-access"),
        "credentials file must store the real oauth access token"
    );
    // … and it re-reads (toml -> schema -> Resolved) into the same secret.
    let config = build_config(&sample_config_inputs());
    let resolved = resolve(&config, &loaded, &NoEnv).expect("resolves");
    assert_eq!(
        resolved
            .forge
            .admin_token
            .as_ref()
            .map(ExposeSecret::expose_secret),
        Some("agent-tok")
    );
    match &resolved.agent.provider.credential {
        crate::ProviderCredential::OAuthInline { access, .. } => {
            assert_eq!(access.expose_secret(), "secret-access");
        }
        other => panic!("expected inline oauth, got {other:?}"),
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "credentials must be 0600, got {mode:o}"
        );
    }
}

#[test]
fn write_refuses_existing_without_force() {
    let dir = tempdir();
    let path = dir.join("config.toml");
    let config = build_config(&sample_config_inputs());
    write_config(&config, &path, false).expect("first write");
    let err = write_config(&config, &path, false).expect_err("second write rejected");
    assert!(matches!(err, ConfigError::Write { .. }), "got {err:?}");
    // force overwrites.
    write_config(&config, &path, true).expect("force overwrites");
}

#[test]
fn write_creates_missing_parent_directories() {
    let dir = tempdir();
    let path = dir.join("nested/deeper/config.toml");
    let config = build_config(&sample_config_inputs());
    write_config(&config, &path, false).expect("creates parents and writes");
    assert!(path.exists());
}

/// A unique throwaway temp directory, cleaned up by the OS test sandbox. Avoids
/// adding a `tempfile` dependency to this dependency-light crate.
fn tempdir() -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nonce = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let dir = std::env::temp_dir().join(format!("temper-config-build-test-{pid}-{nonce}"));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}
