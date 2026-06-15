// SPDX-License-Identifier: MPL-2.0

//! Failure-path application: permanent/protocol worker failures label the source
//! issue for human attention and add a one-time, idempotency-marked audit
//! comment. Transient and canceled failures are not applied here.

use temper_forge::{CreateComment, Forge, UpdateIssue};
use temper_worker_protocol::{FailureClass, JobResult};

use crate::InFlightJob;
use crate::forge_applier::ForgeApplier;

const FAILURE_AUDIT_COMMENT_KEY_PREFIX: &str = "daemon_failure_audit:";

impl<F: Forge + ?Sized> ForgeApplier<F> {
    pub(super) async fn apply_failure(&self, job: InFlightJob, result: JobResult) {
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
                eprintln!(
                    "temper-daemon: forge applier could not list failed job audit comments for job_id={} repo={} issue={} failure_class={}: {error}",
                    job.job_id, job.repo, issue.number, class
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
            eprintln!(
                "temper-daemon: forge applier could not label failed job source issue for job_id={} repo={} issue={} failure_class={}: {error}",
                job.job_id, job.repo, issue.number, class
            );
            return;
        }

        let body = failure_audit_body(class, &result);
        if let Err(error) = self
            .forge
            .add_issue_comment(&issue.id, CreateComment { body })
            .await
        {
            eprintln!(
                "temper-daemon: forge applier could not add failed job audit comment for job_id={} repo={} issue={} failure_class={}: {error}",
                job.job_id, job.repo, issue.number, class
            );
        }
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
