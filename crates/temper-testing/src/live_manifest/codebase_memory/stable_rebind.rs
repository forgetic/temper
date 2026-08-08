use std::collections::BTreeMap;
use std::fs;

use serde_json::Value as JsonValue;

use super::super::LiveStableRebindEvidence;
use super::{FakeMcpServer, McpToolCallEvidence};

pub(super) fn validate_mcp_contract(
    mcp: &FakeMcpServer,
    calls: &[McpToolCallEvidence],
) -> Result<(), String> {
    if mcp.lifecycle_profile.as_deref() == Some("graph-consumption") {
        return super::graph_consumption::validate(mcp, calls);
    }

    let failure = mcp.forced_systemic_failure.as_ref();
    let graph_tool = failure
        .map(|failure| failure.tool.as_str())
        .unwrap_or("search_graph");
    let expected_graph_calls = failure.map(|failure| failure.after_calls + 1).unwrap_or(1);
    let graph = calls
        .iter()
        .enumerate()
        .filter(|(_, call)| call.name == graph_tool)
        .collect::<Vec<_>>();
    if graph.len() != expected_graph_calls {
        return Err(format!(
            "expected {expected_graph_calls} {graph_tool} MCP call(s), found {} in {}",
            graph.len(),
            mcp.log_path.display()
        ));
    }
    if failure.is_some()
        && (graph[..graph.len().saturating_sub(1)]
            .iter()
            .any(|(_, call)| call.is_error)
            || graph.last().is_some_and(|(_, call)| !call.is_error))
    {
        return Err(format!(
            "{graph_tool} did not preserve one successful graph result before the controlled systemic failure: {graph:?}"
        ));
    }
    let index = calls
        .iter()
        .filter(|call| call.name == "index_repository")
        .collect::<Vec<_>>();
    if index.len() != 1
        || index[0].arguments.get("repo_path").is_none()
        || index[0].arguments.get("name").is_none()
    {
        return Err(format!(
            "index_repository was not exercised exactly once internally with repo_path and stable name: {calls:?}"
        ));
    }
    let requested_provider_project = index[0]
        .arguments
        .get("name")
        .and_then(JsonValue::as_str)
        .filter(|name| name.starts_with("temper-v1-"))
        .ok_or_else(|| {
            format!(
                "index_repository did not receive a stable provider project identity: {index:?}"
            )
        })?;
    let confirmed_provider_project = if mcp.lifecycle_profile.as_deref() == Some("stable-rebind") {
        confirmed_project_from_calls(calls, requested_provider_project)?
    } else {
        let status = calls
            .iter()
            .filter(|call| call.name == "index_status")
            .collect::<Vec<_>>();
        let [discovery] = status.as_slice() else {
            return Err(format!(
                "expected one targeted index_status discovery call for stable provider project {requested_provider_project}"
            ));
        };
        if discovery
            .arguments
            .get("project")
            .and_then(JsonValue::as_str)
            != Some(requested_provider_project)
        {
            return Err(format!(
                "targeted index_status discovery did not use stable provider project {requested_provider_project}: {discovery:?}"
            ));
        }
        requested_provider_project.to_string()
    };
    if graph.iter().any(|(_, call)| {
        call.arguments.get("project").and_then(JsonValue::as_str)
            != Some(confirmed_provider_project.as_str())
    }) {
        return Err(format!(
            "{graph_tool} did not translate the workspace alias to confirmed provider project {confirmed_provider_project}: {graph:?}"
        ));
    }
    if index[0].delay_ms != Some(mcp.readiness_delay_ms) {
        return Err(format!(
            "background index delay was not retained as {}ms: {index:?}",
            mcp.readiness_delay_ms
        ));
    }
    let mut expected_counts = BTreeMap::from([
        ("index_repository".to_string(), 1_usize),
        (
            "index_status".to_string(),
            if mcp.lifecycle_profile.as_deref() == Some("stable-rebind") {
                2
            } else {
                1
            },
        ),
        (graph_tool.to_string(), expected_graph_calls),
    ]);
    if mcp.lifecycle_profile.as_deref() == Some("stable-rebind") {
        expected_counts.insert("get_code_snippet".to_string(), 1);
    }
    let mut actual_counts = BTreeMap::<String, usize>::new();
    for call in calls {
        *actual_counts.entry(call.name.clone()).or_default() += 1;
    }
    if actual_counts != expected_counts {
        return Err(format!(
            "unexpected MCP request inventory; expected {expected_counts:?}, got {actual_counts:?}"
        ));
    }
    if let Some((failure_position, _)) = graph.last() {
        if calls.len() != failure_position + 1 {
            return Err(format!(
                "a codebase-memory MCP call followed the controlled systemic failure: {calls:?}"
            ));
        }
    }
    if mcp.lifecycle_profile.as_deref() == Some("stable-rebind") {
        validate_stable_rebind_contract(mcp, calls, requested_provider_project)?;
    }
    Ok(())
}

