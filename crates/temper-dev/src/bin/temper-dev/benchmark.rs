use std::collections::BTreeSet;

use serde_json::Value;

pub(super) fn verify_ordinary_failure_recovery(trace: &str) -> Result<(), String> {
    let events = trace
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|event| {
            let data = event.pointer("/event/event/data")?;
            data.get("status")?;
            Some((event.pointer("/event/seq")?.as_u64()?, data.clone()))
        })
        .collect::<Vec<_>>();
    let expected = [
        (
            "ordinary_failure_initial",
            "failed",
            Some(("execution_failure", "tool_reported_failure")),
        ),
        (
            "ordinary_failure_repeated",
            "failed",
            Some(("circuit_redirect", "repeated_non_retryable")),
        ),
        ("ordinary_failure_recovery", "succeeded", None),
    ];
    let mut previous_seq = None;
    for (call_id, status, failure) in expected {
        let (seq, data) = events
            .iter()
            .find(|(_, data)| data.get("call_id").and_then(Value::as_str) == Some(call_id))
            .ok_or_else(|| format!("controlled trace omitted {call_id}"))?;
        if previous_seq.is_some_and(|previous| *seq <= previous) {
            return Err("ordinary failure/recovery events were out of order".to_string());
        }
        previous_seq = Some(*seq);
        if data.get("status").and_then(Value::as_str) != Some(status) {
            return Err(format!("{call_id} did not finish as {status}"));
        }
        match failure {
            Some((category, reason)) => {
                let diagnostic = data
                    .get("failure")
                    .and_then(Value::as_object)
                    .ok_or_else(|| format!("{call_id} omitted its typed diagnostic"))?;
                let fields = diagnostic
                    .keys()
                    .map(String::as_str)
                    .collect::<BTreeSet<_>>();
                if fields
                    != BTreeSet::from([
                        "category",
                        "fallback_to_conventional_discovery",
                        "message",
                        "reason",
                        "retry_disposition",
                        "retryable",
                    ])
                    || diagnostic.get("category").and_then(Value::as_str) != Some(category)
                    || diagnostic.get("reason").and_then(Value::as_str) != Some(reason)
                {
                    return Err(format!("{call_id} retained a non-canonical diagnostic"));
                }
            }
            None if data.get("failure").is_some() => {
                return Err("corrected ordinary call unexpectedly retained a failure".to_string());
            }
            None => {}
        }
    }
    let redirects = events
        .iter()
        .filter(|(_, data)| {
            data.pointer("/failure/reason").and_then(Value::as_str)
                == Some("repeated_non_retryable")
        })
        .count();
    if redirects != 1 {
        return Err(format!(
            "ordinary failure sequence retained {redirects} redirects; expected 1"
        ));
    }
    Ok(())
}

pub(super) fn verify_safe_converged_decision_evidence(run: &Value) -> Result<(), String> {
    let evidence = run
        .pointer("/metrics/graph/decision_evidence")
        .and_then(Value::as_array)
        .ok_or_else(|| "enabled run omitted graph decision evidence".to_string())?;
    let expected = BTreeSet::from([
        ("search_graph", "search_code", "graph"),
        ("search_graph", "get_code_snippet", "source"),
        ("search_code", "search_code", "graph"),
        ("trace_path", "search_code", "graph"),
        ("search_code", "get_code_snippet", "source"),
        ("get_code_snippet", "read", "selection"),
    ]);
    if evidence.len() != expected.len() {
        return Err(format!(
            "enabled decision evidence count was {}; expected {}",
            evidence.len(),
            expected.len()
        ));
    }
    let expected_fields = BTreeSet::from([
        "consumer_call_id",
        "consumer_start_seq",
        "consumer_tool",
        "consumption_mode",
        "graph_call_id",
        "graph_finish_seq",
        "graph_tool",
        "kind",
        "target",
    ]);
    let mut observed = BTreeSet::new();
    for entry in evidence {
        let object = entry
            .as_object()
            .ok_or_else(|| "enabled decision evidence entry was not an object".to_string())?;
        let fields = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
        if fields != expected_fields {
            return Err("enabled decision evidence retained unexpected fields".to_string());
        }
        let tuple = (
            object
                .get("graph_tool")
                .and_then(Value::as_str)
                .ok_or_else(|| "enabled decision evidence omitted graph_tool".to_string())?,
            object
                .get("consumer_tool")
                .and_then(Value::as_str)
                .ok_or_else(|| "enabled decision evidence omitted consumer_tool".to_string())?,
            object
                .get("consumption_mode")
                .and_then(Value::as_str)
                .ok_or_else(|| "enabled decision evidence omitted consumption_mode".to_string())?,
        );
        observed.insert(tuple);
    }
    if observed != expected {
        return Err(
            "enabled decision evidence did not preserve the converged root forest".to_string(),
        );
    }
    Ok(())
}

