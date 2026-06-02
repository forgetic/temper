//! The real, LLM-driven architect role agent.
//!
//! [`LlmArchitect`] mirrors the deterministic `FakeArchitect`/`ClosingArchitect`
//! pair: the **decision** (triage an intake issue, or reconcile a landed PR)
//! comes from a DeepSeek model, but every **mutation** goes through
//! [`RoleTools`] — the same authority boundary the fakes use.
//!
//! Like the fakes, the architect has a **closing** variant: after reconciling a
//! landed implementation PR it also closes the PR's produced parent issues,
//! unblocking dependents (the `dependency_chain` scenario). Whether to close is a
//! deterministic post-step of the reconcile, not an LLM choice, exactly as in the
//! fake — the model only decides *that* the PR should be reconciled.

use async_trait::async_trait;
use serde::Deserialize;
use temper_forge::{CreateIssue, Forge, RepositoryId};
use temper_runner::{Agent, AgentError, RoleTools, WorkItem};
use temper_workflow::{
    global_child_correlation_key, parse_metadata_block, render_metadata_block, ArtifactKindId,
    ArtifactRef, ArtifactSource, WorkflowMetadata,
};

use temper_agents::decision::{run_decision, DecisionError};
use temper_agents::ProviderConfig;

use super::common::{build_context, run_or_ignore_stale};
use super::prompts::ARCHITECT_SYSTEM_PROMPT;

/// The action the LLM chose for an architect work item.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case", tag = "action")]
pub enum ArchitectDecision {
    /// Triage an intake design issue into ready code work, optionally fanning out
    /// planned child code issues first.
    TriageToCode {
        /// Child issues to ensure before the parent leaves the triage queue.
        #[serde(default)]
        children: Vec<PlannedChildIssue>,
    },
    /// Reconcile a freshly landed implementation pull request.
    ReconcileLanded,
    /// Do nothing (stale, already handled, or not applicable).
    NoAction,
}

/// One child issue planned by the architect during intake triage.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PlannedChildIssue {
    /// Stable child intent used in the idempotency key.
    pub slug: String,
    /// Issue title for the child work item.
    pub title: String,
    /// Issue body for the child work item.
    #[serde(default)]
    pub body: String,
    /// Target repository. Omitted means the same repository as the parent.
    #[serde(default, alias = "target_repository")]
    pub target_repo: Option<RepositoryId>,
}

/// A real architect agent: decide with the LLM, act through [`RoleTools`].
///
/// `close_parent_issues` selects the **closing** behavior variant: when set, a
/// successful `reconcile_landed` is followed by closing the PR's produced parent
/// issues (mirroring `ClosingArchitect`).
pub struct LlmArchitect {
    provider: ProviderConfig,
    close_parent_issues: bool,
}

impl LlmArchitect {
    /// Builds the default architect (reconciles, leaves parent issues open).
    pub fn new(provider: ProviderConfig) -> Self {
        Self {
            provider,
            close_parent_issues: false,
        }
    }

    /// Builds the **closing** architect (also closes a merged PR's parent issues).
    pub fn closing(provider: ProviderConfig) -> Self {
        Self {
            provider,
            close_parent_issues: true,
        }
    }

    async fn decide(
        &self,
        item: &WorkItem,
        context: &str,
    ) -> Result<ArchitectDecision, AgentError> {
        match run_decision::<ArchitectDecision>(&self.provider, ARCHITECT_SYSTEM_PROMPT, context)
            .await
        {
            Ok(decision) => Ok(decision),
            Err(DecisionError::Provider(error)) => Err(AgentError::message(error.to_string())),
            Err(error) => {
                eprintln!(
                    "temper-agents: architect LLM decision failed for {:?} on queue '{}', \
                     treating as no-action: {error}",
                    item.target,
                    item.queue.as_str()
                );
                Ok(ArchitectDecision::NoAction)
            }
        }
    }

    /// Ensures every planned child issue exists and is linked from the parent.
    async fn ensure_planned_children<F: Forge + ?Sized>(
        &self,
        item: &WorkItem,
        tools: &RoleTools<'_, F>,
        children: &[PlannedChildIssue],
    ) -> Result<bool, AgentError> {
        if children.is_empty() {
            return Ok(false);
        }
        let ArtifactSource::Issue { number: parent } = item.target else {
            return Ok(false);
        };
        let mut changed = false;
        for child in children {
            validate_child(child)?;
            let target_repo = child
                .target_repo
                .clone()
                .unwrap_or_else(|| tools.repo().clone());
            let correlation_key = global_child_correlation_key(tools.repo(), parent, &child.slug);
            let outcome = tools
                .ensure_issue_in_repo(
                    &target_repo,
                    &correlation_key,
                    ArtifactRef::same_repo(parent),
                    child_issue_input(child),
                )
                .await?;
            let child_number = outcome.artifact().number;
            changed |= outcome.was_created();
            changed |= tools
                .add_issue_dependency_metadata(
                    parent,
                    ArtifactRef::in_repo(target_repo.clone(), child_number),
                )
                .await?;
        }
        Ok(changed)
    }

