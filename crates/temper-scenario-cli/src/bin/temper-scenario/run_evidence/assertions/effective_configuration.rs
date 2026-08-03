// SPDX-License-Identifier: MPL-2.0

use toml::Value;

use super::super::model::{AssertionResultEvidence, RunEvidenceArtifact};
use super::support::{ResultBuilder, required_assertion};

const CADENCE_FIELDS: &[&str] = &[
    "ci_poll_cadence_secs",
    "poll_cadence_secs",
    "mechanical_cadence_secs",
];
const SUPPORTED_FIELDS: &[&str] = &[
    "id",
    "required",
    "ci_poll_cadence_secs",
    "poll_cadence_secs",
    "mechanical_cadence_secs",
];

pub(super) fn evaluate(
    expect: &toml::Table,
    artifact: &RunEvidenceArtifact,
    results: &mut Vec<AssertionResultEvidence>,
) {
    let Some(value) = expect.get("effective_configuration") else {
        return;
    };
    let Some(expectation) = value.as_table() else {
        results.push(
            ResultBuilder::new(
                "expect.effective_configuration".to_string(),
                "The standalone engine used the exact declared cadences.".to_string(),
                None,
            )
            .failed("expect.effective_configuration must be a table")
            .build(),
        );
        return;
    };
    results.push(evaluate_one(expectation, artifact));
}

fn evaluate_one(
    expectation: &toml::Table,
    artifact: &RunEvidenceArtifact,
) -> AssertionResultEvidence {
    let id = expectation
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("effective-configuration")
        .to_string();
    let mut builder = ResultBuilder::new(
        id,
        "The standalone engine used the exact declared dedicated-CI, role-feed, and mechanical cadences."
            .to_string(),
        None,
    );
    match required_assertion(expectation) {
        Ok(required) => builder = builder.required(required),
        Err(message) => return builder.failed(message).build(),
    }
    for field in expectation.keys() {
        if !SUPPORTED_FIELDS.contains(&field.as_str()) {
            builder = builder.failed(format!(
                "unsupported effective configuration expectation field `{field}`"
            ));
        }
    }

    let Some(configuration) = artifact.effective_configuration.as_ref() else {
        return builder
            .missing_fact("run evidence has no effective standalone configuration record")
            .build();
    };
    for field in CADENCE_FIELDS {
        let Some(value) = expectation.get(*field) else {
            builder = builder.failed(format!(
                "effective configuration assertion must declare `{field}`"
            ));
            continue;
        };
        let Some(expected) = value
            .as_integer()
            .and_then(|value| u64::try_from(value).ok())
        else {
            builder = builder.failed(format!("{field} must be a non-negative integer"));
            continue;
        };
        let actual = match *field {
            "ci_poll_cadence_secs" => configuration.ci_poll_cadence_secs,
            "poll_cadence_secs" => configuration.poll_cadence_secs,
            "mechanical_cadence_secs" => configuration.mechanical_cadence_secs,
            _ => unreachable!("closed cadence field list"),
        };
        if actual == expected {
            builder = builder.passed(format!("{field} matched {expected}"));
        } else {
            builder = builder.failed(format!("expected {field} {expected}, observed {actual}"));
        }
    }
    builder.build()
}
