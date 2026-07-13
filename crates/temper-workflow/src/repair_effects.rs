//! Effect capabilities shared by workflow validation and PR repair publication.

use crate::Effect;

impl Effect {
    /// Returns the stable workflow-spec token for this effect.
    #[must_use]
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::AddLabel(_) => "add_label",
            Self::RemoveLabel(_) | Self::RemoveLabelIfPresent(_) => "remove_label",
            Self::SetAssignee(_) => "set_assignee",
            Self::RemoveAssignee(_) => "remove_assignee",
            Self::CreateComment { .. } => "create_comment",
            Self::CreatePullRequest { .. } => "create_pull_request",
            Self::RequestReviewers { .. } => "request_reviewers",
            Self::SubmitReview { .. } => "submit_review",
            Self::SetBody { .. } => "set_body",
            Self::AttachReview { .. } => "attach_review",
            Self::CreateIssues { .. } => "create_issues",
            Self::MergePullRequest => "merge_pull_request",
            Self::CloseParentIssues => "close_parent_issues",
        }
    }

    /// Whether writable PR repair publication supports this effect.
    ///
    /// Labels and assignees commit with the repaired-head marker. Reviewer
    /// requests are an explicitly best-effort post-commit notification. Other
    /// effects require independent durable semantics and are rejected.
    #[must_use]
    pub fn supports_pull_request_repair_publication(&self) -> bool {
        matches!(
            self,
            Self::AddLabel(_)
                | Self::RemoveLabel(_)
                | Self::RemoveLabelIfPresent(_)
                | Self::SetAssignee(_)
                | Self::RemoveAssignee(_)
                | Self::RequestReviewers { .. }
        )
    }
}
