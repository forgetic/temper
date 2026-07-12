// SPDX-License-Identifier: MPL-2.0

//! The standalone in-process coding-agent runner.
//!
//! [`InProcessAgentRunner`] implements the orchestrator's
//! [`AgentRunner`](temper_worker::AgentRunner) by calling the agent core
//! ([`run_coding_agent_native_with_totals`]) directly on the host event loop —
//! no subprocess, no temp files. `WorkspaceContext` flows in as a value and
//! `WorkspaceResult` comes back as the return value.
//!
//! This is the worker→agent carrier the standalone daemon uses; the distributed
//! deployment keeps the subprocess `OutOfProcessRunner`.

use std::path::{Path, PathBuf};
use std::time::Instant;

use skein::runtime::RuntimeHandle;
use temper_agent::{
    CodingAgentError, ForgeContextHost, ProviderConfig, RunTotals, SubmitForPrHost,
    run_coding_agent_native_with_totals_tool_config_and_hosts,
};
use temper_log::WorkItemRef;
use temper_log::emit::{AgentFinished, AgentStarted, emit_agent_finished, emit_agent_started};
use temper_protocol_agent::{AgentToolConfig, WorkspaceContext};
use temper_worker::{
    AcceptedSubmitProofStore, AgentForgeContextHost, AgentRunError, AgentRunOutput, AgentRunner,
};

/// Runs coding/triage/review turns in-process on the host loop.
pub struct InProcessAgentRunner {
    handle: RuntimeHandle,
    provider: ProviderConfig,
    max_iterations: usize,
    config_dir: Option<PathBuf>,
    enable_subagents: bool,
    tool_config: Option<AgentToolConfig>,
    submit_for_pr: SubmitForPrHost,
    forge_context: Option<AgentForgeContextHost>,
}

impl InProcessAgentRunner {
    pub fn new(
        handle: RuntimeHandle,
        provider: ProviderConfig,
        max_iterations: usize,
        config_dir: Option<PathBuf>,
        enable_subagents: bool,
    ) -> Self {
        Self {
            handle,
            provider,
            max_iterations,
            config_dir,
            enable_subagents,
            tool_config: None,
            submit_for_pr: std::sync::Arc::new(|request, context, cwd| {
                temper_worker::submit_for_pr_pre_push_response_blocking(request, context, cwd)
            }),
            forge_context: None,
        }
    }

    /// Sets the non-secret agent tool config stored with this in-process
    /// runner. The native coding loop registers codebase-memory tools from it
    /// when it applies to the current role.
    #[must_use]
    pub fn with_tool_config(mut self, tool_config: Option<AgentToolConfig>) -> Self {
        self.tool_config = tool_config;
        self
    }

    /// Returns the stored tool config when it applies to `role`.
    pub fn tool_config_for_role(&self, role: &str) -> Option<&AgentToolConfig> {
        self.tool_config
            .as_ref()
            .filter(|config| config.enabled_for_role(role))
    }

    /// Overrides the host-controlled `submit_for_pr` gate used by writable
    /// engineer sessions.
    #[must_use]
    pub fn with_submit_for_pr_host(mut self, submit_for_pr: SubmitForPrHost) -> Self {
        self.submit_for_pr = submit_for_pr;
        self
    }

    /// Installs an asynchronous assignment-bound Forge context host.
    #[must_use]
    pub fn with_forge_context_host(mut self, forge_context: AgentForgeContextHost) -> Self {
        self.forge_context = Some(forge_context);
        self
    }
}

