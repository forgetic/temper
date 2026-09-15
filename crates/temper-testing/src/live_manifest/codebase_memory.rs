use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use jig_core::RequestView;
use serde_json::Value as JsonValue;
use temper_forge_forgejo::ForgejoForge;
use temper_forge_model::{
    CiJobConclusion, IssueState, ItemNumber, PullRequest, PullRequestQuery, PullRequestState,
    RepositoryId, UserId,
};
use temper_workflow::{CiStatus, parse_metadata_block};

use super::convergence::{
    ci_observation_evidence, completed_ci_observation, issue_evidence, poll_until, pr_evidence,
    reject_labels, require_labels,
};
use super::{
    ENGINEER, FinalStateEvidence, ForcedSystemicFailureFixture, LiveCodebaseMemoryEvidence,
    LivePrivacySafeCodebaseMemoryBindingEvidence,
};

mod aggregate;
mod configuration;
mod fake_llm;
mod graph_consumption;
mod mapped_batched_edits_fake;
mod mapped_companion_read_fake;
mod mapped_decision_gap_recovery;
mod mapped_decision_gap_recovery_fake;
mod mapped_denied_shell_classification;
mod mapped_denied_shell_classification_fake;
mod mapped_exact_source_selection;
mod mapped_exact_source_selection_fake;
mod mapped_focused_test_relevance;
mod mapped_focused_test_relevance_fake;
mod mapped_graph_consumption;
mod mapped_graph_consumption_fake;
mod mapped_graph_convergence;
mod mapped_graph_convergence_fake;
mod mapped_ordinary_convergence_fake;
mod mapped_patch_creation_fake;
mod mapped_patch_framing_fake;
mod mapped_rust_format_fake;
mod model_observations;
pub(super) use fake_llm::CodebaseMemoryFake;
mod privacy;
mod provider_result_anchor;
mod result_driven_fake;
mod result_driven_guidance;
mod scoped_graph_evidence;
mod sequential_graph_evidence;
pub(super) mod shared_lifecycle;
mod stable_rebind;
mod typed_lineage_anchor;
mod typed_lineage_fake;
use aggregate::privacy_safe_checkpoints;
pub(super) use configuration::{ToolConfiguration, tune_codebase_memory_config};
use model_observations::ModelObservations;
use privacy::write_privacy_safe_mcp_log;
use stable_rebind::{stable_rebind_evidence, validate_mcp_contract};

const MEMORY_FILE: &str = "src/lib.rs";
const MEMORY_RESULT_NEEDLE: &str = "FAKE_MCP_GRAPH_RESULT";
const CURRENT_ROOT_SOURCE_BINDING: &str = "current_prepared_checkout";
const ENGINEER_SUMMARY: &str =
    "Used codebase-memory graph evidence, then validated the retry-worker repair.";
const PROVIDER_NEUTRAL_ENGINEER_SUMMARY: &str =
    "Consumed provider-neutral typed current-root lineage before the minimal repair.";
const GRAPH_CONVERGENCE_ENGINEER_SUMMARY: &str = "Consumed bounded current-root graph evidence and local convergence guidance before the minimal repair.";
const RAW_PROVIDER_FAILURE_NEEDLE: &str = "MCP-FIXTURE-SECRET";
const SAFE_PROVIDER_FAILURE: &str = "codebase-memory provider or protocol request failed; do not retry codebase-memory immediately; continue with read, grep, find, shell, or other conventional discovery instead";
const BOUNDED_GRAPH_RESULT_NEEDLE: &str = "[codebase-memory output truncated to 16384 bytes]";
const MAX_MODEL_MESSAGE_BYTES: usize = 20 * 1024;