pub(super) fn confirmed_project_from_calls(
    calls: &[McpToolCallEvidence],
    requested_provider_project: &str,
) -> Result<String, String> {
    let confirmations = calls
        .iter()
        .filter(|call| call.name == "index_status")
        .collect::<Vec<_>>();
    let [discovery, confirmation] = confirmations.as_slice() else {
        return Err(format!(
            "expected targeted discovery followed by normalized ready confirmation, found {confirmations:?}"
        ));
    };
    if discovery
        .arguments
        .get("project")
        .and_then(JsonValue::as_str)
        != Some(requested_provider_project)
        || discovery.fixture_event.as_deref() != Some("fresh_prior_binding")
    {
        return Err(format!(
            "initial targeted discovery did not use requested stable provider project {requested_provider_project}: {discovery:?}"
        ));
    }
    let confirmed = confirmation
        .arguments
        .get("project")
        .and_then(JsonValue::as_str)
        .filter(|project| *project != requested_provider_project)
        .filter(|project| !project.trim().is_empty())
        .ok_or_else(|| {
            format!(
                "targeted ready confirmation did not use a normalized provider identity after {requested_provider_project}: {confirmation:?}"
            )
        })?;
    if confirmation.fixture_event.as_deref() != Some("current_root_confirmed") {
        return Err(format!(
            "targeted normalized provider confirmation did not report the active root binding: {confirmation:?}"
        ));
    }
    Ok(confirmed.to_string())
}

pub(super) fn stable_rebind_evidence(
    mcp: &FakeMcpServer,
    calls: &[McpToolCallEvidence],
) -> Result<Option<LiveStableRebindEvidence>, String> {
    if !uses_stable_rebind(mcp) {
        return Ok(None);
    }
    let requested_stable_project = calls
        .iter()
        .find(|call| call.name == "index_repository")
        .and_then(|call| call.arguments.get("name"))
        .and_then(JsonValue::as_str)
        .ok_or("stable-rebind index_repository call omitted stable project name")?
        .to_string();
    let confirmations = calls
        .iter()
        .filter(|call| call.name == "index_status")
        .collect::<Vec<_>>();
    let confirmed_provider_project = confirmations
        .iter()
        .find(|call| call.fixture_event.as_deref() == Some("current_root_confirmed"))
        .and_then(|call| call.arguments.get("project"))
        .and_then(JsonValue::as_str)
        .ok_or("stable-rebind targeted ready confirmation omitted normalized provider project")?
        .to_string();
    let graph_reads = calls
        .iter()
        .filter(|call| call.name == "search_graph")
        .collect::<Vec<_>>();
    let source_reads = calls
        .iter()
        .filter(|call| call.name == "get_code_snippet")
        .collect::<Vec<_>>();
    let state = read_stable_rebind_state(mcp)?;
    let retained_project_count = state
        .get("projects")
        .and_then(JsonValue::as_object)
        .map_or(0, |projects| projects.len());
    Ok(Some(LiveStableRebindEvidence {
        requested_stable_project: requested_stable_project.clone(),
        confirmed_provider_project: confirmed_provider_project.clone(),
        retained_project_count,
        confirmation_call_count: confirmations.len(),
        initial_discovery_targeted: confirmations.iter().any(|call| {
            call.arguments.get("project").and_then(JsonValue::as_str)
                == Some(requested_stable_project.as_str())
                && call.fixture_event.as_deref() == Some("fresh_prior_binding")
        }),
        normalized_provider_identity: requested_stable_project != confirmed_provider_project,
        targeted_ready_confirmation: confirmations.iter().any(|call| {
            call.arguments.get("project").and_then(JsonValue::as_str)
                == Some(confirmed_provider_project.as_str())
                && call.fixture_event.as_deref() == Some("current_root_confirmed")
        }),
        fresh_prior_binding: calls.iter().any(|call| {
            call.name == "index_status"
                && call.fixture_event.as_deref() == Some("fresh_prior_binding")
        }),
        current_root_rebound: calls.iter().any(|call| {
            call.name == "index_repository"
                && call.fixture_event.as_deref() == Some("normalized_current_root_upsert")
        }),
        graph_reads_use_confirmed_project: !graph_reads.is_empty()
            && graph_reads.iter().all(|call| {
                call.arguments.get("project").and_then(JsonValue::as_str)
                    == Some(confirmed_provider_project.as_str())
            }),
        source_reads_use_confirmed_project: !source_reads.is_empty()
            && source_reads.iter().all(|call| {
                call.arguments.get("project").and_then(JsonValue::as_str)
                    == Some(confirmed_provider_project.as_str())
            }),
        source_served_from_current_root: calls.iter().any(|call| {
            call.name == "get_code_snippet"
                && call.fixture_event.as_deref() == Some("served_current_root_source")
        }),
        global_inventory_avoided: calls.iter().all(|call| call.name != "list_projects"),
    }))
}