    /// Closes every parent issue recorded in the landed PR's workflow metadata.
    /// Mirrors `ClosingArchitect::close_produced_parent_issues`.
    async fn close_produced_parent_issues<F: Forge + ?Sized>(
        &self,
        item: &WorkItem,
        tools: &RoleTools<'_, F>,
    ) -> Result<bool, AgentError> {
        let ArtifactSource::PullRequest { number } = item.target else {
            return Ok(false);
        };
        let Some(pull_request) = tools.get_pull_request(number).await? else {
            return Ok(false);
        };
        let Some(metadata) = parse_metadata_block(&pull_request.body).map_err(|error| {
            AgentError::message(format!("invalid PR workflow metadata: {error}"))
        })?
        else {
            return Ok(false);
        };

        let mut closed = false;
        for parent in metadata.parents {
            if parent.is_same_repo() {
                closed |= tools.close_issue(parent.number).await?;
            }
        }
        Ok(closed)
    }
}

fn validate_child(child: &PlannedChildIssue) -> Result<(), AgentError> {
    if child.slug.trim().is_empty() {
        return Err(AgentError::message(
            "architect child slug must not be empty",
        ));
    }
    if child.title.trim().is_empty() {
        return Err(AgentError::message(format!(
            "architect child `{}` title must not be empty",
            child.slug
        )));
    }
    Ok(())
}

fn child_issue_input(child: &PlannedChildIssue) -> CreateIssue {
    CreateIssue {
        title: child.title.clone(),
        body: child_body(&child.body),
        labels: vec!["code".to_string(), "ready".to_string()],
        assignees: Vec::new(),
    }
}

fn child_body(body: &str) -> String {
    let metadata = WorkflowMetadata {
        kind: Some(ArtifactKindId::new("code")),
        ..WorkflowMetadata::default()
    };
    if body.trim().is_empty() {
        render_metadata_block(&metadata)
    } else {
        format!("{body}\n\n{}", render_metadata_block(&metadata))
    }
}

#[async_trait]
impl<F: Forge + ?Sized> Agent<F> for LlmArchitect {
    async fn service(&self, item: &WorkItem, tools: &RoleTools<'_, F>) -> Result<bool, AgentError> {
        let context = build_context(item, tools).await?;
        match self.decide(item, &context).await? {
            ArchitectDecision::TriageToCode { children } => {
                let children_changed = self.ensure_planned_children(item, tools, &children).await?;
                let transition = if children.is_empty() {
                    "triage_to_code"
                } else {
                    "triage_to_blocked_code"
                };
                let triaged = run_or_ignore_stale(tools, item.target, transition).await?;
                Ok(children_changed || triaged)
            }
            ArchitectDecision::ReconcileLanded => {
                let reconciled =
                    run_or_ignore_stale(tools, item.target, "reconcile_landed").await?;
                if reconciled && self.close_parent_issues {
                    self.close_produced_parent_issues(item, tools).await?;
                }
                Ok(reconciled)
            }
            ArchitectDecision::NoAction => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_legacy_triage_decision_without_children() {
        let decision: ArchitectDecision = serde_json::from_str(
            r#"{"action":"triage_to_code","reason":"plain same-repo triage"}"#,
        )
        .expect("decision parses");

        assert_eq!(
            decision,
            ArchitectDecision::TriageToCode {
                children: Vec::new()
            }
        );
    }

    #[test]
    fn parses_triage_decision_with_target_repo_children() {
        let decision: ArchitectDecision = serde_json::from_str(
            r#"{
              "action": "triage_to_code",
              "children": [
                {
                  "slug": "canary",
                  "target_repo": "forgejo:acme/service-canary",
                  "title": "Canary work",
                  "body": "Implement canary side."
                }
              ]
            }"#,
        )
        .expect("decision parses");

        let ArchitectDecision::TriageToCode { children } = decision else {
            panic!("expected triage decision");
        };
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].slug, "canary");
        assert_eq!(
            children[0].target_repo,
            Some(RepositoryId::new("forgejo:acme/service-canary"))
        );
    }
}