impl AgentRunner for InProcessAgentRunner {
    fn run(
        &self,
        job_id: &str,
        context: &WorkspaceContext,
        cwd: &Path,
    ) -> impl std::future::Future<Output = Result<AgentRunOutput, AgentRunError>> + Send {
        // §7 agent boundary events. The `item` ref is the work-item subject tag
        // (`[repo#n]` / `[repo PR#n]`); `kind` is the role's activity verb
        // (architect→triage, engineer→coding). We emit `agent.started` here,
        // up-front and synchronously — *before* the async block / model call —
        // so the start line appears even if the run later stalls on the model.
        let role = context.work_item.role.clone();
        let item = work_item_ref(context);
        let kind = run_kind(&role);
        let started = Instant::now();
        if let Some(item) = item.as_ref() {
            emit_agent_started(AgentStarted {
                item,
                role: &role,
                kind,
                detail: started_detail(kind),
            });
        }

        let handle = self.handle.clone();
        let provider = self.provider.clone();
        let max_iterations = self.max_iterations;
        let config_dir = self.config_dir.clone();
        let enable_subagents = self.enable_subagents;
        let tool_config = self.tool_config.clone();
        let submit_for_pr = self.submit_for_pr.clone();
        let accepted_submit = AcceptedSubmitProofStore::new();
        let accepted_submit_for_host = accepted_submit.clone();
        let submit_for_pr: SubmitForPrHost = std::sync::Arc::new(move |request, context, cwd| {
            temper_worker::handle_submit_for_pr_with_proof(
                &accepted_submit_for_host,
                |request, context, cwd| submit_for_pr(request, context, cwd),
                request,
                context,
                cwd,
            )
        });
        let submit_for_pr = Some(submit_for_pr);
        let forge_context: Option<ForgeContextHost> = self.forge_context.clone().map(|host| {
            let job_id = job_id.to_string();
            std::sync::Arc::new(move |operation| host(job_id.clone(), operation))
                as ForgeContextHost
        });
        let context = context.clone();
        let cwd = cwd.to_path_buf();

        async move {
            let outcome = run_coding_agent_native_with_totals_tool_config_and_hosts(
                handle,
                &provider,
                &context,
                &cwd,
                max_iterations,
                config_dir.as_deref(),
                enable_subagents,
                tool_config.as_ref(),
                submit_for_pr,
                forge_context,
            )
            .await
            .map_err(classify_coding_agent_error);

            // §7 `agent.finished` on BOTH paths: a stalled/failed run still
            // gets a `done in <dur> | <summary>` line so the agent plane never
            // shows a dangling `start` with no terminus. The summary carries
            // the verdict on success (e.g. `verdict=ready_code`) or the error
            // classification on failure. On success we also append the run's
            // humanized token totals (`<Nk> in / <Nk> out, <N> tool calls`); a
            // failed run has no meaningful totals to report.
            let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            if let Some(item) = item.as_ref() {
                let summary = match &outcome {
                    Ok((result, totals)) => {
                        let base = result.summary.clone().unwrap_or_else(|| "done".to_string());
                        format!("{base} | {}", totals_suffix(*totals))
                    }
                    Err(error) => format!("failed: {}", error.message),
                };
                emit_agent_finished(AgentFinished {
                    item,
                    role: &role,
                    kind,
                    duration_ms,
                    summary: &summary,
                });
            }

            let (result, _totals) = outcome?;

            Ok(AgentRunOutput {
                result,
                accepted_submit: accepted_submit.latest(),
            })
        }
    }
}

/// The §7 run-kind (`role/kind`) for a worker role.
///
/// The human renderer formats `role/kind` (e.g. `architect/triage`,
/// `engineer/coding`). We map the role's *activity verb* here: architect
/// triages, the engineer codes, the reviewer reviews. Unknown roles fall back
/// to a neutral `run` so the line still reads sensibly.
fn run_kind(role: &str) -> &'static str {
    match role {
        "architect" => "triage",
        "engineer" => "coding",
        "reviewer" => "review",
        _ => "run",
    }
}

/// The one-line `detail` for the `agent.started` line, per §7's examples.
fn started_detail(kind: &str) -> &'static str {
    match kind {
        "triage" => "reading issue + repo context",
        "coding" => "preparing workspace, implementing",
        "review" => "reviewing changes",
        _ => "running",
    }
}