pub(super) fn converge(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    issue: ItemNumber,
    admin_user: &str,
    standalone: &mut super::process::ChildGuard,
    timeout: Duration,
    fake: &CodebaseMemoryFake,
    mcp: &FakeMcpServer,
) -> Result<(FinalStateEvidence, LiveCodebaseMemoryEvidence), String> {
    let final_state = drive_codebase_memory_convergence(
        forge, repository, issue, admin_user, standalone, timeout,
    )?;
    let calls = logged_tool_calls(&mcp.log_path)?;
    validate_mcp_contract(mcp, &calls)?;
    fake.validate_observations(mcp)?;
    let mut mcp_call_counts = BTreeMap::<String, usize>::new();
    for call in &calls {
        *mcp_call_counts.entry(call.name.clone()).or_default() += 1;
    }
    let mcp_search_calls = mcp_call_counts
        .get("search_graph")
        .copied()
        .unwrap_or_default();
    let privacy_safe_aggregate = privacy::is_privacy_safe_profile(mcp.lifecycle_profile.as_deref());
    let aggregate_checkpoints = privacy_safe_checkpoints(mcp, &calls);
    let stable_rebind = stable_rebind_evidence(mcp, &calls)?;
    let evidence_mcp_log = if privacy_safe_aggregate {
        write_privacy_safe_mcp_log(mcp, &calls)?
    } else {
        mcp.log_path.clone()
    };
    let privacy_safe_binding = privacy_safe_aggregate
        .then(|| {
            stable_rebind
                .as_ref()
                .map(|binding| LivePrivacySafeCodebaseMemoryBindingEvidence {
                    confirmation_call_count: binding.confirmation_call_count,
                    targeted_ready_confirmation: binding.targeted_ready_confirmation,
                    current_root_rebound: binding.current_root_rebound,
                    graph_reads_use_confirmed_project: binding.graph_reads_use_confirmed_project,
                    source_reads_use_confirmed_project: binding.source_reads_use_confirmed_project,
                    source_served_from_current_root: binding.source_served_from_current_root,
                    global_inventory_avoided: binding.global_inventory_avoided,
                })
        })
        .flatten();
    let expected_result = if matches!(
        mcp.lifecycle_profile.as_deref(),
        Some(
            "sequential-graph-evidence"
                | "result-driven-decision-guidance"
                | "provider-result-anchor"
                | "provider-neutral-anchor-lineage"
                | "mapped-live-graph-consumption"
                | "mapped-live-denied-shell-classification"
                | "mapped-live-ordinary-tool-convergence"
                | "mapped-live-graph-convergence"
                | "mapped-live-decision-gap-recovery"
                | "mapped-live-exact-source-selection"
                | "mapped-live-focused-test-source-relevance"
        )
    ) {
        "one successful provider-shaped graph result".to_string()
    } else {
        MEMORY_RESULT_NEEDLE.to_string()
    };
    Ok((
        final_state,
        LiveCodebaseMemoryEvidence {
            produced_file: (!privacy_safe_aggregate).then(|| MEMORY_FILE.to_string()),
            expected_result: (!privacy_safe_aggregate).then_some(expected_result),
            fake_mcp_log: evidence_mcp_log,
            mcp_search_calls,
            mcp_call_counts: mcp_call_counts.into_iter().collect(),
            readiness_delay_ms: (!privacy_safe_aggregate).then_some(mcp.readiness_delay_ms),
            forced_failure_tool: mcp
                .forced_systemic_failure
                .as_ref()
                .map(|failure| failure.tool.clone()),
            aggregate_checkpoints,
            safe_tools: mcp
                .safe_tools
                .iter()
                .map(|tool| format!("codebase_memory_{tool}"))
                .collect(),
            hidden_tools: mcp
                .hidden_tools
                .iter()
                .map(|tool| format!("codebase_memory_{tool}"))
                .collect(),
            lifecycle: mcp.lifecycle_profile.clone(),
            stable_rebind: (!privacy_safe_aggregate).then_some(stable_rebind).flatten(),
            privacy_safe_binding,
        },
    ))
}

