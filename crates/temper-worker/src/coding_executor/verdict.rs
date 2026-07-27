//! Fenced read-only verdict routing and workspace cleanup.

use temper_protocol_agent::WorkspaceResult;
use temper_protocol_worker::{FailureClass, JobChild};

use crate::executor::{AttemptFence, JobOutcome};
use crate::workspace::Workspace;

use super::{allowed_verdicts_display, cancelled_attempt, failure, workspace_failure};

pub(super) async fn verdict_only_outcome(
    workspace: &Workspace,
    result: WorkspaceResult,
    allowed_verdicts: &[String],
    correlation_key: &str,
    details: Option<serde_json::Value>,
    fence: &AttemptFence,
) -> JobOutcome {
    if !fence.is_open() {
        return cancelled_attempt();
    }
    let WorkspaceResult {
        verdict,
        summary,
        title,
        body,
        review_body,
        children,
        // `labels` is a head-path PR-label override; read-only verdict routing
        // does not consume it.
        labels: _labels,
    } = result;
    let Some(verdict) = verdict else {
        return failure(FailureClass::Permanent, "read-only job returned no verdict");
    };
    if !allowed_verdicts.contains(&verdict) {
        return failure(
            FailureClass::Permanent,
            format!(
                "read-only job returned undeclared verdict `{verdict}`; allowed verdicts: {}",
                allowed_verdicts_display(allowed_verdicts)
            ),
        );
    }

    if !fence.is_open() {
        return cancelled_attempt();
    }
    if let Err(error) = workspace.discard_changes().await {
        return workspace_failure("discard verdict workspace changes", error);
    }
    if !fence.is_open() {
        return cancelled_attempt();
    }

    let children = children
        .into_iter()
        .map(|child| JobChild {
            slug: child.slug,
            title: child.title,
            body: child.body,
            kind: child.kind,
            labels: child.labels,
            depends_on: child.depends_on,
            target_repo: child.target_repo,
        })
        .collect();

    JobOutcome::Verdict {
        verdict,
        title,
        body: body.or(review_body),
        summary: summary.or_else(|| Some(format!("implemented {correlation_key}"))),
        children,
        details,
    }
}
