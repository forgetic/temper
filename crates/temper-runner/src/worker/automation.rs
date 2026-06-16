use super::{Progress, WorkerError, saturating_u64};
use crate::ExternalToolExecutors;
use crate::observability::{
    artifact_ref, execution_error_diagnostic_classes, execution_error_failure_class, labels_delta,
};
use crate::scan::{AutomatedWorkItem, scan_automated_queues};
use crate::workspace_automation::{WorkspaceAutomationOutcome, execute_workspace_automation};
use chrono::{DateTime, Utc};
use temper_forge::{Forge, ItemNumber, RepositoryId};
use temper_log::emit::{TransitionApplied, emit_transition_applied};
use temper_workflow::WorkflowEffect;
use temper_workflow::{
    ArtifactSource, CompiledWorkflow, ExecutionError, Executor, PlanDiagnostic, TransitionId,
    ValidatedWorkflow, VerdictId,
};

#[derive(Default)]
struct AutomationCounts {
    candidates: usize,
    applied: usize,
    merge_conflicts_routed: usize,
    workspace_routed: usize,
    unchanged: usize,
    gate_not_satisfied: usize,
    errors: usize,
}

impl AutomationCounts {
    fn unchanged_total(&self) -> usize {
        self.unchanged.saturating_add(self.gate_not_satisfied)
    }
}

enum ExpectedPreconditionOutcome {
    Unchanged,
    GateNotSatisfied,
}