fn drive_codebase_memory_convergence(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    issue: ItemNumber,
    admin_user: &str,
    standalone: &mut super::process::ChildGuard,
    timeout: Duration,
) -> Result<FinalStateEvidence, String> {
    let deadline = Instant::now() + timeout;
    match poll_until(deadline, standalone, || {
        super::process::engine_block_on(assert_codebase_memory_checkpoint(
            forge, repository, issue, admin_user,
        ))
    })? {
        CodebaseMemoryCheckpoint::OpenPr => poll_until(deadline, standalone, || {
            super::process::engine_block_on(assert_codebase_memory_converged(
                forge, repository, issue, admin_user,
            ))
        }),
        CodebaseMemoryCheckpoint::Converged(final_state) => Ok(final_state),
    }
}

enum CodebaseMemoryCheckpoint {
    OpenPr,
    Converged(FinalStateEvidence),
}

async fn assert_codebase_memory_checkpoint(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    issue: ItemNumber,
    admin_user: &str,
) -> Result<CodebaseMemoryCheckpoint, String> {
    let mut errors = Vec::new();

    match assert_pr_open_with_memory_diff(forge, repository, issue).await {
        Ok(()) => return Ok(CodebaseMemoryCheckpoint::OpenPr),
        Err(error) => errors.push(("open implementation PR with memory diff", error)),
    }

    match assert_codebase_memory_converged(forge, repository, issue, admin_user).await {
        Ok(final_state) => Ok(CodebaseMemoryCheckpoint::Converged(final_state)),
        Err(error) => {
            errors.push(("final convergence", error));
            Err(format_codebase_memory_checkpoint_errors(&errors))
        }
    }
}

