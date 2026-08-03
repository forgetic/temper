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
    checks::evaluate_checks(expect, artifact, &mut results);
    ci_provenance::evaluate(expect, artifact, &mut results);
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

    const CADENCE_PROOF_MANIFEST: &str = r#"
schema = "temper.scenario.v1"
name = "ci-cadence-proof-contract"
status = "active"
stability = "provisional"
intent = "Exercise effective cadence and verified-failure assertions."

[runner]
uses = "manifest"

[expect]
[expect.effective_configuration]
id = "effective-cadences"
ci_poll_cadence_secs = 1
poll_cadence_secs = 600
mechanical_cadence_secs = 7

[[expect.verified_failure_proof]]
id = "ordinary-failure-proof"
pull_request = "implementation"
job_name = "status_only_failure"
exactly = 1
category = "test"
producer_id = "forgejo-actions"
issuer_id = "temper-proof-issuer"
verification = "protected_producer"
"#;

    #[test]
    fn effective_cadence_and_verified_failure_assertions_pass_exact_evidence() {
        let artifact = proof_artifact();
        let serialized = serde_json::to_string(&artifact).expect("artifact serializes");
        let round_trip: RunEvidenceArtifact =
            serde_json::from_str(&serialized).expect("artifact round trips");
        assert_eq!(round_trip, artifact);
        let assertions = evaluate(CADENCE_PROOF_MANIFEST, round_trip).expect("assertions evaluate");
        assert_eq!(
            assertions.status,
            super::super::model::ASSERTION_STATUS_PASSED
        );
        assert_eq!(assertions.results.len(), 2);
        assert!(
            assertions
                .results
                .iter()
                .all(|result| result.status == super::super::model::ASSERTION_STATUS_PASSED)
        );
    }

    #[test]
    fn cadence_and_proof_assertions_fail_mismatches_and_block_missing_facts() {
        let mut mismatched = proof_artifact_json();
        mismatched["effective_configuration"]["ci_poll_cadence_secs"] = json!(2);
        mismatched["final_state"]["ci"]["jobs"][1]["verified_failure"]["attempt"] = json!("9");
        let assertions =
            evaluate(CADENCE_PROOF_MANIFEST, deserialize(mismatched)).expect("mismatches evaluate");
        assert!(
            assertions
                .results
                .iter()
                .all(|result| result.status == super::super::model::ASSERTION_STATUS_FAILED)
        );

        let mut missing = proof_artifact_json();
        missing["effective_configuration"] = JsonValue::Null;
        missing["final_state"]["ci"]["jobs"][1]["verified_failure"]["pull_request_id"] =
            JsonValue::Null;
        let assertions =
            evaluate(CADENCE_PROOF_MANIFEST, deserialize(missing)).expect("missing facts evaluate");
        assert!(
            assertions
                .results
                .iter()
                .all(|result| result.status == super::super::model::ASSERTION_STATUS_MISSING_FACT)
        );
        assert_eq!(assertions.blocked_required, 2);
    }

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

    fn proof_artifact() -> RunEvidenceArtifact {
        deserialize(proof_artifact_json())
    }

    fn proof_artifact_json() -> JsonValue {
        let mut value = artifact_json();
        value["effective_configuration"] = json!({
            "ci_poll_cadence_secs": 1,
            "poll_cadence_secs": 600,
            "mechanical_cadence_secs": 7
        });
        value["final_state"]["ci"]["jobs"][1]["verified_failure"] = json!({
            "schema_version": 1,
            "category": "test",
            "repository_id": "forgejo:acme/service",
            "pull_request_id": "forgejo:acme/service:pull:7",
            "commit_sha": "exact-head",
            "run_id": "900",
            "job_id": "32",
            "attempt": "1",
            "task_id": "42",
            "producer_id": "forgejo-actions",
            "issuer_id": "temper-proof-issuer",
            "verification": "protected_producer",
            "created_at": "2026-07-26T12:00:00+00:00",
            "expires_at": "2026-07-26T12:05:00+00:00"
        });
        value
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
