// SPDX-License-Identifier: MPL-2.0

//! The [`ResultApplier`] trait impl for [`ForgeApplier`] and the step-progress
//! checkpoints it records.

use temper_forge::{CreateComment, Forge};
use temper_protocol_worker::{JobContext, JobProgress, JobResult, ResultStatus};
use temper_workflow::{Effect, RoleId};

use crate::InFlightJob;
use crate::applier::ResultApplier;
use crate::forge_applier::ForgeApplier;

#[async_trait::async_trait]
impl<F: Forge + ?Sized + 'static> ResultApplier for ForgeApplier<F> {
    async fn apply(&self, job: InFlightJob, result: JobResult) {
        match result.status {
            ResultStatus::Success => self.apply_success(job, result).await,
            ResultStatus::Failure => self.apply_failure(job, result).await,
        }
    }

    /// Applies terminal step-progress checkpoints when they carry a useful
    /// final summary.
    ///
    /// Non-terminal `started` progress is intentionally left to daemon/worker
    /// logs, lease, assignment, and heartbeat signals, except that open-PR
    /// engineer starts claim the source issue. Ordinary checkpoint labels are
    /// resumability markers and do not create issue-thread chatter. A terminal
    /// issue comment is retained only for an explicit final-summary checkpoint
    /// (`finish …` with a non-empty note), and remains idempotent via the
    /// machine-readable progress marker. Engineer issue runs whose success path
    /// opens an implementation PR keep that final handoff in the PR body instead
    /// of duplicating it on the source issue.
    async fn apply_progress(&self, job: InFlightJob, progress: JobProgress) {
        if progress.state == "started" {
            self.apply_source_action_claim(&job).await;
            return;
        }

        if progress.state != "done" {
            return;
        }

        if !should_comment_progress(&progress) {
            return;
        }
        if self.final_progress_uses_implementation_pr_body(&job) {
            return;
        }

        let Some((_, issue)) = self.resolve_issue(&job).await else {
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
                tracing::warn!(
                    target: "temper_daemon",
                    job_id = %job.job_id,
                    repo = %job.repo,
                    issue = %number,
                    %error,
                    "forge applier could not list comments for progress"
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
            tracing::warn!(
                target: "temper_daemon",
                job_id = %job.job_id,
                repo = %job.repo,
                issue = %number,
                %error,
                "forge applier could not record progress"
            );
        }
    }
}

impl<F: Forge + ?Sized> ForgeApplier<F> {
    fn final_progress_uses_implementation_pr_body(&self, job: &InFlightJob) -> bool {
        if job.role != "engineer" || job.artifact.kind != "issue" {
            return false;
        }

        let Ok(context) = serde_json::from_value::<JobContext>(job.job_payload.clone()) else {
            return false;
        };
        let Some(action) = context.action.as_deref() else {
            return false;
        };

        let role_id = RoleId::new(job.role.as_str());
        self.compiled.role(&role_id).is_some_and(|role| {
            role.tools.iter().any(|tool| {
                tool.name == action
                    && tool
                        .effects
                        .iter()
                        .any(|effect| matches!(effect, Effect::CreatePullRequest { .. }))
            })
        })
    }
}

/// Terminal issue comments are reserved for useful final summaries, not generic
/// step bookkeeping. The in-tree agent's final marker uses `finish …`; phase
/// checkpoint labels do not, so notes on phase checkpoints still stay off the
/// issue thread.
fn should_comment_progress(progress: &JobProgress) -> bool {
    progress.state == "done"
        && progress
            .note
            .as_deref()
            .is_some_and(|note| !note.trim().is_empty())
        && progress.status.trim_start().starts_with("finish ")
}

/// The machine-readable idempotency marker for one final progress checkpoint.
fn progress_marker(progress: &JobProgress) -> String {
    format!(
        "<!-- temper-progress correlation_key={} step={} state={} -->",
        progress.correlation_key, progress.step, progress.state
    )
}

/// The human-facing final progress comment (marker line + concise summary).
fn progress_comment_body(role: &str, progress: &JobProgress, marker: &str) -> String {
    let mut line = format!(
        "Final {role} checkpoint: step {} ({})",
        progress.step, progress.status
    );
    if let Some(sha) = progress.pushed_sha.as_deref() {
        let short = &sha[..sha.len().min(12)];
        line.push_str(&format!(", pushed {short}"));
    }
    let note = progress.note.as_deref().map(str::trim).unwrap_or_default();
    if note.is_empty() {
        format!("{marker}\n{line}")
    } else {
        format!("{marker}\n{line}\n\n{note}")
    }
}
