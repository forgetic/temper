use std::time::Duration;

use serde_json::Value;

use super::{parse_config, parse_credentials};
use crate::{Credentials, NoEnv, config_json_schema, config_template, resolve};

fn resolve_text(text: &str) -> Result<crate::Resolved, crate::ConfigError> {
    resolve(&parse_config(text), &Credentials::default(), &NoEnv)
}

#[test]
fn deadline_and_liveness_defaults_apply_without_new_toml() {
    let resolved = resolve_text("schema_version = 1\n").expect("legacy config resolves");

    assert_eq!(
        resolved.deployment.standalone_shutdown_budget,
        Duration::from_secs(30)
    );
    assert_eq!(
        resolved.agent.operation_limits.tool_timeout,
        Duration::from_secs(600)
    );
    assert_eq!(
        resolved.agent.operation_limits.model_connect_timeout,
        Duration::from_secs(120)
    );
    assert_eq!(
        resolved.agent.operation_limits.model_idle_timeout,
        Duration::from_secs(120)
    );
    assert_eq!(
        resolved.worker.liveness_limits.max_no_progress,
        Duration::from_secs(900)
    );
    assert_eq!(resolved.worker.liveness_limits.max_run, None);
    assert_eq!(resolved.worker.session_recovery.session_failure_limit, 1);
    assert_eq!(resolved.worker.session_recovery.fresh_session_limit, 1);
    assert_eq!(resolved.worker.session_recovery.provider_deferral_limit, 3);
    assert_eq!(
        resolved.worker.session_recovery.provider_deferral_delay,
        Duration::from_secs(300)
    );
    assert_eq!(
        resolved.worker.session_recovery.recovery_slo,
        Duration::from_secs(7_200)
    );
    assert_eq!(
        resolved.worker.liveness_limits.graceful_cancellation_grace,
        Duration::from_secs(10)
    );
    assert_eq!(
        resolved.worker.liveness_limits.forced_termination_grace,
        Duration::from_secs(5)
    );
    assert_eq!(
        resolved.worker.result_root,
        std::path::PathBuf::from(".temper/workspace/.temper/worker-results")
    );
}

#[test]
fn config_template_resolves_the_documented_liveness_contract() {
    let template = config_template();
    for documented in [
        "standalone_shutdown_budget_secs = 30",
        "max_no_progress_secs = 900",
        "graceful_cancellation_grace_secs = 10",
        "forced_termination_grace_secs = 5",
        "session_failure_limit = 1",
        "fresh_session_limit = 1",
        "provider_deferral_limit = 3",
        "provider_deferral_delay_secs = 300",
        "model_recovery_slo_secs = 7200",
        "tool_timeout_secs = 600",
        "model_connect_timeout_secs = 120",
        "model_idle_timeout_secs = 120",
    ] {
        assert!(template.contains(documented), "missing `{documented}`");
    }

    let resolved = resolve_text(&template).expect("starter template resolves");
    assert_eq!(
        resolved.worker.liveness_limits.max_no_progress,
        Duration::from_secs(900)
    );
    assert_eq!(
        resolved.agent.operation_limits.tool_timeout,
        Duration::from_secs(600)
    );
}

#[test]
fn standalone_shutdown_budget_strictly_exceeds_all_fixed_and_worker_allowances() {
    let resolved =
        resolve_text("schema_version = 1\n[deployment]\nstandalone_shutdown_budget_secs = 26\n")
            .expect("one second beyond all default allowances is valid");
    assert_eq!(
        resolved.deployment.standalone_shutdown_budget,
        Duration::from_secs(26)
    );

    let error = resolve_text(
        "schema_version = 1\n[deployment]\nstandalone_shutdown_budget_secs = 30\n[worker]\ngraceful_cancellation_grace_secs = 15\nforced_termination_grace_secs = 5\n",
    )
    .expect_err("equality with all allowances is invalid");
    assert!(
        error
            .to_string()
            .contains("must strictly exceed worker graceful_cancellation_grace_secs"),
        "{error}"
    );
}

