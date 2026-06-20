use std::ffi::OsString;
use std::path::{Path, PathBuf};

mod git;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoleGitIdentity {
    pub user: String,
    pub email: String,
    pub token: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceConfig {
    pub root: PathBuf,
    pub base_branch: String,
}

pub struct Workspace {
    path: PathBuf,
    base_branch: String,
    remote_url: String,
    identity: RoleGitIdentity,
}

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    #[error("git command `{command}` failed (status {status}): {stderr}")]
    Git {
        command: String,
        status: String,
        stderr: String,
    },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid utf-8 in git output: {0}")]
    Utf8(String),
    #[error("invalid repo `{0}`: expected owner/name")]
    InvalidRepo(String),
}

pub fn forgejo_remote_url(base_url: &str, repo: &str) -> Result<String, WorkspaceError> {
    validate_repo(repo)?;

    Ok(format!("{}/{}.git", base_url.trim_end_matches('/'), repo))
}

impl Workspace {
    pub fn new(
        config: &WorkspaceConfig,
        repo: &str,
        role: &str,
        identity: RoleGitIdentity,
        remote_url: impl Into<String>,
    ) -> Result<Self, WorkspaceError> {
        validate_repo(repo)?;

        Ok(Self {
            path: config.root.join(repo.replace('/', "__")).join(role),
            base_branch: config.base_branch.clone(),
            remote_url: remote_url.into(),
            identity,
        })
    }

