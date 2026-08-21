// SPDX-License-Identifier: MPL-2.0

use toml::Value;

use super::super::model::{AssertionResultEvidence, RunEvidenceArtifact};
use super::support::{ResultBuilder, nonnegative_integer, required_assertion};

const SUPPORTED_FIELDS: &[&str] = &[
    "id",
    "required",
    "seeded_run_count",
    "payload_bytes_per_run",
    "full_inventory_exceeds_transport_cap",
    "largest_paged_response_below_transport_cap",
    "minimum_pages_observed",
    "minimum_target_run_page",
    "later_page_selection",
    "webhooks_disabled",
    "provenance_drop_count",
];

pub(super) fn evaluate(
    expect: &toml::Table,
    artifact: &RunEvidenceArtifact,
    results: &mut Vec<AssertionResultEvidence>,
) {
    let Some(value) = expect.get("actions_history") else {
        return;
    };
    let Some(expectation) = value.as_table() else {
        results.push(
            ResultBuilder::new(
                "expect.actions_history".to_string(),
                "The oversized Actions fixture stayed bounded and selected a later-page run."
                    .to_string(),
                None,
            )
            .failed("expect.actions_history must be a table")
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
        .unwrap_or("bounded-actions-history")
        .to_string();
    let mut builder = ResultBuilder::new(
        id,
        "The oversized Actions fixture stayed below the transport cap per page and selected the exact-head run after page one."
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
                "unsupported Actions history expectation field `{field}`"
            ));
        }
    }

    let Some(history) = artifact.final_state.ci.actions_history.as_ref() else {
        return builder
            .missing_fact("run evidence has no bounded Actions history record")
            .build();
    };

    builder = exact_count(
        builder,
        expectation,
        "seeded_run_count",
        history.seeded_run_count,
    );
    builder = exact_count(
        builder,
        expectation,
        "payload_bytes_per_run",
        history.payload_bytes_per_run,
    );
    builder = relationship(
        builder,
        expectation,
        "full_inventory_exceeds_transport_cap",
        history.full_inventory_lower_bound_bytes > history.transport_cap_bytes,
    );
    builder = relationship(
        builder,
        expectation,
        "largest_paged_response_below_transport_cap",
        history.largest_paged_response_bytes < history.transport_cap_bytes,
    );
    builder = minimum(
        builder,
        expectation,
        "minimum_pages_observed",
        history.pages_observed,
    );
    builder = minimum(
        builder,
        expectation,
        "minimum_target_run_page",
        history.target_run_page,
    );
    builder = relationship(
        builder,
        expectation,
        "later_page_selection",
        history.later_page_selection,
    );
    builder = relationship(
        builder,
        expectation,
        "webhooks_disabled",
        history.webhooks_disabled,
    );
    exact_count(
        builder,
        expectation,
        "provenance_drop_count",
        history.provenance_drop_count,
    )
    .build()
}

fn exact_count(
    builder: ResultBuilder,
    expectation: &toml::Table,
    field: &str,
    actual: usize,
) -> ResultBuilder {
    let Some(value) = expectation.get(field) else {
        return builder;
    };
    let expected = match nonnegative_integer(value).and_then(|value| {
        usize::try_from(value).map_err(|_| format!("{field} is too large for this platform"))
    }) {
        Ok(expected) => expected,
        Err(message) => return builder.failed(message),
    };
    if actual == expected {
        builder.passed(format!("{field} matched {expected}"))
    } else {
        builder.failed(format!("expected {field} {expected}, observed {actual}"))
    }
}

fn minimum(
    builder: ResultBuilder,
    expectation: &toml::Table,
    field: &str,
    actual: usize,
) -> ResultBuilder {
    let Some(value) = expectation.get(field) else {
        return builder;
    };
    let expected = match nonnegative_integer(value).and_then(|value| {
        usize::try_from(value).map_err(|_| format!("{field} is too large for this platform"))
    }) {
        Ok(expected) => expected,
        Err(message) => return builder.failed(message),
    };
    if actual >= expected {
        builder.passed(format!(
            "{field} required at least {expected}, observed {actual}"
        ))
    } else {
        builder.failed(format!(
            "{field} required at least {expected}, observed {actual}"
        ))
    }
}

fn relationship(
    builder: ResultBuilder,
    expectation: &toml::Table,
    field: &str,
    actual: bool,
) -> ResultBuilder {
    let Some(value) = expectation.get(field) else {
        return builder;
    };
    let Some(expected) = value.as_bool() else {
        return builder.failed(format!("{field} must be a boolean"));
    };
    if actual == expected {
        builder.passed(format!("{field} matched {expected}"))
    } else {
        builder.failed(format!("expected {field} {expected}, observed {actual}"))
    }
}
