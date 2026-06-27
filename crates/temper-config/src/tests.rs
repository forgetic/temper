// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeMap;

use secrecy::{ExposeSecret, SecretString};

use crate::{
    Config, Credentials, DeploymentTopology, FileKind, NoEnv, ProviderCredential, ProviderKind,
    ResolveOptions, lint, resolve, resolve_with_options,
};

/// The raw value behind an optional secret, for assertions.
fn exposed(secret: &Option<SecretString>) -> Option<&str> {
    secret.as_ref().map(|secret| secret.expose_secret())
}

fn parse_config(text: &str) -> Config {
    Config::parse(text, std::path::Path::new("config.toml"), FileKind::Config)
        .expect("config parses")
}

fn parse_credentials(text: &str) -> Credentials {
    Credentials::parse(
        text,
        std::path::Path::new("credentials.toml"),
        FileKind::Credentials,
    )
    .expect("credentials parse")
}

const FULL_CONFIG: &str = r#"
schema_version = 1
[forge]
type = "forgejo"
url = "http://localhost:3000/"
admin = "agent"
[engine]
port = 4000
workflow = "/wf.json"
repos = ["acme/widgets", "acme/docs"]
roles = ["engineer", "architect"]
mechanical_cadence_secs = 5
[worker]
workspace = "/ws"
[agent]
provider = "anthropic"
[agent.providers.anthropic]
url = "http://fake-llm"
models = { main = "claude-opus-4-8", investigate = "claude-haiku-4-5" }
"#;

const FULL_CREDENTIALS: &str = r#"
schema_version = 1
[forge.users.agent]
password = "agent-pw"
token = "agent-tok"
[forge.users.engineer]
password = "eng-pw"
token = "eng-tok"
email = "eng@example.test"
[forge.users.bot]
password = "bot-pw"
token = "bot-tok"
[agent.providers.anthropic]
type = "oauth"
access = "secret-access-jwt"
refresh = "secret-refresh-token"
expires = 1781371005373
"#;

#[test]
fn resolves_full_deployment() {
    let config = parse_config(FULL_CONFIG);
    let credentials = parse_credentials(FULL_CREDENTIALS);
    let resolved = resolve(&config, &credentials, &NoEnv).expect("resolves");

    // forge: trailing slash stripped, admin token from the admin user.
    assert_eq!(resolved.forge.url.as_deref(), Some("http://localhost:3000"));
    assert_eq!(exposed(&resolved.forge.admin_token), Some("agent-tok"));
    // web-ui from the default ci_user "bot".
    let web = resolved.forge.web_ui.as_ref().expect("web ui");
    assert_eq!(web.username, "bot");
    assert_eq!(web.password.expose_secret(), "bot-pw");

    // engine
    assert_eq!(resolved.engine.bind.to_string(), "127.0.0.1:4000");
    assert_eq!(resolved.engine.repos.len(), 2);
    assert_eq!(resolved.engine.repos[0].display(), "acme/widgets");
    assert_eq!(resolved.engine.roles, vec!["engineer", "architect"]);
    assert_eq!(
        resolved.engine.workflow_file.as_deref(),
        Some(std::path::Path::new("/wf.json"))
    );
    assert_eq!(
        resolved.engine.mechanical_cadence,
        Some(std::time::Duration::from_secs(5))
    );

    // worker capabilities default to repos x roles.
    assert_eq!(resolved.worker.capabilities.len(), 4);
    assert!(
        resolved
            .worker
            .capabilities
            .iter()
            .any(|c| c.repo == "acme/widgets" && c.role == "engineer")
    );
    assert_eq!(resolved.worker.daemon_url, "http://127.0.0.1:4000");

    // role identities: engineer has an explicit email; architect has none in the
    // credentials, so no identity is produced for it (no token).
    let eng = resolved
        .forge
        .role_identities
        .get("engineer")
        .expect("engineer identity");
    assert_eq!(eng.user, "engineer");
    assert_eq!(eng.email, "eng@example.test");
    assert_eq!(eng.token.expose_secret(), "eng-tok");
    assert!(!resolved.forge.role_identities.contains_key("architect"));
    assert_eq!(
        resolved
            .forge
            .role_tokens
            .get("engineer")
            .map(ExposeSecret::expose_secret),
        Some("eng-tok")
    );

    // agent provider
    assert_eq!(resolved.agent.provider.kind, ProviderKind::Anthropic);
    assert!(!resolved.agent.enable_checkpoints);
    assert_eq!(
        resolved.agent.provider.main_model.as_deref(),
        Some("claude-opus-4-8")
    );
    assert_eq!(
        resolved.agent.provider.investigate_model.as_deref(),
        Some("claude-haiku-4-5")
    );
    assert_eq!(
        resolved.agent.provider.base_url.as_deref(),
        Some("http://fake-llm")
    );
    match &resolved.agent.provider.credential {
        ProviderCredential::OAuthInline {
            access,
            refresh,
            expires,
        } => {
            assert_eq!(access.expose_secret(), "secret-access-jwt");
            assert_eq!(
                refresh.as_ref().map(ExposeSecret::expose_secret),
                Some("secret-refresh-token")
            );
            assert_eq!(*expires, 1781371005373);
        }
        other => panic!("expected inline oauth, got {other:?}"),
    }
}

