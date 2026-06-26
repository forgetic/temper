// SPDX-License-Identifier: MPL-2.0

//! Writable in-process agent hooks for standalone mode.
//!
//! The distributed worker gets these hooks from the `temper-agent` subprocess.
//! Standalone runs the same coding loop in-process, so it provides equivalent
//! checkpoint hooks here and routes progress through the worker's in-memory
//! [`ProgressSink`].

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use temper_agent::CheckpointHook;
use temper_protocol_agent::{StepProgress, StepState, WorkspaceContext};
use temper_worker::{PrFreshnessGuard, ProgressSink};

#[derive(Default)]
pub(super) struct HookSet {
    pub(super) turn_hook: Option<Arc<dyn temper_agent::TurnHook>>,
    pub(super) checkpoint_hook: Option<Arc<dyn CheckpointHook>>,
}

pub(super) fn hooks_for_context(
    context: &WorkspaceContext,
    cwd: &Path,
    progress: Arc<dyn ProgressSink>,
    pr_freshness_guard: Option<Arc<dyn PrFreshnessGuard>>,
    step: Arc<AtomicU32>,
) -> HookSet {
    if !checkpoint_hooks_enabled(context) {
        return HookSet::default();
    }
    let hook = Arc::new(WritableHooks::new(
        context,
        cwd,
        progress,
        pr_freshness_guard,
        step,
    ));
    let turn_hook: Arc<dyn temper_agent::TurnHook> = hook.clone();
    let checkpoint_hook: Arc<dyn CheckpointHook> = hook.clone();
    HookSet {
        turn_hook: Some(turn_hook),
        checkpoint_hook: Some(checkpoint_hook),
    }
}

fn checkpoint_hooks_enabled(context: &WorkspaceContext) -> bool {
    matches!(
        context.checkout.as_deref().unwrap_or("writable"),
        "writable" | "pull_request_writable"
    )
}

#[derive(Clone)]
struct HookRepo {
    dir: String,
    branch: String,
}

struct WritableHooks {
    cwd: PathBuf,
    repos: Vec<HookRepo>,
    correlation_key: String,
    progress: Arc<dyn ProgressSink>,
    pr_freshness_guard: Option<Arc<dyn PrFreshnessGuard>>,
    pull_request_freshness: Option<temper_protocol_agent::PullRequestFreshness>,
    step: Arc<AtomicU32>,
    last_checkpoint: Mutex<Instant>,
}

impl WritableHooks {
    fn new(
        context: &WorkspaceContext,
        cwd: &Path,
        progress: Arc<dyn ProgressSink>,
        pr_freshness_guard: Option<Arc<dyn PrFreshnessGuard>>,
        step: Arc<AtomicU32>,
    ) -> Self {
        let repos = context
            .repos
            .iter()
            .filter(|repo| repo.is_writable())
            .map(|repo| HookRepo {
                dir: repo.dir.clone(),
                branch: repo.branch_hint.clone().unwrap_or_default(),
            })
            .collect();
        Self {
            cwd: cwd.to_path_buf(),
            repos,
            correlation_key: context.correlation_key.clone(),
            progress,
            pr_freshness_guard,
            pull_request_freshness: context.pull_request_freshness.clone(),
            step,
            last_checkpoint: Mutex::new(Instant::now()),
        }
    }

