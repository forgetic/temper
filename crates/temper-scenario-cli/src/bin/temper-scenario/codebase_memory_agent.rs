// SPDX-License-Identifier: MPL-2.0

use std::path::Path;

use temper_testing::codebase_memory_agent::{
    CodebaseMemoryAgentScenarioEvidence, run_codebase_memory_agent_scenario,
};

use super::run_context::ScenarioRunFacts;
use super::run_evidence;

pub(super) const SCENARIO_NAME: &str = "codebase-memory-agent";

pub(super) fn run_and_print(
    _scenario_path: &Path,
    _manifest_path: &Path,
    facts: &ScenarioRunFacts,
    context: &run_evidence::RunEvidenceContext,
) -> Result<run_evidence::RunEvidenceArtifact, String> {
    let evidence = run_codebase_memory_agent_scenario()?;
    print_outcome(&evidence, facts);
    Ok(outcome_artifact(&evidence, context))
}

pub(super) fn run_evidence_lines(
    _scenario_path: &Path,
    _manifest_path: &Path,
) -> Result<Vec<String>, String> {
    let evidence = run_codebase_memory_agent_scenario()?;
    Ok(outcome_evidence_lines(&evidence))
}

fn print_outcome(evidence: &CodebaseMemoryAgentScenarioEvidence, facts: &ScenarioRunFacts) {
    println!("scenario: {SCENARIO_NAME}");
    facts.print_stdout();
    println!("verdict: passed");
    println!("evidence:");
    for line in outcome_evidence_lines(evidence) {
        println!("  {line}");
    }
}

fn outcome_evidence_lines(evidence: &CodebaseMemoryAgentScenarioEvidence) -> Vec<String> {
    let search = evidence
        .mcp_tool_calls
        .iter()
        .find(|call| call.name == "search_code");
    let search_detail = search
        .map(|call| {
            let query = call
                .arguments
                .get("query")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("<missing>");
            let project = call
                .arguments
                .get("project")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("<missing>");
            format!("search_code query={query} project={project}")
        })
        .unwrap_or_else(|| "search_code call missing".to_string());
    let index_detail = evidence
        .mcp_tool_calls
        .iter()
        .find(|call| call.name == "index_repository")
        .and_then(|call| call.arguments.get("repo_path"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("<missing>");
    vec![
        "tool config: enabled AgentToolConfig codebase_memory required/blocking for engineer"
            .to_string(),
        format!(
            "registered tools: safe codebase_memory_* exposed to model; index_repository absent ({} total model tools)",
            evidence.model_tool_names.len()
        ),
        format!(
            "prompt guidance: CODEBASE MEMORY present only after tools registered = {}",
            evidence.prompt_guidance_seen
        ),
        format!("fake MCP tool call: {search_detail}"),
        format!(
            "workspace defaulting: model omitted project; wrapper injected actual `{}` discovered from root_path {}",
            evidence.actual_project,
            evidence.repo_root.display()
        ),
        format!(
            "internal indexing: index_repository used repo_path {index_detail} and is not model-callable"
        ),
        format!(
            "model consumed MCP result: wrote {} containing FAKE_MCP_SEARCH_RESULT; summary `{}`",
            evidence.produced_file.display(),
            evidence.final_summary
        ),
        format!("fake LLM requests: {}", evidence.fake_llm_requests),
    ]
}

fn outcome_artifact(
    evidence: &CodebaseMemoryAgentScenarioEvidence,
    context: &run_evidence::RunEvidenceContext,
) -> run_evidence::RunEvidenceArtifact {
    let mut artifact = context.artifact(run_evidence::FinalStateEvidence {
        issues: Vec::new(),
        pull_requests: Vec::new(),
        repositories: vec![run_evidence::RepositoryStateEvidence {
            id: Some("repo-1".to_string()),
            slug: Some(evidence.repo_slug.clone()),
            branches: vec![run_evidence::RepositoryBranchStateEvidence {
                name: "workspace".to_string(),
                head_sha: None,
                contains_engineer_diff: Some(true),
            }],
        }],
        ci: run_evidence::CiStateEvidence::default(),
    });
    artifact.provider = Some(run_evidence::ProviderEvidence {
        fake_llm_url: Some("jig fake LLM (in-process)".to_string()),
        repo_slug: Some(evidence.repo_slug.clone()),
        ..run_evidence::ProviderEvidence::default()
    });
    artifact.evidence_lines = outcome_evidence_lines(evidence);
    artifact
}
