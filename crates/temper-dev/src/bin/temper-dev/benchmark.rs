use std::collections::{BTreeMap, BTreeSet};

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

pub(super) fn verify_post_source_exact_read_recovery(trace: &str) -> Result<(), String> {
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
        ("read_route_before_source_chain", "read", "succeeded"),
        (
            "recovery_active_root_implementation",
            "codebase_memory_get_code_snippet",
            "succeeded",
        ),
        (
            "patch_retry_affinity_before_exact_read",
            "apply_patch",
            "failed",
        ),
        ("read_route_after_source_chain", "read", "succeeded"),
        ("patch_retry_affinity", "apply_patch", "succeeded"),
    ];
    let mut previous_seq = None;
    for (call_id, tool, status) in expected {
        let (seq, data) = events
            .iter()
            .find(|(_, data)| data.get("call_id").and_then(Value::as_str) == Some(call_id))
            .ok_or_else(|| format!("enabled trace omitted {call_id}"))?;
        if previous_seq.is_some_and(|previous| *seq <= previous) {
            return Err("post-source exact-read recovery events were out of order".to_string());
        }
        previous_seq = Some(*seq);
        if data.get("name").and_then(Value::as_str) != Some(tool) {
            return Err(format!("{call_id} did not finish as {tool}"));
        }
        if data.get("status").and_then(Value::as_str) != Some(status) {
            return Err(format!("{call_id} did not finish as {status}"));
        }
    }

    let denial = events
        .iter()
        .find_map(|(_, data)| {
            (data.get("call_id").and_then(Value::as_str)
                == Some("patch_retry_affinity_before_exact_read"))
            .then_some(data)
        })
        .expect("denied mutation was checked above")
        .get("failure")
        .and_then(Value::as_object)
        .ok_or_else(|| "denied mutation omitted its typed diagnostic".to_string())?;
    let denial_fields = denial.keys().map(String::as_str).collect::<BTreeSet<_>>();
    if denial_fields
        != BTreeSet::from([
            "category",
            "fallback_to_conventional_discovery",
            "message",
            "reason",
            "retry_disposition",
            "retryable",
        ])
        || denial.get("category").and_then(Value::as_str) != Some("policy_denial")
        || denial.get("reason").and_then(Value::as_str) != Some("policy_precondition")
    {
        return Err("denied mutation retained a non-canonical diagnostic".to_string());
    }

    let denied_start = trace
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find_map(|event| {
            let data = event.pointer("/event/event/data")?;
            (event.pointer("/event/event/type").and_then(Value::as_str) == Some("tool.started")
                && data.get("call_id").and_then(Value::as_str)
                    == Some("patch_retry_affinity_before_exact_read"))
            .then_some(data.clone())
        })
        .ok_or_else(|| "denied mutation omitted its start event".to_string())?;
    if denied_start.get("arguments").is_some() {
        return Err("denied mutation retained private arguments".to_string());
    }
    Ok(())
}

