//! Forgejo-specific prep hook for the real LLM engineer.
//!
//! A real Forgejo PR requires the source branch to exist and to differ from the
//! target branch. The reference demo CI also gates on a commit-message marker.
//! This module keeps those provider-specific git/CI side effects outside the
//! portable `harness-forge` trait and injects them through `harness-agents`'
//! `EngineerPrep` seam.

use async_trait::async_trait;
use harness_forge::{CreatePullRequest, Forge};
use harness_runner::{AgentError, RoleTools};
use harness_workflow::ArtifactSource;

use crate::forgejo_rest::{self, RestError};

const PREP_DIR: &str = ".harness-pr-prep";
const CI_SENTINEL_DIR: &str = ".harness-ci";
pub const CI_PASS_MARKER: &str = "[ci-pass]";

/// Forgejo engineer prep for production workers.
pub struct ForgejoLlmPrep {
    base_url: String,
    token: String,
    allow_synthetic_bookkeeping: bool,
}

impl ForgejoLlmPrep {
    pub fn new(base_url: String, token: String, allow_synthetic_bookkeeping: bool) -> Self {
        Self {
            base_url,
            token,
            allow_synthetic_bookkeeping,
        }
    }

    fn require_synthetic_bookkeeping(&self) -> Result<(), AgentError> {
        if self.allow_synthetic_bookkeeping {
            Ok(())
        } else {
            Err(AgentError::message(
                "Forgejo engineer coding path is disabled: refusing to create synthetic \
                 .harness-pr-prep/.harness-ci commits; wire a real coding workspace or use \
                 --allow-synthetic-pr-prep only for throwaway demos",
            ))
        }
    }

    async fn repo_path<F: Forge + ?Sized>(
        &self,
        tools: &RoleTools<'_, F>,
    ) -> Result<(String, String), AgentError> {
        let repository = tools
            .get_repository()
            .await?
            .ok_or_else(|| AgentError::message(format!("repository {} not found", tools.repo())))?;
        Ok((repository.owner, repository.name))
    }

    async fn pr_head_branch<F: Forge + ?Sized>(
        &self,
        tools: &RoleTools<'_, F>,
        target: ArtifactSource,
    ) -> Result<Option<String>, AgentError> {
        let ArtifactSource::PullRequest { number } = target else {
            return Ok(None);
        };
        let Some(pull_request) = tools.get_pull_request(number).await? else {
            return Ok(None);
        };
        Ok(Some(pull_request.source.branch))
    }
}

#[async_trait]
impl<F: Forge + ?Sized> harness_agents::EngineerPrep<F> for ForgejoLlmPrep {
    async fn before_open_pr(
        &self,
        tools: &RoleTools<'_, F>,
        input: &CreatePullRequest,
    ) -> Result<(), AgentError> {
        self.require_synthetic_bookkeeping()?;
        let (owner, name) = self.repo_path(tools).await?;
        prepare_pull_request_head(&self.base_url, &self.token, &owner, &name, input)
            .await
            .map_err(|error| AgentError::message(format!("forgejo PR prep failed: {error}")))?;
        commit_ci_sentinel(
            &self.base_url,
            &self.token,
            &owner,
            &name,
            input.source.branch.as_str(),
        )
        .await
        .map_err(|error| AgentError::message(format!("forgejo CI sentinel seed failed: {error}")))
    }

    async fn before_address_ci_failure(
        &self,
        tools: &RoleTools<'_, F>,
        target: ArtifactSource,
    ) -> Result<(), AgentError> {
        self.require_synthetic_bookkeeping()?;
        let Some(branch) = self.pr_head_branch(tools, target).await? else {
            return Ok(());
        };
        let (owner, name) = self.repo_path(tools).await?;
        commit_ci_sentinel(&self.base_url, &self.token, &owner, &name, &branch)
            .await
            .map_err(|error| AgentError::message(format!("forgejo CI fix commit failed: {error}")))
    }
}

pub async fn prepare_pull_request_head(
    base_url: &str,
    token: &str,
    owner: &str,
    name: &str,
    input: &CreatePullRequest,
) -> Result<(), RestError> {
    let head = input.source.branch.as_str();
    let base_branch = input.target.branch.as_str();
    if head.is_empty() {
        return Err(RestError::Shape {
            what: "pr-prep head branch".into(),
            detail: "CreatePullRequest.source.branch is empty".into(),
        });
    }
    if base_branch.is_empty() {
        return Err(RestError::Shape {
            what: "pr-prep base branch".into(),
            detail: "CreatePullRequest.target.branch is empty".into(),
        });
    }

    let client = forgejo_rest::http_client()?;
    forgejo_rest::create_branch(&client, base_url, token, owner, name, head, base_branch).await?;
    forgejo_rest::commit_file(
        &client,
        base_url,
        token,
        owner,
        name,
        &prep_file_path(head),
        &prep_file_contents(head),
        &format!("prep PR head {head}"),
        head,
    )
    .await
}

pub async fn commit_ci_sentinel(
    base_url: &str,
    token: &str,
    owner: &str,
    name: &str,
    branch: &str,
) -> Result<(), RestError> {
    if branch.is_empty() {
        return Err(RestError::Shape {
            what: "ci-sentinel branch".into(),
            detail: "target branch is empty".into(),
        });
    }
    let client = forgejo_rest::http_client()?;
    let safe: String = branch
        .chars()
        .map(|c| if c == '/' { '-' } else { c })
        .collect();
    let path = format!("{CI_SENTINEL_DIR}/{safe}.txt");
    let message = format!("ci pass for {branch} {CI_PASS_MARKER}");
    forgejo_rest::commit_file(
        &client,
        base_url,
        token,
        owner,
        name,
        &path,
        &format!("ci pass marker for {branch}\n"),
        &message,
        branch,
    )
    .await
}

fn prep_file_path(head: &str) -> String {
    let safe: String = head
        .chars()
        .map(|c| if c == '/' { '-' } else { c })
        .collect();
    format!("{PREP_DIR}/{safe}.txt")
}

fn prep_file_contents(head: &str) -> String {
    format!("PR head branch: {head}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prep_file_path_flattens_slashes_under_prep_dir() {
        assert_eq!(
            prep_file_path("fake/pr-for-code-3"),
            ".harness-pr-prep/fake-pr-for-code-3.txt"
        );
    }
}
