//! Production Forgejo PR-diff guardrails.
//!
//! These checks stay out of `harness-forge`: changed-file inspection is a
//! provider-specific Forgejo REST concern. The guard wraps reviewer/owner agents
//! in production dogfood so they cannot approve or merge PRs that contain only
//! Harness bookkeeping files.

use std::sync::Arc;

use async_trait::async_trait;
use harness_forge::Forge;
use harness_runner::{Agent, AgentError, RoleTools, WorkItem};
use harness_workflow::{ArtifactSource, TransitionId};

use crate::forgejo_rest::{self, RestError};

const COMMENT_MARKER: &str = "<!-- harness:dogfood-pr-diff-guard -->";
const IGNORED_PREFIXES: &[&str] = &[".harness-pr-prep/", ".harness-ci/"];
const IGNORED_PATHS: &[&str] = &[".forgejo/workflows/ci.yml"];

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiffSafety {
    Meaningful { files: Vec<String> },
    BookkeepingOnly { files: Vec<String> },
}

impl DiffSafety {
    fn is_bookkeeping_only(&self) -> bool {
        matches!(self, DiffSafety::BookkeepingOnly { .. })
    }

    fn files(&self) -> &[String] {
        match self {
            DiffSafety::Meaningful { files } | DiffSafety::BookkeepingOnly { files } => files,
        }
    }
}

pub fn safety_for_files(files: Vec<String>) -> DiffSafety {
    if files.is_empty() || files.iter().all(|path| is_ignored_internal_path(path)) {
        DiffSafety::BookkeepingOnly { files }
    } else {
        DiffSafety::Meaningful { files }
    }
}

pub fn is_ignored_internal_path(path: &str) -> bool {
    IGNORED_PREFIXES
        .iter()
        .any(|prefix| path.starts_with(prefix))
        || IGNORED_PATHS.contains(&path)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GuardRole {
    Reviewer,
    Owner,
}

pub struct PullRequestDiffGuard<F: Forge + ?Sized> {
    inner: Arc<dyn Agent<F>>,
    role: GuardRole,
    base_url: String,
    token: String,
}

impl<F: Forge + ?Sized> PullRequestDiffGuard<F> {
    pub fn new(inner: Arc<dyn Agent<F>>, role: GuardRole, base_url: String, token: String) -> Self {
        Self {
            inner,
            role,
            base_url,
            token,
        }
    }

    fn should_guard(&self, item: &WorkItem) -> bool {
        match self.role {
            GuardRole::Reviewer => item.queue.as_str() == "pr_needs_review",
            GuardRole::Owner => item.queue.as_str() == "merge_ready",
        }
    }

    async fn inspect(&self, owner: &str, name: &str, number: u64) -> Result<DiffSafety, RestError> {
        let client = forgejo_rest::http_client()?;
        let files = forgejo_rest::list_pull_request_files(
            &client,
            &self.base_url,
            &self.token,
            owner,
            name,
            number,
        )
        .await?;
        Ok(safety_for_files(files))
    }

    async fn ensure_guard_comment(
        &self,
        owner: &str,
        name: &str,
        number: u64,
        safety: &DiffSafety,
    ) -> Result<bool, RestError> {
        let client = forgejo_rest::http_client()?;
        let comments = forgejo_rest::list_issue_comment_bodies(
            &client,
            &self.base_url,
            &self.token,
            owner,
            name,
            number,
        )
        .await?;
        if comments.iter().any(|body| body.contains(COMMENT_MARKER)) {
            return Ok(false);
        }
        let body = guard_comment_body(self.role, safety.files());
        forgejo_rest::create_issue_comment(
            &client,
            &self.base_url,
            &self.token,
            owner,
            name,
            number,
            &body,
        )
        .await?;
        Ok(true)
    }
}

#[async_trait]
impl<F: Forge + ?Sized> Agent<F> for PullRequestDiffGuard<F> {
    async fn service(&self, item: &WorkItem, tools: &RoleTools<'_, F>) -> Result<bool, AgentError> {
        if !self.should_guard(item) {
            return self.inner.service(item, tools).await;
        }
        let ArtifactSource::PullRequest { number } = item.target else {
            return self.inner.service(item, tools).await;
        };
        let Some(repository) = tools.get_repository().await? else {
            return Err(AgentError::message(format!(
                "repository {} not found while checking PR diff",
                tools.repo()
            )));
        };
        let safety = self
            .inspect(&repository.owner, &repository.name, number.get())
            .await
            .map_err(|error| AgentError::message(format!("PR diff guard failed: {error}")))?;
        if !safety.is_bookkeeping_only() {
            return self.inner.service(item, tools).await;
        }

        let commented = self
            .ensure_guard_comment(&repository.owner, &repository.name, number.get(), &safety)
            .await
            .map_err(|error| {
                AgentError::message(format!("PR diff guard comment failed: {error}"))
            })?;
        match self.role {
            GuardRole::Reviewer => {
                tools
                    .run(item.target, &TransitionId::new("request_changes"))
                    .await?;
                Ok(true)
            }
            GuardRole::Owner => Ok(commented),
        }
    }
}

fn guard_comment_body(role: GuardRole, files: &[String]) -> String {
    let action = match role {
        GuardRole::Reviewer => "requesting changes",
        GuardRole::Owner => "refusing to merge",
    };
    let mut body = format!(
        "{COMMENT_MARKER}\nHarness dogfood safety guard is {action}: this PR has no meaningful product diff after excluding internal Harness bookkeeping paths."
    );
    if files.is_empty() {
        body.push_str("\n\nChanged files reported by Forgejo: (none)");
    } else {
        body.push_str("\n\nChanged files reported by Forgejo:");
        for path in files.iter().take(20) {
            body.push_str(&format!("\n- `{path}`"));
        }
        if files.len() > 20 {
            body.push_str(&format!("\n- … and {} more", files.len() - 20));
        }
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_empty_or_internal_only_diff_as_bookkeeping() {
        assert_eq!(
            safety_for_files(Vec::new()),
            DiffSafety::BookkeepingOnly { files: Vec::new() }
        );
        assert_eq!(
            safety_for_files(vec![
                ".harness-pr-prep/agent-pr-for-code-5.txt".into(),
                ".harness-ci/agent-pr-for-code-5.txt".into(),
                ".forgejo/workflows/ci.yml".into(),
            ]),
            DiffSafety::BookkeepingOnly {
                files: vec![
                    ".harness-pr-prep/agent-pr-for-code-5.txt".into(),
                    ".harness-ci/agent-pr-for-code-5.txt".into(),
                    ".forgejo/workflows/ci.yml".into(),
                ]
            }
        );
    }

    #[test]
    fn classifies_product_file_as_meaningful() {
        assert_eq!(
            safety_for_files(vec!["crates/harness-production/src/product_chat.rs".into()]),
            DiffSafety::Meaningful {
                files: vec!["crates/harness-production/src/product_chat.rs".into()]
            }
        );
    }
}
