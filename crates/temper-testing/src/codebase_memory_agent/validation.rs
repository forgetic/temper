use std::collections::BTreeSet;

use serde_json::Value;

use super::{CodebaseMemoryAgentScenarioEvidence, McpToolCallEvidence};

pub(super) fn validate_evidence(
    evidence: &CodebaseMemoryAgentScenarioEvidence,
    actual_project: &str,
) -> Result<(), String> {
    let codebase_tools = evidence
        .model_tool_names
        .iter()
        .filter(|name| name.starts_with("codebase_memory_"))
        .cloned()
        .collect::<BTreeSet<_>>();
    let safe = [
        "codebase_memory_search_code",
        "codebase_memory_list_projects",
        "codebase_memory_index_status",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<BTreeSet<_>>();
    if !safe.is_superset(&codebase_tools) || !codebase_tools.contains("codebase_memory_search_code")
    {
        return Err(format!(
            "unexpected model codebase-memory tools: {codebase_tools:?}"
        ));
    }
    if !evidence.prompt_guidance_seen {
        return Err("fake LLM did not receive CODEBASE MEMORY prompt guidance".to_string());
    }
    if !evidence.memory_result_seen_by_model {
        return Err("fake LLM did not receive the fake MCP search result".to_string());
    }
    assert_search_call(evidence, actual_project)?;
    assert_index_call(evidence)?;
    if !evidence
        .produced_file_content
        .contains("FAKE_MCP_SEARCH_RESULT")
    {
        return Err("produced file does not include the fake MCP result".to_string());
    }
    Ok(())
}

fn assert_search_call(
    evidence: &CodebaseMemoryAgentScenarioEvidence,
    actual_project: &str,
) -> Result<(), String> {
    let search = calls_named(&evidence.mcp_tool_calls, "search_code");
    if search.len() != 1 {
        return Err(format!(
            "expected one search_code MCP call, found {}",
            search.len()
        ));
    }
    if search[0].arguments.get("project").and_then(Value::as_str) != Some(actual_project) {
        return Err(format!(
            "search_code did not receive project {actual_project}"
        ));
    }
    Ok(())
}

fn assert_index_call(evidence: &CodebaseMemoryAgentScenarioEvidence) -> Result<(), String> {
    let index = calls_named(&evidence.mcp_tool_calls, "index_repository");
    if index.len() != 1 || index[0].arguments.get("repo_path").is_none() {
        return Err("index_repository was not exercised with repo_path".to_string());
    }
    if index[0].arguments.get("path").is_some() {
        return Err("index_repository used path instead of repo_path".to_string());
    }
    Ok(())
}

fn calls_named<'a>(calls: &'a [McpToolCallEvidence], name: &str) -> Vec<&'a McpToolCallEvidence> {
    calls.iter().filter(|call| call.name == name).collect()
}
