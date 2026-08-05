// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::Value as JsonValue;
use temper_testing::live_manifest::LiveManifestEvidence;

use crate::run_evidence;

pub(super) fn capture_observability(
    evidence: &LiveManifestEvidence,
    standalone_logs: &[&Path],
) -> run_evidence::ObservabilityEvidence {
    let events = capture_structured_events(evidence, standalone_logs);
    run_evidence::ObservabilityEvidence {
        scenario_run_id: evidence.scenario_run_id.clone(),
        log_format: evidence.temper_log_format.clone(),
        rust_log: evidence.rust_log.clone(),
        event_log_path: standalone_logs
            .last()
            .map(|path| path.display().to_string())
            .unwrap_or_default(),
        event_log_paths: standalone_logs
            .iter()
            .map(|path| path.display().to_string())
            .collect(),
        captured_events: events.len(),
        events,
    }
}

fn capture_structured_events(
    evidence: &LiveManifestEvidence,
    standalone_logs: &[&Path],
) -> Vec<run_evidence::StructuredEventEvidence> {
    let source_issue_ref = format!(
        "{}#{}",
        evidence.repo_slug, evidence.final_state.issue.number
    );
    let implementation_pr_ref = format!(
        "{} PR#{}",
        evidence.repo_slug, evidence.final_state.pull_request.number
    );
    let mut records = Vec::new();
    for (source_index, standalone_log) in standalone_logs.iter().enumerate() {
        let Ok(contents) = fs::read_to_string(standalone_log) else {
            continue;
        };
        for (line_index, line) in contents.lines().enumerate() {
            let Ok(record) = serde_json::from_str::<JsonValue>(line) else {
                continue;
            };
            let Some(fields) = record.get("fields").and_then(JsonValue::as_object) else {
                continue;
            };
            // Measurements are structured evidence too. Preserve their stable
            // name as the event selector when the producer has no `event` field.
            let Some(event) = fields
                .get("event")
                .or_else(|| fields.get("measurement"))
                .and_then(JsonValue::as_str)
            else {
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
            let timestamp = record
                .get("timestamp")
                .and_then(JsonValue::as_str)
                .unwrap_or_default()
                .to_string();
            records.push((
                timestamp.clone(),
                source_index,
                line_index,
                run_evidence::StructuredEventEvidence {
                    sequence: 0,
                    timestamp: timestamp.clone(),
                    event: event.to_string(),
                    service: fact_fields.get("service").cloned(),
                    target: record
                        .get("target")
                        .and_then(JsonValue::as_str)
                        .map(str::to_string),
                    fields: fact_fields,
                },
            ));
        }
    }
    if let Some(history) = &evidence.terminal_history {
        let fields = BTreeMap::from([
            ("event".to_string(), "history.seeded".to_string()),
            (
                "scenario.run_id".to_string(),
                evidence.scenario_run_id.clone(),
            ),
            (
                "history.actionable_issue_number".to_string(),
                history.actionable_issue_number.to_string(),
            ),
            (
                "history.actionable_pull_request_number".to_string(),
                history.actionable_pull_request_number.to_string(),
            ),
            (
                "history.first_irrelevant_pull_request_number".to_string(),
                history.first_history_pull_request_number.to_string(),
            ),
            (
                "history.target_closed_issues".to_string(),
                history.target_closed_issues.to_string(),
            ),
            (
                "history.target_closed_pull_requests".to_string(),
                history.target_closed_pull_requests.to_string(),
            ),
            (
                "history.sibling_repo".to_string(),
                history.sibling_repo_slug.clone(),
            ),
            (
                "history.sibling_closed_issues".to_string(),
                history.sibling_closed_issues.to_string(),
            ),
            (
                "history.webhook_delivery".to_string(),
                history.webhook_delivery.clone(),
            ),
            (
                "history.actionable_older_than_history".to_string(),
                history.actionable_older_than_history.to_string(),
            ),
            (
                "history.actionable_recovered".to_string(),
                history.actionable_recovered.to_string(),
            ),
            (
                "history.cold_authority_rebuilt".to_string(),
                history.cold_authority_rebuilt.to_string(),
            ),
        ]);
        records.push((
            String::new(),
            standalone_logs.len(),
            0,
            run_evidence::StructuredEventEvidence {
                sequence: 0,
                timestamp: String::new(),
                event: "history.seeded".to_string(),
                service: Some("scenario-harness".to_string()),
                target: Some("temper::scenario".to_string()),
                fields,
            },
        ));
    }
    records.sort_by(|left, right| (&left.0, left.1, left.2).cmp(&(&right.0, right.1, right.2)));
    records
        .into_iter()
        .enumerate()
        .map(|(index, (_, _, _, mut event))| {
            event.sequence = index + 1;
            event
        })
        .collect()
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
