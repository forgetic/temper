// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeMap;

use toml::Value;

use super::super::model::{AssertionResultEvidence, RunEvidenceArtifact};
use super::support::{ResultBuilder, required_assertion, string_array};

#[path = "events/support.rs"]
mod event_support;

use event_support::{CountConstraints, EventMatcher, field_alias};

const PRESENCE_CONTROL_FIELDS: &[&str] = &["id", "description", "fields", "required"];
const SEQUENCE_CONTROL_FIELDS: &[&str] =
    &["id", "description", "events", "scope", "fields", "required"];
const COUNT_CONTROL_FIELDS: &[&str] = &[
    "id",
    "description",
    "fields",
    "group_by",
    "min",
    "max",
    "exactly",
    "count",
    "required",
];

pub(super) fn evaluate_event_expectations(
    expect: &toml::Table,
    artifact: &RunEvidenceArtifact,
    results: &mut Vec<AssertionResultEvidence>,
) {
    evaluate_presence(expect, artifact, results);
    evaluate_sequences(expect, artifact, results);
    evaluate_counts(expect, artifact, results);
}

fn evaluate_presence(
    expect: &toml::Table,
    artifact: &RunEvidenceArtifact,
    results: &mut Vec<AssertionResultEvidence>,
) {
    let Some(value) = expect.get("events") else {
        return;
    };
    let Some(events) = value.as_array() else {
        results.push(
            ResultBuilder::new(
                "expect.events".to_string(),
                "Structured event expectations are well-formed.".to_string(),
                None,
            )
            .failed("expect.events must be an array of tables")
            .build(),
        );
        return;
    };

    for (index, value) in events.iter().enumerate() {
        let Some(table) = value.as_table() else {
            results.push(
                ResultBuilder::new(
                    format!("expect.events[{index}]"),
                    "Structured event expectation is well-formed.".to_string(),
                    None,
                )
                .failed("expect.events entries must be tables")
                .build(),
            );
            continue;
        };
        results.push(evaluate_event_presence(index, table, artifact));
    }
}

fn evaluate_event_presence(
    index: usize,
    table: &toml::Table,
    artifact: &RunEvidenceArtifact,
) -> AssertionResultEvidence {
    let id = table
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("expect.events[{index}]"));
    let description = table
        .get("description")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("Structured event expectation `{id}` is observed."));
    let mut builder = ResultBuilder::new(id, description, None);
    match required_assertion(table) {
        Ok(required) => builder = builder.required(required),
        Err(message) => return builder.failed(message).build(),
    }
    let Some(events) = artifact
        .observability
        .as_ref()
        .map(|observability| observability.events.as_slice())
    else {
        return builder
            .missing_fact("run evidence has no captured structured Temper events")
            .build();
    };
    let matcher = match EventMatcher::from_table(table, PRESENCE_CONTROL_FIELDS, artifact) {
        Ok(matcher) => matcher,
        Err(message) => return builder.failed(message).build(),
    };
    let matches = events
        .iter()
        .filter(|event| matcher.matches(event))
        .collect::<Vec<_>>();
    if matches.is_empty() {
        builder = builder.failed(format!(
            "expected event `{}` with {}, observed {} structured event(s)",
            matcher.event,
            matcher.field_summary(),
            events.len()
        ));
    } else {
        builder = builder.passed(format!(
            "matched event `{}` at sequence(s) {:?}",
            matcher.event,
            matches
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>()
        ));
    }
    builder.build()
}

fn evaluate_sequences(
    expect: &toml::Table,
    artifact: &RunEvidenceArtifact,
    results: &mut Vec<AssertionResultEvidence>,
) {
    let Some(value) = expect.get("sequence") else {
        return;
    };
    let Some(sequences) = value.as_array() else {
        results.push(
            ResultBuilder::new(
                "expect.sequence".to_string(),
                "Structured event sequence expectations are well-formed.".to_string(),
                None,
            )
            .failed("expect.sequence must be an array of tables")
            .build(),
        );
        return;
    };

    for (index, value) in sequences.iter().enumerate() {
        let Some(table) = value.as_table() else {
            results.push(
                ResultBuilder::new(
                    format!("expect.sequence[{index}]"),
                    "Structured event sequence expectation is well-formed.".to_string(),
                    None,
                )
                .failed("expect.sequence entries must be tables")
                .build(),
            );
            continue;
        };
        results.push(evaluate_sequence(index, table, artifact));
    }
}