#[test]
fn agent_checkpoints_default_off_and_can_be_enabled() {
    let config = parse_config(
        r#"
schema_version = 1
[agent]
enable_checkpoints = true
"#,
    );
    let resolved = resolve(&config, &Credentials::default(), &NoEnv).expect("resolves");

    assert!(resolved.agent.enable_checkpoints);

    let default_config = parse_config("schema_version = 1\n");
    let default_resolved =
        resolve(&default_config, &Credentials::default(), &NoEnv).expect("resolves");
    assert!(!default_resolved.agent.enable_checkpoints);
}

#[test]
fn poll_cadence_defaults_to_300_when_omitted() {
    let config = parse_config(
        r#"
schema_version = 1
[engine]
repos = ["a/b"]
roles = ["architect", "engineer"]
"#,
    );
    let resolved = resolve(&config, &Credentials::default(), &NoEnv).expect("resolves");
    assert_eq!(
        resolved.engine.poll_cadence,
        std::time::Duration::from_secs(300),
        "omitted poll cadence should use the default backstop interval"
    );
}

#[test]
fn explicit_poll_cadence_is_honored() {
    let config = parse_config(
        r#"
schema_version = 1
[engine]
repos = ["a/b"]
roles = ["architect", "engineer"]
poll_cadence_secs = 2
"#,
    );
    let resolved = resolve(&config, &Credentials::default(), &NoEnv).expect("resolves");
    assert_eq!(
        resolved.engine.poll_cadence,
        std::time::Duration::from_secs(2)
    );
}

#[test]
fn zero_poll_cadence_is_invalid() {
    let config = parse_config(
        r#"
schema_version = 1
[engine]
repos = ["a/b"]
roles = ["architect", "engineer"]
poll_cadence_secs = 0
"#,
    );
    let err = resolve(&config, &Credentials::default(), &NoEnv).expect_err("rejects zero");
    assert!(
        format!("{err}").contains("engine.poll_cadence_secs"),
        "error should identify the invalid field: {err}"
    );
}

#[test]
fn mechanical_backstop_on_by_default_when_omitted() {
    // Omitting `mechanical_cadence_secs` must leave the backstop enabled: it is
    // the level-triggered safety net that stamps intake and lands PRs. A
    // minimal config that forgot the key should still get a working bot.
    let config = parse_config(
        r#"
schema_version = 1
[engine]
repos = ["a/b"]
roles = ["architect", "engineer"]
"#,
    );
    let resolved = resolve(&config, &Credentials::default(), &NoEnv).expect("resolves");
    assert_eq!(
        resolved.engine.mechanical_cadence,
        Some(std::time::Duration::from_secs(120)),
        "omitted cadence must default the mechanical backstop on"
    );
}

#[test]
fn mechanical_backstop_disabled_with_explicit_zero() {
    // `0` is the explicit opt-out: no mechanical worker is spawned.
    let config = parse_config(
        r#"
schema_version = 1
[engine]
repos = ["a/b"]
roles = ["architect", "engineer"]
mechanical_cadence_secs = 0
"#,
    );
    let resolved = resolve(&config, &Credentials::default(), &NoEnv).expect("resolves");
    assert_eq!(
        resolved.engine.mechanical_cadence, None,
        "explicit 0 must disable the mechanical backstop"
    );
}

