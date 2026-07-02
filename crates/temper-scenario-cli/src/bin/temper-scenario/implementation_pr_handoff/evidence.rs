// SPDX-License-Identifier: MPL-2.0

use temper_forge_model::PullRequestState;

use crate::run_evidence;

pub(super) fn outcome_artifact(
    outcome: &super::RunOutcome,
    context: &run_evidence::RunEvidenceContext,
) -> run_evidence::RunEvidenceArtifact {
    let mut artifact = context.artifact(run_evidence::FinalStateEvidence {
        issues: vec![
            run_evidence::IssueStateEvidence {
                number: outcome.create.issue_number.get(),
                id: Some("source:create".to_string()),
                title: None,
                state: Some("open".to_string()),
                labels: vec!["code".to_string(), "ready".to_string()],
            },
            run_evidence::IssueStateEvidence {
                number: outcome.refresh.issue_number.get(),
                id: Some("source:refresh".to_string()),
                title: None,
                state: Some("open".to_string()),
                labels: vec!["code".to_string(), "ready".to_string()],
            },
        ],
        pull_requests: vec![
            run_evidence::PullRequestStateEvidence {
                number: outcome.create.pr_number.get(),
                id: Some("create".to_string()),
                title: Some(outcome.create.title.clone()),
                state: Some(outcome.create.pr_state.clone()),
                labels: outcome.create.labels.clone(),
                head_branch: Some(outcome.create.head_branch.clone()),
                head_sha: outcome.create.head_sha.clone(),
                merged_sha: None,
            },
            run_evidence::PullRequestStateEvidence {
                number: outcome.refresh.pr_number.get(),
                id: Some("refresh".to_string()),
                title: Some(outcome.refresh.title.clone()),
                state: Some(outcome.refresh.pr_state.clone()),
                labels: outcome.refresh.labels.clone(),
                head_branch: Some(outcome.refresh.head_branch.clone()),
                head_sha: outcome.refresh.head_sha.clone(),
                merged_sha: None,
            },
        ],
        ci: run_evidence::CiStateEvidence::default(),
    });
    artifact.evidence_lines = super::outcome_evidence_lines(outcome);
    artifact
}

pub(super) fn pr_state_value(state: PullRequestState) -> &'static str {
    match state {
        PullRequestState::Open => "open",
        PullRequestState::Closed => "closed",
        PullRequestState::Merged => "merged",
    }
}