fn format_codebase_memory_checkpoint_errors(errors: &[(&'static str, String)]) -> String {
    let details = errors
        .iter()
        .map(|(phase, error)| format!("{phase}: {error}"))
        .collect::<Vec<_>>()
        .join("; ");
    format!(
        "codebase-memory workflow has not reached open implementation PR with memory diff or final convergence yet ({details})"
    )
}

async fn assert_pr_open_with_memory_diff(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    issue: ItemNumber,
) -> Result<(), String> {
    let pr = implementation_pr(forge, repository, issue).await?;
    verify_engineer_pr(&pr, issue)?;
    if pr.state != PullRequestState::Open {
        return Err(format!(
            "implementation PR #{} is not open yet (state {:?})",
            pr.number, pr.state
        ));
    }
    require_labels(&pr.labels, &["implementation", "landing"])?;
    assert_pr_body_contains_engineer_summary(&pr)?;
    Ok(())
}

fn assert_pr_body_contains_engineer_summary(pr: &PullRequest) -> Result<(), String> {
    if !pr.body.contains(ENGINEER_SUMMARY)
        && !pr.body.contains(PROVIDER_NEUTRAL_ENGINEER_SUMMARY)
        && !pr.body.contains(GRAPH_CONVERGENCE_ENGINEER_SUMMARY)
    {
        return Err(format!(
            "implementation PR body does not contain an approved engineer summary:\n{}",
            pr.body
        ));
    }
    Ok(())
}

async fn assert_codebase_memory_converged(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    issue: ItemNumber,
    admin_user: &str,
) -> Result<FinalStateEvidence, String> {
    let pr = implementation_pr(forge, repository, issue).await?;
    verify_engineer_pr(&pr, issue)?;
    if pr.state != PullRequestState::Merged {
        return Err(format!(
            "implementation PR #{} is not merged yet (state {:?})",
            pr.number, pr.state
        ));
    }
    let merge = pr.merge.as_ref().ok_or("merged PR has no merge record")?;
    let expected_automation = [UserId::new(admin_user), UserId::new("bot")];
    if !expected_automation
        .iter()
        .any(|user| user == &merge.merged_by)
    {
        return Err(format!(
            "PR was merged by {:?}, expected automation identity {:?}",
            merge.merged_by, expected_automation
        ));
    }
    require_labels(&pr.labels, &["implementation"])?;
    reject_labels(&pr.labels, &["landing"])?;

    let ci_observation = completed_ci_observation(forge, repository, &pr).await?;
    let jobs = &ci_observation.jobs;
    if jobs.is_empty() {
        return Err(format!("no completed CI jobs for PR #{}", pr.number));
    }
    if jobs.last().and_then(|job| job.conclusion) != Some(CiJobConclusion::Success) {
        return Err(format!(
            "latest CI verdict for PR #{} is not success: {:?}",
            pr.number,
            jobs.last()
        ));
    }
    if !CiStatus::from_jobs(jobs).is_passed() {
        return Err("latest CI aggregate is not passing".to_string());
    }

    let issue = forge
        .get_issue_by_number(repository, issue)
        .await
        .map_err(|error| format!("source issue lookup failed: {error}"))?
        .ok_or("source issue disappeared")?;
    if issue.state != IssueState::Closed {
        return Err(format!(
            "source issue #{} not closed after merge (state {:?}, labels {:?})",
            issue.number, issue.state, issue.labels
        ));
    }
    require_labels(&issue.labels, &["code"])?;
    reject_labels(&issue.labels, &["untriaged", "ready", "in-progress"])?;

    Ok(FinalStateEvidence {
        issue: issue_evidence(&issue),
        pull_request: pr_evidence(&pr),
        ci_jobs: jobs
            .iter()
            .map(super::convergence::ci_job_evidence)
            .collect(),
        ci_observations: vec![ci_observation_evidence(&ci_observation)],
        ci_heads: Vec::new(),
    })
}

async fn implementation_pr(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    issue: ItemNumber,
) -> Result<PullRequest, String> {
    let pull_requests: Vec<PullRequest> = forge
        .list_pull_requests(repository, PullRequestQuery::default())
        .await
        .map_err(|error| format!("list_pull_requests failed: {error}"))?
        .into_iter()
        .filter(|pr| pr.labels.iter().any(|label| label == "implementation"))
        .collect();
    if pull_requests.len() != 1 {
        return Err(format!(
            "expected exactly one implementation PR, found {}",
            pull_requests.len()
        ));
    }
    let pr = pull_requests.into_iter().next().expect("one PR");
    verify_metadata(&pr, issue)?;
    Ok(pr)
}

fn verify_engineer_pr(pr: &PullRequest, issue: ItemNumber) -> Result<(), String> {
    verify_metadata(pr, issue)?;
    if pr.author_id != UserId::new(ENGINEER) {
        return Err(format!(
            "implementation PR #{} authored by {:?}, not engineer {:?}",
            pr.number, pr.author_id, ENGINEER
        ));
    }
    Ok(())
}

fn verify_metadata(pr: &PullRequest, issue: ItemNumber) -> Result<(), String> {
    let metadata = parse_metadata_block(&pr.body)
        .map_err(|error| format!("implementation PR metadata is malformed: {error}"))?
        .ok_or("implementation PR is missing workflow metadata")?;
    let expected_key = format!("pr-for-code-{issue}");
    if metadata.correlation_key.as_deref() != Some(expected_key.as_str()) {
        return Err(format!(
            "implementation PR correlation key {:?} != {expected_key:?}",
            metadata.correlation_key
        ));
    }
    if !metadata
        .parents
        .iter()
        .any(|parent| parent.is_same_repo() && parent.number == issue)
    {
        return Err(format!(
            "implementation PR parents {:?} do not include issue #{issue}",
            metadata.parents
        ));
    }
    Ok(())
}

pub(super) struct FakeMcpServer {
    pub(super) script_path: PathBuf,
    pub(super) log_path: PathBuf,
    state_path: PathBuf,
    pub(super) project: String,
    pub(super) lifecycle_profile: Option<String>,
    pub(super) safe_tools: Vec<String>,
    pub(super) hidden_tools: Vec<String>,
    pub(super) readiness_delay_ms: u64,
    pub(super) forced_systemic_failure: Option<ForcedSystemicFailureFixture>,
}

pub(super) fn write_fake_mcp(
    root: &Path,
    project: &str,
    lifecycle_profile: Option<&str>,
    safe_tools: &[String],
    hidden_tools: &[String],
    readiness_delay_ms: u64,
    forced_systemic_failure: Option<&ForcedSystemicFailureFixture>,
) -> Result<FakeMcpServer, String> {
    let script_path = root.join("fake-codebase-memory-mcp.py");
    let log_path = root.join("logs/fake-codebase-memory-mcp.jsonl");
    fs::write(
        &script_path,
        if lifecycle_profile == Some("shared-codebase-memory-lifecycle") {
            include_str!("codebase_memory/shared_lifecycle/provider.py")
        } else {
            FAKE_MCP_SCRIPT
        },
    )
    .map_err(|error| format!("write fake MCP server {}: {error}", script_path.display()))?;
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("create fake MCP log dir {}: {error}", parent.display()))?;
    }
    fs::write(&log_path, "")
        .map_err(|error| format!("create fake MCP log {}: {error}", log_path.display()))?;
    let state_path = PathBuf::from(format!("{}.state.json", log_path.display()));
    Ok(FakeMcpServer {
        script_path,
        log_path,
        state_path,
        project: project.to_string(),
        lifecycle_profile: lifecycle_profile.map(str::to_string),
        safe_tools: safe_tools.to_vec(),
        hidden_tools: hidden_tools.to_vec(),
        readiness_delay_ms,
        forced_systemic_failure: forced_systemic_failure.cloned(),
    })
}