    async fn do_checkpoint(&self, label: Option<&str>) -> Result<Option<String>, String> {
        self.ensure_fresh().await?;
        let step = self.step.fetch_add(1, Ordering::SeqCst);
        let job = HookJob {
            cwd: self.cwd.clone(),
            repos: self.repos.clone(),
        };
        let label_owned = label.map(str::to_string);
        let outcome = skein::runtime::spawn_blocking(move || {
            job.checkpoint_sync(step, label_owned.as_deref())
        })
        .await;
        match outcome {
            Ok(Some(sha)) => {
                self.mark_checkpoint_pushed();
                self.progress.report(StepProgress {
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
                self.hand_back_step(step);
                Ok(None)
            }
            Err(error) => {
                self.hand_back_step(step);
                Err(error)
            }
        }
    }

    fn mark_checkpoint_pushed(&self) {
        if let Ok(mut last) = self.last_checkpoint.lock() {
            *last = Instant::now();
        }
    }

    fn hand_back_step(&self, step: u32) {
        let _ = self
            .step
            .compare_exchange(step + 1, step, Ordering::SeqCst, Ordering::SeqCst);
    }

    async fn ensure_fresh(&self) -> Result<(), String> {
        let Some(guard) = self.pr_freshness_guard.as_deref() else {
            return Ok(());
        };
        let Some(freshness) = self.pull_request_freshness.as_ref() else {
            return Ok(());
        };
        guard.check(freshness).await.map_err(|error| match error {
            temper_worker::PrFreshnessFailure::Stale(reason) => {
                format!("pull request is stale: {reason}")
            }
            temper_worker::PrFreshnessFailure::Unavailable(reason) => {
                format!("pull request freshness unavailable: {reason}")
            }
        })
    }

    fn backstop_due(&self) -> bool {
        self.last_checkpoint
            .lock()
            .map(|last| last.elapsed() >= temper_agent_session::DEFAULT_CHECKPOINT_INTERVAL)
            .unwrap_or(false)
    }
}

#[async_trait::async_trait]
impl temper_agent::TurnHook for WritableHooks {
    async fn before_model_call(&self, turn: usize) {
        if turn == 0 || !self.backstop_due() {
            return;
        }
        if let Err(error) = self.do_checkpoint(None).await {
            tracing::warn!(target: "temper::agent", %error, "backstop checkpoint skipped");
        }
    }
}

#[async_trait::async_trait]
impl CheckpointHook for WritableHooks {
    async fn checkpoint(&self, label: &str) -> Result<Option<String>, String> {
        self.do_checkpoint(Some(label)).await
    }
}

struct HookJob {
    cwd: PathBuf,
    repos: Vec<HookRepo>,
}

impl HookJob {
    fn checkpoint_sync(&self, step: u32, label: Option<&str>) -> Result<Option<String>, String> {
        let summary = label.unwrap_or("work in progress");
        let mut pushed = None;
        for repo in &self.repos {
            self.git_in(repo, &["add", "-A"])?;
            if self.git_in(repo, &["diff", "--cached", "--quiet"]).is_ok() {
                continue;
            }
            self.git_in(
                repo,
                &[
                    "commit",
                    "-m",
                    &format!("checkpoint(step {step}): {summary}"),
                ],
            )?;
            let sha = self.push_current_head(repo)?;
            if pushed.is_none() {
                pushed = Some(sha);
            }
        }
        Ok(pushed)
    }

    fn push_current_head(&self, repo: &HookRepo) -> Result<String, String> {
        let push_ref = format!("HEAD:refs/heads/{}", repo.branch);
        self.git_in(repo, &["push", "origin", &push_ref])?;
        Ok(self
            .git_in(repo, &["rev-parse", "HEAD"])?
            .trim()
            .to_string())
    }

    fn git_in(&self, repo: &HookRepo, args: &[&str]) -> Result<String, String> {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(self.cwd.join(&repo.dir))
            .output()
            .map_err(|error| format!("spawn git: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "git {} failed ({}): {}",
                args.first().copied().unwrap_or(""),
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        String::from_utf8(output.stdout).map_err(|error| format!("git output not UTF-8: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use temper_protocol_agent::{WorkspaceGuidance, WorkspaceRepository, WorkspaceWorkItem};

    #[test]
    fn writable_jobs_get_checkpoint_hooks() {
        assert!(checkpoint_hooks_enabled(&context(Some("writable"))));
        assert!(checkpoint_hooks_enabled(&context(None)));
    }

    #[test]
    fn pull_request_writable_gets_checkpoint_hooks() {
        assert!(checkpoint_hooks_enabled(&context(Some(
            "pull_request_writable"
        ))));
    }

    #[test]
    fn read_only_jobs_are_hookless() {
        assert!(!checkpoint_hooks_enabled(&context(Some("read_only"))));
        assert!(!checkpoint_hooks_enabled(&context(Some(
            "pull_request_read_only"
        ))));
    }

    fn context(checkout: Option<&str>) -> WorkspaceContext {
        WorkspaceContext {
            repos: vec![WorkspaceRepository {
                id: "repo".to_string(),
                owner: "acme".to_string(),
                name: "service".to_string(),
                default_branch: "main".to_string(),
                dir: "service".to_string(),
                access: "writable".to_string(),
                base_branch: "main".to_string(),
                branch_hint: Some("agent/pr-for-code-7".to_string()),
            }],
            work_item: WorkspaceWorkItem {
                role: "engineer".to_string(),
                queue: "code".to_string(),
                kind: "issue".to_string(),
                target: "Issue { number: ItemNumber(7) }".to_string(),
                context: "{}".to_string(),
            },
            action: "open_pr".to_string(),
            correlation_key: "pr-for-code-7".to_string(),
            checkout: checkout.map(str::to_string),
            allowed_verdicts: Vec::new(),
            guidance: WorkspaceGuidance::default(),
            pull_request_freshness: None,
        }
    }
}
