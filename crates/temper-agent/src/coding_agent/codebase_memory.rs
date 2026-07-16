use crate::codebase_memory::{
    CodebaseMemoryToolMetadata, CodebaseMemoryToolset, build_codebase_memory_toolset_with_timeout,
};
use std::path::Path;
use std::time::Duration;

use temper_protocol_agent::{AgentToolConfig, WorkspaceContext};
use tongs::tools::ToolRegistry;

use super::CodingAgentError;

pub(super) struct PreparedCodebaseMemoryTools {
    prompt_section: Option<String>,
    pub(super) toolset: CodebaseMemoryToolset,
}

pub(super) struct PreparedCodebaseMemoryGuidance {
    prompt_section: Option<String>,
    registered_safe_names: Vec<String>,
}

impl PreparedCodebaseMemoryTools {
    /// Appends the prepared safe tools and retains only the metadata needed to
    /// check the finalized registry before rendering guidance.
    pub(super) fn append_to_registry(
        self,
        registry: &mut ToolRegistry,
    ) -> PreparedCodebaseMemoryGuidance {
        let registered_safe_names = self.toolset.registered_tool_names().to_vec();
        self.toolset.append_to_registry(registry);
        PreparedCodebaseMemoryGuidance {
            prompt_section: self.prompt_section,
            registered_safe_names,
        }
    }
}

impl PreparedCodebaseMemoryGuidance {
    /// Returns guidance only when at least one safe tool from this prepared
    /// toolset survived into the finalized provider registry.
    pub(super) fn prompt_section_for_registry(&self, registry: &ToolRegistry) -> Option<&str> {
        if self
            .registered_safe_names
            .iter()
            .any(|name| registry.get(name).is_some())
        {
            self.prompt_section.as_deref()
        } else {
            None
        }
    }
}

#[cfg(test)]
pub(super) async fn prepare_codebase_memory_tools(
    tool_config: Option<&AgentToolConfig>,
    role: &str,
    context: &WorkspaceContext,
    cwd: &Path,
) -> Result<PreparedCodebaseMemoryTools, CodingAgentError> {
    prepare_codebase_memory_tools_with_timeout(tool_config, role, context, cwd, Duration::MAX).await
}

pub(super) async fn prepare_codebase_memory_tools_with_timeout(
    tool_config: Option<&AgentToolConfig>,
    role: &str,
    context: &WorkspaceContext,
    cwd: &Path,
    generic_tool_timeout: Duration,
) -> Result<PreparedCodebaseMemoryTools, CodingAgentError> {
    let toolset = build_codebase_memory_toolset_with_timeout(
        tool_config,
        role,
        context,
        cwd,
        generic_tool_timeout,
    )
    .await
    .map_err(|error| CodingAgentError::CodebaseMemory(error.to_string()))?;
    let prompt_section = codebase_memory_prompt_section_with_status(
        toolset.registered_tool_metadata(),
        toolset.prompt_status(),
    );
    Ok(PreparedCodebaseMemoryTools {
        prompt_section,
        toolset,
    })
}

#[cfg(test)]
pub(crate) fn codebase_memory_prompt_section(
    tools: &[CodebaseMemoryToolMetadata],
) -> Option<String> {
    codebase_memory_prompt_section_with_status(tools, None)
}

