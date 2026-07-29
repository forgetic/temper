// SPDX-License-Identifier: MPL-2.0

use std::path::{Path, PathBuf};
use std::time::Duration;

use toml::Value as TomlValue;

use super::{
    LateStreamFailureBurst, LateStreamFailureFixture, ManifestAction, ManifestExecutionPlan,
    ScenarioBundle, StimulusKind,
};

#[test]
fn declarative_stimuli_are_bounded_and_ordered_before_convergence() {
    let path = scenarios_root().join("basic-delivery/scenario.toml");
    let mut manifest =
        temper_scenario_core::load_resolved_manifest_toml(path).expect("resolved basic manifest");
    let ci_fixture = manifest
        .get("repos")
        .and_then(TomlValue::as_array)
        .and_then(|repos| repos.first())
        .and_then(|repo| repo.get("ci_source"))
        .and_then(TomlValue::as_str)
        .expect("resolved CI fixture");
    let additions = format!(
        r#"
        [[steps]]
        id = "restart-temper"
        action = "temper.restart"
        timeout_ms = 15000
        max_attempts = 2

        [[steps]]
        id = "fail-ci"
        action = "ci.fail"
        repo = "service"
        fixture = "{ci_fixture}"

        [[steps]]
        id = "recover-ci"
        action = "ci.recover"
        repo = "service"
        fixture = "{ci_fixture}"

        [[steps]]
        id = "repeat-source-delivery"
        action = "delivery.repeat"
        artifact = "issue:intake"
        deliveries = 3
    "#
    )
    .parse::<TomlValue>()
    .expect("stimulus TOML");
    let added = additions
        .get("steps")
        .and_then(TomlValue::as_array)
        .expect("added steps")
        .clone();
    let steps = manifest
        .get_mut("steps")
        .and_then(TomlValue::as_array_mut)
        .expect("manifest steps");
    let convergence = convergence_step(steps);
    steps.splice(convergence..convergence, added);

    let plan = ManifestExecutionPlan::from_manifest(&manifest).expect("stimuli resolve");

    assert_eq!(plan.stimuli.len(), 4);
    assert_eq!(plan.stimuli[0].timeout, Duration::from_secs(15));
    assert_eq!(plan.stimuli[0].max_attempts, 2);
    assert!(matches!(
        plan.stimuli[3].kind,
        StimulusKind::RepeatDelivery { deliveries: 3, .. }
    ));
}

#[test]
fn ci_failure_requires_recovery_and_stimulus_bounds_are_enforced() {
    let path = scenarios_root().join("basic-delivery/scenario.toml");
    let mut manifest =
        temper_scenario_core::load_resolved_manifest_toml(path).expect("resolved basic manifest");
    let steps = manifest
        .get_mut("steps")
        .and_then(TomlValue::as_array_mut)
        .expect("manifest steps");
    let convergence = convergence_step(steps);
    let stimulus = r#"
        id = "fail-ci"
        action = "ci.fail"
        repo = "service"
        max_attempts = 99
    "#
    .parse::<TomlValue>()
    .expect("stimulus TOML");
    steps.insert(convergence, stimulus);

    let error =
        ManifestExecutionPlan::from_manifest(&manifest).expect_err("unbounded attempts must fail");
    assert!(
        error.contains("max_attempts must be an integer from 1 through 3"),
        "{error}"
    );
}

