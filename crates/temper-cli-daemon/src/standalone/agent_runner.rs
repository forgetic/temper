// SPDX-License-Identifier: MPL-2.0

//! The standalone in-process coding-agent runner.
//!
//! [`InProcessAgentRunner`] implements the orchestrator's
//! [`AgentRunner`](temper_worker::AgentRunner) by calling the agent core
//! ([`run_coding_agent_native_with_hooks`]) directly on the host event loop — no
//! subprocess, no temp files. `WorkspaceContext` flows in as a value and
//! `WorkspaceResult` comes back as the return value; step-progress is reported
//! through the injected [`ProgressSink`] in memory.
//!
//! This is the worker→agent carrier the standalone daemon uses; the distributed
//! deployment keeps the subprocess `OutOfProcessRunner`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

use skein::runtime::RuntimeHandle;
use temper_agent::{
    CodingAgentError, ProviderConfig, RunTotals, run_coding_agent_native_with_hooks,
};
use temper_log::WorkItemRef;
use temper_log::emit::{AgentFinished, AgentStarted, emit_agent_finished, emit_agent_started};
use temper_protocol_agent::{PROTOCOL_VERSION, StepProgress, StepState, WorkspaceContext};
use temper_worker::{
    AgentRunError, AgentRunner, PrFreshnessGuard, ProgressSink, WorkspaceResult,
};

/// Runs coding/triage/review turns in-process on the host loop.
pub struct InProcessAgentRunner {
    handle: RuntimeHandle,
    provider: ProviderConfig,
    max_iterations: usize,
    config_dir: Option<PathBuf>,
    enable_subagents: bool,
    pr_freshness_guard: Option<Arc<dyn PrFreshnessGuard>>,
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
            pr_freshness_guard: None,
        }
    }

    pub fn with_pr_freshness_guard(mut self, guard: Arc<dyn PrFreshnessGuard>) -> Self {
        self.pr_freshness_guard = Some(guard);
        self
    }
}

