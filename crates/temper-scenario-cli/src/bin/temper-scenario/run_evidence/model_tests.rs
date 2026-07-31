// SPDX-License-Identifier: MPL-2.0

use super::*;

fn result(id: &str, status: &str, required: bool) -> AssertionResultEvidence {
    AssertionResultEvidence {
        id: id.to_string(),
        required,
        status: status.to_string(),
        description: id.to_string(),
        artifact: None,
        kind: None,
        phase: None,
        command: None,
        cwd: None,
        context_path: None,
        stdout_path: None,
        stderr_path: None,
        status_path: None,
        exit_status: None,
        timeout_ms: None,
        duration_ms: None,
        details: Vec::new(),
    }
}

#[test]
fn unsupported_optional_assertion_remains_visible_without_blocking() {
    let assertions = AssertionEvidence::from_results(vec![
        result("required", ASSERTION_STATUS_PASSED, true),
        result("optional", ASSERTION_STATUS_UNSUPPORTED, false),
    ]);

    assert_eq!(assertions.status, ASSERTION_STATUS_PASSED);
    assert_eq!(assertions.unsupported, 1);
    assert_eq!(assertions.blocked_required, 0);
    assert!(!assertions.has_failures());
}

#[test]
fn every_nonpassing_required_outcome_blocks_success() {
    for (status, verdict) in [
        (ASSERTION_STATUS_FAILED, RunEvidenceVerdict::Failed),
        (ASSERTION_STATUS_TIMED_OUT, RunEvidenceVerdict::Failed),
        (
            ASSERTION_STATUS_MISSING_FACT,
            RunEvidenceVerdict::Inconclusive,
        ),
        (
            ASSERTION_STATUS_UNSUPPORTED,
            RunEvidenceVerdict::Inconclusive,
        ),
    ] {
        let assertions = AssertionEvidence::from_results(vec![result("proof", status, true)]);
        assert!(assertions.has_failures(), "{status}");
        assert_eq!(assertions.verdict(), verdict, "{status}");
    }
}

#[test]
fn legacy_ci_evidence_defaults_new_provenance_fields() {
    let state: CiStateEvidence = serde_json::from_value(serde_json::json!({
        "completed_jobs": 1,
        "jobs": [{
            "name": "test",
            "status": "Completed",
            "conclusion": "Success"
        }]
    }))
    .unwrap();

    assert_eq!(state.jobs[0].job_id, None);
    assert_eq!(state.jobs[0].provider_run_id, None);
    assert!(state.observations.is_empty());
    assert!(state.requests.is_empty());
    assert_eq!(state.request_capture_dropped, None);
}

#[test]
fn serialized_request_provenance_contains_no_header_or_query_values() {
    let state = CiStateEvidence {
        completed_jobs: None,
        jobs: Vec::new(),
        observations: Vec::new(),
        requests: vec![CiRequestEvidence {
            method: "GET".to_string(),
            path: "/api/v1/repos/acme/service/actions/runs".to_string(),
            query_keys: vec!["limit".to_string()],
            authentication_present: true,
            authentication_scheme: Some("token".to_string()),
            accepts_json: true,
        }],
        request_capture_dropped: Some(0),
    };

    let serialized = serde_json::to_string(&state).unwrap();
    assert!(serialized.contains("authentication_scheme"));
    assert!(serialized.contains("query_keys"));
    assert!(!serialized.contains("Authorization"));
    assert!(!serialized.contains("secret-token"));
    assert!(!serialized.contains("application/json"));
}