pub(super) fn validate_stable_rebind_contract(
    mcp: &FakeMcpServer,
    calls: &[McpToolCallEvidence],
    provider_project: &str,
) -> Result<(), String> {
    let evidence = stable_rebind_evidence(mcp, calls)?
        .ok_or("stable-rebind fixture did not produce rebind evidence")?;
    if evidence.requested_stable_project != provider_project
        || evidence.confirmed_provider_project == provider_project
        || !evidence.fresh_prior_binding
        || !evidence.initial_discovery_targeted
        || !evidence.normalized_provider_identity
        || !evidence.targeted_ready_confirmation
        || evidence.confirmation_call_count != 2
        || !evidence.current_root_rebound
        || !evidence.graph_reads_use_confirmed_project
        || !evidence.source_reads_use_confirmed_project
        || !evidence.source_served_from_current_root
        || !evidence.global_inventory_avoided
        || evidence.retained_project_count != 1
    {
        return Err(format!(
            "stable-rebind fixture did not retain one normalized project after targeted ready confirmation and serve confirmed graph/source reads without inventory discovery: {evidence:?}"
        ));
    }
    let index_root = calls
        .iter()
        .find(|call| call.name == "index_repository")
        .and_then(|call| call.arguments.get("repo_path"))
        .and_then(JsonValue::as_str)
        .ok_or("stable-rebind index_repository call omitted repo_path")?;
    let state = read_stable_rebind_state(mcp)?;
    let binding = state
        .get("projects")
        .and_then(JsonValue::as_object)
        .and_then(|projects| projects.get(&evidence.confirmed_provider_project))
        .ok_or("stable-rebind fixture omitted retained normalized provider project")?;
    if binding.get("repo_path").and_then(JsonValue::as_str) != Some(index_root)
        || binding.get("binding").and_then(JsonValue::as_str) != Some("current_prepared_checkout")
        || binding
            .get("requested_stable_project")
            .and_then(JsonValue::as_str)
            != Some(provider_project)
        || state
            .get("counters")
            .and_then(|counters| counters.get("project_creations"))
            .and_then(JsonValue::as_u64)
            != Some(1)
        || state
            .get("counters")
            .and_then(|counters| counters.get("rebinds"))
            .and_then(JsonValue::as_u64)
            != Some(1)
    {
        return Err(
            "stable-rebind fixture did not replace the prior checkout binding with the active root under one normalized provider project"
                .to_string(),
        );
    }
    Ok(())
}

fn read_stable_rebind_state(mcp: &FakeMcpServer) -> Result<JsonValue, String> {
    let raw = fs::read_to_string(&mcp.state_path).map_err(|error| {
        format!(
            "read stable-rebind fixture state {}: {error}",
            mcp.state_path.display()
        )
    })?;
    serde_json::from_str(&raw).map_err(|error| {
        format!(
            "parse stable-rebind fixture state {}: {error}",
            mcp.state_path.display()
        )
    })
}

