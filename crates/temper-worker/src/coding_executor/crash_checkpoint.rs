use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use temper_protocol_agent::{StepProgress, StepState};

use crate::agent_runner::ProgressSink;

use super::PreparedRepo;

/// Wraps the worker's progress sink so the executor can append a host-authored
/// crash checkpoint marker after an agent/provider failure without reusing a
/// step index the agent already emitted.
pub(super) struct TrackingProgressSink {
    inner: Arc<dyn ProgressSink>,
    max_step: AtomicU32,
}

impl TrackingProgressSink {
    pub(super) fn new(inner: Arc<dyn ProgressSink>) -> Self {
        Self {
            inner,
            max_step: AtomicU32::new(0),
        }
    }

    fn next_step(&self) -> u32 {
        self.max_step.load(Ordering::SeqCst).saturating_add(1)
    }
}

impl ProgressSink for TrackingProgressSink {
    fn report(&self, progress: StepProgress) {
        self.max_step.fetch_max(progress.step, Ordering::SeqCst);
        self.inner.report(progress);
    }
}

/// Pushes one best-effort crash-recovery checkpoint after a transient
/// agent/provider failure. Returns the first pushed sha, or `None` when every
/// writable repo was clean and either matched base or had already pushed HEAD.
pub(super) async fn push_crash_checkpoint(
    prepared: &[PreparedRepo],
    correlation_key: &str,
    progress: &TrackingProgressSink,
) -> Result<Option<String>, String> {
    let step = progress.next_step();
    let pushed_sha = push_checkpoint_repos(prepared, step).await?;
    let Some(sha) = pushed_sha else {
        return Ok(None);
    };

    progress.report(StepProgress {
        correlation_key: correlation_key.to_string(),
        step,
        status: "push crash checkpoint".to_string(),
        state: StepState::Done,
        pushed_sha: Some(sha.clone()),
        note: Some("transient failure recovery checkpoint".to_string()),
    });
    Ok(Some(sha))
}

async fn push_checkpoint_repos(
    prepared: &[PreparedRepo],
    step: u32,
) -> Result<Option<String>, String> {
    let mut first_pushed = None;
    for prepared in prepared.iter().filter(|repo| repo.writable) {
        let sha = push_checkpoint_repo(prepared, step).await?;
        if first_pushed.is_none() {
            first_pushed = sha;
        }
    }
    Ok(first_pushed)
}

async fn push_checkpoint_repo(
    prepared: &PreparedRepo,
    step: u32,
) -> Result<Option<String>, String> {
    let branch = prepared
        .branch_hint
        .as_deref()
        .ok_or_else(|| format!("writable repo {} is missing a branch hint", prepared.repo))?;

    let has_tree_changes = prepared
        .workspace
        .has_changes()
        .await
        .map_err(|error| format!("inspect workspace changes in {}: {error}", prepared.repo))?;
    let has_product_diff = has_tree_changes
        || prepared
            .workspace
            .tree_differs_from_base()
            .await
            .map_err(|error| {
                format!(
                    "inspect workspace tree diff from base in {}: {error}",
                    prepared.repo
                )
            })?;
    if !has_product_diff {
        return Ok(None);
    }

    if has_tree_changes {
        prepared
            .workspace
            .commit_all(&format!("checkpoint(step {step}): crash recovery"))
            .await
            .map_err(|error| format!("commit crash checkpoint in {}: {error}", prepared.repo))?;
    } else {
        let head_sha = prepared
            .workspace
            .head_sha()
            .await
            .map_err(|error| format!("inspect HEAD in {}: {error}", prepared.repo))?;
        if prepared
            .workspace
            .remote_branch_head(branch)
            .await
            .map_err(|error| {
                format!(
                    "inspect remote checkpoint branch {branch} in {}: {error}",
                    prepared.repo
                )
            })?
            .as_deref()
            == Some(head_sha.as_str())
        {
            return Ok(None);
        }
    }

    prepared
        .workspace
        .push_branch(branch)
        .await
        .map(Some)
        .map_err(|error| format!("push crash checkpoint for {}: {error}", prepared.repo))
}