pub(super) fn verify_typed_graph_correlation_records(trace: &str) -> Result<(), String> {
    let expected = std::collections::BTreeMap::from([
        (("search_graph", "graph_query"), 2_u64),
        (("search_code", "pattern"), 2),
        (("trace_path", "function_name"), 1),
        (("get_code_snippet", "qualified_name"), 2),
    ]);
    let observed = trace
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|event| {
            event
                .pointer("/event/event/data/graph_correlation")
                .cloned()
        })
        .collect::<Vec<_>>();
    if observed.len() != 7 {
        return Err(format!(
            "enabled trace retained {} typed graph correlations; expected 7",
            observed.len()
        ));
    }
    let mut observed_counts = std::collections::BTreeMap::new();
    for record in &observed {
        let Some(object) = record.as_object() else {
            return Err("enabled trace retained a non-object graph correlation".to_string());
        };
        let fields = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
        if fields != BTreeSet::from(["target_digest", "target_kind", "tool", "version"])
            || object.get("version").and_then(Value::as_u64) != Some(1)
            || !object
                .get("target_digest")
                .and_then(Value::as_str)
                .is_some_and(|digest| {
                    digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
        {
            return Err(
                "enabled trace did not retain only complete typed graph correlations".to_string(),
            );
        }
        let Some(tool) = object.get("tool").and_then(Value::as_str) else {
            return Err("enabled typed graph correlation omitted tool".to_string());
        };
        let Some(target_kind) = object.get("target_kind").and_then(Value::as_str) else {
            return Err("enabled typed graph correlation omitted target_kind".to_string());
        };
        *observed_counts.entry((tool, target_kind)).or_insert(0) += 1;
    }
    if observed_counts != expected {
        return Err(format!(
            "enabled typed graph correlations were {observed_counts:?}; expected {expected:?}"
        ));
    }
    Ok(())
}

pub(super) fn trace_has_confirmed_graph_read(trace: &str) -> bool {
    trace
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .any(|event| value_has_confirmed_graph_read(&event))
}

pub(super) fn verify_provider_invocations(trace: &str, expected: u64) -> Result<(), String> {
    let mut invocations = BTreeSet::new();
    for event in trace
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
    {
        collect_provider_invocations(&event, &mut invocations);
    }
    let expected = (1..=expected).collect::<BTreeSet<_>>();
    if invocations == expected {
        Ok(())
    } else {
        Err(format!(
            "provider invocation sequence was {invocations:?}; expected {expected:?}"
        ))
    }
}

fn collect_provider_invocations(value: &Value, invocations: &mut BTreeSet<u64>) {
    match value {
        Value::String(text) => {
            if let Some(invocation) = provider_payload(text)
                .and_then(|payload| payload.get("provider_invocation").and_then(Value::as_u64))
            {
                invocations.insert(invocation);
            }
        }
        Value::Array(values) => values
            .iter()
            .for_each(|value| collect_provider_invocations(value, invocations)),
        Value::Object(values) => values
            .values()
            .for_each(|value| collect_provider_invocations(value, invocations)),
        _ => {}
    }
}

fn value_has_confirmed_graph_read(value: &Value) -> bool {
    match value {
        Value::String(text) => {
            provider_payload(text).is_some_and(|payload| confirmed_graph_read_payload(&payload))
        }
        Value::Array(values) => values.iter().any(value_has_confirmed_graph_read),
        Value::Object(values) => values.values().any(value_has_confirmed_graph_read),
        _ => false,
    }
}

fn provider_payload(text: &str) -> Option<Value> {
    let mut values = serde_json::Deserializer::from_str(text).into_iter::<Value>();
    let payload = values.next()?.ok()?;
    let suffix = text.get(values.byte_offset()..)?.trim();
    (suffix.is_empty() || (suffix.starts_with("[Decision anchor:") && suffix.ends_with(']')))
        .then_some(payload)
}

fn confirmed_graph_read_payload(payload: &Value) -> bool {
    let requested = payload
        .get("requested_stable_project")
        .and_then(Value::as_str);
    requested.is_some_and(|requested| {
        requested.starts_with("temper-v1-")
            && requested != "temper-benchmark-codebase-memory-routing-repair"
    }) && payload.get("project_route").and_then(Value::as_str) == Some("confirmed_identity")
        && payload.get("confirmed_project").and_then(Value::as_str)
            == Some("temper-benchmark-codebase-memory-routing-repair")
        && payload.get("graph_read_project") == payload.get("confirmed_project")
}

pub(super) fn trace_has_confirmed_current_root_source(trace: &str, symbol: &str) -> bool {
    trace
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .any(|event| value_has_confirmed_current_root_source(&event, symbol))
}

fn value_has_confirmed_current_root_source(value: &Value, symbol: &str) -> bool {
    match value {
        Value::String(text) => provider_payload(text)
            .is_some_and(|payload| confirmed_current_root_source_payload(&payload, symbol)),
        Value::Array(values) => values
            .iter()
            .any(|value| value_has_confirmed_current_root_source(value, symbol)),
        Value::Object(values) => values
            .values()
            .any(|value| value_has_confirmed_current_root_source(value, symbol)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{provider_payload, verify_typed_graph_correlation_records};

    fn correlation_event(tool: &str, target_kind: &str) -> String {
        serde_json::json!({
            "event": {
                "event": {
                    "data": {
                        "graph_correlation": {
                            "version": 1,
                            "tool": tool,
                            "target_kind": target_kind,
                            "target_digest": "a".repeat(64),
                        }
                    }
                }
            }
        })
        .to_string()
    }

    #[test]
    fn typed_graph_correlations_allow_parallel_completion_order() {
        let records = [
            ("search_graph", "graph_query"),
            ("trace_path", "function_name"),
            ("search_code", "pattern"),
            ("get_code_snippet", "qualified_name"),
            ("search_graph", "graph_query"),
            ("get_code_snippet", "qualified_name"),
            ("search_code", "pattern"),
        ]
        .map(|(tool, target_kind)| correlation_event(tool, target_kind))
        .join("\n");

        verify_typed_graph_correlation_records(&records).unwrap();
    }

    #[test]
    fn provider_payload_accepts_only_plain_or_decision_anchored_json() {
        let payload = r#"{"project_route":"confirmed_identity"}"#;
        assert!(provider_payload(payload).is_some());
        assert!(
            provider_payload(&format!(
                "{payload}\n\n[Decision anchor: bounded successful result.]"
            ))
            .is_some()
        );
        assert!(provider_payload(&format!("{payload}\nnot an anchor")).is_none());
    }
}

fn confirmed_current_root_source_payload(payload: &Value, symbol: &str) -> bool {
    payload.get("source_root").and_then(Value::as_str) == Some("confirmed_current_root")
        && payload.get("qualified_name").and_then(Value::as_str) == Some(symbol)
        && payload
            .get("source_path")
            .and_then(Value::as_str)
            .is_some_and(|path| path.starts_with("src/") || path.starts_with("tests/"))
        && payload.get("source").and_then(Value::as_str).is_some()
        && confirmed_graph_read_payload(payload)
}
