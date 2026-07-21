//! Idempotent transition-completion audit comments.
//!
//! These comments are runtime bindings, not static workflow effects. They run
//! at the transition commit boundary: after all separately durable products
//! exist, but before the source labels and assignees are changed.

use super::{ExecutionError, Executor, Loaded};
use crate::context::TransitionCompletionAudit;
use temper_forge::{CreateComment, Forge, Issue, ItemNumber, RepositoryId};

/// Final child identity used when rendering a persisted fan-out audit.
pub(super) struct CompletionAuditIssue {
    pub(super) slug: String,
    pub(super) title: String,
    pub(super) repository_id: RepositoryId,
    pub(super) number: ItemNumber,
}

impl<F: Forge + ?Sized> Executor<'_, F> {
    /// Ensures an audit on the freshly loaded transition source.
    pub(super) async fn ensure_loaded_completion_audit(
        &self,
        loaded: &Loaded,
        audit: &TransitionCompletionAudit,
    ) -> Result<(), ExecutionError> {
        let exists = match loaded {
            Loaded::Issue { id, .. } => self
                .forge
                .list_issue_comments(id)
                .await?
                .iter()
                .any(|comment| comment.body.contains(&audit.marker)),
            Loaded::PullRequest { id, .. } => self
                .forge
                .list_pull_request_comments(id)
                .await?
                .iter()
                .any(|comment| comment.body.contains(&audit.marker)),
        };
        if exists {
            return Ok(());
        }
        let body = self.render_completion_audit(audit, None, &[]).await?;
        match loaded {
            Loaded::Issue { id, .. } => {
                self.forge
                    .add_issue_comment(id, CreateComment { body })
                    .await?;
            }
            Loaded::PullRequest { id, .. } => {
                self.forge
                    .add_pull_request_comment(id, CreateComment { body })
                    .await?;
            }
        }
        Ok(())
    }

    /// Ensures a persisted fan-out audit on its source issue.
    ///
    /// Callers provide only final child identities and invoke this after every
    /// child is wired and activated. The marker lookup precedes every create,
    /// so replay after an uncertain create response converges without editing
    /// or duplicating the comment.
    pub(super) async fn ensure_issue_completion_audit(
        &self,
        parent: &Issue,
        audit: &TransitionCompletionAudit,
        children: &[CompletionAuditIssue],
    ) -> Result<(), ExecutionError> {
        let comments = self.forge.list_issue_comments(&parent.id).await?;
        if comments
            .iter()
            .any(|comment| comment.body.contains(&audit.marker))
        {
            return Ok(());
        }
        let body = self
            .render_completion_audit(audit, Some(&parent.repo_id), children)
            .await?;
        self.forge
            .add_issue_comment(&parent.id, CreateComment { body })
            .await?;
        Ok(())
    }

    async fn render_completion_audit(
        &self,
        audit: &TransitionCompletionAudit,
        parent_repo: Option<&RepositoryId>,
        children: &[CompletionAuditIssue],
    ) -> Result<String, ExecutionError> {
        let mut body = audit.body.trim_end().to_string();
        if !children.is_empty() {
            if !body.is_empty() {
                body.push_str("\n\n");
            }
            body.push_str("Follow-up issues:\n");
            for child in children {
                let reference = if parent_repo == Some(&child.repository_id) {
                    format!("#{}", child.number.get())
                } else {
                    let repository = self
                        .forge
                        .get_repository(&child.repository_id)
                        .await?
                        .ok_or_else(|| ExecutionError::Backend {
                            message: format!(
                                "cannot render completion audit reference for missing repository `{}`",
                                child.repository_id
                            ),
                        })?;
                    format!(
                        "{}/{}#{}",
                        repository.owner,
                        repository.name,
                        child.number.get()
                    )
                };
                body.push_str(&format!(
                    "- {reference} — {} (`{}`)\n",
                    child.title, child.slug
                ));
            }
            body = body.trim_end().to_string();
        }
        if !body.contains(&audit.marker) {
            if !body.is_empty() {
                body.push_str("\n\n");
            }
            body.push_str(&audit.marker);
        }
        Ok(body)
    }
}

pub(super) fn validate_completion_audit(
    audit: Option<&TransitionCompletionAudit>,
) -> Result<(), ExecutionError> {
    if audit.is_some_and(|audit| audit.marker.trim().is_empty()) {
        return Err(ExecutionError::Backend {
            message: "transition-completion audit marker must not be empty".into(),
        });
    }
    Ok(())
}
