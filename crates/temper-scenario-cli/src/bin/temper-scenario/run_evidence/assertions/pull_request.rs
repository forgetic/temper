// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::path::Path;

use temper_workflow::{ArtifactKindId, ArtifactRef, parse_metadata_block};

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
        Err(SelectionProblem::MissingFact(message)) => {
            return builder.missing_fact(message).build();
        }
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
    if let Some(value) = check.get("title") {
        if let Some(expected) = value.as_str() {
            builder = evaluate_title(builder, expected, pull_request);
        } else {
            builder = builder.failed("title must be a string");
        }
    }
    if let Some(value) = check.get("body_prefix") {
        if let Some(expected) = value.as_str() {
            builder = evaluate_body_prefix(builder, expected, pull_request);
        } else {
            builder = builder.failed("body_prefix must be a string");
        }
    }
    if let Some(value) = check.get("body_prefix_file") {
        if let Some(path) = value.as_str() {
            builder = evaluate_body_prefix_file(builder, path, pull_request, artifact);
        } else {
            builder = builder.failed("body_prefix_file must be a string");
        }
    }
    if let Some(value) = check.get("stale_body_absent") {
        if let Some(stale) = value.as_str() {
            builder = evaluate_stale_body_absent(builder, stale, pull_request);
        } else {
            builder = builder.failed("stale_body_absent must be a string");
        }
    }
    if let Some(value) = check.get("metadata_kind") {
        if let Some(expected) = value.as_str() {
            builder = evaluate_metadata_kind(builder, expected, pull_request);
        } else {
            builder = builder.failed("metadata_kind must be a string");
        }
    }
    if let Some(value) = check.get("metadata_parent") {
        if let Some(expected) = value.as_str() {
            builder = evaluate_metadata_parent(builder, expected, pull_request, artifact);
        } else {
            builder = builder.failed("metadata_parent must be a string");
        }
    }
    if let Some(value) = check.get("correlation_key") {
        if let Some(expected) = value.as_str() {
            builder = evaluate_correlation_key(builder, expected, pull_request, artifact);
        } else {
            builder = builder.failed("correlation_key must be a string");
        }
    }
    builder = reject_repo_fields_for_non_repo(builder, check);

    builder.build()
}

fn evaluate_title(
    builder: ResultBuilder,
    expected: &str,
    pull_request: &PullRequestStateEvidence,
) -> ResultBuilder {
    match pull_request.title.as_deref() {
        Some(actual) if actual == expected => builder.passed(format!(
            "pull request #{} title matched `{expected}`",
            pull_request.number
        )),
        Some(actual) => builder.failed(format!(
            "expected pull request #{} title `{expected}`, observed `{actual}`",
            pull_request.number
        )),
        None => builder.missing_fact(format!(
            "run evidence has no title fact for pull request #{}",
            pull_request.number
        )),
    }
}

fn evaluate_body_prefix(
    builder: ResultBuilder,
    expected: &str,
    pull_request: &PullRequestStateEvidence,
) -> ResultBuilder {
    let Some(body) = pull_request.body.as_deref() else {
        return builder.missing_fact(format!(
            "run evidence has no body fact for pull request #{}",
            pull_request.number
        ));
    };
    let expected = expected.trim();
    if body.starts_with(expected) {
        builder.passed(format!(
            "pull request #{} body starts with expected authored prefix",
            pull_request.number
        ))
    } else {
        builder.failed(format!(
            "pull request #{} body did not start with expected authored prefix `{expected}`",
            pull_request.number
        ))
    }
}

fn evaluate_body_prefix_file(
    builder: ResultBuilder,
    path: &str,
    pull_request: &PullRequestStateEvidence,
    artifact: &RunEvidenceArtifact,
) -> ResultBuilder {
    let scenario_path = Path::new(&artifact.scenario.scenario_path);
    let path = scenario_path.join(path);
    match fs::read_to_string(&path) {
        Ok(expected) => evaluate_body_prefix(builder, &expected, pull_request),
        Err(error) => builder.failed(format!("read body_prefix_file {}: {error}", path.display())),
    }
}

fn evaluate_stale_body_absent(
    builder: ResultBuilder,
    stale: &str,
    pull_request: &PullRequestStateEvidence,
) -> ResultBuilder {
    let Some(body) = pull_request.body.as_deref() else {
        return builder.missing_fact(format!(
            "run evidence has no body fact for pull request #{}",
            pull_request.number
        ));
    };
    let stale = stale.trim();
    if stale.is_empty() || !body.contains(stale) {
        builder.passed(format!(
            "pull request #{} body does not contain stale handoff text",
            pull_request.number
        ))
    } else {
        builder.failed(format!(
            "pull request #{} body still contains stale handoff text `{stale}`",
            pull_request.number
        ))
    }
}

