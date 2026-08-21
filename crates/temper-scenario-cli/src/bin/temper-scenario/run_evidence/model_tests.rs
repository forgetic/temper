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
    assert_eq!(state.jobs[0].verified_failure, None);
    assert!(state.observations.is_empty());
    assert!(state.requests.is_empty());
    assert_eq!(state.request_capture_dropped, None);
    assert_eq!(state.actions_history, None);
}

#[test]
fn effective_configuration_and_verified_proof_round_trip_without_secrets() {
    let configuration = EffectiveConfigurationEvidence {
        ci_poll_cadence_secs: 1,
        poll_cadence_secs: 600,
        mechanical_cadence_secs: 7,
    };
    let proof = VerifiedFailureProofEvidence {
        schema_version: 1,
        category: "test".to_string(),
        repository_id: "forgejo:acme/service".to_string(),
        pull_request_id: Some("forgejo:acme/service:pull:7".to_string()),
        commit_sha: "0123456789abcdef0123456789abcdef01234567".to_string(),
        run_id: "591".to_string(),
        job_id: "42".to_string(),
        attempt: "2".to_string(),
        task_id: Some("9001".to_string()),
        producer_id: "forgejo-actions".to_string(),
        issuer_id: "temper-proof-issuer".to_string(),
        verification: "protected_producer".to_string(),
        created_at: "2026-07-26T12:00:00+00:00".to_string(),
        expires_at: "2026-07-26T12:05:00+00:00".to_string(),
    };

    let configuration_json = serde_json::to_string(&configuration).unwrap();
    let proof_json = serde_json::to_string(&proof).unwrap();
    assert_eq!(
        serde_json::from_str::<EffectiveConfigurationEvidence>(&configuration_json).unwrap(),
        configuration
    );
    assert_eq!(
        serde_json::from_str::<VerifiedFailureProofEvidence>(&proof_json).unwrap(),
        proof
    );
    for forbidden in ["signature", "credential", "secret", "token"] {
        assert!(!proof_json.contains(forbidden), "{forbidden}: {proof_json}");
    }
}

#[test]
fn serialized_request_provenance_contains_no_header_or_query_values() {
    let state = CiStateEvidence {
        completed_jobs: None,
        jobs: Vec::new(),
        observations: Vec::new(),
        heads: Vec::new(),
        failure_evidence: None,
        requests: vec![CiRequestEvidence {
            method: "GET".to_string(),
            path: "/api/v1/repos/acme/service/actions/runs".to_string(),
            query_keys: vec!["page".to_string(), "limit".to_string()],
            authentication_present: true,
            authentication_scheme: Some("token".to_string()),
            accepts_json: true,
        }],
        request_capture_dropped: Some(0),
        actions_history: Some(ActionsHistoryEvidence {
            seeded_run_count: 201,
            payload_bytes_per_run: 90_000,
            transport_cap_bytes: 16 * 1024 * 1024,
            full_inventory_lower_bound_bytes: 18_000_000,
            largest_paged_response_bytes: 5_000_000,
            pages_observed: 5,
            target_run_page: 5,
            later_page_selection: true,
            webhooks_disabled: true,
            provenance_drop_count: 0,
        }),
    };

    let serialized = serde_json::to_string(&state).unwrap();
    assert!(serialized.contains("authentication_scheme"));
    assert!(serialized.contains("query_keys"));
    assert!(!serialized.contains("Authorization"));
    assert!(!serialized.contains("secret-token"));
    assert!(!serialized.contains("application/json"));
    assert!(!serialized.contains("event_payload"));
    assert!(!serialized.contains("provider-record"));
    assert!(serialized.contains("full_inventory_lower_bound_bytes"));
}
