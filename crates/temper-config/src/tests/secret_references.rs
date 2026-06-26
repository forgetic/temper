// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn engine_secret_name_references_resolve_from_credentials_toml() {
    let config = parse_config(
        r#"
schema_version = 1
[forge]
url = "http://localhost:3000"
admin = "legacy"
[engine]
forge_token = "forge-engine-token"
webhook_secret = "webhook-secret"
repos = ["a/b"]
roles = ["engineer"]
"#,
    );
    let credentials = parse_credentials(
        r#"
schema_version = 1
[forge.users.legacy]
token = "legacy-token"
[secrets]
forge-engine-token = "named-forge-token"
webhook-secret = "named-webhook-secret"
"#,
    );

    let resolved = resolve(&config, &credentials, &NoEnv).expect("resolves");

    assert_eq!(exposed(&resolved.forge.admin_token), Some("named-forge-token"));
    assert_eq!(
        resolved
            .engine
            .forge_token
            .as_ref()
            .map(|reference| (reference.name.as_str(), reference.available)),
        Some(("forge-engine-token", true))
    );
    assert_eq!(
        resolved
            .engine
            .webhook_secret
            .as_ref()
            .map(|reference| (reference.name.as_str(), reference.available)),
        Some(("webhook-secret", true))
    );
    assert_eq!(
        resolved
            .engine
            .webhook_secret_value
            .as_ref()
            .map(|secret| secret.expose_secret()),
        Some("named-webhook-secret")
    );
    let rendered = format!("{resolved:?}");
    assert!(
        !rendered.contains("named-forge-token"),
        "secret leaked: {rendered}"
    );
    assert!(
        !rendered.contains("named-webhook-secret"),
        "secret leaked: {rendered}"
    );
}

#[test]
fn engine_secret_name_references_accept_structured_credentials_toml() {
    let config = parse_config(
        r#"
schema_version = 1
[forge]
url = "http://localhost:3000"
[engine]
forge_token = "forge-engine-token"
webhook_secret = "webhook-secret"
repos = ["a/b"]
roles = ["engineer"]
"#,
    );
    let credentials = parse_credentials(
        r#"
schema_version = 1
[secrets.forge-engine-token]
kind = "forge-token"
token = "structured-forge-token"
[secrets.webhook-secret]
kind = "webhook-secret"
value = "structured-webhook-secret"
"#,
    );

    let resolved = resolve(&config, &credentials, &NoEnv).expect("resolves");

    assert_eq!(
        exposed(&resolved.forge.admin_token),
        Some("structured-forge-token")
    );
    assert_eq!(
        resolved
            .engine
            .webhook_secret_value
            .as_ref()
            .map(|secret| secret.expose_secret()),
        Some("structured-webhook-secret")
    );
}

#[test]
fn missing_named_secret_references_error_with_clear_fields() {
    let cases: &[(&str, &str)] = &[
        ("engine.forge_token", "[engine]\nforge_token = \"missing\"\n"),
        (
            "engine.webhook_secret",
            "[engine]\nwebhook_secret = \"missing\"\n",
        ),
        (
            "worker.pools[0].worker_token",
            "[[worker.pools]]\nname = \"engineers\"\nroles = [\"engineer\"]\nworker_token = \"missing\"\n",
        ),
        (
            "agent.profiles.coding.credential",
            "[agent.profiles.coding]\ncredential = \"missing\"\n",
        ),
    ];

    for (field, body) in cases {
        let config = parse_config(&format!("schema_version = 1\n{body}"));
        let err = resolve(&config, &Credentials::default(), &NoEnv)
            .expect_err("missing reference should fail");
        let message = err.to_string();
        assert!(message.contains(field), "{field}: {message}");
        assert!(message.contains("missing"), "{field}: {message}");
    }
}

#[test]
fn invalid_secret_names_error_with_clear_fields() {
    let config = parse_config(
        r#"
schema_version = 1
[engine]
forge_token = "../token"
"#,
    );
    let err = resolve(&config, &Credentials::default(), &NoEnv)
        .expect_err("path-like secret name should fail");
    assert!(err.to_string().contains("engine.forge_token"), "{err}");
}

#[test]
fn credentials_debug_leaks_no_secret() {
    let credentials = parse_credentials(
        r#"
schema_version = 1
[forge.users.agent]
password = "agent-pw"
token = "agent-token"
[agent.providers.anthropic]
type = "api-key"
key = "provider-key"
[secrets]
named = "named-secret-value"
"#,
    );
    let rendered = format!("{credentials:?}");
    for secret in [
        "agent-pw",
        "agent-token",
        "provider-key",
        "named-secret-value",
    ] {
        assert!(
            !rendered.contains(secret),
            "secret `{secret}` leaked into Credentials Debug: {rendered}"
        );
    }
    assert!(
        rendered.contains("[REDACTED]"),
        "no redaction marker: {rendered}"
    );
}
