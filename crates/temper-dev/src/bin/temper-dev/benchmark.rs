use std::collections::BTreeSet;

use serde_json::Value;

pub(super) fn verify_safe_five_call_decision_evidence(run: &Value) -> Result<(), String> {
    let evidence = run
        .pointer("/metrics/graph/decision_evidence")
        .and_then(Value::as_array)
        .ok_or_else(|| "enabled run omitted graph decision evidence".to_string())?;
    let expected = [
        ("search_graph", "search_code", "graph"),
        ("search_code", "trace_path", "graph"),
        ("trace_path", "get_code_snippet", "source"),
        ("get_code_snippet", "get_code_snippet", "source"),
        ("get_code_snippet", "read", "selection"),
    ];
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
    for (entry, (graph_tool, consumer_tool, mode)) in evidence.iter().zip(expected) {
        let object = entry
            .as_object()
            .ok_or_else(|| "enabled decision evidence entry was not an object".to_string())?;
        let fields = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
        if fields != expected_fields {
            return Err("enabled decision evidence retained unexpected fields".to_string());
        }
        if object.get("graph_tool").and_then(Value::as_str) != Some(graph_tool)
            || object.get("consumer_tool").and_then(Value::as_str) != Some(consumer_tool)
            || object.get("consumption_mode").and_then(Value::as_str) != Some(mode)
        {
            return Err(
                "enabled decision evidence did not preserve the five-call chain".to_string(),
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

fn value_has_confirmed_graph_read(value: &Value) -> bool {
    match value {
        Value::String(text) => serde_json::from_str::<Value>(text)
            .ok()
            .is_some_and(|payload| confirmed_graph_read_payload(&payload)),
        Value::Array(values) => values.iter().any(value_has_confirmed_graph_read),
        Value::Object(values) => values.values().any(value_has_confirmed_graph_read),
        _ => false,
    }
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
        Value::String(text) => serde_json::from_str::<Value>(text)
            .ok()
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

fn confirmed_current_root_source_payload(payload: &Value, symbol: &str) -> bool {
    payload.get("source_root").and_then(Value::as_str) == Some("confirmed_current_root")
        && payload.get("symbol").and_then(Value::as_str) == Some(symbol)
        && payload
            .get("source_path")
            .and_then(Value::as_str)
            .is_some_and(|path| path.starts_with("src/"))
        && payload.get("source").and_then(Value::as_str).is_some()
        && confirmed_graph_read_payload(payload)
}