impl AgentRunner for InProcessAgentRunner {
    fn run(
        &self,
        context: &WorkspaceContext,
        cwd: &Path,
        progress: Arc<dyn ProgressSink>,
    ) -> impl std::future::Future<Output = Result<WorkspaceResult, AgentRunError>> + Send {
        // Emit the same outer start/finish boundary markers as the subprocess
        // wrapper; writable in-process runs also get live checkpoint and plan
        // hooks wired below.
        let step = Arc::new(AtomicU32::new(1));
        let role = context.work_item.role.clone();
        let correlation = context.correlation_key.clone();

        progress.report(StepProgress {
            correlation_key: correlation.clone(),
            step: step.fetch_add(1, Ordering::SeqCst),
            status: format!("start {role} run"),
            state: StepState::Started,
            pushed_sha: None,
            note: Some(format!("protocol v{PROTOCOL_VERSION} (in-process)")),
        });

        // §7 agent boundary events. The `item` ref is the work-item subject tag
        // (`[repo#n]` / `[repo PR#n]`); `kind` is the role's activity verb
        // (architect→triage, engineer→coding). We emit `agent.started` here,
        // up-front and synchronously — *before* the async block / model call —
        // so the start line appears even if the run later stalls on the model.
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
        let pr_freshness_guard = self.pr_freshness_guard.clone();
        let hook_set = super::hooks::hooks_for_context(
            context,
            cwd,
            Arc::clone(&progress),
            pr_freshness_guard,
            Arc::clone(&step),
        );
        let context = context.clone();
        let cwd = cwd.to_path_buf();

        async move {
            let outcome = run_coding_agent_native_with_hooks(
                handle,
                &provider,
                &context,
                &cwd,
                max_iterations,
                config_dir.as_deref(),
                enable_subagents,
                None,
                hook_set.turn_hook,
                hook_set.checkpoint_hook,
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

            progress.report(StepProgress {
                correlation_key: correlation,
                step: step.fetch_add(1, Ordering::SeqCst),
                status: format!("finish {role} run"),
                state: StepState::Done,
                pushed_sha: None,
                note: result.summary.clone(),
            });

            Ok(result)
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
        | CodingAgentError::Parse { .. } => AgentRunError::transient(error.to_string()),
        CodingAgentError::NoProduct | CodingAgentError::UndeclaredVerdict { .. } => {
            AgentRunError::permanent(error.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use temper_protocol_worker::FailureClass;

    #[test]
    fn run_failure_is_transient() {
        let err = classify_coding_agent_error(CodingAgentError::Run("network".into()));
        assert_eq!(err.class, FailureClass::Transient);
    }

    #[test]
    fn no_product_is_permanent() {
        let err = classify_coding_agent_error(CodingAgentError::NoProduct);
        assert_eq!(err.class, FailureClass::Permanent);
    }

    #[test]
    fn parse_failure_is_transient() {
        let err = classify_coding_agent_error(CodingAgentError::Parse {
            snippet: "some text".into(),
            error: "no JSON object".into(),
        });
        assert_eq!(err.class, FailureClass::Transient);
    }

    #[test]
    fn parses_issue_target_number() {
        assert_eq!(
            parse_target_number("Issue { number: ItemNumber(7) }"),
            Some(7)
        );
    }

    #[test]
    fn parses_pull_request_target_number() {
        assert_eq!(
            parse_target_number("PullRequest { number: ItemNumber(44) }"),
            Some(44)
        );
    }

    #[test]
    fn parses_bare_number_form() {
        // Some fixtures format the target without the `ItemNumber(..)` wrapper.
        assert_eq!(parse_target_number("Issue { number: 7 }"), Some(7));
    }

    #[test]
    fn malformed_target_yields_none() {
        assert_eq!(parse_target_number(""), None);
        assert_eq!(parse_target_number("Issue { }"), None);
        assert_eq!(parse_target_number("number: ItemNumber()"), None);
        assert_eq!(parse_target_number("garbage with no marker"), None);
    }

    #[test]
    fn human_count_below_1000_is_raw() {
        assert_eq!(human_count(0), "0");
        assert_eq!(human_count(1), "1");
        assert_eq!(human_count(999), "999");
    }

    #[test]
    fn human_count_thousands_keep_one_decimal() {
        assert_eq!(human_count(1000), "1k");
        assert_eq!(human_count(1500), "1.5k");
        assert_eq!(human_count(6379), "6.4k");
        assert_eq!(human_count(9999), "10k"); // rounds up out of the decimal band
    }

    #[test]
    fn human_count_large_rounds_to_whole_k() {
        assert_eq!(human_count(10_000), "10k");
        assert_eq!(human_count(10_500), "11k");
        assert_eq!(human_count(470_306), "470k");
        assert_eq!(human_count(1_000_000), "1000k");
    }

    #[test]
    fn totals_suffix_renders_humanized_counts() {
        let totals = RunTotals {
            input: 470_306,
            output: 6379,
            tool_calls: 52,
        };
        assert_eq!(totals_suffix(totals), "470k in / 6.4k out, 52 tool calls");
    }

    #[test]
    fn totals_suffix_singular_tool_call() {
        let totals = RunTotals {
            input: 0,
            output: 0,
            tool_calls: 1,
        };
        assert_eq!(totals_suffix(totals), "0 in / 0 out, 1 tool call");
    }

    #[test]
    fn run_kind_maps_roles_to_activities() {
        assert_eq!(run_kind("architect"), "triage");
        assert_eq!(run_kind("engineer"), "coding");
        assert_eq!(run_kind("reviewer"), "review");
        assert_eq!(run_kind("mechanical"), "run");
    }

    fn ctx(owner: &str, name: &str, kind: &str, target: &str) -> WorkspaceContext {
        use temper_protocol_agent::{WorkspaceRepository, WorkspaceWorkItem};
        WorkspaceContext {
            repos: vec![WorkspaceRepository {
                id: format!("forgejo:{owner}/{name}"),
                owner: owner.to_string(),
                name: name.to_string(),
                default_branch: "main".to_string(),
                dir: name.to_string(),
                access: "writable".to_string(),
                base_branch: "main".to_string(),
                branch_hint: None,
            }],
            work_item: WorkspaceWorkItem {
                role: "architect".to_string(),
                queue: "intake".to_string(),
                kind: kind.to_string(),
                target: target.to_string(),
                context: "{}".to_string(),
            },
            action: "triage_intake".to_string(),
            correlation_key: "k".to_string(),
            checkout: None,
            allowed_verdicts: Vec::new(),
            guidance: Default::default(),
            pull_request_freshness: None,
        }
    }

    #[test]
    fn builds_issue_ref_from_context() {
        let context = ctx("ai", "temper", "issue", "Issue { number: ItemNumber(42) }");
        let item = work_item_ref(&context).expect("issue ref");
        assert_eq!(item.repo(), "ai/temper");
        assert_eq!(item.to_string(), "ai/temper#42");
    }

    #[test]
    fn builds_pull_request_ref_from_context() {
        let context = ctx(
            "ai",
            "temper",
            "pull_request",
            "PullRequest { number: ItemNumber(44) }",
        );
        let item = work_item_ref(&context).expect("pr ref");
        assert_eq!(item.to_string(), "ai/temper PR#44");
    }

    #[test]
    fn work_item_ref_is_none_on_unparseable_target() {
        let context = ctx("ai", "temper", "issue", "Issue { }");
        assert!(work_item_ref(&context).is_none());
    }
}
