// SPDX-License-Identifier: MPL-2.0

use std::path::{Path, PathBuf};
use std::time::Duration;

use toml::Value as TomlValue;

use super::{ManifestExecutionPlan, StimulusKind};

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
