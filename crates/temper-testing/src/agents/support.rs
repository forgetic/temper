use temper_forge::Forge;
use temper_runner::{AgentError, RoleTools};
use temper_workflow::{ArtifactSource, ExecutionError, TransitionId};

pub(super) async fn run_or_ignore_stale<F: Forge + ?Sized>(
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

/// Runs `transition` binding `body` into the keyed `set_body` runtime seam at
/// `effect_index`, treating a stale-state failure as a quiet skip so the next
/// tick retries against fresh state (mirrors [`run_or_ignore_stale`]).
pub(super) async fn run_set_body_or_ignore_stale<F: Forge + ?Sized>(
    tools: &RoleTools<'_, F>,
    target: ArtifactSource,
    transition: &str,
    effect_index: usize,
    correlation_key: String,
    body: String,
) -> Result<bool, AgentError> {
    match tools
        .run_with_set_body_at(
            target,
            &TransitionId::new(transition),
            effect_index,
            correlation_key,
            body,
        )
        .await
    {
        Ok(_) => Ok(true),
        Err(error) if stale_execution(&error) => Ok(false),
        Err(error) => Err(error.into()),
    }
}

/// Runs `transition` binding `input` into the keyed `create_pull_request`
/// runtime seam at `effect_index`, treating a stale-state failure as a quiet
/// skip so the next tick retries against fresh state (mirrors
/// [`run_or_ignore_stale`]).
pub(super) async fn run_pull_request_create_or_ignore_stale<F: Forge + ?Sized>(
    tools: &RoleTools<'_, F>,
    target: ArtifactSource,
    transition: &str,
    effect_index: usize,
    correlation_key: String,
    input: temper_forge::CreatePullRequest,
) -> Result<bool, AgentError> {
    match tools
        .run_with_pull_request_create_at(
            target,
            &TransitionId::new(transition),
            effect_index,
            correlation_key,
            input,
        )
        .await
    {
        Ok(_) => Ok(true),
        Err(error) if stale_execution(&error) => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn stale_execution(error: &ExecutionError) -> bool {
    matches!(
        error,
        ExecutionError::Precondition { .. }
            | ExecutionError::TargetMissing { .. }
            | ExecutionError::Classification(_)
    )
}
