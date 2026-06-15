// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeMap;

use crate::{
    Config, Credentials, FileKind, NoEnv, ProviderCredential, ProviderKind, lint, resolve,
};

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
    assert_eq!(resolved.forge.admin_token.as_deref(), Some("agent-tok"));
    // web-ui from the default ci_user "bot".
    let web = resolved.forge.web_ui.as_ref().expect("web ui");
    assert_eq!(web.username, "bot");
    assert_eq!(web.password, "bot-pw");

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
    assert_eq!(eng.token, "eng-tok");
    assert!(!resolved.forge.role_identities.contains_key("architect"));
    assert_eq!(
        resolved
            .forge
            .role_tokens
            .get("engineer")
            .map(String::as_str),
        Some("eng-tok")
    );

    // agent provider
    assert_eq!(resolved.agent.provider.kind, ProviderKind::Anthropic);
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
            assert_eq!(access, "secret-access-jwt");
            assert_eq!(refresh.as_deref(), Some("secret-refresh-token"));
            assert_eq!(*expires, 1781371005373);
        }
        other => panic!("expected inline oauth, got {other:?}"),
    }
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
fn env_overrides_file() {
    let config = parse_config(FULL_CONFIG);
    let credentials = parse_credentials(FULL_CREDENTIALS);
    let env: BTreeMap<String, String> = BTreeMap::from([
        (
            "FORGEJO_URL".to_string(),
            "http://env-forge:9000".to_string(),
        ),
        (
            "FORGEJO_ACCESS_TOKEN".to_string(),
            "env-admin-tok".to_string(),
        ),
        ("TEMPER_ENGINE_PORT".to_string(), "5555".to_string()),
        ("TEMPER_WORKFLOW".to_string(), "/env-wf.json".to_string()),
    ]);
    let resolved = resolve(&config, &credentials, &env).expect("resolves");
    assert_eq!(resolved.forge.url.as_deref(), Some("http://env-forge:9000"));
    assert_eq!(resolved.forge.admin_token.as_deref(), Some("env-admin-tok"));
    assert_eq!(resolved.engine.bind.to_string(), "127.0.0.1:5555");
    assert_eq!(resolved.worker.daemon_url, "http://127.0.0.1:5555");
    assert_eq!(
        resolved.engine.workflow_file.as_deref(),
        Some(std::path::Path::new("/env-wf.json"))
    );
}

#[test]
fn legacy_role_env_fallback() {
    // No engineer block in the credentials file: the legacy roles.env vars supply
    // the identity, so existing provisioned deployments keep working.
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
    ]);
    let resolved = resolve(&config, &credentials, &env).expect("resolves");
    let eng = resolved
        .forge
        .role_identities
        .get("engineer")
        .expect("engineer identity");
    assert_eq!(eng.user, "eng-login");
    assert_eq!(eng.email, "eng-login@noreply.localhost");
    assert_eq!(eng.token, "eng-env-tok");
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
        ProviderCredential::ApiKey(key) => assert_eq!(key, "sk-secret"),
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
    assert!(rendered.contains("redacted"));
    let web = format!("{:?}", resolved.forge.web_ui);
    assert!(!web.contains("bot-pw"));
}
