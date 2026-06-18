// SPDX-License-Identifier: MPL-2.0

//! Plan-publication branch prep for writable agent sessions.
//!
//! The model-facing `publish_plan` tool reaches this hook. The hook refuses to
//! run on a dirty tree, creates/pushes the deterministic work branch with an
//! empty commit when needed, and only then emits the plan-carrying progress
//! marker on stdout for the worker to relay.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use temper_protocol_agent::{PlanPublication, StepProgress, StepState};

use super::sync::CheckpointJob;
use crate::progress::emit;

use super::Checkpointer;

const PLAN_STATUS: &str = "publish implementation plan";

struct PlanPublicationJob {
    cwd: PathBuf,
    repos: Vec<super::CheckpointRepo>,
    correlation_key: String,
}

impl PlanPublicationJob {
    fn into_checkpointer(self) -> Checkpointer {
        CheckpointJob {
            cwd: self.cwd,
            repos: self.repos,
            correlation_key: self.correlation_key,
        }
        .into_checkpointer()
    }
}

impl Checkpointer {
    async fn do_publish_plan(&self, publication: PlanPublication) -> Result<(), String> {
        let step = self.step.fetch_add(1, Ordering::SeqCst);
        let job = PlanPublicationJob {
            cwd: self.cwd.clone(),
            repos: self.repos.clone(),
            correlation_key: self.correlation_key.clone(),
        };
        let outcome =
            skein::runtime::spawn_blocking(move || job.into_checkpointer().publish_plan_sync())
                .await;
        match outcome {
            Ok(sha) => {
                if let Ok(mut last) = self.last_checkpoint.lock() {
                    *last = Instant::now();
                }
                emit(&self.plan_marker(step, sha, publication));
                Ok(())
            }
            Err(error) => {
                let _ =
                    self.step
                        .compare_exchange(step + 1, step, Ordering::SeqCst, Ordering::SeqCst);
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

    fn publish_plan_sync(&self) -> Result<String, String> {
        if self.repos.is_empty() {
            return Err("publish_plan requires at least one writable repository".to_string());
        }
        for repo in &self.repos {
            if self.worktree_dirty(&repo.dir)? {
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
            if !self.branch_ahead_of_base(&repo.dir, &repo.base_branch)? {
                self.git_in(
                    &repo.dir,
                    &[
                        "commit",
                        "--allow-empty",
                        "-m",
                        &format!("Publish implementation plan for {}", self.correlation_key),
                    ],
                )?;
            }
            let push_ref = format!("HEAD:refs/heads/{}", repo.branch);
            self.git_in(&repo.dir, &["push", "origin", &push_ref])?;
            let sha = self
                .git_in(&repo.dir, &["rev-parse", "HEAD"])?
                .trim()
                .to_string();
            if first_sha.is_none() {
                first_sha = Some(sha);
            }
        }
        first_sha.ok_or_else(|| "publish_plan found no writable repository to push".to_string())
    }

    fn worktree_dirty(&self, dir: &str) -> Result<bool, String> {
        let status = self.git_in(dir, &["status", "--porcelain=v1", "--untracked-files=all"])?;
        Ok(!status.is_empty())
    }

    fn branch_ahead_of_base(&self, dir: &str, base_branch: &str) -> Result<bool, String> {
        let range = format!("origin/{base_branch}..HEAD");
        let count = self.git_in(dir, &["rev-list", "--count", &range])?;
        Ok(count.trim() != "0")
    }

    /// This checkpointer as the model-driven [`PublishPlanHook`].
    ///
    /// [`PublishPlanHook`]: temper_agent::PublishPlanHook
    pub(crate) fn as_publish_plan_hook(self: &Arc<Self>) -> Arc<dyn temper_agent::PublishPlanHook> {
        self.clone()
    }
}

#[async_trait::async_trait]
impl temper_agent::PublishPlanHook for Checkpointer {
    async fn publish_plan(&self, publication: PlanPublication) -> Result<(), String> {
        self.do_publish_plan(publication).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_CHECKPOINT_INTERVAL;
    use std::path::Path;
    use std::time::Duration;
    use temper_protocol_agent::{
        PlanPublicationTarget, WorkspaceContext, WorkspaceGuidance, WorkspaceRepository,
        WorkspaceWorkItem,
    };

    #[test]
    fn plan_marker_carries_publication_payload() {
        let temp = tempfile::tempdir().expect("temp");
        let context = workspace_context(temp.path());
        let checkpointer = Checkpointer::new(temp.path(), &context, None, Duration::from_secs(300));
        let publication = publication();

        let marker = checkpointer.plan_marker(2, "abc123".to_string(), publication.clone());

        assert_eq!(marker.correlation_key, "pr-for-code-7");
        assert_eq!(marker.status, PLAN_STATUS);
        assert_eq!(marker.state, StepState::Done);
        assert_eq!(marker.pushed_sha.as_deref(), Some("abc123"));
        assert_eq!(marker.note.as_deref(), Some("Ship the change"));
        assert_eq!(marker.plan_publication, Some(publication));
    }

    #[test]
    fn publish_plan_pushes_empty_branch_once() {
        temper_agent_io::block_on(async {
            let fixture = GitFixture::new();
            let context = workspace_context(fixture.workspace.as_path());
            let checkpointer = Checkpointer::new(
                fixture.workspace.as_path(),
                &context,
                None,
                DEFAULT_CHECKPOINT_INTERVAL,
            );

            checkpointer
                .do_publish_plan(publication())
                .await
                .expect("first publication succeeds");
            checkpointer
                .do_publish_plan(publication())
                .await
                .expect("repeat publication succeeds");

            let branch = "refs/heads/agent/pr-for-code-7";
            let head = git_output(&fixture.origin, &["rev-parse", branch]);
            assert_eq!(head.len(), 40);
            assert_eq!(
                git_output(&fixture.origin, &["rev-list", "--count", branch]),
                "2"
            );
            assert_eq!(
                git_output(&fixture.origin, &["log", "-1", "--format=%s", branch]),
                "Publish implementation plan for pr-for-code-7"
            );
            assert_eq!(
                git_output(&fixture.origin, &["diff", "--name-only", "main", branch]),
                "",
                "empty plan commit must not alter the tree"
            );
        });
    }

    #[test]
    fn publish_plan_rejects_dirty_worktree() {
        temper_agent_io::block_on(async {
            let fixture = GitFixture::new();
            std::fs::write(fixture.workspace.join("service/NOTES.md"), "draft")
                .expect("dirty file");
            let context = workspace_context(fixture.workspace.as_path());
            let checkpointer = Checkpointer::new(
                fixture.workspace.as_path(),
                &context,
                None,
                DEFAULT_CHECKPOINT_INTERVAL,
            );

            let error = checkpointer
                .do_publish_plan(publication())
                .await
                .expect_err("dirty tree rejected");

            assert!(
                error.contains("publish_plan must run before product edits"),
                "unexpected error: {error}"
            );
            assert!(
                git(
                    &fixture.origin,
                    &["rev-parse", "--verify", "refs/heads/agent/pr-for-code-7"]
                )
                .is_err(),
                "dirty rejection must not push a branch"
            );
        });
    }

    struct GitFixture {
        _temp: tempfile::TempDir,
        origin: PathBuf,
        workspace: PathBuf,
    }

    impl GitFixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().expect("temp");
            let origin = temp.path().join("origin.git");
            let seed = temp.path().join("seed");
            let workspace = temp.path().join("workspace");
            git_ok(temp.path(), &["init", "--bare", path_str(&origin)]);
            git_ok(temp.path(), &["init", "-b", "main", path_str(&seed)]);
            std::fs::write(seed.join("README.md"), "# seed\n").expect("seed file");
            git_ok(
                &seed,
                &[
                    "-c",
                    "user.name=Seed",
                    "-c",
                    "user.email=seed@example.test",
                    "add",
                    "README.md",
                ],
            );
            git_ok(
                &seed,
                &[
                    "-c",
                    "user.name=Seed",
                    "-c",
                    "user.email=seed@example.test",
                    "commit",
                    "-m",
                    "initial",
                ],
            );
            git_ok(&seed, &["remote", "add", "origin", path_str(&origin)]);
            git_ok(&seed, &["push", "origin", "main"]);
            std::fs::create_dir_all(&workspace).expect("workspace dir");
            git_ok(
                temp.path(),
                &[
                    "clone",
                    path_str(&origin),
                    path_str(&workspace.join("service")),
                ],
            );
            git_ok(
                &workspace.join("service"),
                &["checkout", "-B", "agent/pr-for-code-7", "origin/main"],
            );
            git_ok(
                &workspace.join("service"),
                &["config", "user.name", "Agent"],
            );
            git_ok(
                &workspace.join("service"),
                &["config", "user.email", "agent@example.test"],
            );
            Self {
                _temp: temp,
                origin,
                workspace,
            }
        }
    }

    fn workspace_context(_root: &Path) -> WorkspaceContext {
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
            checkout: Some("writable".to_string()),
            allowed_verdicts: Vec::new(),
            guidance: WorkspaceGuidance::default(),
        }
    }

    fn publication() -> PlanPublication {
        PlanPublication {
            summary: "Ship the change".to_string(),
            phases: vec!["Write test".to_string(), "Implement".to_string()],
            target_repos: vec![PlanPublicationTarget {
                repo_path: "acme/service".to_string(),
                dir: "service".to_string(),
                base_branch: "main".to_string(),
                branch_hint: Some("agent/pr-for-code-7".to_string()),
            }],
        }
    }

    fn git_output(cwd: &Path, args: &[&str]) -> String {
        let output = git(cwd, args).expect("git command succeeds");
        String::from_utf8(output.stdout)
            .expect("utf8")
            .trim()
            .to_string()
    }

    fn git_ok(cwd: &Path, args: &[&str]) {
        git(cwd, args).expect("git command succeeds");
    }

    fn git(cwd: &Path, args: &[&str]) -> Result<std::process::Output, String> {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .map_err(|error| error.to_string())?;
        if output.status.success() {
            Ok(output)
        } else {
            Err(String::from_utf8_lossy(&output.stderr).into_owned())
        }
    }

    fn path_str(path: &Path) -> &str {
        path.to_str().expect("utf8 path")
    }
}
