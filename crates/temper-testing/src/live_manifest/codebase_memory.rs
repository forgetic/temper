use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use jig_core::{Reply, RequestView, Script, ScriptFile};
use jig_server::FakeLlm;
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
mod graph_consumption;
mod mapped_decision_gap_recovery;
mod mapped_decision_gap_recovery_fake;
mod mapped_denied_shell_classification;
mod mapped_denied_shell_classification_fake;
mod mapped_graph_consumption;
mod mapped_graph_consumption_fake;
mod mapped_graph_convergence;
mod mapped_graph_convergence_fake;
mod mapped_ordinary_convergence_fake;
mod privacy;
mod provider_result_anchor;
mod result_driven_fake;
mod result_driven_guidance;
mod sequential_graph_evidence;
mod stable_rebind;
mod typed_lineage_anchor;
mod typed_lineage_fake;
use aggregate::privacy_safe_checkpoints;
pub(super) use configuration::{ToolConfiguration, tune_codebase_memory_config};
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
    fs::write(&script_path, FAKE_MCP_SCRIPT)
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

#[derive(Debug)]
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

pub(super) struct CodebaseMemoryFake {
    fake: FakeLlm,
    engineer_requests: Arc<AtomicUsize>,
    observations: Arc<Mutex<ModelObservations>>,
    require_current_root_source: bool,
    privacy_safe_log: bool,
}

#[derive(Default)]
struct ModelObservations {
    prompt_guidance_seen: bool,
    memory_result_seen: bool,
    current_root_source_seen: bool,
    code_refinement_seen: bool,
    graph_trace_seen: bool,
    current_root_source_results: usize,
    safe_failure_seen: bool,
    raw_provider_text_seen: bool,
    bounded_graph_result_seen: bool,
    oversized_message_seen: bool,
}

impl CodebaseMemoryFake {
    pub(super) fn start(
        script_path: &Path,
        require_current_root_source: bool,
        lifecycle_profile: Option<&str>,
    ) -> Result<Self, String> {
        let script = ScriptFile::load(script_path)
            .map_err(|error| {
                format!(
                    "load scenario Jig script {}: {error}",
                    script_path.display()
                )
            })?
            .into_script();
        let engineer_requests = Arc::new(AtomicUsize::new(0));
        let request_count = Arc::clone(&engineer_requests);
        let observations = Arc::new(Mutex::new(ModelObservations::default()));
        let observations_for_rule = Arc::clone(&observations);
        let fake = if matches!(
            lifecycle_profile,
            Some("result-driven-decision-guidance" | "provider-result-anchor")
        ) {
            result_driven_fake::start(request_count, observations_for_rule)?
        } else if lifecycle_profile == Some("provider-neutral-anchor-lineage") {
            typed_lineage_fake::start(request_count, observations_for_rule)?
        } else if lifecycle_profile == Some("mapped-live-graph-consumption") {
            mapped_graph_consumption_fake::start(request_count, observations_for_rule)?
        } else if lifecycle_profile == Some("mapped-live-denied-shell-classification") {
            mapped_denied_shell_classification_fake::start(request_count, observations_for_rule)?
        } else if lifecycle_profile == Some("mapped-live-ordinary-tool-convergence") {
            mapped_ordinary_convergence_fake::start(request_count, observations_for_rule)?
        } else if lifecycle_profile == Some("mapped-live-graph-convergence") {
            mapped_graph_convergence_fake::start(request_count, observations_for_rule)?
        } else if lifecycle_profile == Some("mapped-live-decision-gap-recovery") {
            mapped_decision_gap_recovery_fake::start(request_count, observations_for_rule)?
        } else {
            FakeLlm::start(Script::rule(move |view| {
                if !messages_contain(view, "ROLE: engineer") {
                    return Reply::text("unexpected codebase-memory fake-LLM request");
                }
                request_count.fetch_add(1, Ordering::SeqCst);
                let mut observations = observations_for_rule.lock().expect("observations lock");
                if messages_contain(view, "CODEBASE MEMORY") {
                    observations.prompt_guidance_seen = true;
                }
                if messages_contain(view, MEMORY_RESULT_NEEDLE)
                    || messages_contain(view, "SEQUENTIAL_GRAPH_RESULT")
                {
                    observations.memory_result_seen = true;
                }
                if messages_contain(view, "FAKE_MCP_CODE_RESULT")
                    || messages_contain(view, "SEQUENTIAL_CODE_RESULT")
                {
                    observations.code_refinement_seen = true;
                }
                if messages_contain(view, "FAKE_MCP_TRACE_RESULT")
                    || messages_contain(view, "SEQUENTIAL_TRACE_RESULT")
                {
                    observations.graph_trace_seen = true;
                }
                let current_root_source_results = view
                    .messages
                    .iter()
                    .filter(|message| is_current_root_source_result(&message.content))
                    .count();
                observations.current_root_source_seen |= current_root_source_results > 0;
                observations.current_root_source_results += current_root_source_results;
                if messages_contain(view, SAFE_PROVIDER_FAILURE) {
                    observations.safe_failure_seen = true;
                }
                if messages_contain(view, RAW_PROVIDER_FAILURE_NEEDLE) {
                    observations.raw_provider_text_seen = true;
                }
                if messages_contain(view, BOUNDED_GRAPH_RESULT_NEEDLE) {
                    observations.bounded_graph_result_seen = true;
                }
                if view
                    .messages
                    .iter()
                    .any(|message| message.content.len() > MAX_MODEL_MESSAGE_BYTES)
                {
                    observations.oversized_message_seen = true;
                }
                drop(observations);
                script.next_reply(view)
            }))
            .map_err(|error| format!("start scenario Jig fake LLM: {error}"))?
        };
        Ok(Self {
            fake,
            engineer_requests,
            observations,
            require_current_root_source,
            privacy_safe_log: privacy::is_privacy_safe_profile(lifecycle_profile),
        })
    }

