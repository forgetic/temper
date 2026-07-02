// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeMap;

use toml::Value;

use super::super::model::{AssertionResultEvidence, IssueStateEvidence, RunEvidenceArtifact};
use super::support::{ResultBuilder, has_label, nonnegative_integer, state_is};

pub(super) fn evaluate_templates(
    expect: &toml::Table,
    artifact: &RunEvidenceArtifact,
    results: &mut Vec<AssertionResultEvidence>,
) {
    for template in template_names(expect) {
        match template.as_str() {
            "single-pr-merged-source-closed" => {
                results.push(evaluate_single_pr_merged_source_closed(artifact));
            }
            "no-duplicate-prs" => results.push(evaluate_no_duplicate_prs(
                "template:no-duplicate-prs".to_string(),
                "No duplicate implementation PRs are present.".to_string(),
                artifact,
            )),
            other => results.push(ResultBuilder::new(
                format!("template:{other}"),
                format!("Assertion template `{other}` is declared."),
                None,
            )
            .unsupported(format!(
                "template `{other}` is known to manifest validation but is not backed by structured run-evidence assertions yet"
            ))
            .build()),
        }
    }
}

fn template_names(expect: &toml::Table) -> Vec<String> {
    let mut names = Vec::new();
    if let Some(name) = expect.get("template").and_then(Value::as_str) {
        names.push(name.to_string());
    }
    if let Some(array) = expect.get("templates").and_then(Value::as_array) {
        names.extend(array.iter().filter_map(Value::as_str).map(str::to_string));
    }
    names
}

fn evaluate_single_pr_merged_source_closed(
    artifact: &RunEvidenceArtifact,
) -> AssertionResultEvidence {
    let mut builder = ResultBuilder::new(
        "template:single-pr-merged-source-closed".to_string(),
        "One implementation PR merges and closes its source issue.".to_string(),
        None,
    );

    if artifact.final_state.pull_requests.is_empty() {
        builder = builder.unsupported("run evidence has no final pull request facts");
    } else {
        let merged = artifact
            .final_state
            .pull_requests
            .iter()
            .filter(|pull_request| state_is(pull_request.state.as_deref(), "merged"))
            .count();
        if merged == 1 {
            builder = builder.passed("observed exactly 1 merged pull request");
        } else {
            builder = builder.failed(format!(
                "expected exactly 1 merged pull request, observed {merged}"
            ));
        }
    }

    match source_issue_candidates(artifact) {
        SourceIssueCandidates::Issues(issues) => {
            let closed = issues
                .iter()
                .filter(|issue| state_is(issue.state.as_deref(), "closed"))
                .count();
            if closed == 1 {
                builder = builder.passed("observed exactly 1 closed source/parent issue");
            } else {
                builder = builder.failed(format!(
                    "expected exactly 1 closed source/parent issue, observed {closed}"
                ));
            }
        }
        SourceIssueCandidates::Unsupported(message) => {
            builder = builder.unsupported(message);
        }
    }

    builder.build()
}

pub(super) fn evaluate_counts(
    expect: &toml::Table,
    artifact: &RunEvidenceArtifact,
    results: &mut Vec<AssertionResultEvidence>,
) {
    for field in [
        "merged_pull_requests",
        "closed_parent_issues",
        "created_pull_requests",
        "refreshed_pull_requests",
    ] {
        let Some(value) = expect.get(field) else {
            continue;
        };
        let expected = match nonnegative_integer(value) {
            Ok(expected) => expected,
            Err(message) => {
                results.push(
                    ResultBuilder::new(
                        format!("expect.{field}"),
                        format!("Count expectation `{field}` is well-formed."),
                        None,
                    )
                    .failed(message)
                    .build(),
                );
                continue;
            }
        };

        match field {
            "merged_pull_requests" => results.push(evaluate_count(
                format!("expect.{field}"),
                "Merged pull request count matches the manifest expectation.".to_string(),
                expected,
                count_merged_pull_requests(artifact),
                "run evidence has no final pull request facts".to_string(),
            )),
            "closed_parent_issues" => results.push(evaluate_count(
                format!("expect.{field}"),
                "Closed parent/source issue count matches the manifest expectation.".to_string(),
                expected,
                count_closed_parent_issues(artifact),
                "run evidence has no final issue facts".to_string(),
            )),
            "created_pull_requests" | "refreshed_pull_requests" => results.push(
                ResultBuilder::new(
                    format!("expect.{field}"),
                    format!("Count expectation `{field}` is declared."),
                    None,
                )
                .unsupported(format!(
                    "run evidence does not yet distinguish created/refreshed pull request actions for `{field}`; script-hook or provider action facts are required"
                ))
                .build(),
            ),
            _ => {}
        }
    }
}

