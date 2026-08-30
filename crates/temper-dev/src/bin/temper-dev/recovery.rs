use serde_json::Value;

use super::call_has_provider_invocation;

pub(super) fn verify_decision_gap_recovery(trace: &str) -> Result<(), String> {
    const MIXED_DENIAL_GUIDANCE: &str = "decision-evidence recovery required; missing evidence: [trace, implementation, caller, focused_test]; permitted action: targeted_current_root_graph_call; remaining allowance: 4";
    const TRACE_PROGRESS_GUIDANCE: &str = "decision-evidence recovery required; missing evidence: [implementation, caller, focused_test]; permitted action: targeted_current_root_graph_call; remaining allowance: 3";
    let events = trace
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|event| {
            let data = event.pointer("/event/event/data")?;
            data.get("status")?;
            Some((event.pointer("/event/seq")?.as_u64()?, data.clone()))
        })
        .collect::<Vec<_>>();
    let groups: &[&[(&str, &str)]] = &[
        &[("sibling_focused_test_non_progress_one", "succeeded")],
        &[("sibling_focused_test_exhausts_budget", "succeeded")],
        &[
            ("recovery_cross_root_caller_denied", "failed"),
            ("recovery_active_root_trace", "succeeded"),
            ("recovery_cross_root_focused_test_denied", "failed"),
        ],
        &[
            ("recovery_active_root_implementation", "succeeded"),
            ("recovery_active_root_caller", "succeeded"),
            ("recovery_active_root_focused_test", "succeeded"),
            ("recovery_satisfied_trace_denied", "failed"),
        ],
        &[
            ("post_decision_broad_architecture", "failed"),
            ("post_decision_graph_search", "failed"),
            ("post_decision_source_read", "failed"),
        ],
    ];
    let mut previous_max = None;
    for group in groups {
        let mut sequences = Vec::new();
        for (call_id, status) in *group {
            let (seq, data) = events
                .iter()
                .find(|(_, data)| data.get("call_id").and_then(Value::as_str) == Some(*call_id))
                .ok_or_else(|| format!("enabled trace omitted {call_id}"))?;
            if data.get("status").and_then(Value::as_str) != Some(*status) {
                return Err(format!("{call_id} did not finish as {status}"));
            }
            sequences.push(*seq);
        }
        let group_min = sequences
            .iter()
            .copied()
            .min()
            .expect("non-empty event group");
        if previous_max.is_some_and(|previous| group_min <= previous) {
            return Err("decision-gap recovery batches were out of order".to_string());
        }
        previous_max = sequences.into_iter().max();
    }

    let recoverable = serde_json::json!({
        "reason": "recoverable_incomplete_evidence",
        "missing_evidence": ["trace", "implementation", "caller", "focused_test"],
        "permitted_action": "targeted_current_root_graph_call",
        "remaining_allowance": 4,
    });
    for call_id in [
        "recovery_cross_root_caller_denied",
        "recovery_cross_root_focused_test_denied",
    ] {
        let data = events
            .iter()
            .find_map(|(_, data)| {
                (data.get("call_id").and_then(Value::as_str) == Some(call_id)).then_some(data)
            })
            .expect("recovery denial was checked above");
        let failure = data
            .get("failure")
            .ok_or_else(|| format!("{call_id} omitted its recovery diagnostic"))?;
        if failure.get("category").and_then(Value::as_str) != Some("graph_lifecycle_denial")
            || failure.get("reason").and_then(Value::as_str) != Some("decision_evidence_incomplete")
            || failure.get("message").and_then(Value::as_str) != Some(MIXED_DENIAL_GUIDANCE)
            || failure.get("graph_exploration") != Some(&recoverable)
            || call_has_provider_invocation(trace, call_id, None, None)
        {
            return Err(format!(
                "{call_id} did not retain the immutable mixed-root recovery denial"
            ));
        }
    }
    let progressed = serde_json::json!({
        "reason": "recoverable_incomplete_evidence",
        "missing_evidence": ["implementation", "caller", "focused_test"],
        "permitted_action": "targeted_current_root_graph_call",
        "remaining_allowance": 3,
    });
    let progressed_denial = events
        .iter()
        .find_map(|(_, data)| {
            (data.get("call_id").and_then(Value::as_str) == Some("recovery_satisfied_trace_denied"))
                .then_some(data)
        })
        .expect("progress denial was checked above");
    let failure = progressed_denial
        .get("failure")
        .ok_or_else(|| "progress denial omitted its recovery diagnostic".to_string())?;
    if failure.get("category").and_then(Value::as_str) != Some("graph_lifecycle_denial")
        || failure.get("reason").and_then(Value::as_str) != Some("decision_evidence_incomplete")
        || failure.get("message").and_then(Value::as_str) != Some(TRACE_PROGRESS_GUIDANCE)
        || failure.get("graph_exploration") != Some(&progressed)
        || call_has_provider_invocation(trace, "recovery_satisfied_trace_denied", None, None)
    {
        return Err(
            "post-trace denial did not retain the exact three-kind, three-slot diagnostic"
                .to_string(),
        );
    }

    let completed = serde_json::json!({
        "reason": "completed",
        "missing_evidence": [],
        "permitted_action": "conventional_discovery",
        "remaining_allowance": 0,
    });
    for call_id in [
        "post_decision_broad_architecture",
        "post_decision_graph_search",
        "post_decision_source_read",
    ] {
        let data = events
            .iter()
            .find_map(|(_, data)| {
                (data.get("call_id").and_then(Value::as_str) == Some(call_id)).then_some(data)
            })
            .expect("post-completion denial was checked above");
        let failure = data
            .get("failure")
            .ok_or_else(|| format!("{call_id} omitted its completion diagnostic"))?;
        if failure.get("category").and_then(Value::as_str) != Some("graph_lifecycle_denial")
            || failure.get("reason").and_then(Value::as_str) != Some("exploration_closed")
            || failure.get("graph_exploration") != Some(&completed)
            || call_has_provider_invocation(trace, call_id, None, None)
        {
            return Err(format!("{call_id} was not denied locally after completion"));
        }
    }

    if !call_has_provider_invocation(
        trace,
        "recovery_active_root_trace",
        Some(5),
        Some("trace_path"),
    ) {
        return Err("only the active-root trace should reach provider invocation 5".to_string());
    }
    for call_id in [
        "recovery_active_root_implementation",
        "recovery_active_root_caller",
        "recovery_active_root_focused_test",
    ] {
        if !call_has_provider_invocation(trace, call_id, None, Some("get_code_snippet")) {
            return Err(format!(
                "{call_id} did not consume one remaining recovery slot"
            ));
        }
    }
    verify_root_local_lineage(trace)
}

