// SPDX-License-Identifier: MPL-2.0

use std::path::Path;

use temper_scenario_core::load_resolved_manifest_toml;
use toml::Value;

use super::model::{AssertionEvidence, RunEvidenceArtifact};

#[path = "assertions/actions_history.rs"]
mod actions_history;
#[path = "assertions/checks.rs"]
mod checks;
#[path = "assertions/ci_provenance.rs"]
mod ci_provenance;
#[path = "assertions/ci_repair.rs"]
mod ci_repair;
#[path = "assertions/common.rs"]
mod common;
#[path = "assertions/effective_configuration.rs"]
mod effective_configuration;
#[path = "assertions/events.rs"]
mod events;
#[path = "assertions/issue.rs"]
mod issue;
#[path = "assertions/pull_request.rs"]
mod pull_request;
#[path = "assertions/recovery.rs"]
mod recovery;
#[path = "assertions/repository.rs"]
mod repository;
#[path = "assertions/summary.rs"]
mod summary;
#[path = "assertions/support.rs"]
mod support;
#[path = "assertions/verified_failure.rs"]
mod verified_failure;

pub(crate) fn evaluate_manifest_assertions(
    manifest_path: &Path,
    artifact: &RunEvidenceArtifact,
) -> Result<Option<AssertionEvidence>, String> {
    let manifest = load_resolved_manifest_toml(manifest_path).map_err(|error| error.to_string())?;
    let Some(expect) = manifest.get("expect").and_then(Value::as_table) else {
        return Ok(None);
    };

    let mut results = Vec::new();
    summary::evaluate_templates(expect, artifact, &mut results);
    summary::evaluate_counts(expect, artifact, &mut results);
    actions_history::evaluate(expect, artifact, &mut results);
    checks::evaluate_checks(expect, artifact, &mut results);
    ci_provenance::evaluate(expect, artifact, &mut results);
    ci_repair::evaluate(expect, artifact, &mut results);
    effective_configuration::evaluate(expect, artifact, &mut results);
    verified_failure::evaluate(expect, artifact, &mut results);
    events::evaluate_event_expectations(expect, artifact, &mut results);
    recovery::evaluate(expect, artifact, &mut results);

    if results.is_empty() {
        Ok(None)
    } else {
        Ok(Some(AssertionEvidence::from_results(results)))
    }
}

pub(crate) fn print_assertions(assertions: &AssertionEvidence) {
    println!("assertions: {}", assertions.summary());
    for result in &assertions.results {
        println!("  [{}] {}", result.status, result.id);
        if let Some(artifact) = result.artifact.as_deref() {
            println!("    artifact: {artifact}");
        }
        println!("    {}", result.description);
        for detail in &result.details {
            println!("    - {detail}");
        }
    }
}

#[cfg(test)]
#[path = "assertions/tests.rs"]
mod tests;
