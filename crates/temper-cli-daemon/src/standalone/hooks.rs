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
use temper_worker::ProgressSink;

#[derive(Default)]
pub(super) struct HookSet {
    pub(super) turn_hook: Option<Arc<dyn temper_agent::TurnHook>>,
    pub(super) checkpoint_hook: Option<Arc<dyn CheckpointHook>>,
}

pub(super) fn hooks_for_context(
    context: &WorkspaceContext,
    cwd: &Path,
    progress: Arc<dyn ProgressSink>,
    step: Arc<AtomicU32>,
) -> HookSet {
    let policy = hook_policy(context);
    if policy == HookPolicy::None {
        return HookSet::default();
    }
    let hook = Arc::new(WritableHooks::new(context, cwd, progress, step));
    let turn_hook: Arc<dyn temper_agent::TurnHook> = hook.clone();
    let checkpoint_hook: Arc<dyn CheckpointHook> = hook.clone();
    let publish_plan_hook = (policy == HookPolicy::CheckpointsAndPlan).then(|| {
        let hook: Arc<dyn PublishPlanHook> = hook.clone();
        hook
    });
    HookSet {
        turn_hook: Some(turn_hook),
        checkpoint_hook: Some(checkpoint_hook),
        publish_plan_hook,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HookPolicy {
    None,
    CheckpointsOnly,
    CheckpointsAndPlan,
}

fn hook_policy(context: &WorkspaceContext) -> HookPolicy {
    match context.checkout.as_deref().unwrap_or("writable") {
        "writable" => HookPolicy::CheckpointsAndPlan,
        "pull_request_writable" => HookPolicy::CheckpointsOnly,
        _ => HookPolicy::None,
    }
}

#[derive(Clone)]
struct HookRepo {
    dir: String,
    branch: String,
    base_branch: String,
}

struct WritableHooks {
    cwd: PathBuf,
    repos: Vec<HookRepo>,
    correlation_key: String,
    progress: Arc<dyn ProgressSink>,
    step: Arc<AtomicU32>,
    last_checkpoint: Mutex<Instant>,
}

impl WritableHooks {
    fn new(
        context: &WorkspaceContext,
        cwd: &Path,
        progress: Arc<dyn ProgressSink>,
        step: Arc<AtomicU32>,
    ) -> Self {
        let repos = context
            .repos
            .iter()
            .filter(|repo| repo.is_writable())
            .map(|repo| HookRepo {
                dir: repo.dir.clone(),
                branch: repo.branch_hint.clone().unwrap_or_default(),
                base_branch: repo.base_branch.clone(),
            })
            .collect();
        Self {
            cwd: cwd.to_path_buf(),
            repos,
            correlation_key: context.correlation_key.clone(),
            progress,
            step,
            last_checkpoint: Mutex::new(Instant::now()),
        }
    }

    async fn do_checkpoint(&self, label: Option<&str>) -> Result<Option<String>, String> {
        let step = self.step.fetch_add(1, Ordering::SeqCst);
        let job = HookJob {
            cwd: self.cwd.clone(),
            repos: self.repos.clone(),
            correlation_key: self.correlation_key.clone(),
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
                    plan_publication: None,
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

    async fn do_publish_plan(&self, publication: PlanPublication) -> Result<(), String> {
        let step = self.step.fetch_add(1, Ordering::SeqCst);
        let job = HookJob {
            cwd: self.cwd.clone(),
            repos: self.repos.clone(),
            correlation_key: self.correlation_key.clone(),
        };
        let outcome = skein::runtime::spawn_blocking(move || job.publish_plan_sync()).await;
        match outcome {
            Ok(sha) => {
                self.mark_checkpoint_pushed();
                self.progress
                    .report(self.plan_marker(step, sha, publication));
                Ok(())
            }
            Err(error) => {
                self.hand_back_step(step);
                Err(error)
            }
        }
    }

    fn plan_marker(
        &self,
        step: u32,
        pushed_sha: String,
        publication: PlanPublication,
    ) -> StepProgress {
        StepProgress {
            correlation_key: self.correlation_key.clone(),
            step,
            status: PLAN_STATUS.to_string(),
            state: StepState::Done,
            pushed_sha: Some(pushed_sha),
            note: Some(publication.summary.clone()),
            plan_publication: Some(publication),
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

#[async_trait::async_trait]
impl PublishPlanHook for WritableHooks {
    async fn publish_plan(&self, publication: PlanPublication) -> Result<(), String> {
        self.do_publish_plan(publication).await
    }
}

struct HookJob {
    cwd: PathBuf,
    repos: Vec<HookRepo>,
    correlation_key: String,
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

    fn publish_plan_sync(&self) -> Result<String, String> {
        if self.repos.is_empty() {
            return Err("publish_plan requires at least one writable repository".to_string());
        }
        for repo in &self.repos {
            if self.worktree_dirty(repo)? {
                return Err(format!(
                    "publish_plan must run before product edits; worktree already has changes in {}",
                    repo.dir
                ));
            }
        }
        let mut first_sha = None;
        for repo in &self.repos {
            if repo.branch.trim().is_empty() {
                return Err(format!(
                    "publish_plan target {} is missing a work branch hint",
                    repo.dir
                ));
            }
            if !self.branch_ahead_of_base(repo)? {
                self.git_in(
                    repo,
                    &[
                        "commit",
                        "--allow-empty",
                        "-m",
                        &format!("Publish implementation plan for {}", self.correlation_key),
                    ],
                )?;
            }
            let sha = self.push_current_head(repo)?;
            if first_sha.is_none() {
                first_sha = Some(sha);
            }
        }
        first_sha.ok_or_else(|| "publish_plan found no writable repository to push".to_string())
    }

    fn worktree_dirty(&self, repo: &HookRepo) -> Result<bool, String> {
        let status = self.git_in(repo, &["status", "--porcelain=v1", "--untracked-files=all"])?;
        Ok(!status.is_empty())
    }

    fn branch_ahead_of_base(&self, repo: &HookRepo) -> Result<bool, String> {
        let range = format!("origin/{}..HEAD", repo.base_branch);
        let count = self.git_in(repo, &["rev-list", "--count", &range])?;
        Ok(count.trim() != "0")
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
    fn writable_jobs_get_checkpoint_and_plan_hooks() {
        assert_eq!(
            hook_policy(&context(Some("writable"))),
            HookPolicy::CheckpointsAndPlan
        );
        assert_eq!(hook_policy(&context(None)), HookPolicy::CheckpointsAndPlan);
    }

    #[test]
    fn pull_request_writable_gets_checkpoints_without_plan_publication() {
        assert_eq!(
            hook_policy(&context(Some("pull_request_writable"))),
            HookPolicy::CheckpointsOnly
        );
    }

    #[test]
    fn read_only_jobs_are_hookless() {
        assert_eq!(hook_policy(&context(Some("read_only"))), HookPolicy::None);
        assert_eq!(
            hook_policy(&context(Some("pull_request_read_only"))),
            HookPolicy::None
        );
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
        }
    }
}
