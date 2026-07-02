// SPDX-License-Identifier: MPL-2.0

use std::path::Path;

use temper_scenario_core::load_resolved_manifest_toml;
use toml::Value;

use super::model::{
    AssertionEvidence, AssertionResultEvidence, CiJobEvidence, PullRequestStateEvidence,
    RepositoryBranchStateEvidence, RepositoryStateEvidence, RunEvidenceArtifact,
};

#[path = "assertions/summary.rs"]
mod summary;
#[path = "assertions/support.rs"]
mod support;

use summary::{evaluate_counts, evaluate_templates};
use support::{
    ArtifactSelector, ResultBuilder, SelectedIssue, SelectedPullRequest, SelectedRepository,
    SelectionProblem, ci_conclusion_failed, ci_conclusion_passed, has_label, same_normalized,
    select_issue, select_pull_request, select_repository, string_array,
};

const CONTROL_FIELDS: &[&str] = &["id", "artifact"];
const SUPPORTED_CHECK_FIELDS: &[&str] = &["state", "labels", "labels_cleared", "ci"];
const SUPPORTED_REPO_CHECK_FIELDS: &[&str] = &["branch", "contains_engineer_diff"];
const SOURCE_LINK_FIELDS: &[&str] = &["source_artifact", "metadata_parent"];
const PROVIDER_REF_FIELDS: &[&str] = &["ref"];

pub(crate) fn evaluate_manifest_assertions(
    manifest_path: &Path,
    artifact: &RunEvidenceArtifact,
) -> Result<Option<AssertionEvidence>, String> {
    let manifest = load_resolved_manifest_toml(manifest_path).map_err(|error| error.to_string())?;
    let Some(expect) = manifest.get("expect").and_then(Value::as_table) else {
        return Ok(None);
    };

    let mut results = Vec::new();
    evaluate_templates(expect, artifact, &mut results);
    evaluate_counts(expect, artifact, &mut results);
    evaluate_checks(expect, artifact, &mut results);

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

fn evaluate_checks(
    expect: &toml::Table,
    artifact: &RunEvidenceArtifact,
    results: &mut Vec<AssertionResultEvidence>,
) {
    let Some(value) = expect.get("checks") else {
        return;
    };
    let Some(checks) = value.as_array() else {
        results.push(
            ResultBuilder::new(
                "expect.checks".to_string(),
                "Manifest expectation checks are well-formed.".to_string(),
                None,
            )
            .failed("expect.checks must be an array of tables")
            .build(),
        );
        return;
    };

    for (index, value) in checks.iter().enumerate() {
        let Some(check) = value.as_table() else {
            results.push(
                ResultBuilder::new(
                    format!("expect.checks[{index}]"),
                    "Manifest expectation check is well-formed.".to_string(),
                    None,
                )
                .failed("expect.checks entries must be tables")
                .build(),
            );
            continue;
        };
        results.push(evaluate_check(index, check, artifact));
    }
}

fn evaluate_check(
    index: usize,
    check: &toml::Table,
    artifact: &RunEvidenceArtifact,
) -> AssertionResultEvidence {
    let id = check
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("expect.checks[{index}]"));
    let artifact_name = check
        .get("artifact")
        .and_then(Value::as_str)
        .map(str::to_string);
    let description = match artifact_name.as_deref() {
        Some(artifact_name) => format!("Manifest check `{id}` for `{artifact_name}`."),
        None => format!("Manifest check `{id}`."),
    };
    let mut builder = ResultBuilder::new(id, description, artifact_name.clone());

    for key in check.keys() {
        if CONTROL_FIELDS.contains(&key.as_str())
            || SUPPORTED_CHECK_FIELDS.contains(&key.as_str())
            || SUPPORTED_REPO_CHECK_FIELDS.contains(&key.as_str())
        {
            continue;
        }
        if SOURCE_LINK_FIELDS.contains(&key.as_str()) {
            builder = builder.unsupported(format!(
                "field `{key}` requires source/parent relationship facts that are not present in structured run evidence yet"
            ));
        } else if PROVIDER_REF_FIELDS.contains(&key.as_str()) {
            builder = builder.unsupported(format!(
                "field `{key}` requires provider branch/ref probing; keep provider-only ref existence checks in script-hook assertions"
            ));
        } else {
            builder = builder.unsupported(format!(
                "field `{key}` is not supported by structured run-evidence assertions yet"
            ));
        }
    }

    let Some(artifact_name) = artifact_name else {
        return builder
            .unsupported(
                "check has no `artifact` selector, so structured evidence cannot choose a target",
            )
            .build();
    };

    match ArtifactSelector::parse(&artifact_name) {
        ArtifactSelector::Issue(id) => {
            evaluate_issue_check(check, artifact, id.as_deref(), builder)
        }
        ArtifactSelector::PullRequest(id) => {
            evaluate_pull_request_check(check, artifact, id.as_deref(), builder)
        }
        ArtifactSelector::Repo(id) => evaluate_repo_check(check, artifact, id.as_deref(), builder),
        ArtifactSelector::Unknown(kind) => builder
            .unsupported(format!(
                "artifact kind `{kind}` is not supported by the assertion engine"
            ))
            .build(),
    }
}

