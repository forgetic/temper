//! Closed published facts, derived only after correlated private checks pass.
use std::collections::BTreeMap;
use std::fs;

use serde_json::{Value, json};

use super::super::FakeMcpServer;
use super::processes::{Cohort, Identity, event};
use crate::live_manifest::LiveCodebaseMemoryEvidence;

pub(super) const CHECKPOINTS: &[&str] = &[
    "cold_parent_bootstrap",
    "continuous_discovery_serving_admission",
    "two_distinct_overlapping_attempts",
    "real_forgejo_ownership_revoked",
    "a_production_containment_complete",
    "b_same_attempt_daemon_work_survived",
    "b_exact_source_consumed_after_cleanup",
    "late_a_has_no_authority",
    "b_host_ci_merged_closed",
    "last_session_natural_cleanup_before_teardown",
];

pub(super) fn survivor_source(
    rows: &[Value],
    cohort: &Cohort,
    frontend: &Identity,
    prepared_root: &str,
    after: u64,
) -> Result<(), String> {
    let graph = event(rows, "graph_result");
    if graph.len() != 5 {
        return Err("B did not complete the five bounded graph requests".into());
    }
    let session = frontend.pid.to_string();
    let expected = [
        "search_graph",
        "get_code_snippet",
        "trace_path",
        "get_code_snippet",
        "get_code_snippet",
    ];
    for (row, tool) in graph.iter().zip(expected) {
        let work: Identity = serde_json::from_value(row["worker"].clone())
            .map_err(|_| "invalid graph work identity")?;
        let root = row["root"].as_str().ok_or("missing graph root")?;
        let returned_frontend: Identity = serde_json::from_value(row["frontend"].clone())
            .map_err(|_| "missing exact result frontend")?;
        if returned_frontend != *frontend
            || row["session"] != session
            || row["tool"] != tool
            || row["at"].as_u64().is_none_or(|at| at <= after)
            || work != cohort.work
            || root != prepared_root
        {
            return Err("B post-cancel result lost exact frontend/work/checkout binding".into());
        }
    }
    let source = &graph[1];
    if source["selector"] != "fixture::retry_worker_topic"
        || source["source"]
            != include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../scenarios/shared-codebase-memory-lifecycle/repo/src/lib.rs"
            ))
    {
        return Err("B source sentinel differs from its exact checkout".into());
    }
    Ok(())
}

pub(super) fn publish(
    mcp: &FakeMcpServer,
    rows: &[Value],
) -> Result<LiveCodebaseMemoryEvidence, String> {
    let path = mcp
        .log_path
        .with_file_name("shared-lifecycle-aggregate.jsonl");
    let safe = CHECKPOINTS
        .iter()
        .enumerate()
        .map(|(index, checkpoint)| {
            json!({"checkpoint":checkpoint,"passed":true,"sequence":index+1}).to_string() + "\n"
        })
        .collect::<String>();
    fs::write(&path, safe).map_err(|error| format!("write lifecycle aggregate: {error}"))?;
    let mut counts = BTreeMap::new();
    for row in event(rows, "graph_result") {
        let name = row["tool"].as_str().ok_or("missing graph tool")?;
        *counts.entry(name.to_string()).or_insert(0) += 1;
    }
    Ok(LiveCodebaseMemoryEvidence {
        produced_file: None,
        expected_result: None,
        fake_mcp_log: path,
        mcp_search_calls: *counts.get("search_graph").unwrap_or(&0),
        mcp_call_counts: counts.into_iter().collect(),
        readiness_delay_ms: None,
        forced_failure_tool: None,
        aggregate_checkpoints: CHECKPOINTS.iter().map(|value| value.to_string()).collect(),
        safe_tools: mcp
            .safe_tools
            .iter()
            .map(|name| format!("codebase_memory_{name}"))
            .collect(),
        hidden_tools: mcp
            .hidden_tools
            .iter()
            .map(|name| format!("codebase_memory_{name}"))
            .collect(),
        lifecycle: mcp.lifecycle_profile.clone(),
        stable_rebind: None,
        privacy_safe_binding: None,
    })
}
