use secrecy::ExposeSecret;
use serde_json::Value;

use super::{parse_config, parse_credentials};
use crate::{NoEnv, ResolveOptions, config_json_schema, resolve, resolve_with_options};

const CONFIG: &str = r#"
schema_version = 1
[forge]
type = "forgejo"
url = "https://forge.example"
[forge.ci_failure_evidence]
endpoint = "https://evidence.example/v1/forgejo-failures"
issuer = "runner-host"
protected_producers = ["protected-ci", "release-ci"]
bearer_token = "ci-evidence-reader"
hmac_key = "ci-evidence-hmac"
"#;

const CREDENTIALS: &str = r#"
schema_version = 1
[secrets]
ci-evidence-reader = "reader-secret"
ci-evidence-hmac = "integrity-secret"
"#;

#[test]
fn evidence_source_resolves_only_from_closed_config_and_secret_sources() {
    let resolved = resolve(
        &parse_config(CONFIG),
        &parse_credentials(CREDENTIALS),
        &NoEnv,
    )
    .unwrap();
    let evidence = resolved
        .forge
        .ci_failure_evidence
        .as_ref()
        .expect("evidence source configured");
    assert_eq!(
        evidence.endpoint,
        "https://evidence.example/v1/forgejo-failures"
    );
    assert_eq!(evidence.issuer, "runner-host");
    assert_eq!(evidence.protected_producers, ["protected-ci", "release-ci"]);
    assert_eq!(evidence.bearer_token.name, "ci-evidence-reader");
    assert_eq!(
        evidence
            .bearer_token_value
            .as_ref()
            .map(ExposeSecret::expose_secret),
        Some("reader-secret")
    );
    assert_eq!(
        evidence
            .hmac_key_value
            .as_ref()
            .map(ExposeSecret::expose_secret),
        Some("integrity-secret")
    );

    let debug = format!("{resolved:?}");
    assert!(!debug.contains("reader-secret"));
    assert!(!debug.contains("integrity-secret"));
}

#[test]
fn ambient_environment_cannot_enable_or_authenticate_evidence() {
    let env = std::collections::BTreeMap::from([
        (
            "TEMPER_CI_EVIDENCE_ENDPOINT".to_string(),
            "https://ambient.example".to_string(),
        ),
        (
            "TEMPER_CI_EVIDENCE_TOKEN".to_string(),
            "ambient-token".to_string(),
        ),
        (
            "TEMPER_CI_EVIDENCE_HMAC_KEY".to_string(),
            "ambient-key".to_string(),
        ),
    ]);
    let absent = parse_config("schema_version = 1\n");
    assert!(
        resolve(&absent, &Default::default(), &env)
            .unwrap()
            .forge
            .ci_failure_evidence
            .is_none()
    );

    let options = ResolveOptions {
        validate_secret_references: false,
        ..Default::default()
    };
    let unresolved =
        resolve_with_options(&parse_config(CONFIG), &Default::default(), &env, &options).unwrap();
    let evidence = unresolved.forge.ci_failure_evidence.unwrap();
    assert!(!evidence.bearer_token.available);
    assert!(evidence.bearer_token_value.is_none());
    assert!(evidence.hmac_key_value.is_none());
}

#[test]
fn strict_resolution_rejects_missing_secrets_and_invalid_source_fields() {
    let options = ResolveOptions {
        validate_secret_references: true,
        ..Default::default()
    };
    let error = resolve_with_options(&parse_config(CONFIG), &Default::default(), &NoEnv, &options)
        .unwrap_err();
    assert!(error.to_string().contains("ci-evidence-reader"));

    for replacement in [
        (
            "https://evidence.example/v1/forgejo-failures",
            "http://remote.example/failures",
        ),
        ("issuer = \"runner-host\"", "issuer = \"bad issuer\""),
        (
            "protected_producers = [\"protected-ci\", \"release-ci\"]",
            "protected_producers = []",
        ),
    ] {
        let invalid = CONFIG.replace(replacement.0, replacement.1);
        assert!(
            resolve(
                &parse_config(&invalid),
                &parse_credentials(CREDENTIALS),
                &NoEnv
            )
            .is_err()
        );
    }
}

#[test]
fn closed_json_schema_exposes_only_the_selected_transport_fields() {
    let source = &config_json_schema()["properties"]["forge"]["properties"]["ci_failure_evidence"];
    assert_eq!(source["additionalProperties"], Value::Bool(false));
    assert_eq!(source["required"].as_array().unwrap().len(), 5);
    let properties = source["properties"].as_object().unwrap();
    assert_eq!(
        properties.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "bearer_token",
            "endpoint",
            "hmac_key",
            "issuer",
            "protected_producers"
        ]
    );
}
