// SPDX-License-Identifier: MPL-2.0

//! Turn-boundary checkpointing (phase 6b): commit + push the working tree at
//! turn boundaries, emit the matching step-progress markers, and recover a
//! prior run's pushed checkpoints from the prepared branch.
//!
//! The same [`Checkpointer`] is both the mechanical backstop ([`TurnHook`]) and
//! the model-driven checkpoint tool ([`CheckpointHook`]). Awaited by the agent
//! shell immediately before each model call — the previous turn's tool batch has
//! fully drained, so the tree is quiescent. Failures are logged to stderr and
//! swallowed: a checkpoint is an opportunistic crash-recovery point, never a
//! reason to fail the run. A marker is emitted only after the push succeeds (it
//! never claims more than the branch holds).
//!
//! [`TurnHook`]: temper_agent_core::TurnHook
//! [`CheckpointHook`]: temper_agent::CheckpointHook

mod backstop;
mod resume;
mod sync;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use temper_agent_protocol::{StepProgress, StepState, WorkspaceContext};

use crate::progress::emit;

use backstop::{CHECKPOINT_DEADLINE_MARGIN, backstop_decision};
use sync::CheckpointJob;

/// One writable repo a checkpoint commits + pushes: its sibling dir under the
/// workspace root, the work branch to push, and the base branch to diff against
/// (ADR 0023 — a single-repo job is just one of these).
#[derive(Clone)]
struct CheckpointRepo {
    dir: String,
    branch: String,
    base_branch: String,
}

/// Fallback backstop cadence when no deadline is supplied, so crash-recovery is
/// still bounded.
const DEFAULT_CHECKPOINT_INTERVAL: Duration = Duration::from_secs(300);

pub(crate) struct Checkpointer {
    cwd: PathBuf,
    /// The writable repos to checkpoint, in manifest order (the first is the
    /// primary — home of the coordinating artifact).
    repos: Vec<CheckpointRepo>,
    correlation_key: String,
    /// Per-role git identity + push token from the worker-provided env
    /// (`TEMPER_FORGEJO_{USER,EMAIL,TOKEN}_<ROLE>`); absent values degrade
    /// gracefully (generic identity, credential-less push).
    user: Option<String>,
    email: Option<String>,
    token: Option<String>,
    /// Wall-clock job deadline (lease expiry) from `TEMPER_AGENT_DEADLINE` (unix
    /// seconds), if the host supplied one. The backstop fires within
    /// [`CHECKPOINT_DEADLINE_MARGIN`] of it.
    deadline: Option<SystemTime>,
    /// Fallback backstop cadence used when there is no deadline (or between
    /// deadline checks).
    interval: Duration,
    /// When the last successful checkpoint pushed — drives the interval backstop.
    last_checkpoint: Mutex<Instant>,
    /// The next step index to hand out (1 = the Started marker).
    step: AtomicU32,
}

impl Checkpointer {
    pub(crate) fn new(cwd: &Path, context: &WorkspaceContext) -> Self {
        let role_key = context.work_item.role.to_uppercase().replace('-', "_");
        let env = |prefix: &str| {
            std::env::var(format!("{prefix}_{role_key}"))
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        };
        let repos = context
            .repos
            .iter()
            .filter(|repo| repo.is_writable())
            .map(|repo| CheckpointRepo {
                dir: repo.dir.clone(),
                branch: repo.branch_hint.clone().unwrap_or_default(),
                base_branch: repo.base_branch.clone(),
            })
            .collect();
        // The host (worker/daemon, which owns the lease) may pass the job
        // deadline as a unix timestamp; absent it, the backstop falls back to a
        // fixed interval.
        let deadline = std::env::var("TEMPER_AGENT_DEADLINE")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .map(|secs| UNIX_EPOCH + Duration::from_secs(secs));
        let interval = std::env::var("TEMPER_AGENT_CHECKPOINT_INTERVAL_SECS")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .map(Duration::from_secs)
            .unwrap_or(DEFAULT_CHECKPOINT_INTERVAL);
        Self {
            cwd: cwd.to_path_buf(),
            repos,
            correlation_key: context.correlation_key.clone(),
            user: env("TEMPER_FORGEJO_USER"),
            email: env("TEMPER_FORGEJO_EMAIL"),
            token: env("TEMPER_FORGEJO_TOKEN"),
            deadline,
            interval,
            last_checkpoint: Mutex::new(Instant::now()),
            step: AtomicU32::new(2),
        }
    }

