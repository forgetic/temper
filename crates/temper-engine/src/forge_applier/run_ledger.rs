// SPDX-License-Identifier: MPL-2.0

//! Host-owned source-issue run ledger for engineer checkpoint progress.
//!
//! The ledger is stored as one managed block in the source issue body, keyed by
//! the worker/daemon correlation key. The block is intentionally coarse-grained:
//! checkpoint labels remain resumability history, not a model-owned plan or
//! checklist.

use std::error::Error;
use std::fmt;

use temper_forge::{Forge, ForgeError, Issue, ItemNumber, PullRequest, RepositoryId, UpdateIssue};
use temper_protocol_worker::{JobContext, JobProgress};
use temper_workflow::{METADATA_BEGIN, METADATA_END, find_pull_request_by_correlation};

use crate::InFlightJob;
use crate::forge_applier::ForgeApplier;
use crate::workflow_meta::implementation_pr_labels;

const LEDGER_END: &str = "<!-- /temper-run-ledger -->";

/// One implementation PR the source-issue ledger can hand off to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct RunLedgerPullRequest {
    pub(super) repo: String,
    pub(super) number: ItemNumber,
}

impl<F: Forge + ?Sized> ForgeApplier<F> {
    /// Applies one engineer progress event to the source issue's managed run
    /// ledger. If the implementation PR already exists, this finalizes the
    /// ledger to the PR handoff instead of replaying stale checkpoint details.
    pub(super) async fn apply_run_ledger_progress(
        &self,
        job: &InFlightJob,
        progress: &JobProgress,
        include_final_note: bool,
    ) {
        let Some((repository, issue)) = self.resolve_issue(job).await else {
            return;
        };

        if run_ledger_finalized(&issue.body, &progress.correlation_key) {
            return;
        }

        if let Some(pull_request) = self
            .existing_implementation_pr(job, &repository.id, &progress.correlation_key)
            .await
        {
            let pull_requests = vec![RunLedgerPullRequest {
                repo: job.repo.clone(),
                number: pull_request.number,
            }];
            let update = RunLedgerUpdate::Continued {
                correlation_key: &progress.correlation_key,
                worker_id: &progress.worker_id,
                pull_requests: &pull_requests,
            };
            self.update_issue_run_ledger(job, issue, &update).await;
            return;
        }

        let pull_requests = self
            .ensure_checkpoint_implementation_prs(job, &repository, &issue, progress)
            .await;
        if !pull_requests.is_empty() {
            // The checkpoint just materialized the same implementation PR that
            // final success would have opened. Mirror the source-artifact
            // lifecycle signals now, then reload the issue so the body update's
            // compare-and-swap version accounts for the label mutations.
            self.apply_source_action_claim(job).await;
            self.clear_source_action_working_labels(job).await;
            let Some((_, issue)) = self.resolve_issue(job).await else {
                return;
            };
            let update = RunLedgerUpdate::Continued {
                correlation_key: &progress.correlation_key,
                worker_id: &progress.worker_id,
                pull_requests: &pull_requests,
            };
            self.update_issue_run_ledger(job, issue, &update).await;
            return;
        }

        let update = RunLedgerUpdate::Progress {
            progress,
            include_final_note,
        };
        self.update_issue_run_ledger(job, issue, &update).await;
    }

    /// Finalizes the source issue ledger once the implementation PR body is the
    /// canonical handoff.
    pub(super) async fn finalize_run_ledger(
        &self,
        job: &InFlightJob,
        correlation_key: &str,
        worker_id: &str,
        pull_requests: &[RunLedgerPullRequest],
    ) {
        if pull_requests.is_empty() {
            return;
        }
        let Some((_, issue)) = self.resolve_issue(job).await else {
            return;
        };
        let update = RunLedgerUpdate::Continued {
            correlation_key,
            worker_id,
            pull_requests,
        };
        self.update_issue_run_ledger(job, issue, &update).await;
    }