    pub(super) fn base_url(&self) -> String {
        self.fake.base_url()
    }

    pub(super) fn engineer_requests(&self) -> usize {
        self.engineer_requests.load(Ordering::SeqCst)
    }

    fn validate_observations(&self, mcp: &FakeMcpServer) -> Result<(), String> {
        let (
            prompt_guidance_seen,
            memory_result_seen,
            current_root_source_seen,
            safe_failure_seen,
            raw_provider_text_seen,
            bounded_graph_result_seen,
            code_refinement_seen,
            graph_trace_seen,
            current_root_source_results,
            oversized_message_seen,
        ) = {
            let observations = self
                .observations
                .lock()
                .map_err(|_| "model observation mutex poisoned".to_string())?;
            (
                observations.prompt_guidance_seen,
                observations.memory_result_seen,
                observations.current_root_source_seen,
                observations.safe_failure_seen,
                observations.raw_provider_text_seen,
                observations.bounded_graph_result_seen,
                observations.code_refinement_seen,
                observations.graph_trace_seen,
                observations.current_root_source_results,
                observations.oversized_message_seen,
            )
        };
        if !prompt_guidance_seen {
            return Err(format!(
                "fake LLM did not receive CODEBASE MEMORY prompt guidance\n{}",
                self.log_tail()
            ));
        }
        if !memory_result_seen {
            return Err(format!(
                "fake LLM did not receive the fake MCP graph result\n{}",
                self.log_tail()
            ));
        }
        if self.require_current_root_source && !current_root_source_seen {
            return Err(format!(
                "fake LLM did not receive source served after current-checkout rebinding\n{}",
                self.log_tail()
            ));
        }
        if !bounded_graph_result_seen
            && !matches!(
                mcp.lifecycle_profile.as_deref(),
                Some(
                    "graph-consumption"
                        | "sequential-graph-evidence"
                        | "result-driven-decision-guidance"
                        | "provider-result-anchor"
                        | "provider-neutral-anchor-lineage"
                        | "mapped-live-graph-consumption"
                        | "mapped-live-denied-shell-classification"
                        | "mapped-live-ordinary-tool-convergence"
                        | "mapped-live-graph-convergence"
                        | "mapped-live-decision-gap-recovery"
                )
            )
        {
            return Err(format!(
                "fake LLM did not receive the bounded graph result marker\n{}",
                self.log_tail()
            ));
        }
        if mcp.forced_systemic_failure.is_some() && !safe_failure_seen {
            return Err(format!(
                "fake LLM did not receive the bounded typed systemic diagnostic\n{}",
                self.log_tail()
            ));
        }
        if matches!(
            mcp.lifecycle_profile.as_deref(),
            Some(
                "graph-consumption"
                    | "sequential-graph-evidence"
                    | "result-driven-decision-guidance"
                    | "provider-result-anchor"
                    | "provider-neutral-anchor-lineage"
                    | "mapped-live-graph-consumption"
                    | "mapped-live-denied-shell-classification"
                    | "mapped-live-ordinary-tool-convergence"
                    | "mapped-live-graph-convergence"
                    | "mapped-live-decision-gap-recovery"
            )
        ) && !(graph_trace_seen
            && current_root_source_results >= 2
            && (matches!(
                mcp.lifecycle_profile.as_deref(),
                Some(
                    "provider-neutral-anchor-lineage"
                        | "mapped-live-graph-consumption"
                        | "mapped-live-denied-shell-classification"
                        | "mapped-live-ordinary-tool-convergence"
                        | "mapped-live-graph-convergence"
                        | "mapped-live-decision-gap-recovery"
                )
            ) || code_refinement_seen))
        {
            return Err(format!(
                "fake LLM did not consume the complete graph-to-graph/current-root source chain\n{}",
                self.log_tail()
            ));
        }
        let minimum_requests =
            if mcp.lifecycle_profile.as_deref() == Some("mapped-live-ordinary-tool-convergence") {
                15
            } else if matches!(
                mcp.lifecycle_profile.as_deref(),
                Some(
                    "provider-neutral-anchor-lineage"
                        | "mapped-live-graph-consumption"
                        | "mapped-live-denied-shell-classification"
                        | "mapped-live-graph-convergence"
                        | "mapped-live-decision-gap-recovery"
                )
            ) {
                8
            } else {
                9
            };
        if self.engineer_requests() < minimum_requests {
            return Err(format!(
                "fake LLM did not complete the codebase-memory validation loop\n{}",
                self.log_tail()
            ));
        }
        if raw_provider_text_seen {
            return Err(format!(
                "raw provider failure text leaked into the fake LLM request\n{}",
                self.log_tail()
            ));
        }
        if oversized_message_seen {
            return Err(format!(
                "a model-visible message exceeded the scenario's bounded result allowance\n{}",
                self.log_tail()
            ));
        }
        Ok(())
    }