#[test]
fn deployment_env_vars_are_ignored() {
    // Deployment shape comes only from the config/credentials files: the former
    // `FORGEJO_URL` / `FORGEJO_ACCESS_TOKEN` / `TEMPER_ENGINE_PORT` /
    // `TEMPER_WORKFLOW` / `TEMPER_DAEMON_URL` / `TEMPER_WORKSPACE` overrides have
    // been removed, so setting them must have NO effect — the TOML values win.
    let config = parse_config(FULL_CONFIG);
    let credentials = parse_credentials(FULL_CREDENTIALS);
    let env: BTreeMap<String, String> = BTreeMap::from([
        (
            "TEMPER_FORGE_URL".to_string(),
            "http://env-forge:9000".to_string(),
        ),
        (
            "FORGEJO_URL".to_string(),
            "http://env-forge:9000".to_string(),
        ),
        (
            "TEMPER_FORGE_TOKEN".to_string(),
            "env-admin-tok".to_string(),
        ),
        (
            "FORGEJO_ACCESS_TOKEN".to_string(),
            "env-admin-tok".to_string(),
        ),
        ("FORGEJO_USERNAME".to_string(), "env-ci-user".to_string()),
        ("FORGEJO_PASSWORD".to_string(), "env-ci-pw".to_string()),
        ("TEMPER_ENGINE_BIND".to_string(), "0.0.0.0:5555".to_string()),
        ("TEMPER_ENGINE_PORT".to_string(), "5555".to_string()),
        ("TEMPER_WORKFLOW".to_string(), "/env-wf.json".to_string()),
        (
            "TEMPER_DAEMON_URL".to_string(),
            "http://env-daemon:1234".to_string(),
        ),
        ("TEMPER_WORKSPACE".to_string(), "/env-ws".to_string()),
    ]);
    let resolved = resolve(&config, &credentials, &env).expect("resolves");
    // forge url + admin token come from the files, not the env.
    assert_eq!(resolved.forge.url.as_deref(), Some("http://localhost:3000"));
    assert_eq!(exposed(&resolved.forge.admin_token), Some("agent-tok"));
    // web-ui creds come from the `bot` credentials user, not FORGEJO_USERNAME/PASSWORD.
    let web = resolved.forge.web_ui.as_ref().expect("web ui");
    assert_eq!(web.username, "bot");
    assert_eq!(web.password.expose_secret(), "bot-pw");
    // engine bind/port + workflow come from the file (`port = 4000`, `workflow = /wf.json`).
    assert_eq!(resolved.engine.bind.to_string(), "127.0.0.1:4000");
    assert_eq!(
        resolved.engine.workflow_file.as_deref(),
        Some(std::path::Path::new("/wf.json"))
    );
    // worker daemon_url defaults off the engine bind (no `[worker] daemon_url` in
    // FULL_CONFIG); workspace comes from `[worker] workspace = /ws`.
    assert_eq!(resolved.worker.daemon_url, "http://127.0.0.1:4000");
    assert_eq!(resolved.worker.workspace_root, std::path::Path::new("/ws"));
}

#[test]
fn role_identity_has_no_env_fallback() {
    // Per-role identity comes ONLY from `[forge.users.<role>]` in the credentials
    // file. With no engineer block, the legacy `TEMPER_FORGEJO_*_ENGINEER` vars
    // are now ignored, so no token resolves and the role gets no identity.
    let config = parse_config(FULL_CONFIG);
    let credentials = parse_credentials(
        r#"
schema_version = 1
[forge.users.agent]
token = "agent-tok"
"#,
    );
    let env: BTreeMap<String, String> = BTreeMap::from([
        (
            "TEMPER_FORGEJO_USER_ENGINEER".to_string(),
            "eng-login".to_string(),
        ),
        (
            "TEMPER_FORGEJO_TOKEN_ENGINEER".to_string(),
            "eng-env-tok".to_string(),
        ),
        (
            "TEMPER_FORGEJO_EMAIL_ENGINEER".to_string(),
            "eng@env.test".to_string(),
        ),
    ]);
    let resolved = resolve(&config, &credentials, &env).expect("resolves");
    // No engineer token in credentials and the env fallback is gone => no identity.
    assert!(
        !resolved.forge.role_identities.contains_key("engineer"),
        "role identity must not be sourced from the env"
    );
    assert!(!resolved.forge.role_tokens.contains_key("engineer"));
}

