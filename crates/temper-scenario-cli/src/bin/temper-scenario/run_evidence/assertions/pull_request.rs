// SPDX-License-Identifier: MPL-2.0

use super::super::model::{
    AssertionResultEvidence, CiJobEvidence, PullRequestStateEvidence, RunEvidenceArtifact,
};
use super::common::{
    evaluate_labels_cleared, evaluate_labels_present, evaluate_state,
    reject_repo_fields_for_non_repo,
};
use super::support::{
    ResultBuilder, SelectedPullRequest, SelectionProblem, ci_conclusion_failed,
    ci_conclusion_passed, select_pull_request,
};

pub(super) fn evaluate_pull_request_check(
    check: &toml::Table,
    artifact: &RunEvidenceArtifact,
    id: Option<&str>,
    mut builder: ResultBuilder,
) -> AssertionResultEvidence {
    let selected = select_pull_request(&artifact.final_state.pull_requests, id);
    let SelectedPullRequest { pull_request, note } = match selected {
        Ok(selected) => selected,
        Err(SelectionProblem::Failed(message)) => return builder.failed(message).build(),
        Err(SelectionProblem::Unsupported(message)) => return builder.unsupported(message).build(),
    };
    if let Some(note) = note {
        builder = builder.passed(note);
    }

    if let Some(value) = check.get("state") {
        if let Some(expected) = value.as_str() {
            builder = evaluate_state(
                builder,
                "pull request",
                id,
                expected,
                pull_request.state.as_deref(),
                &format!("#{}", pull_request.number),
            );
        } else {
            builder = builder.failed("state must be a string");
        }
    }
    if let Some(value) = check.get("labels") {
        builder = evaluate_labels_present(builder, value, &pull_request.labels);
    }
    if let Some(value) = check.get("labels_cleared") {
        builder = evaluate_labels_cleared(builder, value, &pull_request.labels);
    }
    if let Some(value) = check.get("ci") {
        if let Some(expected) = value.as_str() {
            builder = evaluate_ci(builder, expected, pull_request, artifact);
        } else {
            builder = builder.failed("ci must be a string (`passed` or `failed`)");
        }
    }
    builder = reject_repo_fields_for_non_repo(builder, check);

    builder.build()
}

fn evaluate_ci(
    builder: ResultBuilder,
    expected: &str,
    pull_request: &PullRequestStateEvidence,
    artifact: &RunEvidenceArtifact,
) -> ResultBuilder {
    let jobs = ci_jobs_for_pull_request(artifact, pull_request);
    if jobs.is_empty() {
        return builder.unsupported(format!(
            "run evidence has no CI job conclusion facts for pull request #{}",
            pull_request.number
        ));
    }

    let conclusions = jobs
        .iter()
        .filter_map(|job| job.conclusion.as_deref())
        .collect::<Vec<_>>();
    if conclusions.is_empty() {
        return builder.unsupported(format!(
            "CI jobs for pull request #{} do not include conclusion facts",
            pull_request.number
        ));
    }

    let passed = conclusions
        .iter()
        .any(|conclusion| ci_conclusion_passed(conclusion));
    let failed = conclusions
        .iter()
        .any(|conclusion| ci_conclusion_failed(conclusion));
    match expected.trim().to_ascii_lowercase().as_str() {
        "passed" | "pass" | "success" | "successful" => {
            if passed && !failed {
                builder.passed(format!(
                    "CI passed for pull request #{} with conclusions {:?}",
                    pull_request.number, conclusions
                ))
            } else {
                builder.failed(format!(
                    "expected CI passed for pull request #{}, observed conclusions {:?}",
                    pull_request.number, conclusions
                ))
            }
        }
        "failed" | "fail" | "failure" => {
            if failed {
                builder.passed(format!(
                    "CI failed for pull request #{} with conclusions {:?}",
                    pull_request.number, conclusions
                ))
            } else {
                builder.failed(format!(
                    "expected CI failed for pull request #{}, observed conclusions {:?}",
                    pull_request.number, conclusions
                ))
            }
        }
        other => builder.failed(format!(
            "unsupported ci expectation `{other}` (expected `passed` or `failed`)"
        )),
    }
}

fn ci_jobs_for_pull_request<'a>(
    artifact: &'a RunEvidenceArtifact,
    pull_request: &PullRequestStateEvidence,
) -> Vec<&'a CiJobEvidence> {
    let matching = artifact
        .final_state
        .ci
        .jobs
        .iter()
        .filter(|job| job.pull_request_number == Some(pull_request.number))
        .collect::<Vec<_>>();
    if !matching.is_empty() {
        return matching;
    }
    if artifact.final_state.pull_requests.len() == 1 {
        return artifact.final_state.ci.jobs.iter().collect();
    }
    Vec::new()
}