#[derive(Clone, Copy)]
struct AutomationContext<'a> {
    worker: &'a str,
    repo: &'a RepositoryId,
    workflow_id: &'a str,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_automated_queues<F: Forge + ?Sized>(
    worker: &str,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    executor: &Executor<'_, F>,
    executors: &ExternalToolExecutors,
    forge: &F,
    now: DateTime<Utc>,
) -> Result<Progress, WorkerError> {
    if !compiled
        .queues()
        .iter()
        .any(|queue| queue.automation.is_some())
    {
        return Ok(Progress::unchanged());
    }

    let items = scan_automated_queues(forge, repo, workflow, compiled, now).await?;
    execute_automated_items(
        worker, repo, workflow, compiled, executor, executors, forge, items,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_automated_items<F: Forge + ?Sized>(
    worker: &str,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    executor: &Executor<'_, F>,
    executors: &ExternalToolExecutors,
    forge: &F,
    items: Vec<AutomatedWorkItem>,
) -> Result<Progress, WorkerError> {
    let mut counts = AutomationCounts {
        candidates: items.len(),
        ..AutomationCounts::default()
    };
    let mut progress = Progress::unchanged();
    let context = AutomationContext {
        worker,
        repo,
        workflow_id: workflow.name(),
    };

    for item in items {
        if item.executor.is_some() {
            run_workspace_item(
                &context,
                workflow,
                compiled,
                executors,
                forge,
                &item,
                &mut counts,
                &mut progress,
            )
            .await?;
            continue;
        }
        match executor
            .execute(repo, item.target, &item.transition, &item.actor)
            .await
        {
            Ok(report) => {
                counts.applied = counts.applied.saturating_add(1);
                progress.record(true);
                emit_automation_transition_applied(&context, &item, &report.applied);
                log_automation_item(
                    context.worker,
                    context.repo,
                    context.workflow_id,
                    &item,
                    "applied",
                    None,
                    Vec::new(),
                );
            }
            Err(error) => {
                if route_verdict(
                    &context,
                    executor,
                    &item,
                    &error,
                    &mut counts,
                    &mut progress,
                )
                .await?
                {
                    continue;
                }
                if record_expected_precondition(&context, &item, &error, &mut counts) {
                    continue;
                }

                counts.errors = counts.errors.saturating_add(1);
                let failure_class = execution_error_failure_class(&error);
                let diagnostics = execution_error_diagnostic_classes(&error);
                log_automation_item(
                    context.worker,
                    context.repo,
                    context.workflow_id,
                    &item,
                    "error",
                    Some(&failure_class),
                    diagnostics,
                );
                log_automation_summary(
                    context.worker,
                    context.repo,
                    context.workflow_id,
                    &counts,
                    progress,
                );
                return Err(error.into());
            }
        }
    }

    log_automation_summary(
        context.worker,
        context.repo,
        context.workflow_id,
        &counts,
        progress,
    );
    Ok(progress)
}

/// Services one workspace-backed automated item: invokes the bound workspace,
/// routes on its verdict through the action's `outcomes`, and records the
/// outcome. An unbound executor or a stale work item is a quiet no-op (the next
/// tick retries); a genuine execution failure surfaces as the tick error after
/// the summary is logged, exactly as the direct-execute path does.
#[allow(clippy::too_many_arguments)]
async fn run_workspace_item<F: Forge + ?Sized>(
    context: &AutomationContext<'_>,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    executors: &ExternalToolExecutors,
    forge: &F,
    item: &AutomatedWorkItem,
    counts: &mut AutomationCounts,
    progress: &mut Progress,
) -> Result<(), WorkerError> {
    match execute_workspace_automation(workflow, compiled, executors, forge, context.repo, item)
        .await
    {
        Ok(WorkspaceAutomationOutcome::Applied { routed }) => {
            counts.applied = counts.applied.saturating_add(1);
            counts.workspace_routed = counts.workspace_routed.saturating_add(1);
            progress.record(true);
            // The workspace routes through a transition but does not surface the
            // applied effects here, so the §7 `transition.applied` line carries
            // the routed transition id with an empty label delta.
            emit_transition_applied(TransitionApplied {
                item: &artifact_ref(context.repo, item.target),
                transition: routed.as_str(),
                detail: "",
                labels_delta: "",
            });
            log_workspace_item(context, item, &routed, "routed", None, Vec::new());
            Ok(())
        }
        Ok(WorkspaceAutomationOutcome::Skipped { reason }) => {
            counts.unchanged = counts.unchanged.saturating_add(1);
            log_workspace_item(context, item, &item.transition, reason, None, Vec::new());
            Ok(())
        }
        Err(error) => {
            counts.errors = counts.errors.saturating_add(1);
            let failure_class = execution_error_failure_class(&error);
            let diagnostics = execution_error_diagnostic_classes(&error);
            log_workspace_item(
                context,
                item,
                &item.transition,
                "error",
                Some(&failure_class),
                diagnostics,
            );
            log_automation_summary(
                context.worker,
                context.repo,
                context.workflow_id,
                counts,
                *progress,
            );
            Err(error.into())
        }
    }
}

/// Maps an execution error from the primary automation transition to the
/// built-in verdict the engine derives from it, if any. Today the only
/// engine-derived automation verdict is the merge-conflict verdict; this is the
/// first instance of the verdict -> transition routing mechanism (ADR 0022 §B).
fn verdict_for_error(error: &ExecutionError) -> Option<VerdictId> {
    match error {
        ExecutionError::MergeConflict { .. } => Some(VerdictId::merge_conflict()),
        _ => None,
    }
}

/// Routes a primary-transition failure to the outcome transition declared for
/// the verdict the engine derives from the error, if the automation declares
/// one. Behavior, logging, and counts match the original merge-conflict
/// routing; the merge-conflict verdict is its first (and currently only)
/// engine-derived instance.
async fn route_verdict<F: Forge + ?Sized>(
    context: &AutomationContext<'_>,
    executor: &Executor<'_, F>,
    item: &AutomatedWorkItem,
    error: &ExecutionError,
    counts: &mut AutomationCounts,
    progress: &mut Progress,
) -> Result<bool, WorkerError> {
    let Some(verdict) = verdict_for_error(error) else {
        return Ok(false);
    };
    let ExecutionError::MergeConflict { target, message } = error else {
        return Ok(false);
    };
    let Some(fallback) = item.outcomes.get(&verdict) else {
        return Ok(false);
    };
    let summary = provider_message_summary(message);
    match executor
        .execute(context.repo, *target, fallback, &item.actor)
        .await
    {
        Ok(report) => {
            counts.applied = counts.applied.saturating_add(1);
            counts.merge_conflicts_routed = counts.merge_conflicts_routed.saturating_add(1);
            progress.record(true);
            emit_transition_applied(TransitionApplied {
                item: &artifact_ref(context.repo, *target),
                transition: fallback.as_str(),
                detail: "merge conflict routed to fallback",
                labels_delta: &labels_delta(&report.applied),
            });
            log_merge_conflict_route(
                context,
                item,
                fallback,
                "routed",
                None,
                Vec::new(),
                &summary,
            );
            Ok(true)
        }
        Err(fallback_error) => {
            if record_expected_precondition(context, item, &fallback_error, counts) {
                log_merge_conflict_route(
                    context,
                    item,
                    fallback,
                    "fallback_unchanged",
                    None,
                    execution_error_diagnostic_classes(&fallback_error),
                    &summary,
                );
                return Ok(true);
            }

            counts.errors = counts.errors.saturating_add(1);
            let failure_class = execution_error_failure_class(&fallback_error);
            let diagnostics = execution_error_diagnostic_classes(&fallback_error);
            log_merge_conflict_route(
                context,
                item,
                fallback,
                "fallback_error",
                Some(&failure_class),
                diagnostics,
                &summary,
            );
            log_automation_summary(
                context.worker,
                context.repo,
                context.workflow_id,
                counts,
                *progress,
            );
            Err(fallback_error.into())
        }
    }
}

fn record_expected_precondition(
    context: &AutomationContext<'_>,
    item: &AutomatedWorkItem,
    error: &ExecutionError,
    counts: &mut AutomationCounts,
) -> bool {
    let Some(outcome) = expected_precondition_outcome(error) else {
        return false;
    };
    let diagnostics = execution_error_diagnostic_classes(error);
    match outcome {
        ExpectedPreconditionOutcome::Unchanged => {
            counts.unchanged = counts.unchanged.saturating_add(1);
            log_automation_item(
                context.worker,
                context.repo,
                context.workflow_id,
                item,
                "unchanged",
                None,
                diagnostics,
            );
        }
        ExpectedPreconditionOutcome::GateNotSatisfied => {
            counts.gate_not_satisfied = counts.gate_not_satisfied.saturating_add(1);
            log_automation_item(
                context.worker,
                context.repo,
                context.workflow_id,
                item,
                "gate_not_satisfied",
                None,
                diagnostics,
            );
        }
    }
    true
}

fn expected_precondition_outcome(error: &ExecutionError) -> Option<ExpectedPreconditionOutcome> {
    let ExecutionError::Precondition { diagnostics } = error else {
        return None;
    };
    if diagnostics.is_empty() || !diagnostics.iter().all(is_expected_precondition) {
        return None;
    }
    if diagnostics
        .iter()
        .any(|diagnostic| matches!(diagnostic, PlanDiagnostic::GateNotSatisfied { .. }))
    {
        Some(ExpectedPreconditionOutcome::GateNotSatisfied)
    } else {
        Some(ExpectedPreconditionOutcome::Unchanged)
    }
}

fn is_expected_precondition(diagnostic: &PlanDiagnostic) -> bool {
    matches!(
        diagnostic,
        PlanDiagnostic::StalePrecondition { .. }
            | PlanDiagnostic::ContradictedPrecondition { .. }
            | PlanDiagnostic::GateNotSatisfied { .. }
    )
}

/// Emits the §7 `engine` / `transition.applied` line for a directly applied
/// mechanical automation transition.
///
/// The label delta is derived from the applied effects; the human renderer adds
/// the subject tag and formats the delta. Non-label effects (merges, comments)
/// do not contribute to the delta but still appear under the transition id.
fn emit_automation_transition_applied(
    context: &AutomationContext<'_>,
    item: &AutomatedWorkItem,
    applied: &[WorkflowEffect],
) {
    emit_transition_applied(TransitionApplied {
        item: &artifact_ref(context.repo, item.target),
        transition: item.transition.as_str(),
        detail: "",
        labels_delta: &labels_delta(applied),
    });
}

fn log_automation_item(
    worker: &str,
    repo: &RepositoryId,
    workflow_id: &str,
    item: &AutomatedWorkItem,
    outcome: &str,
    failure_class: Option<&str>,
    diagnostic_classes: Vec<String>,
) {
    let (artifact_type, artifact_number) = source_parts(item.target);
    tracing::debug!(
        target: "temper::engine",
        worker_kind = "mechanical",
        worker,
        repo = repo.as_str(),
        workflow_id,
        queue = %item.queue,
        transition = %item.transition,
        actor = %item.actor,
        artifact_type,
        artifact_number = artifact_number.get(),
        artifact_kind = %item.kind,
        failure_class,
        diagnostic_classes = diagnostic_classes.join(",").as_str(),
        outcome,
        "automation: {} {outcome}",
        item.transition,
    );
}

fn log_merge_conflict_route(
    context: &AutomationContext<'_>,
    item: &AutomatedWorkItem,
    fallback: &TransitionId,
    outcome: &str,
    failure_class: Option<&str>,
    diagnostic_classes: Vec<String>,
    provider_message_summary: &str,
) {
    let (artifact_type, artifact_number) = source_parts(item.target);
    tracing::debug!(
        target: "temper::engine",
        worker_kind = "mechanical",
        worker = context.worker,
        repo = context.repo.as_str(),
        workflow_id = context.workflow_id,
        queue = %item.queue,
        original_transition = %item.transition,
        fallback_transition = %fallback,
        actor = %item.actor,
        artifact_type,
        artifact_number = artifact_number.get(),
        artifact_kind = %item.kind,
        provider_message_summary,
        failure_class,
        diagnostic_classes = diagnostic_classes.join(",").as_str(),
        outcome,
        "automation: merge-conflict route {} -> {fallback} ({outcome})",
        item.transition,
    );
}

fn log_workspace_item(
    context: &AutomationContext<'_>,
    item: &AutomatedWorkItem,
    routed: &TransitionId,
    outcome: &str,
    failure_class: Option<&str>,
    diagnostic_classes: Vec<String>,
) {
    let (artifact_type, artifact_number) = source_parts(item.target);
    let executor = item
        .executor
        .as_ref()
        .map(|id| id.as_str())
        .unwrap_or_default();
    tracing::debug!(
        target: "temper::engine",
        worker_kind = "mechanical",
        worker = context.worker,
        repo = context.repo.as_str(),
        workflow_id = context.workflow_id,
        queue = %item.queue,
        executor,
        primary_transition = %item.transition,
        routed_transition = %routed,
        actor = %item.actor,
        artifact_type,
        artifact_number = artifact_number.get(),
        artifact_kind = %item.kind,
        failure_class,
        diagnostic_classes = diagnostic_classes.join(",").as_str(),
        outcome,
        "automation: workspace {} -> {routed} ({outcome})",
        item.transition,
    );
}

fn provider_message_summary(message: &str) -> String {
    let collapsed = message.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return "provider reported a merge rejection".to_string();
    }
    let mut chars = collapsed.chars();
    let summary: String = chars.by_ref().take(160).collect();
    if chars.next().is_some() {
        format!("{summary}…")
    } else {
        summary
    }
}

fn log_automation_summary(
    worker: &str,
    repo: &RepositoryId,
    workflow_id: &str,
    counts: &AutomationCounts,
    progress: Progress,
) {
    tracing::debug!(
        target: "temper::engine",
        worker_kind = "mechanical",
        worker,
        repo = repo.as_str(),
        workflow_id,
        candidate_count = saturating_u64(counts.candidates),
        applied_count = saturating_u64(counts.applied),
        merge_conflict_routed_count = saturating_u64(counts.merge_conflicts_routed),
        workspace_routed_count = saturating_u64(counts.workspace_routed),
        unchanged_count = saturating_u64(counts.unchanged_total()),
        stale_unchanged_count = saturating_u64(counts.unchanged),
        gate_not_satisfied_count = saturating_u64(counts.gate_not_satisfied),
        error_count = saturating_u64(counts.errors),
        changed = progress.changed,
        progress_actions = u64::from(progress.actions),
        "automation pass: {} candidate(s), {} applied, {} error(s)",
        counts.candidates,
        counts.applied,
        counts.errors,
    );
}

fn source_parts(source: ArtifactSource) -> (&'static str, ItemNumber) {
    match source {
        ArtifactSource::Issue { number } => ("issue", number),
        ArtifactSource::PullRequest { number } => ("pull_request", number),
    }
}
