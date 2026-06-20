// SPDX-License-Identifier: MPL-2.0

//! The [`ResultApplier`] trait impl for [`ForgeApplier`] and the step-progress
//! checkpoints it records.

use temper_forge::{
    CreateComment, Forge, ForgeError, PullRequest, Repository, RepositoryPath, UpdatePullRequest,
};
use temper_protocol_worker::{JobContext, JobProgress, JobResult, ResultStatus};
use temper_workflow::{Effect, RoleId, find_pull_request_by_correlation};

use crate::InFlightJob;
use crate::applier::ResultApplier;
use crate::forge_applier::ForgeApplier;
use crate::forge_applier::progress_checklist::{ChecklistTick, tick_implementation_plan_phase};
use crate::workflow_meta::implementation_pr_labels;

#[async_trait::async_trait]
impl<F: Forge + ?Sized + 'static> ResultApplier for ForgeApplier<F> {
    async fn apply(&self, job: InFlightJob, result: JobResult) {
        match result.status {
            ResultStatus::Success => self.apply_success(job, result).await,
            ResultStatus::Failure => self.apply_failure(job, result).await,
        }
    }

    /// Applies terminal step-progress checkpoints to the implementation PR when
    /// there is a matching plan checklist phase.
    ///
    /// Non-terminal `started` progress is intentionally left to daemon/worker
    /// logs, lease, assignment, and heartbeat signals. `done` checkpoints first
    /// try to find the implementation PR by the job correlation key plus the
    /// workflow's stable `implementation_pr` identifying labels, then tick only
    /// the checklist phase whose label matches the checkpoint status. The body
    /// update is idempotent: an already-checked phase is a no-op, unrelated
    /// phases are left untouched, and the workflow metadata block is preserved.
    ///
    /// A terminal issue comment is retained only for an explicit final-summary
    /// checkpoint (`finish …` with a non-empty note), and remains idempotent via
    /// the machine-readable progress marker. Engineer issue runs whose success
    /// path opens an implementation PR keep that final handoff in the PR body
    /// instead of duplicating it on the source issue.
    async fn apply_progress(&self, job: InFlightJob, progress: JobProgress) {
        if self.apply_plan_publication_progress(&job, &progress).await {
            return;
        }

        if progress.state == "started" {
            self.apply_source_action_claim(&job).await;
            return;
        }

        if progress.state != "done" {
            return;
        }

        let phase_was_recorded = self
            .record_progress_on_implementation_prs(&job, &progress)
            .await;
        if phase_was_recorded {
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
    /// Returns true when the checkpoint matched a PR checklist phase (whether it
    /// had to mutate the body or was already checked). The caller uses that to
    /// suppress issue-thread progress chatter for phase checkpoints.
    async fn record_progress_on_implementation_prs(
        &self,
        job: &InFlightJob,
        progress: &JobProgress,
    ) -> bool {
        let lookup_labels = implementation_pr_labels(self.workflow.as_ref());
        let mut matched_any_phase = false;
        for repo_path in progress_repo_paths(job) {
            let Some(repository) = self.resolve_progress_repository(job, &repo_path).await else {
                continue;
            };
            let pull_request = match find_pull_request_by_correlation(
                self.forge.as_ref(),
                &repository.id,
                &progress.correlation_key,
                &lookup_labels,
            )
            .await
            {
                Ok(Some(pull_request)) => pull_request,
                Ok(None) => continue,
                Err(error) => {
                    tracing::warn!(
                        target: "temper_daemon",
                        job_id = %job.job_id,
                        repo = %repo_path,
                        correlation_key = %progress.correlation_key,
                        %error,
                        "forge applier could not find implementation PR for progress"
                    );
                    continue;
                }
            };

            if self.tick_progress_phase(job, progress, pull_request).await {
                matched_any_phase = true;
            }
        }
        matched_any_phase
    }

    async fn tick_progress_phase(
        &self,
        job: &InFlightJob,
        progress: &JobProgress,
        mut pull_request: PullRequest,
    ) -> bool {
        let phase = progress.status.trim();
        if phase.is_empty() {
            return false;
        }

        for _ in 0..3 {
            match tick_implementation_plan_phase(&pull_request.body, phase) {
                ChecklistTick::Changed(body) => {
                    match self
                        .forge
                        .update_pull_request(
                            &pull_request.id,
                            UpdatePullRequest {
                                body: Some(body),
                                expected_version: Some(pull_request.version),
                                ..UpdatePullRequest::default()
                            },
                        )
                        .await
                    {
                        Ok(_) => return true,
                        Err(ForgeError::Conflict(_)) => {
                            match self.forge.get_pull_request(&pull_request.id).await {
                                Ok(Some(reloaded)) => {
                                    pull_request = reloaded;
                                    continue;
                                }
                                Ok(None) => {
                                    tracing::warn!(
                                        target: "temper_daemon",
                                        job_id = %job.job_id,
                                        pull_request = %pull_request.number,
                                        "forge applier could not reload PR after progress conflict"
                                    );
                                    return false;
                                }
                                Err(error) => {
                                    tracing::warn!(
                                        target: "temper_daemon",
                                        job_id = %job.job_id,
                                        pull_request = %pull_request.number,
                                        %error,
                                        "forge applier could not reload PR after progress conflict"
                                    );
                                    return false;
                                }
                            }
                        }
                        Err(error) => {
                            tracing::warn!(
                                target: "temper_daemon",
                                job_id = %job.job_id,
                                pull_request = %pull_request.number,
                                %error,
                                "forge applier could not tick implementation PR progress"
                            );
                            return false;
                        }
                    }
                }
                ChecklistTick::AlreadyDone => return true,
                ChecklistTick::NoChecklist | ChecklistTick::NoMatch => return false,
            }
        }

        tracing::warn!(
            target: "temper_daemon",
            job_id = %job.job_id,
            pull_request = %pull_request.number,
            "forge applier gave up ticking implementation PR progress after conflicts"
        );
        false
    }

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
        self.compiled
            .role(&role_id)
            .is_some_and(|role| {
                role.tools.iter().any(|tool| {
                    tool.name == action
                        && tool
                            .effects
                            .iter()
                            .any(|effect| matches!(effect, Effect::CreatePullRequest { .. }))
                })
            })
    }

    async fn resolve_progress_repository(
        &self,
        job: &InFlightJob,
        repo_path: &str,
    ) -> Option<Repository> {
        let Some((owner, name)) = repo_path.split_once('/') else {
            tracing::warn!(
                target: "temper_daemon",
                job_id = %job.job_id,
                repo = %repo_path,
                "forge applier ignored progress for malformed repo path"
            );
            return None;
        };
        match self
            .forge
            .get_repository_by_path(&RepositoryPath::new(owner, name))
            .await
        {
            Ok(Some(repository)) => Some(repository),
            Ok(None) => {
                tracing::warn!(
                    target: "temper_daemon",
                    job_id = %job.job_id,
                    repo = %repo_path,
                    "forge applier progress repository not found"
                );
                None
            }
            Err(error) => {
                tracing::warn!(
                    target: "temper_daemon",
                    job_id = %job.job_id,
                    repo = %repo_path,
                    %error,
                    "forge applier progress repository lookup failed"
                );
                None
            }
        }
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

fn progress_repo_paths(job: &InFlightJob) -> Vec<String> {
    let mut paths = Vec::new();
    if let Ok(context) = serde_json::from_value::<JobContext>(job.job_payload.clone()) {
        if let Some(workspace) = context.workspace {
            for repo in workspace.writable() {
                push_unique(&mut paths, repo.repo.clone());
            }
        } else if !context.repo.trim().is_empty() {
            push_unique(&mut paths, context.repo);
        }
    }
    if paths.is_empty() {
        push_unique(&mut paths, job.repo.clone());
    }
    paths
}

fn push_unique(paths: &mut Vec<String>, path: String) {
    if !paths.iter().any(|candidate| candidate == &path) {
        paths.push(path);
    }
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