fn evaluate_issue_check(
    check: &toml::Table,
    artifact: &RunEvidenceArtifact,
    id: Option<&str>,
    mut builder: ResultBuilder,
) -> AssertionResultEvidence {
    let selected = select_issue(&artifact.final_state.issues, id);
    let SelectedIssue { issue, note } = match selected {
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
                "issue",
                id,
                expected,
                issue.state.as_deref(),
                &format!("#{}", issue.number),
            );
        } else {
            builder = builder.failed("state must be a string");
        }
    }
    if let Some(value) = check.get("labels") {
        builder = evaluate_labels_present(builder, value, &issue.labels);
    }
    if let Some(value) = check.get("labels_cleared") {
        builder = evaluate_labels_cleared(builder, value, &issue.labels);
    }
    if check.contains_key("ci") {
        builder = builder.unsupported("field `ci` requires a pull_request artifact");
    }
    builder = reject_repo_fields_for_non_repo(builder, check);

    builder.build()
}

fn evaluate_pull_request_check(
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

fn evaluate_repo_check(
    check: &toml::Table,
    artifact: &RunEvidenceArtifact,
    id: Option<&str>,
    mut builder: ResultBuilder,
) -> AssertionResultEvidence {
    let selected = select_repository(&artifact.final_state.repositories, id);
    let SelectedRepository { repository, note } = match selected {
        Ok(selected) => selected,
        Err(SelectionProblem::Failed(message)) => return builder.failed(message).build(),
        Err(SelectionProblem::Unsupported(message)) => return builder.unsupported(message).build(),
    };
    if let Some(note) = note {
        builder = builder.passed(note);
    }

    for key in ["state", "labels", "labels_cleared", "ci"] {
        if check.contains_key(key) {
            builder = builder.failed(format!(
                "field `{key}` requires an issue or pull_request artifact"
            ));
        }
    }

    let repo_name = repository_display_name(repository, id);
    let expected_branch = match check.get("branch") {
        Some(value) => match value.as_str().map(str::trim) {
            Some(branch) if !branch.is_empty() => Some(branch),
            Some(_) => {
                builder = builder.failed("branch must be a non-empty string");
                None
            }
            None => {
                builder = builder.failed("branch must be a string");
                None
            }
        },
        None => None,
    };

    let selected_branch = select_repository_branch(
        repository,
        expected_branch,
        &repo_name,
        check.contains_key("contains_engineer_diff"),
    );
    let branch = match selected_branch {
        BranchSelection::Selected { branch, detail } => {
            builder = builder.passed(detail);
            Some(branch)
        }
        BranchSelection::Missing(detail) => {
            builder = builder.failed(detail);
            None
        }
        BranchSelection::Ambiguous(detail) | BranchSelection::Unsupported(detail) => {
            builder = builder.unsupported(detail);
            None
        }
        BranchSelection::NotNeeded => None,
    };

    if let Some(value) = check.get("contains_engineer_diff") {
        let Some(expected) = value.as_bool() else {
            return builder
                .failed("contains_engineer_diff must be a boolean")
                .build();
        };
        let Some(branch) = branch else {
            return builder
                .unsupported(
                    "contains_engineer_diff could not be evaluated because no branch fact matched",
                )
                .build();
        };
        builder = evaluate_contains_engineer_diff(builder, &repo_name, branch, expected);
    }

    builder.build()
}

fn reject_repo_fields_for_non_repo(
    mut builder: ResultBuilder,
    check: &toml::Table,
) -> ResultBuilder {
    for key in SUPPORTED_REPO_CHECK_FIELDS {
        if check.contains_key(*key) {
            builder = builder.failed(format!("field `{key}` requires a repository artifact"));
        }
    }
    builder
}

fn repository_display_name(repository: &RepositoryStateEvidence, id: Option<&str>) -> String {
    id.map(str::to_string)
        .or_else(|| repository.id.clone())
        .or_else(|| repository.slug.clone())
        .unwrap_or_else(|| "<unknown>".to_string())
}

enum BranchSelection<'a> {
    Selected {
        branch: &'a RepositoryBranchStateEvidence,
        detail: String,
    },
    Missing(String),
    Ambiguous(String),
    Unsupported(String),
    NotNeeded,
}