#[test]
fn role_identity_from_credentials_only() {
    // The positive case: the credentials file alone supplies the role identity,
    // even with conflicting legacy env vars set (which must be ignored).
    let config = parse_config(FULL_CONFIG);
    let credentials = parse_credentials(
        r#"
schema_version = 1
[forge.users.agent]
token = "agent-tok"
[forge.users.engineer]
user = "eng-file"
token = "eng-file-tok"
email = "eng@file.test"
"#,
    );
    let env: BTreeMap<String, String> = BTreeMap::from([
        (
            "TEMPER_FORGEJO_USER_ENGINEER".to_string(),
            "eng-env".to_string(),
        ),
        (
            "TEMPER_FORGEJO_TOKEN_ENGINEER".to_string(),
            "eng-env-tok".to_string(),
        ),
    ]);
    let resolved = resolve(&config, &credentials, &env).expect("resolves");
    let eng = resolved
        .forge
        .role_identities
        .get("engineer")
        .expect("engineer identity from credentials");
    assert_eq!(eng.user, "eng-file");
    assert_eq!(eng.email, "eng@file.test");
    assert_eq!(eng.token.expose_secret(), "eng-file-tok");
}

#[test]
fn explicit_capabilities_override_default() {
    let config = parse_config(
        r#"
schema_version = 1
[engine]
repos = ["a/b"]
roles = ["engineer", "architect"]
[worker]
capabilities = ["a/b:engineer"]
"#,
    );
    let resolved = resolve(&config, &Credentials::default(), &NoEnv).expect("resolves");
    assert_eq!(resolved.worker.capabilities.len(), 1);
    assert_eq!(resolved.worker.capabilities[0].role, "engineer");
}

#[test]
fn schema_version_must_match() {
    let err = Config::parse(
        "schema_version = 2\n",
        std::path::Path::new("c.toml"),
        FileKind::Config,
    )
    .expect_err("rejects v2");
    assert!(format!("{err}").contains("unsupported schema_version 2"));

    let err = Config::parse(
        "[forge]\n",
        std::path::Path::new("c.toml"),
        FileKind::Config,
    )
    .expect_err("rejects missing version");
    assert!(format!("{err}").contains("missing `schema_version`"));
}

#[test]
fn unknown_key_is_rejected() {
    let err = Config::parse(
        "schema_version = 1\n[engine]\nbogus = 1\n",
        std::path::Path::new("c.toml"),
        FileKind::Config,
    )
    .expect_err("rejects unknown key");
    assert!(format!("{err}").contains("bogus"), "got: {err}");
}

#[test]
fn api_key_credential() {
    let config = parse_config(
        r#"
schema_version = 1
[agent]
provider = "deepseek"
"#,
    );
    let credentials = parse_credentials(
        r#"
schema_version = 1
[agent.providers.deepseek]
type = "api-key"
key = "sk-secret"
"#,
    );
    let resolved = resolve(&config, &credentials, &NoEnv).expect("resolves");
    assert_eq!(resolved.agent.provider.kind, ProviderKind::DeepSeek);
    match &resolved.agent.provider.credential {
        ProviderCredential::ApiKey(key) => assert_eq!(key.expose_secret(), "sk-secret"),
        other => panic!("expected api key, got {other:?}"),
    }
}

#[test]
fn lint_flags_missing_essentials() {
    let resolved = resolve(&Config::default(), &Credentials::default(), &NoEnv).expect("resolves");
    let findings = lint(&resolved);
    let errors: Vec<_> = findings
        .iter()
        .filter(|f| f.error)
        .map(|f| f.message.clone())
        .collect();
    assert!(errors.iter().any(|m| m.contains("forge URL")));
    assert!(errors.iter().any(|m| m.contains("no repositories")));
    assert!(errors.iter().any(|m| m.contains("no roles")));
}

#[test]
fn default_workspace_is_xdg_state_path() {
    // With no config and only HOME set, the workspace defaults to the XDG state
    // path `~/.local/state/temper/workspace`.
    let env: BTreeMap<String, String> =
        BTreeMap::from([("HOME".to_string(), "/home/op".to_string())]);
    let resolved = resolve(&Config::default(), &Credentials::default(), &env).expect("resolves");
    assert_eq!(
        resolved.worker.workspace_root,
        std::path::Path::new("/home/op/.local/state/temper/workspace")
    );
}

