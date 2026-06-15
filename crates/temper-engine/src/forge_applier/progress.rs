// SPDX-License-Identifier: MPL-2.0

//! The [`ResultApplier`] trait impl for [`ForgeApplier`] and the step-progress
//! checkpoint comments it records.

use temper_forge_model::{CreateComment, Forge};
use temper_worker_protocol::{JobProgress, JobResult, ResultStatus};

use crate::InFlightJob;
use crate::applier::ResultApplier;
use crate::forge_applier::ForgeApplier;

#[async_trait::async_trait]
impl<F: Forge + 'static> ResultApplier for ForgeApplier<F> {
    async fn apply(&self, job: InFlightJob, result: JobResult) {
        match result.status {
            ResultStatus::Success => self.apply_success(job, result).await,
            ResultStatus::Failure => self.apply_failure(job, result).await,
        }
    }

    /// Records one step-progress checkpoint as a comment on the job's source
    /// issue.
    ///
    /// Idempotent keyed by `(correlation_key, step, state)`: every progress
    /// comment carries a machine-readable marker line, and a checkpoint whose
    /// marker already exists on the issue is skipped, so worker re-delivery
    /// and daemon restarts cannot duplicate forge state.
    async fn apply_progress(&self, job: InFlightJob, progress: JobProgress) {
        let Some((repository, issue)) = self.resolve_issue(&job).await else {
            return;
        };
        let issue_id = issue.id;
        let number = issue.number;

        let marker = progress_marker(&progress);
        match self.forge.list_issue_comments(&issue_id).await {
            Ok(comments) => {
                if comments
                    .iter()
                    .any(|comment| comment.body.contains(&marker))
                {
                    return;
                }
            }
            Err(error) => {
                eprintln!(
                    "temper-daemon: forge applier could not list comments for progress on job_id={} repo={} issue={}: {error}",
                    job.job_id, job.repo, number
                );
                return;
            }
        }

        let body = progress_comment_body(&job.role, &progress, &marker);
        if let Err(error) = self
            .forge
            .add_issue_comment(&issue_id, CreateComment { body })
            .await
        {
            eprintln!(
                "temper-daemon: forge applier could not record progress for job_id={} repo={} issue={}: {error}",
                job.job_id, job.repo, number
            );
        }
        let _ = repository;
    }
}

/// The machine-readable idempotency marker for one progress checkpoint.
fn progress_marker(progress: &JobProgress) -> String {
    format!(
        "<!-- temper-progress correlation_key={} step={} state={} -->",
        progress.correlation_key, progress.step, progress.state
    )
}

/// The human-facing progress comment (marker line + one checklist line).
fn progress_comment_body(role: &str, progress: &JobProgress, marker: &str) -> String {
    let tick = if progress.state == "done" { "x" } else { " " };
    let mut line = format!(
        "- [{tick}] step {}: {} ({role}",
        progress.step, progress.status
    );
    if let Some(sha) = progress.pushed_sha.as_deref() {
        let short = &sha[..sha.len().min(12)];
        line.push_str(&format!(", pushed {short}"));
    }
    line.push(')');
    if let Some(note) = progress.note.as_deref() {
        line.push_str(&format!("\n\n{note}"));
    }
    format!("{marker}\n{line}")
}
