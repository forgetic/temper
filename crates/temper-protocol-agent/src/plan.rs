use serde::{Deserialize, Serialize};

use crate::WorkspaceContext;

/// A model-authored implementation plan publication carried on step-progress.
///
/// The model supplies the short summary/title and ordered phase labels; the
/// host/agent runtime fills in repository routing data from the workspace
/// context so downstream orchestration can decide how (or whether) to publish
/// it without asking the model to perform git or forge actions.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlanPublication {
    /// Short human summary or title for the planned change.
    pub summary: String,
    /// Ordered human-readable phase labels. These should be reused verbatim in
    /// final `WorkspaceResult::plan` and later checkpoint labels.
    #[serde(default)]
    pub phases: Vec<String>,
    /// Target repositories the plan applies to, in workspace/manifest order.
    #[serde(default)]
    pub target_repos: Vec<PlanPublicationTarget>,
}

impl PlanPublication {
    /// Builds a publication by combining model-supplied plan text with the
    /// target repo/base/branch data already trusted in the workspace context.
    pub fn from_context(
        summary: impl Into<String>,
        phases: Vec<String>,
        context: &WorkspaceContext,
    ) -> Self {
        let mut target_repos = context
            .repos
            .iter()
            .filter(|repo| repo.is_writable())
            .map(PlanPublicationTarget::from_workspace_repo)
            .collect::<Vec<_>>();
        if target_repos.is_empty() {
            target_repos = context
                .repos
                .iter()
                .map(PlanPublicationTarget::from_workspace_repo)
                .collect();
        }
        Self {
            summary: summary.into(),
            phases,
            target_repos,
        }
    }
}

/// One repository target included in a [`PlanPublication`].
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlanPublicationTarget {
    /// Repository path in `owner/name` form.
    pub repo_path: String,
    /// Workspace-relative checkout directory for the repository.
    pub dir: String,
    /// Branch the work is based on.
    pub base_branch: String,
    /// Host-provided work branch hint, when the target is writable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_hint: Option<String>,
}

impl PlanPublicationTarget {
    fn from_workspace_repo(repo: &crate::WorkspaceRepository) -> Self {
        Self {
            repo_path: format!("{}/{}", repo.owner, repo.name),
            dir: repo.dir.clone(),
            base_branch: repo.base_branch.clone(),
            branch_hint: repo.branch_hint.clone(),
        }
    }
}
