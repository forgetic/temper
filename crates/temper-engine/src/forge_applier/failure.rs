// SPDX-License-Identifier: MPL-2.0

//! Durable parking and audit records for deterministic worker/result failures.

use temper_forge::{CreateComment, Forge, Issue, PullRequest, UpdateIssue, UpdatePullRequest};
use temper_protocol_worker::{Failure, FailureClass, JobResult, ResultStatus};

use crate::InFlightJob;
use crate::applier::ApplyOutcome;
use crate::forge_applier::ForgeApplier;

const FAILURE_AUDIT_COMMENT_KEY_PREFIX: &str = "daemon_failure_audit:";

impl<F: Forge + ?Sized> ForgeApplier<F> {
    pub(super) async fn apply_failure(&self, job: InFlightJob, result: JobResult) -> ApplyOutcome {
        let class = result
            .failure
            .as_ref()
            .map(|failure| failure.class)
            .unwrap_or(FailureClass::Protocol);
        let reason = result
            .failure
            .as_ref()
            .map(|failure| failure.message.clone())
            .unwrap_or_else(|| "worker failure omitted failure details".to_string());
        match class {
            FailureClass::Transient => {
                return if self.release_source_action_claim_for_retry(&job).await {
                    ApplyOutcome::RetryReleased
                } else {
                    ApplyOutcome::Retryable { reason }
                };
            }
            FailureClass::Canceled => {
                return if self.release_source_action_claim_for_retry(&job).await {
                    ApplyOutcome::RetryReleased
                } else {
                    ApplyOutcome::Retryable { reason }
                };
            }
            FailureClass::Permanent | FailureClass::Protocol => {}
        }

        // Interrupted-CI diagnostics are already bounded by their durable
        // exact-attempt marker. Releasing this assignment marks that one
        // diagnostic exhausted; the recovery loop owns the single structured
        // `needs-human` audit. Do not publish a second generic worker-failure
        // audit or make a code-repair action eligible.
        if is_interrupted_ci_diagnostic(&job) {
            return ApplyOutcome::Rejected { class, reason };
        }

        match self.park_failure(&job, &result, class).await {
            Ok(()) => ApplyOutcome::Rejected { class, reason },
            Err(reason) => ApplyOutcome::Retryable { reason },
        }
    }

    pub(super) async fn reject_success(
        &self,
        job: InFlightJob,
        mut result: JobResult,
        reason: String,
    ) -> ApplyOutcome {
        result.status = ResultStatus::Failure;
        result.failure = Some(Failure {
            class: FailureClass::Protocol,
            message: reason,
        });
        self.apply_failure(job, result).await
    }

    async fn park_failure(
        &self,
        job: &InFlightJob,
        result: &JobResult,
        class: FailureClass,
    ) -> Result<(), String> {
        let target = match job.artifact.kind.as_str() {
            "issue" => self
                .resolve_issue(job)
                .await
                .map(|(_, issue)| AttentionTarget::Issue(Box::new(issue))),
            "pull_request" => self
                .resolve_pull_request(job)
                .await
                .map(|(_, pull)| AttentionTarget::PullRequest(Box::new(pull))),
            other => return Err(format!("cannot park unsupported artifact kind `{other}`")),
        }
        .ok_or_else(|| "could not resolve deterministic failure source artifact".to_string())?;

        let marker = failure_audit_comment_marker(&result.job_id);
        let comments = target
            .list_comments(self.forge.as_ref())
            .await
            .map_err(|error| format!("list failed-job audit comments: {error}"))?;
        let has_comment = comments
            .iter()
            .any(|comment| comment.body.contains(&marker));
        let needs_labels = self
            .attention_labels
            .iter()
            .any(|label| !target.labels().iter().any(|existing| existing == label));

        if needs_labels {
            target
                .add_labels(self.forge.as_ref(), self.attention_labels.clone())
                .await
                .map_err(|error| format!("label failed-job source artifact: {error}"))?;
        }
        if !has_comment {
            target
                .add_comment(
                    self.forge.as_ref(),
                    failure_audit_body(failure_class_name(class), result),
                )
                .await
                .map_err(|error| format!("add failed-job audit comment: {error}"))?;
        }
        Ok(())
    }
}

enum AttentionTarget {
    Issue(Box<Issue>),
    PullRequest(Box<PullRequest>),
}

impl AttentionTarget {
    fn labels(&self) -> &[String] {
        match self {
            Self::Issue(issue) => &issue.labels,
            Self::PullRequest(pull) => &pull.labels,
        }
    }

    async fn list_comments<F: Forge + ?Sized>(
        &self,
        forge: &F,
    ) -> temper_forge::ForgeResult<Vec<temper_forge::Comment>> {
        match self {
            Self::Issue(issue) => forge.list_issue_comments(&issue.id).await,
            Self::PullRequest(pull) => forge.list_pull_request_comments(&pull.id).await,
        }
    }

    async fn add_labels<F: Forge + ?Sized>(
        &self,
        forge: &F,
        labels: Vec<String>,
    ) -> temper_forge::ForgeResult<()> {
        match self {
            Self::Issue(issue) => forge
                .update_issue(
                    &issue.id,
                    UpdateIssue {
                        add_labels: labels,
                        ..UpdateIssue::default()
                    },
                )
                .await
                .map(|_| ()),
            Self::PullRequest(pull) => forge
                .update_pull_request(
                    &pull.id,
                    UpdatePullRequest {
                        add_labels: labels,
                        ..UpdatePullRequest::default()
                    },
                )
                .await
                .map(|_| ()),
        }
    }

    async fn add_comment<F: Forge + ?Sized>(
        &self,
        forge: &F,
        body: String,
    ) -> temper_forge::ForgeResult<()> {
        let input = CreateComment { body };
        match self {
            Self::Issue(issue) => forge.add_issue_comment(&issue.id, input).await.map(|_| ()),
            Self::PullRequest(pull) => forge
                .add_pull_request_comment(&pull.id, input)
                .await
                .map(|_| ()),
        }
    }
}

fn is_interrupted_ci_diagnostic(job: &InFlightJob) -> bool {
    serde_json::from_value::<temper_protocol_worker::JobContext>(job.job_payload.clone())
        .ok()
        .and_then(|context| context.pull_request_freshness)
        .and_then(|freshness| freshness.queue_condition)
        .as_deref()
        == Some("ci_recovery_required")
}

fn failure_class_name(class: FailureClass) -> &'static str {
    match class {
        FailureClass::Transient => "transient",
        FailureClass::Permanent => "permanent",
        FailureClass::Canceled => "canceled",
        FailureClass::Protocol => "protocol",
    }
}

fn failure_audit_comment_marker(job_id: &str) -> String {
    format!("<!-- temper:comment-key={FAILURE_AUDIT_COMMENT_KEY_PREFIX}{job_id} -->")
}

fn failure_audit_body(class: &str, result: &JobResult) -> String {
    let header = format!(
        "Daemon could not complete this work (failure class: {class}).\n\njob_id: `{}`\nworker: `{}`",
        result.job_id, result.worker_id
    );
    let body = match result
        .failure
        .as_ref()
        .map(|failure| failure.message.trim())
    {
        Some(message) if !message.is_empty() => format!("{header}\n\n{message}"),
        _ => header,
    };
    format!(
        "{}\n\n{}",
        body,
        failure_audit_comment_marker(&result.job_id)
    )
}
