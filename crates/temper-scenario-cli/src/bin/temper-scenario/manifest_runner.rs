// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeSet;
use std::path::Path;

use temper_scenario_core::load_resolved_manifest_toml;
use toml::Value;

use super::basic_delivery;
use super::run_context::ScenarioRunFacts;
use super::run_evidence;

// Implementation detail: the manifest runner currently delegates live execution
// to the historical basic-delivery harness adapter. The public registry still
// exposes only RUNNER_ID (`manifest`); `basic-delivery` is not a runner alias.

pub(super) const RUNNER_ID: &str = "manifest";

const REQUIRED_STACK_SUMMARY: &str =
    "real Forgejo + real forgejo-runner CI + real Temper standalone + Jig fake LLM";

const REQUIRED_ACTIONS: &[&str] = &[
    "forgejo.provision",
    "forgejo_runner.ready",
    "repo.seed",
    "issue.seed",
    "jig.fake_llm",
    "temper.launch_standalone",
    "workflow.wait_convergence",
];

pub(super) fn run_live_and_print(
    scenario_path: &Path,
    manifest_path: &Path,
    facts: &ScenarioRunFacts,
    temper_bin: Option<&Path>,
    context: &run_evidence::RunEvidenceContext,
) -> Result<run_evidence::RunEvidenceArtifact, String> {
    validate_manifest_plan(manifest_path)?;
    basic_delivery::run_live_and_print(scenario_path, manifest_path, facts, temper_bin, context)
}

pub(super) fn run_live_evidence_lines_for_report(
    scenario_path: &Path,
    manifest_path: &Path,
    temper_bin: Option<&Path>,
    artifact_dir: &Path,
) -> Result<Vec<String>, String> {
    validate_manifest_plan(manifest_path)?;
    basic_delivery::run_live_evidence_lines_for_report(
        scenario_path,
        manifest_path,
        temper_bin,
        artifact_dir,
    )
}

fn validate_manifest_plan(manifest_path: &Path) -> Result<(), String> {
    let manifest = load_resolved_manifest_toml(manifest_path).map_err(|error| error.to_string())?;
    validate_live_stack(&manifest)?;
    validate_required_steps(&manifest)?;
    validate_observability(&manifest)?;
    Ok(())
}

fn validate_live_stack(manifest: &Value) -> Result<(), String> {
    let topology = manifest
        .get("topology")
        .and_then(Value::as_table)
        .ok_or_else(|| {
            format!(
                "manifest runner requires [topology] declaring the validation-grade stack: {REQUIRED_STACK_SUMMARY}"
            )
        })?;
    for (field, expected) in [
        ("forge", "forgejo"),
        ("runner", "forgejo-actions-host"),
        ("temper", "standalone"),
        ("agent_model", "scripted-fake-llm"),
    ] {
        let actual = topology.get(field).and_then(Value::as_str);
        if actual != Some(expected) {
            return Err(format!(
                "manifest runner supports only the validation-grade live stack ({REQUIRED_STACK_SUMMARY}); topology.{field} must be `{expected}`, got `{}`",
                actual.unwrap_or("<missing>")
            ));
        }
    }
    Ok(())
}

fn validate_required_steps(manifest: &Value) -> Result<(), String> {
    let steps = manifest
        .get("steps")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            "manifest runner requires declarative [[steps]] for live setup; no hard-coded or legacy scenario-name fallback plan will be substituted".to_string()
        })?;
    let actions = steps
        .iter()
        .filter_map(Value::as_table)
        .filter_map(|step| step.get("action").and_then(Value::as_str))
        .collect::<BTreeSet<_>>();
    let missing = REQUIRED_ACTIONS
        .iter()
        .copied()
        .filter(|action| !actions.contains(action))
        .collect::<Vec<_>>();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "manifest runner plan is missing required live setup step action(s): {}; required stack is {REQUIRED_STACK_SUMMARY}",
            missing.join(", ")
        ))
    }
}

fn validate_observability(manifest: &Value) -> Result<(), String> {
    let Some(observability) = manifest.get("observability").and_then(Value::as_table) else {
        return Ok(());
    };
    if let Some(format) = observability.get("log_format").and_then(Value::as_str) {
        if !format.trim().eq_ignore_ascii_case("json") {
            return Err(format!(
                "manifest runner captures structured Temper events and requires observability.log_format = `json`, got `{format}`"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn manifest_plan_requires_real_live_stack() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("scenario.toml");
        fs::write(
            &path,
            "name = \"bad\"\n\
             status = \"ready\"\n\
             stability = \"experimental\"\n\
             intent = \"bad stack\"\n\
             [runner]\n\
             uses = \"manifest\"\n\
             [topology]\n\
             forge = \"memory\"\n\
             runner = \"fake\"\n\
             temper = \"in-process\"\n\
             agent_model = \"scripted-fake-llm\"\n\
             [[steps]]\n\
             action = \"forgejo.provision\"\n",
        )
        .expect("write manifest");

        let error = validate_manifest_plan(&path).expect_err("bad topology rejected");

        assert!(error.contains("validation-grade live stack"), "{error}");
        assert!(error.contains("real Forgejo"), "{error}");
    }
}