#[derive(Clone, Debug)]
struct McpToolCallEvidence {
    name: String,
    arguments: JsonValue,
    delay_ms: Option<u64>,
    is_error: bool,
    fixture_event: Option<String>,
}

fn logged_tool_calls(path: &Path) -> Result<Vec<McpToolCallEvidence>, String> {
    let raw = fs::read_to_string(path)
        .map_err(|error| format!("read MCP call log {}: {error}", path.display()))?;
    Ok(raw
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| {
            let value = serde_json::from_str::<JsonValue>(line).ok()?;
            let name = value.get("tool")?.as_str()?.to_string();
            let arguments = value.get("arguments").cloned().unwrap_or(JsonValue::Null);
            let delay_ms = value.get("delay_ms").and_then(JsonValue::as_u64);
            let is_error = value
                .get("is_error")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false);
            let fixture_event = value
                .get("fixture_event")
                .and_then(JsonValue::as_str)
                .map(str::to_string);
            Some(McpToolCallEvidence {
                name,
                arguments,
                delay_ms,
                is_error,
                fixture_event,
            })
        })
        .collect())
}

fn messages_contain(view: &RequestView, needle: &str) -> bool {
    view.messages
        .iter()
        .any(|message| message.content.contains(needle))
}

fn is_current_root_source_result(content: &str) -> bool {
    let provider_result = content
        .split_once("\n\n[Decision anchor:")
        .map_or(content, |(result, _)| result);
    let Ok(result) = serde_json::from_str::<JsonValue>(provider_result) else {
        return false;
    };
    let selected_source = ["qualified_name", "qualifiedName", "functionName"]
        .iter()
        .any(|field| result.get(field).and_then(JsonValue::as_str).is_some());
    result.get("binding").and_then(JsonValue::as_str) == Some(CURRENT_ROOT_SOURCE_BINDING)
        && selected_source
        && result.get("source").and_then(JsonValue::as_str).is_some()
}

fn snippet(text: &str, max: usize) -> String {
    let mut out = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index >= max {
            out.push('…');
            break;
        }
        out.push(if ch == '\n' { ' ' } else { ch });
    }
    out
}

const FAKE_MCP_SCRIPT: &str = include_str!("fake_codebase_memory_mcp.py");

#[cfg(test)]
#[path = "codebase_memory/result_parsing_tests.rs"]
mod tests;