fn verify_root_local_lineage(trace: &str) -> Result<(), String> {
    let routing = call_lineage(trace, "graph_identify_routing_evidence")
        .ok_or_else(|| "routing root omitted typed lineage".to_string())?;
    let behavior = call_lineage(trace, "graph_identify_behavior_evidence")
        .ok_or_else(|| "behavior root omitted typed lineage".to_string())?;
    if routing.get("stage").and_then(Value::as_str) != Some("root")
        || behavior.get("stage").and_then(Value::as_str) != Some("root")
        || routing.get("root_binding") == behavior.get("root_binding")
    {
        return Err("controlled discovery did not establish two distinct typed roots".to_string());
    }
    let routing_root = routing.get("root_binding");
    let behavior_root = behavior.get("root_binding");
    for call_id in [
        "sibling_focused_test_non_progress_one",
        "sibling_focused_test_exhausts_budget",
    ] {
        let lineage = call_lineage(trace, call_id)
            .ok_or_else(|| format!("{call_id} omitted typed lineage"))?;
        if lineage.get("stage").and_then(Value::as_str) != Some("carry_forward")
            || lineage.get("root_binding") != behavior_root
            || lineage
                .get("decision_evidence_kind")
                .and_then(Value::as_str)
                != Some("focused_test")
        {
            return Err(format!("{call_id} was not bound to the behavioral sibling"));
        }
    }
    for (call_id, evidence_kind) in [
        ("recovery_active_root_trace", None),
        (
            "recovery_active_root_implementation",
            Some("implementation"),
        ),
        ("recovery_active_root_caller", Some("caller")),
        ("recovery_active_root_focused_test", Some("focused_test")),
    ] {
        let lineage = call_lineage(trace, call_id)
            .ok_or_else(|| format!("{call_id} omitted typed lineage"))?;
        if lineage.get("stage").and_then(Value::as_str) != Some("carry_forward")
            || lineage.get("root_binding") != routing_root
            || lineage
                .get("decision_evidence_kind")
                .and_then(Value::as_str)
                != evidence_kind
        {
            return Err(format!(
                "{call_id} was not bound to the active routing root"
            ));
        }
    }
    Ok(())
}

fn call_lineage(trace: &str, call_id: &str) -> Option<Value> {
    trace
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find_map(|event| {
            let data = event.pointer("/event/event/data")?;
            (data.get("call_id").and_then(Value::as_str) == Some(call_id))
                .then(|| data.get("decision_anchor_lineage").cloned())
                .flatten()
        })
}
