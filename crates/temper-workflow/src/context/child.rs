//! The workspace-authored child artifact bound for a `CreateIssues` effect.
//!
//! Split from the context root so the [`CreateIssuesChild`] builder stays
//! separate from the [`ExecutionContext`](super::ExecutionContext) that carries
//! the rest of the runtime bindings.

use temper_forge::RepositoryId;

/// One workspace-authored child artifact bound for a `CreateIssues` effect.
///
/// The child's content (title/body/labels) and its declared sibling
/// dependencies are the workspace work product, supplied through the keyed
/// runtime-input seam exactly as a pull-request head is for `CreatePullRequest`.
/// The parent relation is not carried here: the executor links every child back
/// to the artifact the transition acts on.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CreateIssuesChild {
    /// Caller-chosen stable identifier for this child within the effect.
    ///
    /// It seeds the child's per-child correlation key (so a retry resolves the
    /// same child instead of duplicating it) and lets sibling children reference
    /// this one in their [`dependencies`](Self::dependencies).
    pub slug: String,
    /// Authored child title.
    pub title: String,
    /// Authored child body. The executor stamps the correlation key and parent
    /// back-reference into the body's workflow metadata block before creating.
    pub body: String,
    /// Labels to create the child with (e.g. `code` + `ready`).
    pub labels: Vec<String>,
    /// Slugs of sibling children in the same effect that must land before this
    /// one. Recorded as fallback dependency relations once both children exist,
    /// reusing the cross-repo aggregation stance (non-atomic on real forges).
    pub dependencies: Vec<String>,
    /// Repository the child is created in. `None` (the default) keeps today's
    /// behavior: the child lands in the parent artifact's repository.
    pub target_repo: Option<RepositoryId>,
}

impl CreateIssuesChild {
    /// Builds a child with no labels or dependencies.
    pub fn new(slug: impl Into<String>, title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            slug: slug.into(),
            title: title.into(),
            body: body.into(),
            labels: Vec::new(),
            dependencies: Vec::new(),
            target_repo: None,
        }
    }

    /// Sets the child's labels, returning `self` for chaining.
    pub fn with_labels(mut self, labels: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.labels = labels.into_iter().map(Into::into).collect();
        self
    }

    /// Sets the slugs of sibling children this child depends on, returning
    /// `self` for chaining.
    pub fn with_dependencies(
        mut self,
        dependencies: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.dependencies = dependencies.into_iter().map(Into::into).collect();
        self
    }

    /// Sets the target repository for this child, returning `self` for chaining.
    pub fn with_target_repo(mut self, repo: impl Into<RepositoryId>) -> Self {
        self.target_repo = Some(repo.into());
        self
    }
}