#[test]
fn profiles_inherit_each_missing_deadline_independently() {
    let config = parse_config(
        r#"
schema_version = 1
[paths]
state_dir = "/srv/temper"
[agent.deadlines]
tool_timeout_secs = 500
model_connect_timeout_secs = 100
model_idle_timeout_secs = 80
[agent.profiles.coding]
command = ["temper", "agent"]
[agent.profiles.coding.deadlines]
model_idle_timeout_secs = 40
"#,
    );
    let resolved = resolve(&config, &Credentials::default(), &NoEnv).expect("resolves");
    let limits = resolved.agent.profiles["coding"].operation_limits;

    assert_eq!(limits.tool_timeout, Duration::from_secs(500));
    assert_eq!(limits.model_connect_timeout, Duration::from_secs(100));
    assert_eq!(limits.model_idle_timeout, Duration::from_secs(40));
    assert_eq!(
        resolved.worker.result_root,
        std::path::PathBuf::from("/srv/temper/worker-results")
    );
}

#[test]
fn optional_max_run_is_positive_but_independent_of_other_limits() {
    let resolved = resolve_text(
        r#"
schema_version = 1
[worker]
max_run_secs = 1
"#,
    )
    .expect("short max run is independently valid");
    assert_eq!(
        resolved.worker.liveness_limits.max_run,
        Some(Duration::from_secs(1))
    );

    let error = resolve_text(
        r#"
schema_version = 1
[worker]
max_run_secs = 0
"#,
    )
    .expect_err("zero max run is invalid");
    assert!(error.to_string().contains("worker.max_run_secs"), "{error}");
}

#[test]
fn durable_session_recovery_policy_resolves_and_rejects_unbounded_values() {
    let resolved = resolve_text(
        r#"
schema_version = 1
[worker]
session_failure_limit = 2
fresh_session_limit = 0
provider_deferral_limit = 4
provider_deferral_delay_secs = 30
model_recovery_slo_secs = 90
"#,
    )
    .expect("custom durable recovery policy resolves");
    let policy = resolved.worker.session_recovery;
    assert_eq!(policy.session_failure_limit, 2);
    assert_eq!(policy.fresh_session_limit, 0);
    assert_eq!(policy.provider_deferral_limit, 4);
    assert_eq!(policy.provider_deferral_delay, Duration::from_secs(30));
    assert_eq!(policy.recovery_slo, Duration::from_secs(90));

    for (body, expected) in [
        ("session_failure_limit = 33", "session_failure_limit"),
        ("fresh_session_limit = 33", "fresh_session_limit"),
        ("provider_deferral_limit = 33", "provider_deferral_limit"),
        (
            "provider_deferral_delay_secs = 91\nmodel_recovery_slo_secs = 90",
            "must not exceed",
        ),
    ] {
        let error =
            resolve_text(&format!("schema_version = 1\n[worker]\n{body}\n")).expect_err(expected);
        assert!(error.to_string().contains(expected), "{error}");
    }
}

#[test]
fn invalid_liveness_orderings_are_rejected() {
    let cases = [
        (
            "heartbeat",
            "[worker]\nmax_no_progress_secs = 10\nheartbeat_interval_ms = 10000\n",
            "heartbeat_interval_ms",
        ),
        (
            "combined graces",
            "[worker]\nmax_no_progress_secs = 15\ngraceful_cancellation_grace_secs = 10\nforced_termination_grace_secs = 5\n",
            "graceful_cancellation_grace_secs plus forced_termination_grace_secs",
        ),
        (
            "standalone shutdown budget",
            "[deployment]\nstandalone_shutdown_budget_secs = 25\n",
            "standalone_shutdown_budget_secs",
        ),
        (
            "tool deadline",
            "[worker]\nmax_no_progress_secs = 600\n[agent.deadlines]\ntool_timeout_secs = 600\n",
            "tool_timeout_secs",
        ),
        (
            "connect deadline",
            "[worker]\nmax_no_progress_secs = 120\n[agent.deadlines]\ntool_timeout_secs = 10\nmodel_connect_timeout_secs = 120\nmodel_idle_timeout_secs = 10\n",
            "model_connect_timeout_secs",
        ),
        (
            "idle deadline",
            "[worker]\nmax_no_progress_secs = 120\n[agent.deadlines]\ntool_timeout_secs = 10\nmodel_connect_timeout_secs = 10\nmodel_idle_timeout_secs = 120\n",
            "model_idle_timeout_secs",
        ),
        (
            "profile deadline",
            "[worker]\nmax_no_progress_secs = 30\n[agent.deadlines]\ntool_timeout_secs = 10\nmodel_connect_timeout_secs = 10\nmodel_idle_timeout_secs = 10\n[agent.profiles.coding]\ncommand = [\"temper\", \"agent\"]\n[agent.profiles.coding.deadlines]\ntool_timeout_secs = 30\n",
            "agent.profiles.coding.deadlines.tool_timeout_secs",
        ),
    ];
    for (name, body, expected) in cases {
        let text = format!("schema_version = 1\n{body}");
        let error = resolve_text(&text).expect_err(name);
        assert!(error.to_string().contains(expected), "{name}: {error}");
    }
}