fn evaluate_count(
    id: String,
    description: String,
    expected: u64,
    actual: Option<u64>,
    missing_fact: String,
) -> AssertionResultEvidence {
    let builder = ResultBuilder::new(id, description, None);
    let Some(actual) = actual else {
        return builder.unsupported(missing_fact).build();
    };
    if actual == expected {
        builder
            .passed(format!("expected {expected}, observed {actual}"))
            .build()
    } else {
        builder
            .failed(format!("expected {expected}, observed {actual}"))
            .build()
    }
}

fn evaluate_no_duplicate_prs(
    id: String,
    description: String,
    artifact: &RunEvidenceArtifact,
) -> AssertionResultEvidence {
    let mut builder = ResultBuilder::new(id, description, None);
    if artifact.final_state.pull_requests.is_empty() {
        return builder
            .unsupported("run evidence has no final pull request facts")
            .build();
    }
    let implementation = artifact
        .final_state
        .pull_requests
        .iter()
        .filter(|pull_request| has_label(&pull_request.labels, "implementation"))
        .collect::<Vec<_>>();
    if implementation.is_empty() {
        return builder
            .unsupported("run evidence has pull requests but no implementation label facts")
            .build();
    }
    if implementation
        .iter()
        .any(|pull_request| pull_request.head_branch.is_none())
    {
        return builder
            .unsupported("implementation PR duplicate detection requires head_branch facts")
            .build();
    }

    let mut by_branch = BTreeMap::<&str, Vec<u64>>::new();
    for pull_request in implementation {
        let branch = pull_request
            .head_branch
            .as_deref()
            .expect("head_branch checked above");
        by_branch
            .entry(branch)
            .or_default()
            .push(pull_request.number);
    }
    let duplicates = by_branch
        .iter()
        .filter(|(_, numbers)| numbers.len() > 1)
        .map(|(branch, numbers)| format!("{branch}: {numbers:?}"))
        .collect::<Vec<_>>();
    if duplicates.is_empty() {
        builder = builder.passed("no implementation PR head branch has multiple PRs");
    } else {
        builder = builder.failed(format!(
            "duplicate implementation PRs found by head branch: {}",
            duplicates.join(", ")
        ));
    }
    builder.build()
}

fn count_merged_pull_requests(artifact: &RunEvidenceArtifact) -> Option<u64> {
    if artifact.final_state.pull_requests.is_empty() {
        return None;
    }
    Some(
        artifact
            .final_state
            .pull_requests
            .iter()
            .filter(|pull_request| state_is(pull_request.state.as_deref(), "merged"))
            .count() as u64,
    )
}

fn count_closed_parent_issues(artifact: &RunEvidenceArtifact) -> Option<u64> {
    if artifact.final_state.issues.is_empty() {
        return None;
    }
    Some(
        artifact
            .final_state
            .issues
            .iter()
            .filter(|issue| state_is(issue.state.as_deref(), "closed"))
            .count() as u64,
    )
}

enum SourceIssueCandidates<'a> {
    Issues(Vec<&'a IssueStateEvidence>),
    Unsupported(String),
}

fn source_issue_candidates(artifact: &RunEvidenceArtifact) -> SourceIssueCandidates<'_> {
    if artifact.final_state.issues.is_empty() {
        return SourceIssueCandidates::Unsupported(
            "run evidence has no final issue facts".to_string(),
        );
    }
    if let Some(issue_number) = artifact
        .provider
        .as_ref()
        .and_then(|provider| provider.issue_number)
    {
        let matches = artifact
            .final_state
            .issues
            .iter()
            .filter(|issue| issue.number == issue_number)
            .collect::<Vec<_>>();
        if !matches.is_empty() {
            return SourceIssueCandidates::Issues(matches);
        }
    }
    let named = artifact
        .final_state
        .issues
        .iter()
        .filter(|issue| {
            issue.id.as_deref().is_some_and(|id| {
                matches!(id, "intake" | "source" | "parent" | "parent_issue")
                    || id.starts_with("source:")
            })
        })
        .collect::<Vec<_>>();
    if !named.is_empty() {
        return SourceIssueCandidates::Issues(named);
    }
    if artifact.final_state.issues.len() == 1 {
        return SourceIssueCandidates::Issues(artifact.final_state.issues.iter().collect());
    }
    SourceIssueCandidates::Unsupported(
        "run evidence has multiple issues but no source/parent issue id or provider issue number fact"
            .to_string(),
    )
}