#[test]
fn xdg_state_home_overrides_default_workspace() {
    let env: BTreeMap<String, String> = BTreeMap::from([
        ("HOME".to_string(), "/home/op".to_string()),
        ("XDG_STATE_HOME".to_string(), "/xdg/state".to_string()),
    ]);
    let resolved = resolve(&Config::default(), &Credentials::default(), &env).expect("resolves");
    assert_eq!(
        resolved.worker.workspace_root,
        std::path::Path::new("/xdg/state/temper/workspace")
    );
}

#[test]
fn hand_written_tilde_workspace_expands() {
    let config = parse_config(
        r#"
schema_version = 1
[worker]
workspace = "~/.local/state/temper/workspace"
"#,
    );
    let env: BTreeMap<String, String> =
        BTreeMap::from([("HOME".to_string(), "/home/op".to_string())]);
    let resolved = resolve(&config, &Credentials::default(), &env).expect("resolves");
    assert_eq!(
        resolved.worker.workspace_root,
        std::path::Path::new("/home/op/.local/state/temper/workspace")
    );
}

mod secret_references;
mod target_sections;

#[test]
fn direct_resolve_keeps_relative_paths_relative_without_config_context() {
    let config = parse_config(
        r#"
schema_version = 1
[engine]
workflow = "flows/workflow.json"
webhook_secret_file = "secrets/webhook-secret"
[worker]
workspace = "workspace"
[agent]
config_dir = "agent-config"
"#,
    );
    let resolved = resolve(&config, &Credentials::default(), &NoEnv).expect("resolves");

    assert_eq!(
        resolved.engine.workflow_file.as_deref(),
        Some(std::path::Path::new("flows/workflow.json"))
    );
    assert_eq!(
        resolved.engine.webhook_secret_file.as_deref(),
        Some(std::path::Path::new("secrets/webhook-secret"))
    );
    assert_eq!(
        resolved.worker.workspace_root,
        std::path::Path::new("workspace")
    );
    assert_eq!(
        resolved.agent.config_dir.as_deref(),
        Some(std::path::Path::new("agent-config"))
    );
}

#[test]
fn redacts_secrets_in_debug() {
    let config = parse_config(FULL_CONFIG);
    let credentials = parse_credentials(FULL_CREDENTIALS);
    let resolved = resolve(&config, &credentials, &NoEnv).expect("resolves");
    let rendered = format!("{:?}", resolved.agent.provider.credential);
    assert!(
        !rendered.contains("secret-access-jwt"),
        "access token leaked: {rendered}"
    );
    assert!(
        !rendered.contains("secret-refresh-token"),
        "refresh token leaked: {rendered}"
    );
    // secrecy renders a `SecretString` as `[REDACTED]`.
    assert!(
        rendered.contains("[REDACTED]"),
        "no redaction marker: {rendered}"
    );
    let web = format!("{:?}", resolved.forge.web_ui);
    assert!(!web.contains("bot-pw"), "web-ui password leaked: {web}");
}

/// Anti-leak golden test: a full `Debug` of `Resolved` must contain none of the
/// secret substrings present in the fixture credentials. This is the structural
/// guard against a future field type regressing back to a plain `String`.
#[test]
fn resolved_debug_leaks_no_secret() {
    let config = parse_config(FULL_CONFIG);
    let credentials = parse_credentials(FULL_CREDENTIALS);
    let resolved = resolve(&config, &credentials, &NoEnv).expect("resolves");
    let rendered = format!("{resolved:?}");
    for secret in [
        "agent-tok",
        "agent-pw",
        "eng-tok",
        "eng-pw",
        "bot-tok",
        "bot-pw",
        "secret-access-jwt",
        "secret-refresh-token",
    ] {
        assert!(
            !rendered.contains(secret),
            "secret `{secret}` leaked into Resolved Debug: {rendered}"
        );
    }
}

/// The `Secret` alias round-trips: `expose_secret()` returns the raw value while
/// `Debug` renders `[REDACTED]`.
#[test]
fn secret_alias_round_trips_and_redacts() {
    use crate::Secret;

    let secret = Secret::from("hunter2");
    assert_eq!(secret.expose_secret(), "hunter2");
    let rendered = format!("{secret:?}");
    assert!(
        rendered.contains("[REDACTED]"),
        "no redaction marker: {rendered}"
    );
    assert!(!rendered.contains("hunter2"), "secret leaked: {rendered}");
}
