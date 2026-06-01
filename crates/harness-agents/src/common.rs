//! Shared building blocks for the LLM role adapters.
//!
//! Every role agent runs the same shape: read the work item through
//! [`RoleTools`], serialize it into JSON the model reasons over, ask the model
//! for one structured decision, and map the choice onto authorized `RoleTools`
//! calls. The pieces that do not vary by role live here so each role module is
//! just its prompt, its decision enum, and the mapping.

use harness_forge::Forge;
use harness_runner::{AgentError, RoleTools, WorkItem};
use harness_workflow::{ArtifactSource, ExecutionError, TransitionId};

/// Serializes the work item plus the issue/PR it points at into the JSON the
/// model reasons over. Reads go through [`RoleTools`]; a read failure aborts the
/// tick (it is not the model's fault).
///
/// This is the **same** context every role sees, so the prompts can rely on a
/// stable shape: `{repository, queue, kind, artifact:{type, number, title,
/// body, labels, state}}`.
pub(crate) async fn build_context<F: Forge + ?Sized>(
    item: &WorkItem,
    tools: &RoleTools<'_, F>,
) -> Result<String, AgentError> {
    let artifact = match item.target {
        ArtifactSource::Issue { number } => tools.get_issue(number).await?.map(|issue| {
            serde_json::json!({
                "type": "issue",
                "number": number.get(),
                "title": issue.title,
                "body": issue.body,
                "labels": issue.labels,
                "state": format!("{:?}", issue.state),
            })
        }),
        ArtifactSource::PullRequest { number } => tools.get_pull_request(number).await?.map(|pr| {
            serde_json::json!({
                "type": "pull_request",
                "number": number.get(),
                "title": pr.title,
                "body": pr.body,
                "labels": pr.labels,
                "state": format!("{:?}", pr.state),
            })
        }),
    };

    let context = serde_json::json!({
        "repository": tools.repo().as_str(),
        "queue": item.queue.as_str(),
        "kind": item.kind.as_str(),
        "artifact": artifact,
    });
    Ok(serde_json::to_string_pretty(&context).unwrap_or_else(|_| context.to_string()))
}

/// Runs a transition, treating stale/precondition/classification failures as "no
/// progress" (return `false`) exactly as the fakes do, so a model acting on a
/// stale item degrades gracefully rather than erroring.
pub(crate) async fn run_or_ignore_stale<F: Forge + ?Sized>(
    tools: &RoleTools<'_, F>,
    target: ArtifactSource,
    transition: &str,
) -> Result<bool, AgentError> {
    match tools.run(target, &TransitionId::new(transition)).await {
        Ok(_) => Ok(true),
        Err(error) if stale_execution(&error) => Ok(false),
        Err(error) => Err(error.into()),
    }
}

/// Whether an execution error is a "the world moved on" condition the role
/// should treat as no-progress rather than a hard failure.
pub(crate) fn stale_execution(error: &ExecutionError) -> bool {
    matches!(
        error,
        ExecutionError::Precondition { .. }
            | ExecutionError::TargetMissing { .. }
            | ExecutionError::Classification(_)
    )
}
