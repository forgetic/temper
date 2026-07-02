// SPDX-License-Identifier: MPL-2.0

use super::super::model::{
    AssertionResultEvidence, RepositoryBranchStateEvidence, RepositoryStateEvidence,
    RunEvidenceArtifact,
};
use super::support::{
    ResultBuilder, SelectedRepository, SelectionProblem, same_normalized, select_repository,
};

pub(super) fn evaluate_repo_check(
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