    async fn existing_implementation_pr(
        &self,
        job: &InFlightJob,
        repo_id: &RepositoryId,
        correlation_key: &str,
    ) -> Option<PullRequest> {
        if !self.final_progress_uses_implementation_pr_body(job) {
            return None;
        }

        let labels = implementation_pr_labels(self.workflow.as_ref());
        match find_pull_request_by_correlation(
            self.forge.as_ref(),
            repo_id,
            correlation_key,
            &labels,
        )
        .await
        {
            Ok(pull_request) => pull_request,
            Err(error) => {
                tracing::warn!(
                    target: "temper_daemon",
                    job_id = %job.job_id,
                    repo = %job.repo,
                    correlation_key,
                    %error,
                    "forge applier could not look up implementation PR for run ledger"
                );
                None
            }
        }
    }

    async fn update_issue_run_ledger(
        &self,
        job: &InFlightJob,
        mut issue: Issue,
        update: &RunLedgerUpdate<'_>,
    ) {
        for _ in 0..3 {
            let block = update.render(job);
            let desired_body = match upsert_run_ledger_block(
                &issue.body,
                update.correlation_key(),
                &block,
                update.is_progress(),
            ) {
                Ok(Some(body)) => body,
                Ok(None) => return,
                Err(error) => {
                    tracing::warn!(
                        target: "temper_daemon",
                        job_id = %job.job_id,
                        repo = %job.repo,
                        issue = %issue.number,
                        correlation_key = %update.correlation_key(),
                        %error,
                        "forge applier could not merge run ledger into source issue body"
                    );
                    return;
                }
            };

            match self
                .forge
                .update_issue(
                    &issue.id,
                    UpdateIssue {
                        body: Some(desired_body),
                        expected_version: Some(issue.version),
                        ..UpdateIssue::default()
                    },
                )
                .await
            {
                Ok(_) => return,
                Err(ForgeError::Conflict(_)) => match self.forge.get_issue(&issue.id).await {
                    Ok(Some(reloaded)) => {
                        issue = reloaded;
                        continue;
                    }
                    Ok(None) => {
                        tracing::warn!(
                            target: "temper_daemon",
                            job_id = %job.job_id,
                            repo = %job.repo,
                            issue = %issue.number,
                            "forge applier could not reload source issue after run ledger conflict"
                        );
                        return;
                    }
                    Err(error) => {
                        tracing::warn!(
                            target: "temper_daemon",
                            job_id = %job.job_id,
                            repo = %job.repo,
                            issue = %issue.number,
                            %error,
                            "forge applier could not reload source issue after run ledger conflict"
                        );
                        return;
                    }
                },
                Err(error) => {
                    tracing::warn!(
                        target: "temper_daemon",
                        job_id = %job.job_id,
                        repo = %job.repo,
                        issue = %issue.number,
                        %error,
                        "forge applier could not update source issue run ledger"
                    );
                    return;
                }
            }
        }

        tracing::warn!(
            target: "temper_daemon",
            job_id = %job.job_id,
            repo = %job.repo,
            issue = %issue.number,
            "forge applier gave up updating source issue run ledger after conflicts"
        );
    }
}

enum RunLedgerUpdate<'a> {
    Progress {
        progress: &'a JobProgress,
        include_final_note: bool,
    },
    Continued {
        correlation_key: &'a str,
        worker_id: &'a str,
        pull_requests: &'a [RunLedgerPullRequest],
    },
}

impl RunLedgerUpdate<'_> {
    fn correlation_key(&self) -> &str {
        match self {
            Self::Progress { progress, .. } => &progress.correlation_key,
            Self::Continued {
                correlation_key, ..
            } => correlation_key,
        }
    }

    fn is_progress(&self) -> bool {
        matches!(self, Self::Progress { .. })
    }

    fn render(&self, job: &InFlightJob) -> String {
        match self {
            Self::Progress {
                progress,
                include_final_note,
            } => render_progress_ledger(job, progress, *include_final_note),
            Self::Continued {
                correlation_key,
                worker_id,
                pull_requests,
            } => render_continued_ledger(job, correlation_key, worker_id, pull_requests),
        }
    }
}

