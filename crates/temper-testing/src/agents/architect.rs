use async_trait::async_trait;
use serde::Deserialize;
use temper_forge::{CreateIssue, Forge, ItemNumber, RepositoryId};
use temper_runner::{Agent, AgentError, RoleTools, WorkItem};
use temper_workflow::{
    ArtifactKindId, ArtifactRef, ArtifactSource, WorkflowMetadata, global_child_correlation_key,
    parse_metadata_block, render_metadata_block,
};

use super::support::{run_or_ignore_stale, run_set_body_or_ignore_stale};

#[derive(Clone, Debug, Default)]
pub struct FakeArchitect;

/// The architect fake for the **basic-delivery** workflow.
///
/// basic-delivery has no design/breakdown branch: the architect's only outcome
/// is to rewrite an `untriaged` intake issue into a `code` + `ready` issue with
/// a crisp body. It runs the routed terminal transition `triage_intake_to_code`
/// directly, binding the rewritten body through the same keyed `set_body`
/// runtime seam that [`FakeArchitect`] uses for the reference workflow - there is
/// no fan-out, no `needs_design`/`needs_breakdown`, and no second transition.
#[derive(Clone, Debug, Default)]
pub struct BasicArchitect;

#[derive(Clone, Debug, Default)]
pub struct ClosingArchitect;

/// Marker used by deterministic tests to tell the fake architect which child
/// issues to fan out before applying the normal triage transition.
pub const ARCHITECT_PLAN_BEGIN: &str = "<!-- temper:architect-plan";
const ARCHITECT_PLAN_END: &str = "-->";

#[derive(Clone, Debug, Default, Deserialize)]
struct ArchitectPlan {
    #[serde(default)]
    children: Vec<ArchitectPlanChild>,
}

#[derive(Clone, Debug, Deserialize)]
struct ArchitectPlanChild {
    slug: String,
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default, alias = "target_repository")]
    target_repo: Option<RepositoryId>,
}

#[async_trait]
impl<F: Forge + ?Sized> Agent<F> for FakeArchitect {
    async fn service(&self, item: &WorkItem, tools: &RoleTools<'_, F>) -> Result<bool, AgentError> {
        service_architect(item, tools, false).await
    }
}

#[async_trait]
impl<F: Forge + ?Sized> Agent<F> for ClosingArchitect {
    async fn service(&self, item: &WorkItem, tools: &RoleTools<'_, F>) -> Result<bool, AgentError> {
        service_architect(item, tools, true).await
    }
}

#[async_trait]
impl<F: Forge + ?Sized> Agent<F> for BasicArchitect {
    async fn service(&self, item: &WorkItem, tools: &RoleTools<'_, F>) -> Result<bool, AgentError> {
        basic_architect_service(item, tools).await
    }
}

async fn service_architect<F: Forge + ?Sized>(
    item: &WorkItem,
    tools: &RoleTools<'_, F>,
    close_parent_issues: bool,
) -> Result<bool, AgentError> {
    if item.queue.as_str() == "design_triage" && item.kind.as_str() == "intake" {
        let fanout = fan_out_architect_children(item, tools).await?;
        let transition = if fanout.has_children {
            "triage_to_blocked_code"
        } else {
            "triage_to_code"
        };
        let triaged = run_or_ignore_stale(tools, item.target, transition).await?;
        return Ok(fanout.changed || triaged);
    }
    if item.queue.as_str() == "landed_inbox" && item.kind.as_str() == "implementation_pr" {
        let reconciled = run_or_ignore_stale(tools, item.target, "reconcile_landed").await?;
        if reconciled && close_parent_issues {
            close_produced_parent_issues(item, tools).await?;
        }
        return Ok(reconciled);
    }
    Ok(false)
}

/// Correlation key for the `set_body` rewrite the basic architect binds when it
/// runs `triage_intake_to_code`.
fn basic_triage_body_key(number: ItemNumber) -> String {
    format!("triage-intake-code-{}", number.get())
}