    pub(super) fn log_tail(&self) -> String {
        let requests = self.fake.requests();
        let observations = self.observations.lock().expect("observations lock");
        let mut lines = vec![format!(
            "observations: prompt_guidance_seen={} memory_result_seen={} current_root_source_seen={} code_refinement_seen={} graph_trace_seen={} current_root_source_results={} bounded_graph_result_seen={} safe_failure_seen={} raw_provider_text_seen={} oversized_message_seen={}",
            observations.prompt_guidance_seen,
            observations.memory_result_seen,
            observations.current_root_source_seen,
            observations.code_refinement_seen,
            observations.graph_trace_seen,
            observations.current_root_source_results,
            observations.bounded_graph_result_seen,
            observations.safe_failure_seen,
            observations.raw_provider_text_seen,
            observations.oversized_message_seen,
        )];
        if requests.is_empty() {
            lines.push("<fake LLM received no requests>".to_string());
            return lines.join("\n");
        }
        let start = requests.len().saturating_sub(20);
        lines.extend(
            requests[start..]
                .iter()
                .enumerate()
                .map(|(offset, request)| {
                    let index = start + offset + 1;
                    let view = request.view.as_ref();
                    let prior = view.map(|v| v.prior_tool_results).unwrap_or_default();
                    if self.privacy_safe_log {
                        return format!(
                            "#{index} {} {} role=engineer prior_tool_results={prior}",
                            request.method, request.path
                        );
                    }
                    let last = view
                        .and_then(RequestView::last_message)
                        .map(|m| format!("{}: {}", m.role, snippet(&m.content, 160)))
                        .unwrap_or_else(|| "<no projected message>".to_string());
                    format!(
                        "#{index} {} {} role=engineer prior_tool_results={prior} last={last}",
                        request.method, request.path
                    )
                }),
        );
        lines.join("\n")
    }
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