/// The token-totals suffix appended to a successful `agent.finished` summary.
///
/// Renders `<input> in / <output> out, <N> tool calls` with the token counts
/// humanized via [`human_count`] (the §7 example: `470k in / 6.4k out, 52 tool
/// calls`). The tool-call count is kept raw — it is a small cardinal number, not
/// a token volume — so a one-off run reads `1 tool call`.
fn totals_suffix(totals: RunTotals) -> String {
    let calls = totals.tool_calls;
    let unit = if calls == 1 {
        "tool call"
    } else {
        "tool calls"
    };
    format!(
        "{} in / {} out, {calls} {unit}",
        human_count(totals.input),
        human_count(totals.output),
    )
}

/// Humanizes a token count with a `k` suffix above 1000.
///
/// Under 1000 the raw integer is shown (`0`, `999`). At/above 1000 the value is
/// expressed in thousands: 1000–9999 keep one decimal of precision (`6379` ->
/// `6.4k`), 10_000 and up round to a whole `k` (`470306` -> `470k`). A trailing
/// `.0` is dropped so a round value stays tight (`2000` -> `2k`). Arithmetic is
/// integer-only (half-up rounding) to avoid float surprises.
fn human_count(n: u64) -> String {
    if n < 1000 {
        return n.to_string();
    }
    if n >= 10_000 {
        // Round to the nearest whole thousand: floor((n + 500) / 1000).
        let thousands = (n + 500) / 1000;
        return format!("{thousands}k");
    }
    // 1000–9999: one decimal, i.e. tenths = round(n / 100) half-up.
    let tenths = (n + 50) / 100;
    let whole = tenths / 10;
    let frac = tenths % 10;
    if frac == 0 {
        format!("{whole}k")
    } else {
        format!("{whole}.{frac}k")
    }
}

/// Builds the §7 work-item subject reference from the workspace context.
///
/// `repo` is the primary repository's bare `owner/name` path; the number and
/// issue-vs-PR kind come from the work item. Returns `None` when there is no
/// primary repo or the target number cannot be parsed — in that case the agent
/// events are skipped rather than logged against a bogus `#0` ref.
fn work_item_ref(context: &WorkspaceContext) -> Option<WorkItemRef> {
    let repo = context.primary()?;
    let repo_path = format!("{}/{}", repo.owner, repo.name);
    let number = parse_target_number(&context.work_item.target)?;
    let is_pr = context.work_item.kind == "pull_request";
    Some(if is_pr {
        WorkItemRef::pull_request(repo_path, number)
    } else {
        WorkItemRef::issue(repo_path, number)
    })
}

/// Extracts the artifact number from a Debug-formatted target string.
///
/// The worker builds `target` as e.g. `Issue { number: ItemNumber(7) }` or
/// `PullRequest { number: ItemNumber(44) }`
/// ([`temper_worker`]'s context assembly). For robustness this also accepts the
/// bare `number: 7` form some fixtures use. Returns `None` if no `number:`
/// segment with a parseable integer is found, so the caller can skip the emit.
fn parse_target_number(target: &str) -> Option<u64> {
    // Find the `number:` key, then take the first run of ASCII digits after it
    // (skipping the `ItemNumber(` wrapper if present).
    let after_key = target.split("number:").nth(1)?;
    let digits: String = after_key
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// Map an agent-core error to the worker's transient/permanent classification.
fn classify_coding_agent_error(error: CodingAgentError) -> AgentRunError {
    match error {
        CodingAgentError::Provider(_)
        | CodingAgentError::Run(_)
        | CodingAgentError::AgentStopped(_)
        | CodingAgentError::ModelUnavailable { .. }
        | CodingAgentError::CodebaseMemory(_)
        | CodingAgentError::Parse { .. } => AgentRunError::transient(error.to_string()),
        CodingAgentError::NoProduct
        | CodingAgentError::UndeclaredVerdict { .. }
        | CodingAgentError::InvalidVerdictResult(_) => AgentRunError::permanent(error.to_string()),
    }
}

#[cfg(test)]
mod tests;