/// The basic-delivery architect state machine.
///
/// On the `triage`/`intake` queue it runs the single routed terminal transition
/// `triage_intake_to_code`, binding a rewritten crisp body through the keyed
/// `set_body` runtime seam (effect index 0 - the transition declares exactly one
/// `set_body`). Unlike [`service_architect`] there is no fan-out and no second
/// triage branch: the one outcome marks the issue `code` + `ready`.
async fn basic_architect_service<F: Forge + ?Sized>(
    item: &WorkItem,
    tools: &RoleTools<'_, F>,
) -> Result<bool, AgentError> {
    if item.queue.as_str() != "triage" || item.kind.as_str() != "intake" {
        return Ok(false);
    }
    let ArtifactSource::Issue { number } = item.target else {
        return Ok(false);
    };
    let Some(issue) = tools.get_issue(number).await? else {
        return Ok(false);
    };
    let body = basic_triaged_body(&issue.title, &issue.body);
    run_set_body_or_ignore_stale(
        tools,
        item.target,
        "triage_intake_to_code",
        0,
        basic_triage_body_key(number),
        body,
    )
    .await
}

/// Builds the rewritten code-spec body the basic architect authors when it
/// triages an intake issue. It is deliberately a non-empty, deterministic
/// rewrite of the intake context so the `set_body` effect has real content.
fn basic_triaged_body(title: &str, intake_body: &str) -> String {
    let context = if intake_body.trim().is_empty() {
        String::new()
    } else {
        format!("\n\n## Intake context\n\n{}", intake_body.trim())
    };
    format!("## Code spec\n\nImplement: {title}{context}")
}

struct FanOutResult {
    changed: bool,
    has_children: bool,
}

async fn fan_out_architect_children<F: Forge + ?Sized>(
    item: &WorkItem,
    tools: &RoleTools<'_, F>,
) -> Result<FanOutResult, AgentError> {
    let ArtifactSource::Issue { number: parent } = item.target else {
        return Ok(FanOutResult {
            changed: false,
            has_children: false,
        });
    };
    let Some(issue) = tools.get_issue(parent).await? else {
        return Ok(FanOutResult {
            changed: false,
            has_children: false,
        });
    };
    let plan = parse_architect_plan(&issue.body)?;
    let has_children = !plan.children.is_empty();
    let mut changed = false;
    for child in plan.children {
        validate_architect_child(&child)?;
        let target_repo = child
            .target_repo
            .clone()
            .unwrap_or_else(|| tools.repo().clone());
        let key = global_child_correlation_key(tools.repo(), parent, &child.slug);
        let outcome = tools
            .ensure_issue_in_repo(
                &target_repo,
                &key,
                ArtifactRef::same_repo(parent),
                architect_child_input(&child),
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
    Ok(FanOutResult {
        changed,
        has_children,
    })
}

fn parse_architect_plan(body: &str) -> Result<ArchitectPlan, AgentError> {
    let Some(start) = body.find(ARCHITECT_PLAN_BEGIN) else {
        return Ok(ArchitectPlan::default());
    };
    let after = &body[start + ARCHITECT_PLAN_BEGIN.len()..];
    let Some(end) = after.find(ARCHITECT_PLAN_END) else {
        return Err(AgentError::message(
            "architect plan block was not terminated with `-->`",
        ));
    };
    serde_json::from_str(after[..end].trim()).map_err(|error| {
        AgentError::message(format!(
            "architect plan block contained invalid JSON: {error}"
        ))
    })
}

fn validate_architect_child(child: &ArchitectPlanChild) -> Result<(), AgentError> {
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

fn architect_child_input(child: &ArchitectPlanChild) -> CreateIssue {
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

async fn close_produced_parent_issues<F: Forge + ?Sized>(
    item: &WorkItem,
    tools: &RoleTools<'_, F>,
) -> Result<bool, AgentError> {
    let ArtifactSource::PullRequest { number } = item.target else {
        return Ok(false);
    };
    let Some(pull_request) = tools.get_pull_request(number).await? else {
        return Ok(false);
    };
    let Some(metadata) = parse_metadata_block(&pull_request.body)
        .map_err(|error| AgentError::message(format!("invalid PR workflow metadata: {error}")))?
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
