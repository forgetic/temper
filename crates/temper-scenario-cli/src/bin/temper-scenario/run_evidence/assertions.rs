// SPDX-License-Identifier: MPL-2.0

use std::path::Path;

use temper_scenario_core::load_resolved_manifest_toml;
use toml::Value;

use super::model::{AssertionEvidence, RunEvidenceArtifact};

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
  { method = "GET", route = "/api/v1/repos/{repo}/actions/runs", authentication_scheme = "token", accepts_json = true, query_keys = ["page", "limit"], all_matching = true },
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

    const CI_REPAIR_MANIFEST: &str = r#"
schema = "temper.scenario.v1"
name = "ci-repair-contract"
status = "active"
stability = "provisional"
intent = "Exercise exact-head CI repair assertions."

[runner]
uses = "manifest"

[expect]
[expect.ci_repair]
id = "repair-causality"
initial_head = "initial"
repaired_head = "repaired"
heads_differ = true
published_proofs = 1
stale_failure_absent_from_repaired = true
completed_before_poll_cadence = true

[[expect.ci_provenance]]
id = "initial-provenance"
pull_request = "implementation"
head = "initial"
matching_provider_run = true
materialized_jobs = true
job_count = 1
provider_run_count = 1
stable_identities = true
exact_head = true
job_outcomes = [
  { name = "ordinary-source-check", status = "completed", conclusion = "failure", provider_conclusion = "failure", exactly = 1 },
]

[[expect.verified_failure_proof]]
id = "initial-proof"
pull_request = "implementation"
head = "initial"
job_name = "ordinary-source-check"
exactly = 1
category = "source"
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
    fn exact_head_repair_assertions_pass_retained_history_and_fail_closed() {
        let artifact = deserialize(repair_artifact_json());
        let assertions =
            evaluate(CI_REPAIR_MANIFEST, artifact).expect("repair assertions evaluate");
        assert_eq!(assertions.results.len(), 3);
        assert!(
            assertions
                .results
                .iter()
                .all(|result| result.status == super::super::model::ASSERTION_STATUS_PASSED)
        );

        let mut stale = repair_artifact_json();
        stale["final_state"]["ci"]["failure_evidence"]["published_proofs"] = json!(2);
        let initial_proof =
            stale["final_state"]["ci"]["heads"][0]["jobs"][0]["verified_failure"].clone();
        stale["final_state"]["ci"]["heads"][1]["jobs"][0]["verified_failure"] = initial_proof;
        let assertions =
            evaluate(CI_REPAIR_MANIFEST, deserialize(stale)).expect("stale evidence evaluates");
        let repair = assertions
            .results
            .iter()
            .find(|result| result.id == "repair-causality")
            .expect("repair result");
        assert_eq!(repair.status, super::super::model::ASSERTION_STATUS_FAILED);

        let mut missing = repair_artifact_json();
        missing["final_state"]["ci"]["heads"] = json!([]);
        let assertions =
            evaluate(CI_REPAIR_MANIFEST, deserialize(missing)).expect("missing heads evaluate");
        assert!(
            assertions.results.iter().all(|result| {
                result.status == super::super::model::ASSERTION_STATUS_MISSING_FACT
            })
        );
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
    fn universal_request_rule_rejects_one_unpaged_read_and_fails_closed() {
        let mut unpaged = artifact_json();
        let mut bad = unpaged["final_state"]["ci"]["requests"][0].clone();
        bad["query_keys"] = json!(["limit"]);
        unpaged["final_state"]["ci"]["requests"]
            .as_array_mut()
            .expect("request array")
            .push(bad);
        let assertions = evaluate(MANIFEST, deserialize(unpaged)).expect("unpaged evaluates");
        assert_eq!(
            assertions.results[0].status,
            super::super::model::ASSERTION_STATUS_FAILED
        );
        assert!(
            assertions.results[0]
                .details
                .iter()
                .any(|detail| detail.contains("rejected 1 of 2"))
        );

        let mut missing = artifact_json();
        missing["final_state"]["ci"]["requests"][0]["path"] =
            json!("/api/v1/repos/acme/service/issues");
        let assertions = evaluate(MANIFEST, deserialize(missing)).expect("missing evaluates");
        assert_eq!(
            assertions.results[0].status,
            super::super::model::ASSERTION_STATUS_MISSING_FACT
        );

        let mut dropped = artifact_json();
        dropped["final_state"]["ci"]["request_capture_dropped"] = json!(1);
        let assertions = evaluate(MANIFEST, deserialize(dropped)).expect("dropped evaluates");
        assert_eq!(
            assertions.results[0].status,
            super::super::model::ASSERTION_STATUS_FAILED
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

    fn repair_artifact_json() -> JsonValue {
        let mut value = proof_artifact_json();
        value["effective_configuration"] = json!({
            "ci_poll_cadence_secs": 1,
            "poll_cadence_secs": 600,
            "mechanical_cadence_secs": 600
        });
        value["convergence"] = json!({
            "total_elapsed_ms": 5000
        });
        value["final_state"]["pull_requests"][0]["head_sha"] = json!("repaired-head");

        let mut initial_job = value["final_state"]["ci"]["jobs"][1].clone();
        initial_job["name"] = json!("ordinary-source-check");
        initial_job["commit_sha"] = json!("initial-head");
        initial_job["conclusion"] = json!("Failure");
        initial_job["verified_failure"]["category"] = json!("source");
        initial_job["verified_failure"]["commit_sha"] = json!("initial-head");

        let mut repaired_job = value["final_state"]["ci"]["jobs"][0].clone();
        repaired_job["name"] = json!("ordinary-source-check");
        repaired_job["commit_sha"] = json!("repaired-head");
        repaired_job["verified_failure"] = JsonValue::Null;

        value["final_state"]["ci"]["completed_jobs"] = json!(1);
        value["final_state"]["ci"]["jobs"] = json!([repaired_job.clone()]);
        value["final_state"]["ci"]["observations"] = json!([
            { "matching_provider_run": true, "jobs": [repaired_job.clone()] },
            { "matching_provider_run": true, "jobs": [repaired_job.clone()] }
        ]);
        value["final_state"]["ci"]["heads"] = json!([
            {
                "phase": "initial",
                "head_sha": "initial-head",
                "observed_after_ms": 2000,
                "jobs": [initial_job.clone()],
                "observations": [
                    { "matching_provider_run": true, "jobs": [initial_job.clone()] },
                    { "matching_provider_run": true, "jobs": [initial_job] }
                ]
            },
            {
                "phase": "repaired",
                "head_sha": "repaired-head",
                "observed_after_ms": 4500,
                "jobs": [repaired_job.clone()],
                "observations": [
                    { "matching_provider_run": true, "jobs": [repaired_job.clone()] },
                    { "matching_provider_run": true, "jobs": [repaired_job] }
                ]
            }
        ]);
        value["final_state"]["ci"]["failure_evidence"] = json!({
            "endpoint_path": "/v1/forgejo-failures",
            "issuer": "temper-proof-issuer",
            "protected_producers": ["forgejo-actions"],
            "published_proofs": 1
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
                            "query_keys": ["page", "limit"],
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
