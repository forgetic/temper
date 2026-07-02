// SPDX-License-Identifier: MPL-2.0

use toml::Value;

use super::support::{ResultBuilder, has_label, same_normalized, string_array};

pub(super) const SUPPORTED_REPO_CHECK_FIELDS: &[&str] = &["branch", "contains_engineer_diff"];

pub(super) fn reject_repo_fields_for_non_repo(
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

pub(super) fn evaluate_state(
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

pub(super) fn evaluate_labels_present(
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

pub(super) fn evaluate_labels_cleared(
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
