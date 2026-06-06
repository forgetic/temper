use super::{saturating_u64, Progress, WorkerError};
use crate::observability::{
    execution_error_diagnostic_classes, execution_error_failure_class, StructuredEvent,
};
use crate::scan::{scan_automated_queues, AutomatedWorkItem};
use chrono::{DateTime, Utc};
use temper_forge::{Forge, ItemNumber, RepositoryId};
use temper_workflow::{
    ArtifactSource, CompiledWorkflow, ExecutionError, Executor, PlanDiagnostic, TransitionId,
    ValidatedWorkflow, VerdictId,
};

#[derive(Default)]
struct AutomationCounts {
    candidates: usize,
    applied: usize,
    merge_conflicts_routed: usize,
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

pub(crate) async fn execute_automated_queues<F: Forge + ?Sized>(
    worker: &str,
    repo: &RepositoryId,
    workflow: &ValidatedWorkflow,
    compiled: &CompiledWorkflow,
    executor: &Executor<'_, F>,
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
        match executor
            .execute(repo, item.target, &item.transition, &item.actor)
            .await
        {
            Ok(_) => {
                counts.applied = counts.applied.saturating_add(1);
                progress.record(true);
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
        Ok(_) => {
            counts.applied = counts.applied.saturating_add(1);
            counts.merge_conflicts_routed = counts.merge_conflicts_routed.saturating_add(1);
            progress.record(true);
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
    let mut event = StructuredEvent::new("mechanical_automation_execution")
        .string("worker_kind", "mechanical")
        .string("worker", worker)
        .string("repo", repo.to_string())
        .string("workflow_id", workflow_id)
        .string("queue", item.queue.to_string())
        .string("transition", item.transition.to_string())
        .string("actor", item.actor.to_string())
        .string("artifact_type", artifact_type)
        .number("artifact_number", artifact_number.get())
        .string("artifact_kind", item.kind.to_string())
        .string("outcome", outcome);
    if let Some(failure_class) = failure_class {
        event = event.string("failure_class", failure_class);
    }
    if !diagnostic_classes.is_empty() {
        event = event
            .number("diagnostic_count", saturating_u64(diagnostic_classes.len()))
            .string_array("diagnostic_classes", diagnostic_classes);
    }
    eprintln!("{}", event.render());
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
    let mut event = StructuredEvent::new("mechanical_automation_merge_conflict_route")
        .string("worker_kind", "mechanical")
        .string("worker", context.worker)
        .string("repo", context.repo.to_string())
        .string("workflow_id", context.workflow_id)
        .string("queue", item.queue.to_string())
        .string("original_transition", item.transition.to_string())
        .string("fallback_transition", fallback.to_string())
        .string("actor", item.actor.to_string())
        .string("artifact_type", artifact_type)
        .number("artifact_number", artifact_number.get())
        .string("artifact_kind", item.kind.to_string())
        .string("provider_message_summary", provider_message_summary)
        .string("outcome", outcome);
    if let Some(failure_class) = failure_class {
        event = event.string("failure_class", failure_class);
    }
    if !diagnostic_classes.is_empty() {
        event = event
            .number("diagnostic_count", saturating_u64(diagnostic_classes.len()))
            .string_array("diagnostic_classes", diagnostic_classes);
    }
    eprintln!("{}", event.render());
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
    eprintln!(
        "{}",
        StructuredEvent::new("mechanical_automation_summary")
            .string("worker_kind", "mechanical")
            .string("worker", worker)
            .string("repo", repo.to_string())
            .string("workflow_id", workflow_id)
            .number("candidate_count", saturating_u64(counts.candidates))
            .number("applied_count", saturating_u64(counts.applied))
            .number(
                "merge_conflict_routed_count",
                saturating_u64(counts.merge_conflicts_routed),
            )
            .number("unchanged_count", saturating_u64(counts.unchanged_total()))
            .number("stale_unchanged_count", saturating_u64(counts.unchanged))
            .number(
                "gate_not_satisfied_count",
                saturating_u64(counts.gate_not_satisfied),
            )
            .number("error_count", saturating_u64(counts.errors))
            .boolean("changed", progress.changed)
            .number("progress_actions", u64::from(progress.actions))
            .render()
    );
}

fn source_parts(source: ArtifactSource) -> (&'static str, ItemNumber) {
    match source {
        ArtifactSource::Issue { number } => ("issue", number),
        ArtifactSource::PullRequest { number } => ("pull_request", number),
    }
}
