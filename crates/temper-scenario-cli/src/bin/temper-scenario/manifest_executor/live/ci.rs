// SPDX-License-Identifier: MPL-2.0

use temper_testing::live_manifest::{
    CiJobEvidence as LiveCiJobEvidence, CiObservationEvidence as LiveCiObservationEvidence,
    LiveManifestEvidence, VerifiedFailureProofEvidence as LiveVerifiedFailureProofEvidence,
};

use crate::run_evidence;

pub(super) fn ci_observation(
    observation: &LiveCiObservationEvidence,
    pull_request_number: u64,
) -> run_evidence::CiObservationEvidence {
    run_evidence::CiObservationEvidence {
        matching_provider_run: Some(observation.matching_provider_run),
        jobs: observation
            .jobs
            .iter()
            .map(|job| ci_job(job, pull_request_number))
            .collect(),
    }
}

pub(super) fn ci_job(
    job: &LiveCiJobEvidence,
    pull_request_number: u64,
) -> run_evidence::CiJobEvidence {
    run_evidence::CiJobEvidence {
        job_id: Some(job.job_id.clone()),
        provider_run_id: job.provider_run_id.clone(),
        provider_attempt: job.provider_attempt.clone(),
        commit_sha: Some(job.commit_sha.clone()),
        name: job.name.clone(),
        status: job.status.clone(),
        pull_request_number: Some(pull_request_number),
        conclusion: job.conclusion.clone(),
        provider_conclusion: job.provider_conclusion.clone(),
        url: job.url.clone(),
        verified_failure: job.verified_failure.as_ref().map(verified_failure_proof),
    }
}

pub(crate) fn verified_failure_proof(
    proof: &LiveVerifiedFailureProofEvidence,
) -> run_evidence::VerifiedFailureProofEvidence {
    run_evidence::VerifiedFailureProofEvidence {
        schema_version: proof.schema_version,
        category: proof.category.clone(),
        repository_id: proof.repository_id.clone(),
        pull_request_id: proof.pull_request_id.clone(),
        commit_sha: proof.commit_sha.clone(),
        run_id: proof.run_id.clone(),
        job_id: proof.job_id.clone(),
        attempt: proof.attempt.clone(),
        task_id: proof.task_id.clone(),
        producer_id: proof.producer_id.clone(),
        issuer_id: proof.issuer_id.clone(),
        verification: proof.verification.clone(),
        created_at: proof.created_at.clone(),
        expires_at: proof.expires_at.clone(),
    }
}

pub(crate) fn ci_requests(evidence: &LiveManifestEvidence) -> Vec<run_evidence::CiRequestEvidence> {
    evidence
        .ci_requests
        .iter()
        .map(|request| run_evidence::CiRequestEvidence {
            method: request.method.clone(),
            path: request.path.clone(),
            query_keys: request.query_keys.clone(),
            authentication_present: request.authentication_present,
            authentication_scheme: request.authentication_scheme.clone(),
            accepts_json: request.accepts_json,
        })
        .collect()
}

pub(crate) fn actions_history(
    evidence: &LiveManifestEvidence,
) -> Option<run_evidence::ActionsHistoryEvidence> {
    evidence
        .actions_history
        .as_ref()
        .map(project_actions_history)
}

fn project_actions_history(
    history: &temper_testing::live_manifest::LiveActionsHistoryEvidence,
) -> run_evidence::ActionsHistoryEvidence {
    run_evidence::ActionsHistoryEvidence {
        seeded_run_count: history.seeded_run_count,
        payload_bytes_per_run: history.payload_bytes_per_run,
        transport_cap_bytes: history.transport_cap_bytes,
        full_inventory_lower_bound_bytes: history.full_inventory_lower_bound_bytes,
        largest_paged_response_bytes: history.largest_paged_response_bytes,
        pages_observed: history.pages_observed,
        target_run_page: history.target_run_page,
        later_page_selection: history.later_page_selection,
        provenance_drop_count: history.provenance_drop_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_history_projection_keeps_only_bounded_aggregate_facts() {
        let projected =
            project_actions_history(&temper_testing::live_manifest::LiveActionsHistoryEvidence {
                seeded_run_count: 201,
                payload_bytes_per_run: 90_000,
                transport_cap_bytes: 16 * 1024 * 1024,
                full_inventory_lower_bound_bytes: 18_000_000,
                largest_paged_response_bytes: 5_000_000,
                pages_observed: 5,
                target_run_page: 5,
                later_page_selection: true,
                provenance_drop_count: 0,
            });
        assert_eq!(projected.seeded_run_count, 201);
        assert_eq!(projected.target_run_page, 5);
        assert!(projected.later_page_selection);
        let serialized = serde_json::to_string(&projected).expect("projection serializes");
        for forbidden in [
            "event_payload",
            "Authorization",
            "secret-token",
            "provider-record",
        ] {
            assert!(!serialized.contains(forbidden));
        }
    }
}
