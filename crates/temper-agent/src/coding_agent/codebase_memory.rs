use crate::codebase_memory::{
    CodebaseMemoryToolMetadata, CodebaseMemoryToolset, build_codebase_memory_toolset,
};
use temper_protocol_agent::AgentToolConfig;

use super::CodingAgentError;

pub(super) struct PreparedCodebaseMemoryTools {
    pub(super) prompt_section: Option<String>,
    pub(super) toolset: CodebaseMemoryToolset,
}

pub(super) async fn prepare_codebase_memory_tools(
    tool_config: Option<&AgentToolConfig>,
    role: &str,
) -> Result<PreparedCodebaseMemoryTools, CodingAgentError> {
    let toolset = build_codebase_memory_toolset(tool_config, role)
        .await
        .map_err(|error| CodingAgentError::CodebaseMemory(error.to_string()))?;
    let prompt_section = codebase_memory_prompt_section(toolset.registered_tool_metadata());
    Ok(PreparedCodebaseMemoryTools {
        prompt_section,
        toolset,
    })
}

pub(crate) fn codebase_memory_prompt_section(
    tools: &[CodebaseMemoryToolMetadata],
) -> Option<String> {
    if tools.is_empty() {
        return None;
    }

    let mut tools = tools.to_vec();
    tools.sort_by(|left, right| left.name.cmp(&right.name));
    let rendered_tools = tools
        .iter()
        .map(|tool| {
            let summary = tool.description.lines().next().unwrap_or_default().trim();
            if summary.is_empty() {
                format!("- `{}`", tool.name)
            } else {
                format!("- `{}`: {summary}", tool.name)
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    Some(format!(
        "\nCODEBASE MEMORY:\n\
         You have repository-index tools for architecture, symbol search, code search,\n\
         and call/impact tracing.\n\n\
         Use them early for non-trivial tasks:\n\
         - architect: map affected areas before triage/breakdown;\n\
         - engineer: find relevant symbols/callers before editing;\n\
         - reviewer: inspect impacted code paths and callers before verdicts.\n\n\
         Treat the graph as an index, not truth. Verify exact code with read/grep/git diff\n\
         before editing or making final claims.\n\n\
         Registered codebase-memory tools:\n{rendered_tools}\n"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use temper_protocol_agent::{
        CodebaseMemoryIndex, CodebaseMemoryMode, CodebaseMemoryToolConfig,
    };
    use tongs::tools::ToolEffects;

    fn fake_server_script() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(
            dir.path().join("fake_codebase_memory_mcp.py"),
            r#"
import json
import sys

TOOLS = [
    {"name": "search_code", "description": "Search indexed code", "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}},
    {"name": "delete_project", "description": "Delete project", "inputSchema": {"type": "object", "properties": {}}},
]

def send(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    if not line.strip():
        continue
    request = json.loads(line)
    if "id" not in request:
        continue
    method = request.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": request["id"], "result": {"protocolVersion": "2024-11-05", "serverInfo": {"name": "fake-codebase-memory", "version": "1"}, "capabilities": {"tools": {}}}})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": request["id"], "result": {"tools": TOOLS}})
    elif method == "tools/call":
        send({"jsonrpc": "2.0", "id": request["id"], "result": {"content": [{"type": "text", "text": "fake result"}], "isError": False}})
    else:
        send({"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32601, "message": "unknown method"}})
"#,
        )
        .expect("write fake server");
        dir
    }

    fn script_path(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("fake_codebase_memory_mcp.py")
    }

    fn config(
        dir: &tempfile::TempDir,
        mode: CodebaseMemoryMode,
        roles: Vec<&str>,
    ) -> AgentToolConfig {
        AgentToolConfig {
            codebase_memory: Some(CodebaseMemoryToolConfig {
                mode,
                command: "python3".to_string(),
                args: vec!["-u".to_string(), script_path(dir).display().to_string()],
                roles: roles.into_iter().map(str::to_string).collect(),
                index: CodebaseMemoryIndex::Off,
                startup_timeout_secs: 1,
                index_timeout_secs: 2,
            }),
        }
    }

    fn bad_command_config(mode: CodebaseMemoryMode) -> AgentToolConfig {
        AgentToolConfig {
            codebase_memory: Some(CodebaseMemoryToolConfig {
                mode,
                command: "definitely-not-a-temper-codebase-memory-mcp".to_string(),
                args: Vec::new(),
                roles: vec!["engineer".to_string()],
                index: CodebaseMemoryIndex::Off,
                startup_timeout_secs: 1,
                index_timeout_secs: 1,
            }),
        }
    }

    #[test]
    fn codebase_memory_prompt_appears_only_after_safe_tools_registered() {
        let dir = fake_server_script();
        temper_agent_io::block_on(async move {
            let absent = prepare_codebase_memory_tools(None, "engineer")
                .await
                .expect("absent config is ok");
            assert!(absent.prompt_section.is_none());
            assert!(absent.toolset.registered_tool_names().is_empty());

            let role_mismatch = config(&dir, CodebaseMemoryMode::Required, vec!["reviewer"]);
            let mismatch = prepare_codebase_memory_tools(Some(&role_mismatch), "engineer")
                .await
                .expect("role mismatch is ok");
            assert!(mismatch.prompt_section.is_none());
            assert!(mismatch.toolset.registered_tool_names().is_empty());

            let auto_unavailable = bad_command_config(CodebaseMemoryMode::Auto);
            let unavailable = prepare_codebase_memory_tools(Some(&auto_unavailable), "engineer")
                .await
                .expect("auto startup failure is best effort");
            assert!(unavailable.prompt_section.is_none());
            assert!(unavailable.toolset.registered_tool_names().is_empty());

            let enabled = config(&dir, CodebaseMemoryMode::Required, vec!["engineer"]);
            let prepared = prepare_codebase_memory_tools(Some(&enabled), "engineer")
                .await
                .expect("required fake server starts");
            let prompt = prepared
                .prompt_section
                .as_deref()
                .expect("registered tools produce prompt section");
            assert!(prompt.contains("CODEBASE MEMORY"));
            assert!(prompt.contains("codebase_memory_search_code"));
            assert!(prompt.contains("Search indexed code"));
            assert!(!prompt.contains("codebase_memory_delete_project"));

            let mut registry = tongs::tools::ToolRegistry::new();
            prepared.toolset.append_to_registry(&mut registry);
            let tool = registry
                .get("codebase_memory_search_code")
                .expect("safe tool registered");
            assert_eq!(tool.effects(), ToolEffects::read());
        });
    }

    #[test]
    fn required_codebase_memory_startup_failure_is_coding_agent_error() {
        let required = bad_command_config(CodebaseMemoryMode::Required);
        temper_agent_io::block_on(async move {
            let error = match prepare_codebase_memory_tools(Some(&required), "engineer").await {
                Ok(_) => panic!("required mode startup failure must fail the run"),
                Err(error) => error,
            };
            match error {
                CodingAgentError::CodebaseMemory(message) => {
                    assert!(message.contains("required codebase-memory MCP startup failed"));
                    assert!(message.contains("spawn MCP command"));
                }
                other => panic!("expected codebase-memory setup error, got {other:?}"),
            }
        });
    }
}