fn select_repository_branch<'a>(
    repository: &'a RepositoryStateEvidence,
    expected_branch: Option<&str>,
    repo_name: &str,
    branch_required: bool,
) -> BranchSelection<'a> {
    if let Some(expected) = expected_branch {
        let matches = repository
            .branches
            .iter()
            .filter(|branch| same_normalized(&branch.name, expected))
            .collect::<Vec<_>>();
        return match matches.as_slice() {
            [branch] => BranchSelection::Selected {
                branch,
                detail: format!(
                    "repository `{repo_name}` branch `{}` is present",
                    branch.name
                ),
            },
            [] => BranchSelection::Missing(format!(
                "expected repository `{repo_name}` branch `{expected}` was absent; observed branches {:?}",
                repository_branch_names(repository)
            )),
            _ => BranchSelection::Ambiguous(format!(
                "repository `{repo_name}` branch `{expected}` matched multiple branch facts"
            )),
        };
    }

    match repository.branches.as_slice() {
        [] if branch_required => BranchSelection::Unsupported(format!(
            "run evidence has no branch facts for repository `{repo_name}`"
        )),
        [] => BranchSelection::NotNeeded,
        [branch] => BranchSelection::Selected {
            branch,
            detail: format!(
                "matched sole repository branch `{}` because check has no `branch` selector",
                branch.name
            ),
        },
        _ if branch_required => BranchSelection::Unsupported(format!(
            "contains_engineer_diff requires a `branch` selector because repository `{repo_name}` has multiple branch facts"
        )),
        _ => BranchSelection::NotNeeded,
    }
}

fn repository_branch_names(repository: &RepositoryStateEvidence) -> Vec<&str> {
    repository
        .branches
        .iter()
        .map(|branch| branch.name.as_str())
        .collect()
}

fn evaluate_contains_engineer_diff(
    builder: ResultBuilder,
    repo_name: &str,
    branch: &RepositoryBranchStateEvidence,
    expected: bool,
) -> ResultBuilder {
    let Some(actual) = branch.contains_engineer_diff else {
        return builder.unsupported(format!(
            "repository `{repo_name}` branch `{}` is missing contains_engineer_diff fact",
            branch.name
        ));
    };
    if actual == expected {
        let state = if actual {
            "contains"
        } else {
            "does not contain"
        };
        builder.passed(format!(
            "repository `{repo_name}` branch `{}` {state} the engineer diff",
            branch.name
        ))
    } else {
        builder.failed(format!(
            "expected repository `{repo_name}` branch `{}` contains_engineer_diff={expected}, observed {actual}",
            branch.name
        ))
    }
}

fn evaluate_state(
    builder: ResultBuilder,
    kind: &str,
    id: Option<&str>,
    expected: &str,
    actual: Option<&str>,
    fallback_name: &str,
) -> ResultBuilder {
    let Some(actual) = actual else {
        return builder.unsupported(format!("{kind} state fact is missing"));
    };
    let display = id.unwrap_or(fallback_name);
    if same_normalized(actual, expected) {
        builder.passed(format!(
            "{kind} `{display}` state matched `{}`",
            expected.trim()
        ))
    } else {
        builder.failed(format!(
            "expected {kind} `{display}` state `{}`, observed `{actual}`",
            expected.trim()
        ))
    }
}

fn evaluate_labels_present(
    builder: ResultBuilder,
    value: &Value,
    labels: &[String],
) -> ResultBuilder {
    let expected = match string_array(value, "labels") {
        Ok(expected) => expected,
        Err(message) => return builder.failed(message),
    };
    let missing = expected
        .iter()
        .filter(|label| !has_label(labels, label))
        .cloned()
        .collect::<Vec<_>>();
    if missing.is_empty() {
        builder.passed(format!("labels include {:?}", expected))
    } else {
        builder.failed(format!(
            "missing expected labels {:?}; observed labels {:?}",
            missing, labels
        ))
    }
}

fn evaluate_labels_cleared(
    builder: ResultBuilder,
    value: &Value,
    labels: &[String],
) -> ResultBuilder {
    let expected = match string_array(value, "labels_cleared") {
        Ok(expected) => expected,
        Err(message) => return builder.failed(message),
    };
    let present = expected
        .iter()
        .filter(|label| has_label(labels, label))
        .cloned()
        .collect::<Vec<_>>();
    if present.is_empty() {
        builder.passed(format!("labels cleared {:?}", expected))
    } else {
        builder.failed(format!(
            "labels expected to be cleared are still present {:?}; observed labels {:?}",
            present, labels
        ))
    }
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