fn uses_stable_rebind(mcp: &FakeMcpServer) -> bool {
    matches!(
        mcp.lifecycle_profile.as_deref(),
        Some("stable-rebind" | "graph-consumption")
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn call(
        name: &str,
        arguments: JsonValue,
        delay_ms: Option<u64>,
        is_error: bool,
        fixture_event: Option<&str>,
    ) -> McpToolCallEvidence {
        McpToolCallEvidence {
            name: name.to_string(),
            arguments,
            delay_ms,
            is_error,
            fixture_event: fixture_event.map(str::to_string),
        }
    }

    #[test]
    fn contract_requires_normalized_ready_confirmation_before_confirmed_reads() {
        let workspace = tempfile::tempdir().expect("workspace");
        let log_path = workspace.path().join("mcp.jsonl");
        let state_path = workspace.path().join("mcp.jsonl.state.json");
        let requested = "temper-v1-demo";
        let confirmed = "normalized-temper-v1-demo";
        fs::write(
            &state_path,
            serde_json::json!({
                "projects": {
                    confirmed: {
                        "requested_stable_project": requested,
                        "repo_path": "/workspace/demo",
                        "binding": "current_prepared_checkout"
                    }
                },
                "counters": {"project_creations": 1, "rebinds": 1}
            })
            .to_string(),
        )
        .expect("write provider state");
        let mcp = FakeMcpServer {
            script_path: workspace.path().join("fake.py"),
            log_path,
            state_path,
            project: "demo".to_string(),
            lifecycle_profile: Some("stable-rebind".to_string()),
            safe_tools: vec!["search_graph".to_string(), "get_code_snippet".to_string()],
            hidden_tools: vec!["index_repository".to_string()],
            readiness_delay_ms: 750,
            forced_systemic_failure: Some(super::super::ForcedSystemicFailureFixture {
                tool: "search_graph".to_string(),
                after_calls: 1,
            }),
        };
        let calls = vec![
            call(
                "index_status",
                serde_json::json!({"project": requested}),
                None,
                false,
                Some("fresh_prior_binding"),
            ),
            call(
                "index_repository",
                serde_json::json!({"name": requested, "repo_path": "/workspace/demo"}),
                Some(750),
                false,
                Some("normalized_current_root_upsert"),
            ),
            call(
                "index_status",
                serde_json::json!({"project": confirmed}),
                None,
                false,
                Some("current_root_confirmed"),
            ),
            call(
                "search_graph",
                serde_json::json!({"project": confirmed}),
                None,
                false,
                Some("served_current_root_graph"),
            ),
            call(
                "get_code_snippet",
                serde_json::json!({"project": confirmed, "qualified_name": "retry_worker_topic"}),
                None,
                false,
                Some("served_current_root_source"),
            ),
            call(
                "search_graph",
                serde_json::json!({"project": confirmed}),
                None,
                true,
                None,
            ),
        ];

        let evidence = stable_rebind_evidence(&mcp, &calls)
            .expect("read stable rebind evidence")
            .expect("stable profile evidence");
        assert_eq!(evidence.requested_stable_project, requested);
        assert_eq!(evidence.confirmed_provider_project, confirmed);
        assert_eq!(evidence.confirmation_call_count, 2);
        assert!(evidence.normalized_provider_identity);
        assert!(evidence.graph_reads_use_confirmed_project);
        assert!(evidence.source_reads_use_confirmed_project);
        validate_stable_rebind_contract(&mcp, &calls, requested)
            .expect("exact confirmation inventory is accepted");
    }

    #[test]
    fn graph_consumption_contract_requires_the_declared_current_root_chain() {
        let workspace = tempfile::tempdir().expect("workspace");
        let requested = "temper-v1-demo";
        let confirmed = "normalized-temper-v1-demo";
        let state_path = workspace.path().join("mcp.jsonl.state.json");
        fs::write(
            &state_path,
            serde_json::json!({
                "projects": {
                    confirmed: {
                        "requested_stable_project": requested,
                        "repo_path": "/workspace/demo",
                        "binding": "current_prepared_checkout"
                    }
                },
                "counters": {"project_creations": 1, "rebinds": 1}
            })
            .to_string(),
        )
        .expect("write provider state");
        let mcp = FakeMcpServer {
            script_path: workspace.path().join("fake.py"),
            log_path: workspace.path().join("mcp.jsonl"),
            state_path,
            project: "demo".to_string(),
            lifecycle_profile: Some("graph-consumption".to_string()),
            safe_tools: vec![
                "search_graph".to_string(),
                "search_code".to_string(),
                "trace_path".to_string(),
                "get_code_snippet".to_string(),
            ],
            hidden_tools: vec!["index_repository".to_string()],
            readiness_delay_ms: 750,
            forced_systemic_failure: None,
        };
        let calls = vec![
            call(
                "index_status",
                serde_json::json!({"project": requested}),
                None,
                false,
                Some("fresh_prior_binding"),
            ),
            call(
                "index_repository",
                serde_json::json!({"name": requested, "repo_path": "/workspace/demo"}),
                Some(750),
                false,
                Some("normalized_current_root_upsert"),
            ),
            call(
                "index_status",
                serde_json::json!({"project": confirmed}),
                None,
                false,
                Some("current_root_confirmed"),
            ),
            call(
                "search_graph",
                serde_json::json!({"project": confirmed, "query": "alias retry worker affinity"}),
                None,
                false,
                Some("served_current_root_graph"),
            ),
            call(
                "search_code",
                serde_json::json!({"project": confirmed, "pattern": "retry_worker_topic"}),
                None,
                false,
                Some("served_current_root_code_refinement"),
            ),
            call(
                "trace_path",
                serde_json::json!({"project": confirmed, "function_name": "retry_worker_topic"}),
                None,
                false,
                Some("served_current_root_graph_trace"),
            ),
            call(
                "get_code_snippet",
                serde_json::json!({"project": confirmed, "qualified_name": "retry_worker_topic"}),
                None,
                false,
                Some("served_current_root_source"),
            ),
            call(
                "get_code_snippet",
                serde_json::json!({"project": confirmed, "qualified_name": "retry_worker_topic_retry_affinity"}),
                None,
                false,
                Some("served_current_root_source"),
            ),
        ];

        validate_mcp_contract(&mcp, &calls).expect("declared graph-consumption chain is accepted");
    }
}
