// SPDX-License-Identifier: MPL-2.0

use std::time::Duration;

use serde_json::Value;

use super::parse_config;
use crate::{Config, Credentials, FileKind, NoEnv, config_json_schema, resolve};

fn resolved_ci_cadence(toml: &str) -> Option<Duration> {
    let resolved =
        resolve(&parse_config(toml), &Credentials::default(), &NoEnv).expect("config resolves");
    resolved.engine.ci_poll_cadence
}

fn resolved_ci_missing_grace(toml: &str) -> Duration {
    let resolved =
        resolve(&parse_config(toml), &Credentials::default(), &NoEnv).expect("config resolves");
    resolved.engine.ci_missing_grace
}

#[test]
fn ci_poll_cadence_defaults_to_60_when_omitted() {
    assert_eq!(
        resolved_ci_cadence(
            r#"
schema_version = 1
[engine]
repos = ["a/b"]
roles = ["architect", "engineer"]
"#,
        ),
        Some(Duration::from_secs(60)),
        "omitted CI poll cadence should use the dedicated backstop default"
    );
}

#[test]
fn explicit_ci_poll_cadence_is_honored() {
    assert_eq!(
        resolved_ci_cadence(
            r#"
schema_version = 1
[engine]
repos = ["a/b"]
roles = ["architect", "engineer"]
ci_poll_cadence_secs = 17
"#,
        ),
        Some(Duration::from_secs(17))
    );
}

#[test]
fn missing_ci_grace_defaults_to_300_when_omitted() {
    assert_eq!(
        resolved_ci_missing_grace(
            r#"
schema_version = 1
[engine]
repos = ["a/b"]
roles = ["architect", "engineer"]
"#,
        ),
        Duration::from_secs(300)
    );
}

#[test]
fn positive_missing_ci_grace_is_honored() {
    assert_eq!(
        resolved_ci_missing_grace(
            r#"
schema_version = 1
[engine]
repos = ["a/b"]
roles = ["architect", "engineer"]
ci_missing_grace_secs = 37
"#,
        ),
        Duration::from_secs(37)
    );
}

#[test]
fn zero_missing_ci_grace_is_rejected() {
    let config = parse_config(
        r#"
schema_version = 1
[engine]
ci_missing_grace_secs = 0
"#,
    );
    let error = resolve(&config, &Credentials::default(), &NoEnv)
        .expect_err("zero must not disable the missing-CI grace");

    assert!(error.to_string().contains("engine.ci_missing_grace_secs"));
}

#[test]
fn invalid_missing_ci_grace_is_rejected() {
    Config::parse(
        "schema_version = 1\n[engine]\nci_missing_grace_secs = \"later\"\n",
        std::path::Path::new("config.toml"),
        FileKind::Config,
    )
    .expect_err("non-integer grace must fail config parsing");
}

#[test]
fn missing_ci_grace_typo_is_rejected_as_an_unknown_field() {
    let error = Config::parse(
        "schema_version = 1\n[engine]\nci_missing_grace_seconds = 30\n",
        std::path::Path::new("config.toml"),
        FileKind::Config,
    )
    .expect_err("rejects unknown key");

    assert!(
        error.to_string().contains("ci_missing_grace_seconds"),
        "got: {error}"
    );
}

#[test]
fn zero_ci_poll_cadence_disables_only_the_dedicated_backstop() {
    let config = parse_config(
        r#"
schema_version = 1
[engine]
repos = ["a/b"]
roles = ["architect", "engineer"]
ci_poll_cadence_secs = 0
ci_missing_grace_secs = 41
"#,
    );
    let resolved = resolve(&config, &Credentials::default(), &NoEnv).expect("resolves");

    assert_eq!(resolved.engine.ci_poll_cadence, None);
    assert_eq!(resolved.engine.ci_missing_grace, Duration::from_secs(41));
    assert_eq!(
        resolved.engine.poll_cadence,
        Duration::from_secs(300),
        "disabling the dedicated CI poll must not disable or change the role poll"
    );
    assert_eq!(
        resolved.engine.mechanical_cadence,
        Some(Duration::from_secs(120)),
        "the CI setting must not be coupled to the mechanical cadence"
    );
}

#[test]
fn ci_poll_typo_is_rejected_as_an_unknown_field() {
    let error = Config::parse(
        "schema_version = 1\n[engine]\nci_poll_cadence_seconds = 1\n",
        std::path::Path::new("config.toml"),
        FileKind::Config,
    )
    .expect_err("rejects unknown key");

    assert!(
        error.to_string().contains("ci_poll_cadence_seconds"),
        "got: {error}"
    );
}

#[test]
fn ci_poll_cadence_is_present_in_the_closed_json_schema() {
    let schema = config_json_schema();
    let engine = &schema["properties"]["engine"];

    assert_eq!(engine["additionalProperties"], Value::Bool(false));
    assert_eq!(
        engine["properties"]["ci_poll_cadence_secs"]["minimum"],
        Value::from(0)
    );
    assert_eq!(
        engine["properties"]["ci_missing_grace_secs"]["minimum"],
        Value::from(1)
    );
    assert_eq!(
        engine["properties"]["poll_cadence_secs"]["minimum"],
        Value::from(1),
        "the mandatory role-poll runtime cadence remains positive"
    );
}