    /// Construct a workspace at an explicit checkout path (one sibling repo of a
    /// coordinated multi-repo workspace, ADR 0023), rather than the persistent
    /// per-(repo, role) layout `Workspace::new` derives.
    pub fn at(
        path: PathBuf,
        base_branch: String,
        identity: RoleGitIdentity,
        remote_url: impl Into<String>,
    ) -> Self {
        Self {
            path,
            base_branch,
            remote_url: remote_url.into(),
            identity,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Prepare a read-only sibling: clone-or-reuse, fetch the base branch, and
    /// check it out without creating a work branch. Such a repo is present only
    /// so the combined build resolves; it is never committed or pushed.
    pub async fn prepare_read_only(&self) -> Result<(), WorkspaceError> {
        self.prepare_base_checkout().await?;
        let start_point = format!("origin/{}", self.base_branch);
        self.run_workspace_git(
            false,
            format!("git checkout -B {} {start_point}", self.base_branch),
            vec![
                OsString::from("checkout"),
                OsString::from("-B"),
                OsString::from(self.base_branch.clone()),
                OsString::from(start_point),
            ],
        )
        .await?;
        Ok(())
    }

    pub async fn prepare(&self, work_branch: &str) -> Result<(), WorkspaceError> {
        self.prepare_base_checkout().await?;

        // Crash-resume: when the work branch already exists on the forge (a
        // prior dispatch pushed checkpoints or a full result before dying, or
        // a revise round is re-entering an existing PR head), resume from it
        // instead of resetting to base — resetting would orphan the pushed
        // commits and make the agent's next checkpoint push non-fast-forward.
        // A fetch failure (most commonly: the branch does not exist yet) falls
        // back to a fresh branch from base.
        let start_point = match self.try_fetch_work_branch(work_branch).await {
            true => format!("origin/{work_branch}"),
            false => format!("origin/{}", self.base_branch),
        };
        self.run_workspace_git(
            false,
            format!("git checkout -B {work_branch} {start_point}"),
            vec![
                OsString::from("checkout"),
                OsString::from("-B"),
                OsString::from(work_branch),
                OsString::from(start_point),
            ],
        )
        .await?;

        Ok(())
    }

    /// Fetches the remote work branch if it exists. `false` when the fetch
    /// fails (branch absent, or transient trouble — in which case the fresh
    /// start is safe: a later non-fast-forward push fails loudly rather than
    /// clobbering remote state).
    async fn try_fetch_work_branch(&self, work_branch: &str) -> bool {
        let refspec = format!("+refs/heads/{work_branch}:refs/remotes/origin/{work_branch}");
        self.run_workspace_git(
            true,
            format!("git fetch origin {work_branch}"),
            vec![
                OsString::from("fetch"),
                OsString::from("origin"),
                OsString::from(refspec),
            ],
        )
        .await
        .is_ok()
    }

    /// Prepare the workspace at a pull request's head (read-only review checkout):
    /// same clone-or-reuse + base-branch fetch as `prepare`, then fetch the forge's
    /// `refs/pull/<n>/head` into the local ref `refs/temper/pr/<n>/head` and
    /// `checkout -B <work_branch>` from it. Nothing is ever pushed from this state.
    pub async fn prepare_pull_request_head(
        &self,
        pull_request_number: u64,
        work_branch: &str,
    ) -> Result<(), WorkspaceError> {
        self.prepare_base_checkout().await?;

        let remote_ref = format!("refs/pull/{pull_request_number}/head");
        let local_ref = format!("refs/temper/pr/{pull_request_number}/head");
        let refspec = format!("+{remote_ref}:{local_ref}");
        self.run_workspace_git(
            true,
            format!("git fetch origin {refspec}"),
            vec![
                OsString::from("fetch"),
                OsString::from("origin"),
                OsString::from(refspec),
            ],
        )
        .await?;

        self.run_workspace_git(
            false,
            format!("git checkout -B {work_branch} {local_ref}"),
            vec![
                OsString::from("checkout"),
                OsString::from("-B"),
                OsString::from(work_branch),
                OsString::from(local_ref),
            ],
        )
        .await?;

        Ok(())
    }

    async fn prepare_base_checkout(&self) -> Result<(), WorkspaceError> {
        if self.path.exists() {
            self.run_workspace_git(
                false,
                "git remote set-url origin <remote>".to_string(),
                vec![
                    OsString::from("remote"),
                    OsString::from("set-url"),
                    OsString::from("origin"),
                    OsString::from(self.remote_url.as_str()),
                ],
            )
            .await?;
        } else {
            if let Some(parent) = self.path.parent() {
                // Offload the mkdir to the blocking pool: in unified mode the
                // worker shares the single-threaded loop with the daemon and
                // agent, so even a fast filesystem syscall must not run inline
                // (a slow/networked FS would stall every other task).
                let parent = parent.to_path_buf();
                skein::runtime::spawn_blocking(move || std::fs::create_dir_all(&parent)).await?;
            }

            self.run_git(
                None,
                true,
                "git clone --no-checkout <remote> <checkout>".to_string(),
                vec![
                    OsString::from("clone"),
                    OsString::from("--no-checkout"),
                    OsString::from(self.remote_url.as_str()),
                    self.path.as_os_str().to_os_string(),
                ],
            )
            .await?;
        }

        let refspec = format!(
            "+refs/heads/{}:refs/remotes/origin/{}",
            self.base_branch, self.base_branch
        );
        self.run_workspace_git(
            true,
            format!("git fetch origin {}", self.base_branch),
            vec![
                OsString::from("fetch"),
                OsString::from("origin"),
                OsString::from(refspec),
            ],
        )
        .await?;

        Ok(())
    }

    pub async fn commit_all(&self, message: &str) -> Result<String, WorkspaceError> {
        self.run_workspace_git(
            false,
            "git add -A".to_string(),
            vec![OsString::from("add"), OsString::from("-A")],
        )
        .await?;

        self.run_workspace_git(
            false,
            "git commit -m <message>".to_string(),
            vec![
                OsString::from("commit"),
                OsString::from("-m"),
                OsString::from(message),
            ],
        )
        .await?;

        self.head_sha().await
    }

    pub async fn push_branch(&self, branch_name: &str) -> Result<String, WorkspaceError> {
        self.run_workspace_git(
            true,
            format!("git push origin HEAD:refs/heads/{branch_name}"),
            vec![
                OsString::from("push"),
                OsString::from("origin"),
                OsString::from(format!("HEAD:refs/heads/{branch_name}")),
            ],
        )
        .await?;

        self.head_sha().await
    }

    /// Discard all local tracked and untracked working-tree changes.
    pub async fn discard_changes(&self) -> Result<(), WorkspaceError> {
        self.run_workspace_git(
            false,
            "git reset --hard HEAD".to_string(),
            vec![
                OsString::from("reset"),
                OsString::from("--hard"),
                OsString::from("HEAD"),
            ],
        )
        .await?;
        self.run_workspace_git(
            false,
            "git clean -ffd".to_string(),
            vec![OsString::from("clean"), OsString::from("-ffd")],
        )
        .await?;

        Ok(())
    }

    pub async fn head_sha(&self) -> Result<String, WorkspaceError> {
        let output = self
            .run_workspace_git(
                false,
                "git rev-parse HEAD".to_string(),
                vec![OsString::from("rev-parse"), OsString::from("HEAD")],
            )
            .await?;
        let stdout = String::from_utf8(output.stdout)
            .map_err(|error| WorkspaceError::Utf8(error.to_string()))?;

        Ok(stdout.trim().to_string())
    }

    /// True when HEAD's tree differs from the fetched base branch. A clean
    /// working tree can still hold checkpoint-committed product work, but an
    /// empty checkpoint commit leaves this false even though HEAD is ahead.
    pub async fn tree_differs_from_base(&self) -> Result<bool, WorkspaceError> {
        let base = format!("origin/{}", self.base_branch);
        let output = self
            .run_workspace_git(
                false,
                format!("git diff --name-only {base} HEAD"),
                vec![
                    OsString::from("diff"),
                    OsString::from("--name-only"),
                    OsString::from(base),
                    OsString::from("HEAD"),
                ],
            )
            .await?;
        Ok(!output.stdout.is_empty())
    }

    /// True when the working tree has any staged, unstaged, or untracked change.
    pub async fn has_changes(&self) -> Result<bool, WorkspaceError> {
        let output = self
            .run_workspace_git(
                false,
                "git status --porcelain=v1 --untracked-files=all".to_string(),
                vec![
                    OsString::from("status"),
                    OsString::from("--porcelain=v1"),
                    OsString::from("--untracked-files=all"),
                ],
            )
            .await?;

        Ok(!output.stdout.is_empty())
    }
}

fn validate_repo(repo: &str) -> Result<(), WorkspaceError> {
    let mut parts = repo.split('/');
    let owner = parts.next().unwrap_or_default();
    let name = parts.next().unwrap_or_default();
    if owner.is_empty() || name.is_empty() || parts.next().is_some() {
        Err(WorkspaceError::InvalidRepo(repo.to_string()))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forgejo_remote_url_accepts_owner_name_repo() {
        assert_eq!(
            forgejo_remote_url("http://localhost:3000", "ai/smith").expect("valid repo"),
            "http://localhost:3000/ai/smith.git"
        );
        assert_eq!(
            forgejo_remote_url("http://localhost:3000/", "ai/smith").expect("valid repo"),
            "http://localhost:3000/ai/smith.git"
        );
    }

    #[test]
    fn forgejo_remote_url_rejects_malformed_repo_names() {
        for repo in ["smith", "ai/", "/smith", "ai/smith/extra"] {
            let error =
                forgejo_remote_url("http://localhost:3000", repo).expect_err("invalid repo");
            match error {
                WorkspaceError::InvalidRepo(invalid) => assert_eq!(invalid, repo),
                other => panic!("unexpected error for {repo:?}: {other:?}"),
            }
        }
    }
}