fn render_progress_ledger(
    job: &InFlightJob,
    progress: &JobProgress,
    include_final_note: bool,
) -> String {
    let mut lines = common_ledger_lines(job, &progress.correlation_key, &progress.worker_id);
    lines.push(format!("- Current status: {}", progress_status(progress)));
    lines.push(format!(
        "- Latest progress: step {} — {}",
        progress.step,
        one_line_or(&progress.status, "progress reported")
    ));

    if progress.state == "done" || progress.pushed_sha.is_some() {
        let mut checkpoint = format!(
            "step {} — {}",
            progress.step,
            one_line_or(&progress.status, "checkpoint")
        );
        if let Some(sha) = progress.pushed_sha.as_deref().and_then(short_sha) {
            checkpoint.push_str(&format!(" ({sha})"));
        }
        lines.push(format!("- Latest checkpoint: {checkpoint}"));
    }

    if include_final_note
        && let Some(note) = progress.note.as_deref().map(str::trim)
        && !note.is_empty()
    {
        lines.push(String::new());
        lines.push("Final note:".to_string());
        for line in note.lines() {
            let quoted = if line.trim().is_empty() {
                ">".to_string()
            } else {
                format!("> {}", line.trim_end())
            };
            lines.push(quoted);
        }
    }

    finish_block(lines)
}

fn render_continued_ledger(
    job: &InFlightJob,
    correlation_key: &str,
    worker_id: &str,
    pull_requests: &[RunLedgerPullRequest],
) -> String {
    let mut lines = common_ledger_lines(job, correlation_key, worker_id);
    lines.push(format!(
        "- Current status: continued in {}",
        pull_request_refs(job, pull_requests)
    ));
    lines.push("- Final handoff: implementation PR body".to_string());
    finish_block(lines)
}

fn common_ledger_lines(job: &InFlightJob, correlation_key: &str, worker_id: &str) -> Vec<String> {
    vec![
        ledger_marker(correlation_key),
        "### Temper run ledger".to_string(),
        format!("- Role: {}", one_line_or(&job.role, "worker")),
        format!(
            "- Work branch: `{}`",
            one_line_or(&work_branch(job, correlation_key), "-")
        ),
        format!("- Worker: `{}`", one_line_or(worker_id, "unknown")),
    ]
}

fn finish_block(mut lines: Vec<String>) -> String {
    lines.push(LEDGER_END.to_string());
    lines.join("\n")
}

fn progress_status(progress: &JobProgress) -> &'static str {
    let status = progress.status.trim_start().to_ascii_lowercase();
    match progress.state.as_str() {
        "started" => "editing",
        "done" if status.starts_with("finish ") => "finalizing",
        "done" if status.contains("validat") => "validating",
        "done" => "checkpointed",
        _ if status.contains("validat") => "validating",
        _ => "editing",
    }
}

fn pull_request_refs(job: &InFlightJob, pull_requests: &[RunLedgerPullRequest]) -> String {
    let refs = pull_requests
        .iter()
        .map(|pull_request| {
            if pull_request.repo == job.repo {
                format!("PR #{}", pull_request.number.get())
            } else {
                format!("{} PR #{}", pull_request.repo, pull_request.number.get())
            }
        })
        .collect::<Vec<_>>();
    match refs.as_slice() {
        [] => "implementation PR".to_string(),
        [one] => one.clone(),
        many => many.join(", "),
    }
}

fn work_branch(job: &InFlightJob, correlation_key: &str) -> String {
    serde_json::from_value::<JobContext>(job.job_payload.clone())
        .ok()
        .and_then(|context| context.workspace)
        .and_then(|workspace| {
            workspace
                .repos
                .iter()
                .find(|repo| repo.repo == job.repo)
                .and_then(|repo| repo.branch_hint.clone())
                .or_else(|| {
                    workspace
                        .repos
                        .iter()
                        .find_map(|repo| repo.branch_hint.clone())
                })
        })
        .unwrap_or_else(|| format!("agent/{correlation_key}"))
}