pub(super) fn verify_safe_converged_decision_evidence(run: &Value) -> Result<(), String> {
    let public_summary = serde_json::to_string(run)
        .map_err(|error| format!("serialize controlled run summary: {error}"))?;
    for private in [
        "provider_invocation",
        "provider_tool",
        "requested_stable_project",
        "root_binding",
    ] {
        if public_summary.contains(private) {
            return Err("enabled run summary retained private provider or root state".to_string());
        }
    }
    let evidence = run
        .pointer("/metrics/graph/decision_evidence")
        .and_then(Value::as_array)
        .ok_or_else(|| "enabled run omitted graph decision evidence".to_string())?;
    let expected = BTreeMap::from([
        (("search_graph", "trace_path", "graph"), 1_u64),
        (("search_graph", "get_code_snippet", "source"), 1),
        (("trace_path", "get_code_snippet", "source"), 1),
        (("get_code_snippet", "read", "selection"), 1),
    ]);
    let expected_count = expected.values().sum::<u64>() as usize;
    if evidence.len() != expected_count {
        return Err(format!(
            "enabled decision evidence count was {}; expected {}",
            evidence.len(),
            expected_count
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
    let excluded_consumers = [
        "read_route_before_source_chain",
        "patch_retry_affinity_before_exact_read",
        "recovery_cross_root_caller_denied",
        "recovery_cross_root_focused_test_denied",
        "recovery_satisfied_trace_denied",
        "post_decision_source_read",
    ];
    let mut observed = BTreeMap::new();
    for entry in evidence {
        let object = entry
            .as_object()
            .ok_or_else(|| "enabled decision evidence entry was not an object".to_string())?;
        let fields = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
        if fields != expected_fields {
            return Err("enabled decision evidence retained unexpected fields".to_string());
        }
        if ["graph_call_id", "consumer_call_id"]
            .into_iter()
            .any(|field| {
                object
                    .get(field)
                    .and_then(Value::as_str)
                    .is_some_and(|call_id| excluded_consumers.contains(&call_id))
            })
        {
            return Err(
                "pre-source or locally denied actions received decision relevance credit"
                    .to_string(),
            );
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
        *observed.entry(tuple).or_insert(0) += 1;
    }
    if observed != expected {
        return Err(
            "enabled decision evidence did not preserve the converged root forest".to_string(),
        );
    }
    let selection = evidence
        .iter()
        .filter(|entry| entry.get("consumption_mode").and_then(Value::as_str) == Some("selection"))
        .collect::<Vec<_>>();
    if selection.len() != 1 {
        return Err(format!(
            "enabled decision evidence retained {} exact source selections; expected 1",
            selection.len()
        ));
    }
    let selection = selection[0];
    if selection.get("graph_call_id").and_then(Value::as_str)
        != Some("recovery_active_root_implementation")
        || selection.get("graph_tool").and_then(Value::as_str) != Some("get_code_snippet")
        || selection.get("consumer_call_id").and_then(Value::as_str)
            != Some("read_route_after_source_chain")
        || selection.get("consumer_tool").and_then(Value::as_str) != Some("read")
        || selection.get("target").and_then(Value::as_str) != Some("repo/src/route.rs")
        || selection.get("kind").and_then(Value::as_str) != Some("implementation")
    {
        return Err("enabled decision evidence omitted the exact source selection".to_string());
    }
    Ok(())
}

mod recovery;

pub(super) fn verify_decision_gap_recovery(trace: &str) -> Result<(), String> {
    recovery::verify_decision_gap_recovery(trace)
}

pub(super) fn verify_typed_graph_correlation_records(trace: &str) -> Result<(), String> {
    let expected = std::collections::BTreeMap::from([
        (("search_graph", "graph_query"), 2_u64),
        (("trace_path", "function_name"), 1),
        (("get_code_snippet", "qualified_name"), 5),
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
    if observed.len() != 8 {
        return Err(format!(
            "enabled trace retained {} typed graph correlations; expected 8",
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

pub(super) fn verify_provider_invocations(trace: &str) -> Result<(), String> {
    let mut invocations = BTreeMap::<u64, BTreeSet<String>>::new();
    for event in trace
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
    {
        if let Some((invocation, tool)) = provider_invocation_record(&event) {
            invocations
                .entry(invocation)
                .or_default()
                .insert(tool.to_string());
        }
    }
    let expected_keys = (1..=8).collect::<BTreeSet<_>>();
    if invocations.keys().copied().collect::<BTreeSet<_>>() != expected_keys {
        return Err(format!(
            "provider invocation sequence was {:?}; expected {expected_keys:?}",
            invocations.keys().collect::<Vec<_>>()
        ));
    }
    for (invocation, expected_tool) in [
        (1, "search_graph"),
        (2, "search_graph"),
        (3, "get_code_snippet"),
        (4, "get_code_snippet"),
        (5, "trace_path"),
        (6, "get_code_snippet"),
        (7, "get_code_snippet"),
        (8, "get_code_snippet"),
    ] {
        if invocations.get(&invocation) != Some(&BTreeSet::from([expected_tool.to_string()])) {
            return Err(format!(
                "provider invocation {invocation} was not the expected {expected_tool} call"
            ));
        }
    }
    for call_id in [
        "recovery_cross_root_caller_denied",
        "recovery_cross_root_focused_test_denied",
        "recovery_satisfied_trace_denied",
        "post_decision_broad_architecture",
        "post_decision_graph_search",
        "post_decision_source_read",
    ] {
        if call_has_provider_invocation(trace, call_id, None, None) {
            return Err(format!(
                "locally denied {call_id} reached the private provider"
            ));
        }
    }
    Ok(())
}

pub(super) fn verify_structured_readiness(trace: &str) -> Result<(), String> {
    for expected in [1, 2] {
        let ready = trace
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .any(|record| {
                if provider_invocation_record(&record) != Some((expected, "search_graph")) {
                    return false;
                }
                record
                    .pointer("/record/model_result_text/text")
                    .and_then(Value::as_str)
                    .and_then(provider_payload)
                    .is_some_and(|payload| {
                        payload.get("project").and_then(Value::as_str)
                            == Some("temper-benchmark-codebase-memory-routing-repair")
                            && payload
                                .get("cold_stable_upsert_ready")
                                .and_then(Value::as_bool)
                                == Some(expected == 1)
                            && payload
                                .get("warm_stable_project_ready")
                                .and_then(Value::as_bool)
                                == Some(expected == 2)
                    })
            });
        if !ready {
            return Err(format!(
                "provider invocation {expected} omitted structured readiness"
            ));
        }
    }
    Ok(())
}

fn call_has_provider_invocation(
    trace: &str,
    call_id: &str,
    invocation: Option<u64>,
    tool: Option<&str>,
) -> bool {
    trace
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .any(|record| {
            record.pointer("/record/call_id").and_then(Value::as_str) == Some(call_id)
                && provider_invocation_record(&record).is_some_and(|(observed, observed_tool)| {
                    invocation.is_none_or(|expected| observed == expected)
                        && tool.is_none_or(|expected| observed_tool == expected)
                })
        })
}

// Use the operator transcript's tool identity. Unknown provider text fields
// are intentionally removed before source presentation; the numeric fixture
// invocation counter survives and still proves whether a denied call ran.
fn provider_invocation_record(value: &Value) -> Option<(u64, &str)> {
    let record = value.get("record")?;
    let tool = record
        .get("tool_name")?
        .as_str()?
        .strip_prefix("codebase_memory_")?;
    let result = record.get("model_result_text")?;
    if result.get("truncated")?.as_bool()? {
        return None;
    }
    let payload = provider_payload(result.get("text")?.as_str()?)?;
    Some((payload.get("provider_invocation")?.as_u64()?, tool))
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
            ("get_code_snippet", "qualified_name"),
            ("search_graph", "graph_query"),
            ("get_code_snippet", "qualified_name"),
            ("get_code_snippet", "qualified_name"),
            ("get_code_snippet", "qualified_name"),
            ("get_code_snippet", "qualified_name"),
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
    let path = match symbol {
        "worker_slot" => "src/route.rs",
        "DeliveryRouter::worker_for" => "src/delivery.rs",
        "alias_retries_stay_on_the_original_ordered_worker" => "tests/alias_retry.rs",
        "public_facade_keeps_operational_helpers_cohesive" => "tests/public_api.rs",
        _ => return false,
    };
    // Check the actual fixture bytes after Temper's current-checkout source
    // guard, rather than a provider-supplied source_root text assertion.
    // The independent trace check above still proves normalized project binding.
    let root = std::path::Path::new(
        "benchmarks/agent-sessions/codebase-memory-routing-repair/fixture/repo",
    );
    let Ok(source) = std::fs::read_to_string(root.join(path)) else {
        return false;
    };
    payload.get("project").and_then(Value::as_str)
        == Some("temper-benchmark-codebase-memory-routing-repair")
        && payload.get("qualified_name").and_then(Value::as_str) == Some(symbol)
        && payload.get("file_path").and_then(Value::as_str) == Some(path)
        && payload.get("source").and_then(Value::as_str) == Some(source.as_str())
}
