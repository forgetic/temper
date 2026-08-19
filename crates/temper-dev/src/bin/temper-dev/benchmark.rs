use std::collections::BTreeSet;

use serde_json::Value;

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
    let expected = [
        ("search_graph", "graph_query"),
        ("search_graph", "graph_query"),
        ("search_code", "pattern"),
        ("trace_path", "function_name"),
        ("get_code_snippet", "qualified_name"),
        ("search_code", "pattern"),
        ("get_code_snippet", "qualified_name"),
    ];
    let observed = trace
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|event| {
            event
                .pointer("/event/event/data/graph_correlation")
                .cloned()
        })
        .collect::<Vec<_>>();
    if observed.len() != expected.len() {
        return Err(format!(
            "enabled trace retained {} typed graph correlations; expected {}",
            observed.len(),
            expected.len()
        ));
    }
    for (record, (tool, target_kind)) in observed.iter().zip(expected) {
        let Some(object) = record.as_object() else {
            return Err("enabled trace retained a non-object graph correlation".to_string());
        };
        let fields = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
        if fields != BTreeSet::from(["target_digest", "target_kind", "tool", "version"])
            || object.get("version").and_then(Value::as_u64) != Some(1)
            || object.get("tool").and_then(Value::as_str) != Some(tool)
            || object.get("target_kind").and_then(Value::as_str) != Some(target_kind)
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
    use super::provider_payload;

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