fn evaluate_sequence(
    index: usize,
    table: &toml::Table,
    artifact: &RunEvidenceArtifact,
) -> AssertionResultEvidence {
    let id = table
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("expect.sequence[{index}]"));
    let description = table
        .get("description")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("Structured event sequence `{id}` occurs in order."));
    let mut builder = ResultBuilder::new(id, description, None);
    match required_assertion(table) {
        Ok(required) => builder = builder.required(required),
        Err(message) => return builder.failed(message).build(),
    }
    let Some(events) = artifact
        .observability
        .as_ref()
        .map(|observability| observability.events.as_slice())
    else {
        return builder
            .missing_fact("run evidence has no captured structured Temper events")
            .build();
    };
    let Some(items) = table.get("events").and_then(Value::as_array) else {
        return builder
            .failed("expect.sequence entry must include an `events` array")
            .build();
    };
    let mut matchers = Vec::new();
    for (matcher_index, item) in items.iter().enumerate() {
        let Some(item_table) = item.as_table() else {
            return builder
                .failed(format!(
                    "expect.sequence events[{matcher_index}] must be an inline table"
                ))
                .build();
        };
        match EventMatcher::from_table(item_table, SEQUENCE_CONTROL_FIELDS, artifact) {
            Ok(matcher) => matchers.push(matcher),
            Err(message) => return builder.failed(message).build(),
        }
    }
    let mut start = 0usize;
    let mut matched_sequences = Vec::new();
    for matcher in &matchers {
        let Some((offset, event)) = events[start..]
            .iter()
            .enumerate()
            .find(|(_, event)| matcher.matches(event))
        else {
            return builder
                .failed(format!(
                    "missing ordered event `{}` with {} after sequence index {}",
                    matcher.event,
                    matcher.field_summary(),
                    matched_sequences.last().copied().unwrap_or(0)
                ))
                .build();
        };
        matched_sequences.push(event.sequence);
        start += offset + 1;
    }
    builder = builder.passed(format!(
        "matched ordered event sequence {:?}",
        matched_sequences
    ));
    if let Some(scope) = table.get("scope").and_then(Value::as_str) {
        builder = builder.passed(format!(
            "sequence scope `{scope}` evaluated over captured events"
        ));
    }
    builder.build()
}

fn evaluate_counts(
    expect: &toml::Table,
    artifact: &RunEvidenceArtifact,
    results: &mut Vec<AssertionResultEvidence>,
) {
    let Some(value) = expect.get("count") else {
        return;
    };
    let Some(counts) = value.as_array() else {
        results.push(
            ResultBuilder::new(
                "expect.count".to_string(),
                "Structured event count expectations are well-formed.".to_string(),
                None,
            )
            .failed("expect.count must be an array of tables")
            .build(),
        );
        return;
    };

    for (index, value) in counts.iter().enumerate() {
        let Some(table) = value.as_table() else {
            results.push(
                ResultBuilder::new(
                    format!("expect.count[{index}]"),
                    "Structured event count expectation is well-formed.".to_string(),
                    None,
                )
                .failed("expect.count entries must be tables")
                .build(),
            );
            continue;
        };
        results.push(evaluate_count(index, table, artifact));
    }
}

fn evaluate_count(
    index: usize,
    table: &toml::Table,
    artifact: &RunEvidenceArtifact,
) -> AssertionResultEvidence {
    let id = table
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("expect.count[{index}]"));
    let description = table
        .get("description")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("Structured event count `{id}` matches."));
    let mut builder = ResultBuilder::new(id, description, None);
    match required_assertion(table) {
        Ok(required) => builder = builder.required(required),
        Err(message) => return builder.failed(message).build(),
    }
    let Some(events) = artifact
        .observability
        .as_ref()
        .map(|observability| observability.events.as_slice())
    else {
        return builder
            .missing_fact("run evidence has no captured structured Temper events")
            .build();
    };
    let matcher = match EventMatcher::from_table(table, COUNT_CONTROL_FIELDS, artifact) {
        Ok(matcher) => matcher,
        Err(message) => return builder.failed(message).build(),
    };
    let matched = events
        .iter()
        .filter(|event| matcher.matches(event))
        .collect::<Vec<_>>();
    let constraints = match CountConstraints::from_table(table) {
        Ok(constraints) => constraints,
        Err(message) => return builder.failed(message).build(),
    };
    let group_by = match table.get("group_by") {
        Some(value) => match string_array(value, "group_by") {
            Ok(fields) => fields,
            Err(message) => return builder.failed(message).build(),
        },
        None => Vec::new(),
    };
    if group_by.is_empty() {
        builder = constraints.evaluate(builder, matched.len(), "matching event(s)".to_string());
    } else {
        let mut groups = BTreeMap::<String, usize>::new();
        for event in &matched {
            let key = group_by
                .iter()
                .map(|field| {
                    let key = field_alias(field).unwrap_or(field.as_str());
                    event
                        .fields
                        .get(key)
                        .cloned()
                        .unwrap_or_else(|| format!("<missing:{key}>"))
                })
                .collect::<Vec<_>>()
                .join("|");
            *groups.entry(key).or_default() += 1;
        }
        if groups.is_empty() {
            builder = constraints.evaluate(builder, 0, "matching grouped event(s)".to_string());
        } else {
            for (key, count) in groups {
                builder = constraints.evaluate(builder, count, format!("group `{key}`"));
            }
        }
    }
    builder.build()
}
