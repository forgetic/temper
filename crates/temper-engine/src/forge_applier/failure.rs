// SPDX-License-Identifier: MPL-2.0

//! Failure-path application: permanent/protocol worker failures label the source
//! issue for human attention and add a one-time, idempotency-marked audit
//! comment. Transient failures are applied only as retry bookkeeping: release a
//! still-current source claim back to the ready queue and mark the run ledger as
//! retryable. Canceled failures are not applied here.

use temper_forge::{CreateComment, Forge, ItemNumber, UpdateIssue};
use temper_protocol_worker::{FailureClass, JobContext, JobResult};
use temper_runner::pr_correlation_key;
use temper_workflow::ArtifactKindId;

use crate::InFlightJob;
use crate::forge_applier::ForgeApplier;

const FAILURE_AUDIT_COMMENT_KEY_PREFIX: &str = "daemon_failure_audit:";

impl<F: Forge + ?Sized> ForgeApplier<F> {
    pub(super) async fn apply_failure(&self, job: InFlightJob, result: JobResult) {
        if matches!(
            result.failure.as_ref().map(|failure| failure.class),
            Some(FailureClass::Transient)
        ) {
            self.apply_transient_retry(job, result).await;
            return;
        }

        let failure = result.failure.as_ref();
        let class = match failure.map(|failure| failure.class) {
            Some(FailureClass::Permanent) => "permanent",
            Some(FailureClass::Protocol) => "protocol",
            None => "unknown",
            Some(FailureClass::Transient | FailureClass::Canceled) => return,
        };

        let Some((_repository, issue)) = self.resolve_issue(&job).await else {
            return;
        };

        if self
            .attention_labels
            .iter()
            .all(|label| issue.labels.iter().any(|existing| existing == label))
        {
            return;
        }

        let comment_marker = failure_audit_comment_marker(&result.job_id);
        match self.forge.list_issue_comments(&issue.id).await {
            Ok(comments) => {
                if comments
                    .iter()
                    .any(|comment| comment.body.contains(&comment_marker))
                {
                    return;
                }
            }
            Err(error) => {
                tracing::error!(
                    target: "temper_daemon",
                    job_id = %job.job_id,
                    repo = %job.repo,
                    issue = %issue.number,
                    failure_class = %class,
                    %error,
                    "forge applier could not list failed job audit comments"
                );
                return;
            }
        }

        if let Err(error) = self
            .forge
            .update_issue(
                &issue.id,
                UpdateIssue {
                    add_labels: self.attention_labels.clone(),
                    ..Default::default()
                },
            )
            .await
        {
            tracing::error!(
                target: "temper_daemon",
                job_id = %job.job_id,
                repo = %job.repo,
                issue = %issue.number,
                failure_class = %class,
                %error,
                "forge applier could not label failed job source issue"
            );
            return;
        }

        let body = failure_audit_body(class, &result);
        if let Err(error) = self
            .forge
            .add_issue_comment(&issue.id, CreateComment { body })
            .await
        {
            tracing::error!(
                target: "temper_daemon",
                job_id = %job.job_id,
                repo = %job.repo,
                issue = %issue.number,
                failure_class = %class,
                %error,
                "forge applier could not add failed job audit comment"
            );
        }
    }

    async fn apply_transient_retry(&self, job: InFlightJob, result: JobResult) {
        let Some(correlation_key) = retry_correlation_key(&job) else {
            return;
        };
        if !self
            .mark_run_ledger_retryable(&job, &correlation_key, &result.worker_id)
            .await
        {
            return;
        }
        let released = self.release_source_action_claim_for_retry(&job).await;
        tracing::debug!(
            target: "temper_daemon",
            job_id = %job.job_id,
            repo = %job.repo,
            correlation_key,
            worker = %result.worker_id,
            released,
            "forge applier applied transient retry bookkeeping"
        );
    }
}

fn retry_correlation_key(job: &InFlightJob) -> Option<String> {
    if let Ok(context) = serde_json::from_value::<JobContext>(job.job_payload.clone()) {
        if let Some(workspace) = context.workspace {
            return Some(workspace.coordination_key);
        }
        let number = job.artifact.item.as_u64().map(ItemNumber::new)?;
        let kind = ArtifactKindId::new(context.artifact_kind);
        return Some(pr_correlation_key(&kind, number));
    }

    job.job_payload
        .get("correlation_key")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
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