fn one_line_or(value: &str, fallback: &str) -> String {
    let collapsed = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        fallback.to_string()
    } else {
        collapsed
    }
}

fn short_sha(sha: &str) -> Option<String> {
    let sha = sha.trim();
    if sha.is_empty() {
        return None;
    }
    Some(sha.chars().take(12).collect())
}

fn ledger_marker(correlation_key: &str) -> String {
    format!(
        "<!-- temper-run-ledger correlation_key={} -->",
        one_line_or(correlation_key, "unknown")
    )
}

fn run_ledger_finalized(body: &str, correlation_key: &str) -> bool {
    match run_ledger_span(body, correlation_key) {
        Ok(Some((start, end))) => ledger_block_finalized(&body[start..end]),
        Ok(None) | Err(_) => false,
    }
}

fn ledger_block_finalized(block: &str) -> bool {
    block.contains("Current status: continued in ")
}

fn upsert_run_ledger_block(
    body: &str,
    correlation_key: &str,
    block: &str,
    skip_if_finalized: bool,
) -> Result<Option<String>, RunLedgerMergeError> {
    if let Some((start, end)) = run_ledger_span(body, correlation_key)? {
        let current = &body[start..end];
        if skip_if_finalized && ledger_block_finalized(current) {
            return Ok(None);
        }
        if current == block {
            return Ok(None);
        }
        let updated = format!("{}{}{}", &body[..start], block, &body[end..]);
        return if updated == body {
            Ok(None)
        } else {
            Ok(Some(updated))
        };
    }

    insert_run_ledger_block(body, block).map(Some)
}

fn run_ledger_span(
    body: &str,
    correlation_key: &str,
) -> Result<Option<(usize, usize)>, RunLedgerMergeError> {
    let marker = ledger_marker(correlation_key);
    let Some(start) = body.find(&marker) else {
        return Ok(None);
    };
    let after_marker = start + marker.len();
    let Some(end_relative) = body[after_marker..].find(LEDGER_END) else {
        return Err(RunLedgerMergeError::UnterminatedLedger);
    };
    let end = after_marker + end_relative + LEDGER_END.len();
    Ok(Some((start, end)))
}

fn insert_run_ledger_block(body: &str, block: &str) -> Result<String, RunLedgerMergeError> {
    if let Some(index) = workflow_metadata_start(body)? {
        let before = body[..index].trim_end();
        let after = body[index..].trim_start_matches('\n');
        return if before.is_empty() {
            Ok(format!("{block}\n\n{after}"))
        } else {
            Ok(format!("{before}\n\n{block}\n\n{after}"))
        };
    }

    if body.trim().is_empty() {
        Ok(block.to_string())
    } else {
        Ok(format!("{}\n\n{block}", body.trim_end()))
    }
}

fn workflow_metadata_start(body: &str) -> Result<Option<usize>, RunLedgerMergeError> {
    let Some(start) = body.find(METADATA_BEGIN) else {
        return Ok(None);
    };
    let after_begin = start + METADATA_BEGIN.len();
    if body[after_begin..].find(METADATA_END).is_none() {
        return Err(RunLedgerMergeError::UnterminatedWorkflowMetadata);
    }
    Ok(Some(start))
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum RunLedgerMergeError {
    UnterminatedLedger,
    UnterminatedWorkflowMetadata,
}

impl fmt::Display for RunLedgerMergeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnterminatedLedger => formatter.write_str("run ledger block was not terminated"),
            Self::UnterminatedWorkflowMetadata => {
                formatter.write_str("workflow metadata block was not terminated")
            }
        }
    }
}

impl Error for RunLedgerMergeError {}
