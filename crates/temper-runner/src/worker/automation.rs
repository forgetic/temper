use super::{saturating_u64, Progress, WorkerError};
use crate::observability::{
    execution_error_diagnostic_classes, execution_error_failure_class, StructuredEvent,
};
use crate::scan::{scan_automated_queues, AutomatedWorkItem};
use chrono::{DateTime, Utc};
use temper_forge::{Forge, ItemNumber, RepositoryId};
use temper_workflow::{
    ArtifactSource, CompiledWorkflow, ExecutionError, Executor, PlanDiagnostic, ValidatedWorkflow,
};

#[derive(Default)]
struct AutomationCounts {
    candidates: usize,
    applied: usize,
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

    for item in items {
        match executor
            .execute(repo, item.target, &item.transition, &item.actor)
            .await
        {
            Ok(_) => {
                counts.applied = counts.applied.saturating_add(1);
                progress.record(true);
                log_automation_item(
                    worker,
                    repo,
                    workflow.name(),
                    &item,
                    "applied",
                    None,
                    Vec::new(),
                );
            }
            Err(error) => {
                if let Some(outcome) = expected_precondition_outcome(&error) {
                    let diagnostics = execution_error_diagnostic_classes(&error);
                    match outcome {
                        ExpectedPreconditionOutcome::Unchanged => {
                            counts.unchanged = counts.unchanged.saturating_add(1);
                            log_automation_item(
                                worker,
                                repo,
                                workflow.name(),
                                &item,
                                "unchanged",
                                None,
                                diagnostics,
                            );
                        }
                        ExpectedPreconditionOutcome::GateNotSatisfied => {
                            counts.gate_not_satisfied = counts.gate_not_satisfied.saturating_add(1);
                            log_automation_item(
                                worker,
                                repo,
                                workflow.name(),
                                &item,
                                "gate_not_satisfied",
                                None,
                                diagnostics,
                            );
                        }
                    }
                    continue;
                }

                counts.errors = counts.errors.saturating_add(1);
                let failure_class = execution_error_failure_class(&error);
                let diagnostics = execution_error_diagnostic_classes(&error);
                log_automation_item(
                    worker,
                    repo,
                    workflow.name(),
                    &item,
                    "error",
                    Some(&failure_class),
                    diagnostics,
                );
                log_automation_summary(worker, repo, workflow.name(), &counts, progress);
                return Err(error.into());
            }
        }
    }

    log_automation_summary(worker, repo, workflow.name(), &counts, progress);
    Ok(progress)
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
