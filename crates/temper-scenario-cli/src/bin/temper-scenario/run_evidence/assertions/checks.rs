// SPDX-License-Identifier: MPL-2.0

use toml::Value;

use super::super::model::{AssertionResultEvidence, RunEvidenceArtifact};
use super::common::SUPPORTED_REPO_CHECK_FIELDS;
use super::issue::evaluate_issue_check;
use super::pull_request::evaluate_pull_request_check;
use super::repository::evaluate_repo_check;
use super::support::{ArtifactSelector, ResultBuilder, required_assertion};

const CONTROL_FIELDS: &[&str] = &["id", "artifact", "required"];
const SUPPORTED_CHECK_FIELDS: &[&str] = &[
    "state",
    "labels",
    "labels_cleared",
    "ci",
    "title",
    "author",
    "merged_by_one_of",
    "body_prefix",
    "body_prefix_file",
    "stale_body_absent",
    "metadata_kind",
    "metadata_parent",
    "correlation_key",
];
const SOURCE_LINK_FIELDS: &[&str] = &["source_artifact", "metadata_parent"];
const PROVIDER_REF_FIELDS: &[&str] = &["ref"];

pub(super) fn evaluate_checks(
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
    match required_assertion(check) {
        Ok(required) => builder = builder.required(required),
        Err(message) => return builder.failed(message).build(),
    }

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
