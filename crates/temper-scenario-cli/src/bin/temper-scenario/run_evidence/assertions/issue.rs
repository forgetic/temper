// SPDX-License-Identifier: MPL-2.0

use super::super::model::{AssertionResultEvidence, RunEvidenceArtifact};
use super::common::{
    evaluate_labels_cleared, evaluate_labels_present, evaluate_state,
    reject_repo_fields_for_non_repo,
};
use super::support::{ResultBuilder, SelectedIssue, SelectionProblem, select_issue};

pub(super) fn evaluate_issue_check(
    check: &toml::Table,
    artifact: &RunEvidenceArtifact,
    id: Option<&str>,
    mut builder: ResultBuilder,
) -> AssertionResultEvidence {
    let selected = select_issue(&artifact.final_state.issues, id);
    let SelectedIssue { issue, note } = match selected {
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
