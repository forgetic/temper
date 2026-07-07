// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::Value as JsonValue;
use temper_testing::live_basic_delivery::LiveBasicDeliveryEvidence;

use crate::run_evidence;

pub(super) fn capture_observability(
    evidence: &LiveBasicDeliveryEvidence,
    standalone_log: &Path,
) -> run_evidence::ObservabilityEvidence {
    let events = capture_structured_events(evidence, standalone_log);
    run_evidence::ObservabilityEvidence {
        scenario_run_id: evidence.scenario_run_id.clone(),
        log_format: evidence.temper_log_format.clone(),
        rust_log: evidence.rust_log.clone(),
        event_log_path: standalone_log.display().to_string(),
        captured_events: events.len(),
        events,
    }
}

fn capture_structured_events(
    evidence: &LiveBasicDeliveryEvidence,
    standalone_log: &Path,
) -> Vec<run_evidence::StructuredEventEvidence> {
    let Ok(contents) = fs::read_to_string(standalone_log) else {
        return Vec::new();
    };
    let source_issue_ref = format!(
        "{}#{}",
        evidence.repo_slug, evidence.final_state.issue.number
    );
    let implementation_pr_ref = format!(
        "{} PR#{}",
        evidence.repo_slug, evidence.final_state.pull_request.number
    );
    let mut events = Vec::new();
    for line in contents.lines() {
        let Ok(record) = serde_json::from_str::<JsonValue>(line) else {
            continue;
        };
        let Some(fields) = record.get("fields").and_then(JsonValue::as_object) else {
            continue;
        };
        let Some(event) = fields.get("event").and_then(JsonValue::as_str) else {
            continue;
        };
        let mut fact_fields = BTreeMap::new();
        fact_fields.insert("event".to_string(), event.to_string());
        fact_fields.insert(
            "scenario.run_id".to_string(),
            evidence.scenario_run_id.clone(),
        );
        for (key, value) in fields {
            fact_fields.insert(key.clone(), json_value_to_field(value));
        }
        collect_span_fields(&record, &mut fact_fields);
        enrich_event_fields(
            &mut fact_fields,
            &source_issue_ref,
            &implementation_pr_ref,
            evidence.final_state.issue.number,
        );
        events.push(run_evidence::StructuredEventEvidence {
            sequence: events.len() + 1,
            event: event.to_string(),
            service: fact_fields.get("service").cloned(),
            target: record
                .get("target")
                .and_then(JsonValue::as_str)
                .map(str::to_string),
            fields: fact_fields,
        });
    }
    events
}

fn collect_span_fields(record: &JsonValue, fields: &mut BTreeMap<String, String>) {
    if let Some(span) = record.get("span").and_then(JsonValue::as_object) {
        for (key, value) in span {
            if key != "name" {
                fields
                    .entry(key.clone())
                    .or_insert_with(|| json_value_to_field(value));
            }
        }
    }
    if let Some(spans) = record.get("spans").and_then(JsonValue::as_array) {
        for span in spans.iter().filter_map(JsonValue::as_object) {
            for (key, value) in span {
                if key != "name" {
                    fields
                        .entry(key.clone())
                        .or_insert_with(|| json_value_to_field(value));
                }
            }
        }
    }
}

fn enrich_event_fields(
    fields: &mut BTreeMap<String, String>,
    source_issue_ref: &str,
    implementation_pr_ref: &str,
    issue_number: u64,
) {
    if fields
        .get("for_issue")
        .is_some_and(|for_issue| for_issue == &issue_number.to_string())
    {
        fields
            .entry("source_artifact".to_string())
            .or_insert_with(|| source_issue_ref.to_string());
    }
    if fields
        .get("pr.ref")
        .is_some_and(|pr_ref| pr_ref == implementation_pr_ref)
    {
        fields
            .entry("source_artifact".to_string())
            .or_insert_with(|| source_issue_ref.to_string());
    }
    if fields
        .get("artifact.ref")
        .is_some_and(|artifact_ref| artifact_ref == source_issue_ref)
    {
        fields
            .entry("source_artifact".to_string())
            .or_insert_with(|| source_issue_ref.to_string());
    }
}

fn json_value_to_field(value: &JsonValue) -> String {
    match value {
        JsonValue::Null => "null".to_string(),
        JsonValue::Bool(value) => value.to_string(),
        JsonValue::Number(value) => value.to_string(),
        JsonValue::String(value) => value.clone(),
        JsonValue::Array(_) | JsonValue::Object(_) => value.to_string(),
    }
}