pub(crate) fn codebase_memory_prompt_section_with_status(
    tools: &[CodebaseMemoryToolMetadata],
    status: Option<&str>,
) -> Option<String> {
    // The provider request already contains complete tool names, descriptions,
    // and schemas. Registration metadata controls only whether this guidance is
    // relevant; copying any of it into the prompt would duplicate the tool API.
    if tools.is_empty() {
        return None;
    }

    let status = status
        .map(|status| format!("\nWorkspace/index status:\n{status}\n"))
        .unwrap_or_default();

    Some(format!(
        "\nCODEBASE MEMORY:\n\
         You have repository-index tools for architecture, symbol search, code search,\n\
         and call/impact tracing.\n\n\
         Use them early for non-trivial tasks:\n\
         - architect: map affected areas before triage/breakdown;\n\
         - engineer: find relevant symbols/callers before editing;\n\
         - reviewer: inspect impacted code paths and callers before verdicts.\n\n\
         Treat the graph as an index, not truth. Verify exact code with read/grep/git diff\n\
         before editing or making final claims.\n\
{status}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use temper_protocol_agent::{
        CodebaseMemoryIndex, CodebaseMemoryMode, CodebaseMemoryToolConfig, WorkspaceContext,
        WorkspaceGuidance, WorkspaceRepository, WorkspaceWorkItem,
    };
    use tongs::tools::ToolEffects;

    const FAKE_MCP_DESCRIPTION_SENTINEL: &str = "FAKE-MCP-DESCRIPTION-SENTINEL-384";

    fn fake_server_script() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(
            dir.path().join("fake_codebase_memory_mcp.py"),
            r#"
import json
import sys

TOOLS = [
    {"name": "search_code", "description": "FAKE-MCP-DESCRIPTION-SENTINEL-384", "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}},
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

    fn workspace_context(cwd: &Path) -> WorkspaceContext {
        let repo_dir = cwd.join("demo");
        fs::create_dir_all(&repo_dir).expect("create repo dir");
        WorkspaceContext {
            trace_context: None,
            artifact_context: None,
            repos: vec![WorkspaceRepository {
                id: "repo-1".to_string(),
                owner: "acme".to_string(),
                name: "demo".to_string(),
                default_branch: "main".to_string(),
                dir: "demo".to_string(),
                access: "writable".to_string(),
                base_branch: "main".to_string(),
                branch_hint: Some("agent/pr-for-code-25".to_string()),
            }],
            work_item: WorkspaceWorkItem {
                role: "engineer".to_string(),
                queue: "code_ready".to_string(),
                kind: "code".to_string(),
                target: "Issue { number: ItemNumber(25) }".to_string(),
                context: "{}".to_string(),
            },
            action: "open_pr".to_string(),
            correlation_key: "pr-for-code-25".to_string(),
            checkout: Some("writable".to_string()),
            allowed_verdicts: vec!["needs_architect".to_string()],
            verdict_contracts: Default::default(),
            source_metadata: Default::default(),
            guidance: WorkspaceGuidance::default(),
            pull_request_freshness: None,
            agent_session: None,
        }
    }

    #[test]
    fn codebase_memory_prompt_appears_only_after_safe_tools_registered() {
        let dir = fake_server_script();
        let cwd = tempfile::tempdir().expect("workspace");
        let context = workspace_context(cwd.path());
        let cwd_path = cwd.path().to_path_buf();
        temper_agent_io::block_on(async move {
            let absent = prepare_codebase_memory_tools(None, "engineer", &context, &cwd_path)
                .await
                .expect("absent config is ok");
            assert!(absent.prompt_section.is_none());
            assert!(absent.toolset.registered_tool_names().is_empty());

            let role_mismatch = config(&dir, CodebaseMemoryMode::Required, vec!["reviewer"]);
            let mismatch = prepare_codebase_memory_tools(
                Some(&role_mismatch),
                "engineer",
                &context,
                &cwd_path,
            )
            .await
            .expect("role mismatch is ok");
            assert!(mismatch.prompt_section.is_none());
            assert!(mismatch.toolset.registered_tool_names().is_empty());

            let auto_unavailable = bad_command_config(CodebaseMemoryMode::Auto);
            let unavailable = prepare_codebase_memory_tools(
                Some(&auto_unavailable),
                "engineer",
                &context,
                &cwd_path,
            )
            .await
            .expect("auto startup failure is best effort");
            assert!(unavailable.prompt_section.is_none());
            assert!(unavailable.toolset.registered_tool_names().is_empty());

            let enabled = config(&dir, CodebaseMemoryMode::Required, vec!["engineer"]);
            let prepared =
                prepare_codebase_memory_tools(Some(&enabled), "engineer", &context, &cwd_path)
                    .await
                    .expect("required fake server starts");
            let prompt = prepared
                .prompt_section
                .clone()
                .expect("registered tools produce prompt section");
            for expected in [
                "CODEBASE MEMORY",
                "repository-index tools for architecture, symbol search, code search",
                "Use them early for non-trivial tasks",
                "- engineer: find relevant symbols/callers before editing;",
                "Treat the graph as an index, not truth.",
                "Default project: `acme/demo`",
                "Project aliases accepted in `project`/`repo`",
                "`acme/demo`",
                "`demo`",
                "`repo-1`",
                "Filesystem paths are never accepted as project/repo values",
                "Index setting: `off`",
                "`acme/demo` status:",
                "Note: index=off; no internal indexing was attempted",
            ] {
                assert!(prompt.contains(expected), "prompt omitted {expected:?}");
            }
            for duplicated_api_text in [
                FAKE_MCP_DESCRIPTION_SENTINEL,
                "codebase_memory_search_code",
                "codebase_memory_delete_project",
                "Registered codebase-memory tools:",
            ] {
                assert!(
                    !prompt.contains(duplicated_api_text),
                    "prompt duplicated tool API text {duplicated_api_text:?}"
                );
            }

            let empty_registry = tongs::tools::ToolRegistry::new();
            let mut registry = tongs::tools::ToolRegistry::new();
            let guidance = prepared.append_to_registry(&mut registry);
            assert!(
                guidance
                    .prompt_section_for_registry(&empty_registry)
                    .is_none(),
                "memory guidance requires a safe tool in the finalized registry"
            );
            assert_eq!(
                guidance.prompt_section_for_registry(&registry),
                Some(prompt.as_str()),
                "the finalized registry retains memory guidance"
            );
            let tool = registry
                .get("codebase_memory_search_code")
                .expect("safe tool registered");
            assert_eq!(tool.effects(), ToolEffects::read());
        });
    }

    #[test]
    fn required_codebase_memory_startup_failure_is_coding_agent_error() {
        let required = bad_command_config(CodebaseMemoryMode::Required);
        let cwd = tempfile::tempdir().expect("workspace");
        let context = workspace_context(cwd.path());
        let cwd_path = cwd.path().to_path_buf();
        temper_agent_io::block_on(async move {
            let error = match prepare_codebase_memory_tools(
                Some(&required),
                "engineer",
                &context,
                &cwd_path,
            )
            .await
            {
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