    /// Continue step numbering after a resumed run's markers.
    pub(crate) fn start_after(&self, next: Option<u32>) {
        if let Some(next) = next {
            self.step.store(next + 1, Ordering::SeqCst);
        }
    }

    /// The next unused step index (the terminal Done marker takes it).
    pub(crate) fn next_step(&self) -> u32 {
        self.step.load(Ordering::SeqCst)
    }

    /// One commit+push checkpoint (off the loop thread), emitting the marker and
    /// resetting the backstop timer; returns the pushed sha (`None` if nothing
    /// changed). Shared by the model-driven `checkpoint` tool ([`CheckpointHook`])
    /// and the mechanical backstop.
    ///
    /// [`CheckpointHook`]: temper_agent::CheckpointHook
    async fn do_checkpoint(&self, label: Option<&str>) -> Result<Option<String>, String> {
        let step = self.step.fetch_add(1, Ordering::SeqCst);
        let job = CheckpointJob {
            cwd: self.cwd.clone(),
            repos: self.repos.clone(),
            correlation_key: self.correlation_key.clone(),
            user: self.user.clone(),
            email: self.email.clone(),
            token: self.token.clone(),
        };
        let label_owned = label.map(str::to_string);
        let outcome = skein::runtime::spawn_blocking(move || {
            job.into_checkpointer()
                .checkpoint_sync(step, label_owned.as_deref())
        })
        .await;
        match outcome {
            Ok(Some(sha)) => {
                if let Ok(mut last) = self.last_checkpoint.lock() {
                    *last = Instant::now();
                }
                emit(&StepProgress {
                    correlation_key: self.correlation_key.clone(),
                    step,
                    status: label.unwrap_or("push checkpoint").to_string(),
                    state: StepState::Done,
                    pushed_sha: Some(sha.clone()),
                    note: None,
                });
                Ok(Some(sha))
            }
            Ok(None) => {
                // Clean tree: hand the unused step index back when possible
                // (best effort — a concurrent bump just leaves a gap).
                let _ =
                    self.step
                        .compare_exchange(step + 1, step, Ordering::SeqCst, Ordering::SeqCst);
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    /// Whether the mechanical safety-net checkpoint is due: the job deadline
    /// (lease expiry) is within the margin, or the checkpoint interval has
    /// elapsed since the last push.
    fn backstop_due(&self) -> bool {
        let since_last = self
            .last_checkpoint
            .lock()
            .map(|last| last.elapsed())
            .unwrap_or(Duration::ZERO);
        backstop_decision(
            SystemTime::now(),
            self.deadline,
            CHECKPOINT_DEADLINE_MARGIN,
            since_last,
            self.interval,
        )
    }
}

#[async_trait::async_trait]
impl temper_agent_core::TurnHook for Checkpointer {
    async fn before_model_call(&self, turn: usize) {
        // Turn 0 is the first model call: nothing has run yet. Otherwise this is
        // only a SAFETY NET — the model drives normal checkpoints via the
        // `checkpoint` tool, so we push here only when the deadline is near or
        // the interval has elapsed, not every turn.
        if turn == 0 || !self.backstop_due() {
            return;
        }
        if let Err(error) = self.do_checkpoint(None).await {
            tracing::warn!(target: "temper_agent", %error, "backstop checkpoint skipped");
        }
    }
}

#[async_trait::async_trait]
impl temper_agent::CheckpointHook for Checkpointer {
    async fn checkpoint(&self, label: &str) -> Result<Option<String>, String> {
        self.do_checkpoint(Some(label)).await
    }
}

/// Provides [`Arc`]-typed hook handles for the run, so the caller does not need
/// to name the hook traits.
impl Checkpointer {
    /// This checkpointer as the mechanical [`TurnHook`] backstop.
    ///
    /// [`TurnHook`]: temper_agent_core::TurnHook
    pub(crate) fn as_turn_hook(self: &Arc<Self>) -> Arc<dyn temper_agent_core::TurnHook> {
        self.clone()
    }

    /// This checkpointer as the model-driven [`CheckpointHook`].
    ///
    /// [`CheckpointHook`]: temper_agent::CheckpointHook
    pub(crate) fn as_checkpoint_hook(self: &Arc<Self>) -> Arc<dyn temper_agent::CheckpointHook> {
        self.clone()
    }
}