#[test]
fn late_stream_failure_and_recovery_wake_are_bounded_manifest_primitives() {
    let path = scenarios_root().join("basic-delivery/scenario.toml");
    let mut manifest = temper_scenario_core::load_resolved_manifest_toml(path.clone())
        .expect("resolved basic manifest");
    let recovery = r#"
        model_retry_max_attempts = 2
        model_retry_base_delay_ms = 1
        model_retry_max_delay_ms = 2
        model_retry_jitter_percent = 0
        session_failure_limit = 1
        fresh_session_limit = 1
        provider_deferral_limit = 3
        provider_deferral_delay_secs = 300
        model_recovery_slo_secs = 7200
    "#
    .parse::<TomlValue>()
    .unwrap();
    let mut live_harness = toml::Table::new();
    live_harness.insert("recovery".to_string(), recovery);
    manifest
        .as_table_mut()
        .unwrap()
        .insert("live_harness".to_string(), TomlValue::Table(live_harness));
    let steps = manifest
        .get_mut("steps")
        .and_then(TomlValue::as_array_mut)
        .expect("manifest steps");
    let jig = steps
        .iter_mut()
        .find(|step| step.get("action").and_then(TomlValue::as_str) == Some("jig.fake_llm"))
        .and_then(TomlValue::as_table_mut)
        .expect("Jig step");
    jig.insert(
        "late_stream_failure".to_string(),
        r#"role = "engineer"
bursts = [
  { after_requests = 2, failures = 1 },
  { after_requests = 5, failures = 14 },
]"#
        .parse::<TomlValue>()
        .unwrap(),
    );
    let additions = r#"
        [[steps]]
        id = "wait-provider-deferred"
        action = "provider.wait_deferred"
        artifact = "issue:intake"
        generation = 1
        timeout_ms = 45000

        [[steps]]
        id = "wake-provider"
        action = "provider.health_wake"
        artifact = "issue:intake"
        expected_generation = 1
        event_id = "fixture-provider-healthy-1"
    "#
    .parse::<TomlValue>()
    .unwrap()
    .get("steps")
    .and_then(TomlValue::as_array)
    .unwrap()
    .clone();
    let convergence = convergence_step(steps);
    steps.splice(convergence..convergence, additions);

    let plan = ManifestExecutionPlan::from_manifest(&manifest).expect("recovery plan resolves");
    let late = plan.steps.iter().find_map(|step| match &step.action {
        ManifestAction::StartJig {
            late_stream_failure,
            ..
        } => late_stream_failure.clone(),
        _ => None,
    });
    assert_eq!(
        late,
        Some(LateStreamFailureFixture {
            role: "engineer".to_string(),
            bursts: vec![
                LateStreamFailureBurst {
                    after_requests: 2,
                    failures: 1,
                },
                LateStreamFailureBurst {
                    after_requests: 5,
                    failures: 14,
                },
            ],
        })
    );
    assert!(matches!(
        plan.stimuli[0].kind,
        StimulusKind::WaitProviderDeferred { generation: 1, .. }
    ));
    assert!(matches!(
        plan.stimuli[1].kind,
        StimulusKind::ProviderHealthWake {
            expected_generation: 1,
            ..
        }
    ));
    let bundle =
        ScenarioBundle::from_manifest(path.parent().unwrap().to_path_buf(), path, manifest)
            .expect("recovery fixture resolves with the generic live bundle");
    let recovery = bundle.recovery.expect("bounded recovery fixture");
    assert_eq!(recovery.model_retry_max_attempts, 2);
    assert_eq!(recovery.model_retry_jitter_percent, 0);
    assert_eq!(recovery.provider_deferral_delay_secs, 300);
    assert_eq!(recovery.model_recovery_slo_secs, 7200);
}

#[test]
fn provider_health_wake_requires_a_matching_prior_deferral_observation() {
    let path = scenarios_root().join("basic-delivery/scenario.toml");
    let mut manifest =
        temper_scenario_core::load_resolved_manifest_toml(path).expect("resolved basic manifest");
    let steps = manifest
        .get_mut("steps")
        .and_then(TomlValue::as_array_mut)
        .expect("manifest steps");
    let convergence = convergence_step(steps);
    let wake = r#"
        id = "premature-wake"
        action = "provider.health_wake"
        artifact = "issue:intake"
        expected_generation = 1
        event_id = "fixture-provider-healthy-1"
    "#
    .parse::<TomlValue>()
    .unwrap();
    steps.insert(convergence, wake);

    let error = ManifestExecutionPlan::from_manifest(&manifest)
        .expect_err("wake without observation must fail closed");
    assert!(
        error.contains("requires an earlier provider.wait_deferred"),
        "{error}"
    );
}

fn convergence_step(steps: &[TomlValue]) -> usize {
    steps
        .iter()
        .position(|step| {
            step.get("action").and_then(TomlValue::as_str) == Some("workflow.wait_convergence")
        })
        .expect("convergence step")
}

fn scenarios_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("temper-testing lives under crates/temper-testing")
        .join("scenarios")
}