#[test]
fn every_new_duration_rejects_zero() {
    let fields = [
        ("deployment", "standalone_shutdown_budget_secs"),
        ("worker", "heartbeat_interval_ms"),
        ("worker", "max_no_progress_secs"),
        ("worker", "graceful_cancellation_grace_secs"),
        ("worker", "forced_termination_grace_secs"),
        ("worker", "session_failure_limit"),
        ("worker", "provider_deferral_limit"),
        ("worker", "provider_deferral_delay_secs"),
        ("worker", "model_recovery_slo_secs"),
        ("agent.deadlines", "tool_timeout_secs"),
        ("agent.deadlines", "model_connect_timeout_secs"),
        ("agent.deadlines", "model_idle_timeout_secs"),
    ];
    for (section, field) in fields {
        let text = format!("schema_version = 1\n[{section}]\n{field} = 0\n");
        let error = resolve_text(&text).expect_err(field);
        assert!(error.to_string().contains(field), "{error}");
    }
}

#[test]
fn third_party_profile_deadlines_do_not_constrain_worker_progress() {
    resolve_text(
        r#"
schema_version = 1
[worker]
max_no_progress_secs = 30
[agent.deadlines]
tool_timeout_secs = 10
model_connect_timeout_secs = 10
model_idle_timeout_secs = 10
[agent.profiles.external]
command = ["custom-coder"]
[agent.profiles.external.deadlines]
tool_timeout_secs = 1000
"#,
    )
    .expect("third-party commands use worker fallback supervision");
}

#[test]
fn json_schema_marks_all_duration_seconds_as_positive() {
    let schema = config_json_schema();
    let deployment = &schema["properties"]["deployment"]["properties"];
    assert_eq!(
        deployment["standalone_shutdown_budget_secs"]["minimum"],
        Value::from(1)
    );
    let worker = &schema["properties"]["worker"]["properties"];
    for field in [
        "heartbeat_interval_ms",
        "max_no_progress_secs",
        "max_run_secs",
        "graceful_cancellation_grace_secs",
        "forced_termination_grace_secs",
    ] {
        assert_eq!(worker[field]["minimum"], Value::from(1), "{field}");
    }
    let deadlines = &schema["properties"]["agent"]["properties"]["deadlines"]["properties"];
    for field in [
        "tool_timeout_secs",
        "model_connect_timeout_secs",
        "model_idle_timeout_secs",
    ] {
        assert_eq!(deadlines[field]["minimum"], Value::from(1), "{field}");
    }
}

#[test]
fn schema_round_trips_profile_deadline_tables() {
    let config = parse_config(
        r#"
schema_version = 1
[agent.profiles.coding.deadlines]
tool_timeout_secs = 7
"#,
    );
    let encoded = toml::to_string(&config).expect("serialize config");
    let reparsed = parse_config(&encoded);
    assert_eq!(
        reparsed.agent.profiles["coding"]
            .deadlines
            .tool_timeout_secs,
        Some(7)
    );

    // Keep the credentials parser imported through this test module as a guard
    // that the config-only additions do not alter the secret schema.
    let credentials = parse_credentials("schema_version = 1\n");
    assert!(credentials.secrets.is_empty());
}