fn evaluate_metadata_kind(
    builder: ResultBuilder,
    expected: &str,
    pull_request: &PullRequestStateEvidence,
) -> ResultBuilder {
    let Some(metadata) = pull_request_metadata(pull_request) else {
        return builder.missing_fact(format!(
            "run evidence has no parseable workflow metadata body for pull request #{}",
            pull_request.number
        ));
    };
    let expected = ArtifactKindId::new(expected);
    if metadata.kind == Some(expected.clone()) {
        builder.passed(format!(
            "pull request #{} metadata kind matched `{}`",
            pull_request.number, expected
        ))
    } else {
        builder.failed(format!(
            "pull request #{} metadata kind {:?} did not match `{}`",
            pull_request.number, metadata.kind, expected
        ))
    }
}

fn evaluate_metadata_parent(
    builder: ResultBuilder,
    expected: &str,
    pull_request: &PullRequestStateEvidence,
    artifact: &RunEvidenceArtifact,
) -> ResultBuilder {
    let Some(metadata) = pull_request_metadata(pull_request) else {
        return builder.missing_fact(format!(
            "run evidence has no parseable workflow metadata body for pull request #{}",
            pull_request.number
        ));
    };
    let Some(parent) = expected_parent_ref(expected, artifact) else {
        return builder.missing_fact(format!("could not resolve metadata_parent `{expected}`"));
    };
    if metadata
        .parents
        .iter()
        .any(|candidate| candidate == &parent)
    {
        builder.passed(format!(
            "pull request #{} metadata parent matched `{expected}`",
            pull_request.number
        ))
    } else {
        builder.failed(format!(
            "pull request #{} metadata parents {:?} did not contain `{expected}`",
            pull_request.number, metadata.parents
        ))
    }
}

fn evaluate_correlation_key(
    builder: ResultBuilder,
    expected: &str,
    pull_request: &PullRequestStateEvidence,
    artifact: &RunEvidenceArtifact,
) -> ResultBuilder {
    let Some(metadata) = pull_request_metadata(pull_request) else {
        return builder.missing_fact(format!(
            "run evidence has no parseable workflow metadata body for pull request #{}",
            pull_request.number
        ));
    };
    let expected = resolve_correlation(expected, artifact);
    if metadata.correlation_key.as_deref() == Some(expected.as_str()) {
        builder.passed(format!(
            "pull request #{} metadata correlation matched `{expected}`",
            pull_request.number
        ))
    } else {
        builder.failed(format!(
            "pull request #{} metadata correlation {:?} did not match `{expected}`",
            pull_request.number, metadata.correlation_key
        ))
    }
}

fn pull_request_metadata(
    pull_request: &PullRequestStateEvidence,
) -> Option<temper_workflow::WorkflowMetadata> {
    pull_request
        .body
        .as_deref()
        .and_then(|body| parse_metadata_block(body).ok().flatten())
}

fn expected_parent_ref(expected: &str, artifact: &RunEvidenceArtifact) -> Option<ArtifactRef> {
    let id = expected
        .strip_prefix("issue:")
        .or_else(|| {
            expected
                .strip_prefix('$')
                .and_then(|value| value.strip_prefix("issue:"))
        })
        .unwrap_or(expected);
    let issue = artifact
        .final_state
        .issues
        .iter()
        .find(|issue| issue.id.as_deref() == Some(id))
        .or_else(|| {
            if matches!(id, "source" | "intake") {
                artifact.final_state.issues.first()
            } else {
                None
            }
        })?;
    Some(ArtifactRef::same_repo(temper_forge_model::ItemNumber::new(
        issue.number,
    )))
}

fn resolve_correlation(expected: &str, artifact: &RunEvidenceArtifact) -> String {
    if let Some(id) = expected
        .strip_prefix("$correlation:")
        .or_else(|| expected.strip_prefix("correlation:"))
    {
        if let Some(issue) = artifact
            .final_state
            .issues
            .iter()
            .find(|issue| issue.id.as_deref() == Some(id))
        {
            return format!("pr-for-code-{}", issue.number);
        }
    }
    expected.to_string()
}

fn evaluate_ci(
    builder: ResultBuilder,
    expected: &str,
    pull_request: &PullRequestStateEvidence,
    artifact: &RunEvidenceArtifact,
) -> ResultBuilder {
    let jobs = ci_jobs_for_pull_request(artifact, pull_request);
    if jobs.is_empty() {
        return builder.missing_fact(format!(
            "run evidence has no CI job conclusion facts for pull request #{}",
            pull_request.number
        ));
    }

    let conclusions = jobs
        .iter()
        .filter_map(|job| job.conclusion.as_deref())
        .collect::<Vec<_>>();
    if conclusions.is_empty() {
        return builder.missing_fact(format!(
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
