// SPDX-License-Identifier: MPL-2.0

use std::path::Path;

use temper_scenario_core::load_resolved_manifest_toml;
use toml::Value;

use super::model::{AssertionEvidence, RunEvidenceArtifact};

#[path = "assertions/checks.rs"]
mod checks;
#[path = "assertions/ci_provenance.rs"]
mod ci_provenance;
#[path = "assertions/common.rs"]
mod common;
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
    checks::evaluate_checks(expect, artifact, &mut results);
    ci_provenance::evaluate(expect, artifact, &mut results);
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
mod tests {
    use serde_json::{Value as JsonValue, json};

    use super::*;

    const MANIFEST: &str = r#"
schema = "temper.scenario.v1"
name = "ci-provenance-contract"
status = "active"
stability = "provisional"
intent = "Exercise structured CI provenance assertions."

[runner]
uses = "manifest"

[expect]
[[expect.ci_provenance]]
id = "api-ci-provenance"
required = true
pull_request = "implementation"
matching_provider_run = true
materialized_jobs = true
job_count = 2
provider_run_count = 1
stable_identities = true
exact_head = true
job_outcomes = [
  { status = "completed", conclusion = "success", exactly = 1 },
  { status = "completed", conclusion = "unknown", provider_conclusion = "failure", exactly = 1 },
]
required_requests = [
  { method = "GET", route = "/api/v1/repos/{repo}/actions/runs", authentication_scheme = "token", accepts_json = true, query_keys = ["limit"] },
  { method = "GET", route = "/api/v1/repos/{repo}/actions/runs/{provider_run_id}/jobs", authentication_scheme = "token", accepts_json = true },
]
forbidden_requests = [
  { route_contains = "/actions/tasks" },
  { route_contains = "/user/login" },
  { method = "GET", route = "/api/v1/repos/{repo}/actions" },
  { method = "POST", route_contains = "/actions" },
]
"#;

    #[test]
    fn manifest_ci_provenance_assertion_passes_supported_evidence() {
        let assertions = evaluate(MANIFEST, artifact()).expect("assertions evaluate");
        assert_eq!(
            assertions.status,
            super::super::model::ASSERTION_STATUS_PASSED
        );
        assert_eq!(assertions.results.len(), 1);
        assert_eq!(
            assertions.results[0].status,
            super::super::model::ASSERTION_STATUS_PASSED
        );
    }

    #[test]
    fn manifest_ci_provenance_assertion_fails_closed_for_missing_and_mismatched_facts() {
        let mut missing = artifact_json();
        missing["final_state"]["ci"]["observations"] = json!([]);
        let assertions = evaluate(MANIFEST, deserialize(missing)).expect("missing evaluates");
        assert_eq!(
            assertions.results[0].status,
            super::super::model::ASSERTION_STATUS_MISSING_FACT
        );

        let mut mismatched = artifact_json();
        mismatched["final_state"]["ci"]["jobs"][0]["commit_sha"] = json!("stale-head");
        let assertions = evaluate(MANIFEST, deserialize(mismatched)).expect("mismatch evaluates");
        assert_eq!(
            assertions.results[0].status,
            super::super::model::ASSERTION_STATUS_FAILED
        );
    }

    fn evaluate(
        manifest: &str,
        artifact: RunEvidenceArtifact,
    ) -> Result<AssertionEvidence, String> {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("scenario.toml");
        std::fs::write(&path, manifest).expect("write manifest");
        evaluate_manifest_assertions(&path, &artifact)?.ok_or_else(|| "no assertions".to_string())
    }

    fn artifact() -> RunEvidenceArtifact {
        deserialize(artifact_json())
    }

    fn deserialize(value: JsonValue) -> RunEvidenceArtifact {
        serde_json::from_value(value).expect("artifact deserializes")
    }

    fn artifact_json() -> JsonValue {
        let jobs = json!([
            {
                "job_id": "forgejo:acme/service:actions:900:31:1:41",
                "provider_run_id": "900",
                "provider_attempt": "1",
                "commit_sha": "exact-head",
                "name": "successful_job",
                "status": "Completed",
                "pull_request_number": 7,
                "conclusion": "Success",
                "provider_conclusion": "success",
                "url": "https://forge.example/acme/service/actions/runs/900"
            },
            {
                "job_id": "forgejo:acme/service:actions:900:32:1:42",
                "provider_run_id": "900",
                "provider_attempt": "1",
                "commit_sha": "exact-head",
                "name": "status_only_failure",
                "status": "Completed",
                "pull_request_number": 7,
                "conclusion": "Unknown",
                "provider_conclusion": "failure"
            }
        ]);
        json!({
            "schema": "temper.scenario.run-evidence",
            "version": 2,
            "scenario": {
                "name": "ci-provenance-contract",
                "source": "checked_in",
                "source_description": "checked-in scenario",
                "scenario_path": "scenarios/ci-provenance-contract",
                "manifest_path": "scenarios/ci-provenance-contract/scenario.toml",
                "runner_id": "manifest",
                "runner_selector": "runner.uses",
                "runner_selection": "manifest",
                "tier": "live",
                "tier_description": "live",
                "topology": {}
            },
            "final_state": {
                "pull_requests": [{
                    "number": 7,
                    "id": "implementation",
                    "state": "merged",
                    "head_sha": "exact-head"
                }],
                "ci": {
                    "completed_jobs": 2,
                    "jobs": jobs.clone(),
                    "observations": [
                        { "matching_provider_run": true, "jobs": jobs.clone() },
                        { "matching_provider_run": true, "jobs": jobs }
                    ],
                    "requests": [
                        {
                            "method": "GET",
                            "path": "/api/v1/repos/acme/service/actions/runs",
                            "query_keys": ["limit"],
                            "authentication_present": true,
                            "authentication_scheme": "token",
                            "accepts_json": true
                        },
                        {
                            "method": "GET",
                            "path": "/api/v1/repos/acme/service/actions/runs/900/jobs",
                            "authentication_present": true,
                            "authentication_scheme": "token",
                            "accepts_json": true
                        }
                    ],
                    "request_capture_dropped": 0
                }
            },
            "provider": { "repo_slug": "acme/service" }
        })
    }
}
