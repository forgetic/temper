use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};

mod git;
mod recovery;
mod target_branch;

use recovery::ReadOnlyTarget;
pub use recovery::{PreparationOutcome, QuarantineManifest, RecoveryContext};

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
    recovery_context: RecoveryContext,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CheckoutState {
    Created,
    Reused,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ScopedWorkspaceCleanupOutcome {
    Removed { path: PathBuf },
    NotFound { path: PathBuf },
    SkippedActive { path: PathBuf },
    SkippedEmptyCorrelationKey,
    SkippedNotDirectory { path: PathBuf },
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ScopedWorkspacePathError {
    #[error("invalid workspace role path component `{0}`")]
    InvalidRole(String),
    #[error("scoped workspace path `{path}` escapes workspace root `{root}`")]
    EscapesRoot { root: PathBuf, path: PathBuf },
}

#[derive(Debug, thiserror::Error)]
pub enum ScopedWorkspaceCleanupError {
    #[error(transparent)]
    UnsafePath(#[from] ScopedWorkspacePathError),
    #[error("io error while cleaning scoped workspace `{path}`: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
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
    #[error("workspace recovery requires permanent operator attention: {0}")]
    Recovery(String),
    #[error("workspace is quarantined at `{path}`")]
    Quarantined { path: PathBuf },
    #[error("{0}")]
    BranchMaterialization(String),
}

pub fn forgejo_remote_url(base_url: &str, repo: &str) -> Result<String, WorkspaceError> {
    validate_repo(repo)?;

    Ok(format!("{}/{}.git", base_url.trim_end_matches('/'), repo))
}

/// Percent-encodes a coordination key into one safe path component.
///
/// Common queue-generated keys stay readable (`pr-for-code-7`), while
/// separators, dots, absolute-path markers, percent signs, and non-ASCII bytes
/// are encoded so an unusual key cannot escape the role root or create nested
/// paths.
pub fn workspace_scope_component(coordination_key: &str) -> String {
    if coordination_key.is_empty() {
        return "%EMPTY".to_string();
    }

    let mut component = String::with_capacity(coordination_key.len());
    for &byte in coordination_key.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' => {
                component.push(char::from(byte));
            }
            _ => {
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                component.push('%');
                component.push(char::from(HEX[(byte >> 4) as usize]));
                component.push(char::from(HEX[(byte & 0x0F) as usize]));
            }
        }
    }
    component
}

/// Returns the coordination-scoped workspace root
/// `<workspace_root>/<role>/<safe-coordination-key>`.
///
/// The role is expected to be a configured workflow role id and must already be
/// one normal path component; the coordination key is percent-encoded by
/// [`workspace_scope_component`]. This keeps cleanup and checkout preparation on
/// the same layout without duplicating ad-hoc path logic.
pub fn scoped_workspace_root(
    workspace_root: &Path,
    role: &str,
    coordination_key: &str,
) -> Result<PathBuf, ScopedWorkspacePathError> {
    validate_role_component(role)?;
    let path = workspace_root
        .join(role)
        .join(workspace_scope_component(coordination_key));
    if !path.starts_with(workspace_root) {
        return Err(ScopedWorkspacePathError::EscapesRoot {
            root: workspace_root.to_path_buf(),
            path,
        });
    }
    Ok(path)
}

/// Removes a coordination-scoped workspace directory when it is not known
/// active. Intended for worker-owned cleanup after an implementation PR lands.
///
/// This synchronous core is deterministic and easy to unit test; async callers
/// should use [`cleanup_scoped_workspace`] so filesystem work runs off the
/// runtime thread.
pub fn cleanup_scoped_workspace_sync(
    workspace_root: &Path,
    role: &str,
    correlation_key: &str,
    active: bool,
) -> Result<ScopedWorkspaceCleanupOutcome, ScopedWorkspaceCleanupError> {
    let correlation_key = correlation_key.trim();
    if correlation_key.is_empty() {
        return Ok(ScopedWorkspaceCleanupOutcome::SkippedEmptyCorrelationKey);
    }
    let path = scoped_workspace_root(workspace_root, role, correlation_key)?;
    if active {
        return Ok(ScopedWorkspaceCleanupOutcome::SkippedActive { path });
    }

    remove_scoped_workspace_dir(path)
}

/// Async wrapper for [`cleanup_scoped_workspace_sync`] that offloads blocking
/// filesystem traversal to the worker runtime's blocking pool.
pub async fn cleanup_scoped_workspace(
    workspace_root: PathBuf,
    role: String,
    correlation_key: String,
    active: bool,
) -> Result<ScopedWorkspaceCleanupOutcome, ScopedWorkspaceCleanupError> {
    skein::runtime::spawn_blocking(move || {
        cleanup_scoped_workspace_sync(&workspace_root, &role, &correlation_key, active)
    })
    .await
}

fn remove_scoped_workspace_dir(
    path: PathBuf,
) -> Result<ScopedWorkspaceCleanupOutcome, ScopedWorkspaceCleanupError> {
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ScopedWorkspaceCleanupOutcome::NotFound { path });
        }
        Err(source) => return Err(ScopedWorkspaceCleanupError::Io { path, source }),
    };
    if !metadata.file_type().is_dir() {
        return Ok(ScopedWorkspaceCleanupOutcome::SkippedNotDirectory { path });
    }
    std::fs::remove_dir_all(&path).map_err(|source| ScopedWorkspaceCleanupError::Io {
        path: path.clone(),
        source,
    })?;
    Ok(ScopedWorkspaceCleanupOutcome::Removed { path })
}

fn validate_role_component(role: &str) -> Result<(), ScopedWorkspacePathError> {
    let mut components = Path::new(role).components();
    let valid = matches!(components.next(), Some(Component::Normal(component)) if component == OsStr::new(role))
        && components.next().is_none();
    if valid {
        Ok(())
    } else {
        Err(ScopedWorkspacePathError::InvalidRole(role.to_string()))
    }
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
            recovery_context: RecoveryContext {
                job_id: format!("legacy/{repo}/{role}"),
                correlation_key: role.to_string(),
                repository: repo.to_string(),
            },
        })
    }

    /// Construct a workspace at an explicit checkout path (one sibling repo of a
    /// coordination-scoped multi-repo workspace), rather than the legacy
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
            recovery_context: RecoveryContext::default(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Prepare a read-only sibling: clone-or-reuse, fetch the base branch, and
    /// check it out without creating a work branch. Such a repo is present only
    /// so the combined build resolves; it is never committed or pushed.
    pub async fn prepare_read_only(&self) -> Result<PreparationOutcome, WorkspaceError> {
        self.prepare_read_only_target(
            &self.base_branch,
            ReadOnlyTarget::RemoteBranch(self.base_branch.clone()),
        )
        .await
    }

    pub async fn prepare(&self, work_branch: &str) -> Result<PreparationOutcome, WorkspaceError> {
        self.prepare_writable_recovering(work_branch).await
    }

    /// Prepare the workspace at a pull request's head (read-only review checkout):
    /// same clone-or-reuse + base-branch fetch as `prepare`, then fetch the forge's
    /// `refs/pull/<n>/head` into the local ref `refs/temper/pr/<n>/head` and
    /// `checkout -B <work_branch>` from it. Nothing is ever pushed from this state.
    pub async fn prepare_pull_request_head(
        &self,
        pull_request_number: u64,
        work_branch: &str,
    ) -> Result<PreparationOutcome, WorkspaceError> {
        self.prepare_read_only_target(
            work_branch,
            ReadOnlyTarget::PullRequest(pull_request_number),
        )
        .await
    }

    pub(super) async fn ensure_checkout_repo(&self) -> Result<CheckoutState, WorkspaceError> {
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
            Ok(CheckoutState::Reused)
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
            Ok(CheckoutState::Created)
        }
    }

    pub(super) async fn fetch_remote_branch(&self, branch: &str) -> Result<(), WorkspaceError> {
        let refspec = format!("+refs/heads/{branch}:refs/remotes/origin/{branch}");
        self.run_workspace_git(
            true,
            format!("git fetch origin {branch}"),
            vec![
                OsString::from("fetch"),
                OsString::from("origin"),
                OsString::from(refspec),
            ],
        )
        .await
        .map(|_| ())
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

        let head = self.head_sha().await?;
        let remote_tracking_ref = format!("refs/remotes/origin/{branch_name}");
        self.run_workspace_git(
            false,
            format!("git update-ref {remote_tracking_ref} {head}"),
            vec![
                OsString::from("update-ref"),
                OsString::from(remote_tracking_ref),
                OsString::from(&head),
            ],
        )
        .await?;
        Ok(head)
    }

    /// Discard all local tracked and untracked working-tree changes.
    pub async fn discard_changes(&self) -> Result<(), WorkspaceError> {
        self.discard_changes_to_ref("HEAD").await
    }

    pub(crate) async fn discard_changes_to_ref(&self, target: &str) -> Result<(), WorkspaceError> {
        self.run_workspace_git(
            false,
            format!("git reset --hard {target}"),
            vec![
                OsString::from("reset"),
                OsString::from("--hard"),
                OsString::from(target),
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
    /// working tree can still hold committed product work from a recovered
    /// branch; an empty commit leaves this false even though HEAD is ahead.
    pub async fn tree_differs_from_base(&self) -> Result<bool, WorkspaceError> {
        let base = format!("origin/{}", self.base_branch);
        self.tree_differs_from_ref(&base).await
    }

    /// True when HEAD's tree differs from another commit/ref. This ignores
    /// commit metadata, so an empty commit does not count as product work;
    /// callers that start from an existing PR branch can use the prepared head
    /// SHA as the baseline instead of the target base branch.
    pub async fn tree_differs_from_ref(&self, reference: &str) -> Result<bool, WorkspaceError> {
        let output = self
            .run_workspace_git(
                false,
                format!("git diff --name-only {reference} HEAD"),
                vec![
                    OsString::from("diff"),
                    OsString::from("--name-only"),
                    OsString::from(reference),
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

    /// Repository-relative paths changed in the working tree.
    pub async fn status_paths(&self) -> Result<Vec<String>, WorkspaceError> {
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
        let stdout = String::from_utf8(output.stdout)
            .map_err(|error| WorkspaceError::Utf8(error.to_string()))?;
        Ok(status_porcelain_paths(&stdout))
    }

    /// Repository-relative paths whose trees differ from the fetched base branch.
    pub async fn diff_paths_from_base(&self) -> Result<Vec<String>, WorkspaceError> {
        let base = format!("origin/{}", self.base_branch);
        self.diff_paths_from_ref(&base).await
    }

    /// Repository-relative paths whose HEAD tree differs from another commit/ref.
    pub async fn diff_paths_from_ref(
        &self,
        reference: &str,
    ) -> Result<Vec<String>, WorkspaceError> {
        let output = self
            .run_workspace_git(
                false,
                format!("git diff --name-only {reference} HEAD"),
                vec![
                    OsString::from("diff"),
                    OsString::from("--name-only"),
                    OsString::from(reference),
                    OsString::from("HEAD"),
                ],
            )
            .await?;
        let stdout = String::from_utf8(output.stdout)
            .map_err(|error| WorkspaceError::Utf8(error.to_string()))?;
        Ok(stdout
            .lines()
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(str::to_string)
            .collect())
    }
}

fn status_porcelain_paths(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter_map(|line| {
            let path = line.get(3..)?.trim();
            if path.is_empty() {
                None
            } else {
                Some(path.rsplit(" -> ").next().unwrap_or(path).to_string())
            }
        })
        .collect()
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

    #[test]
    fn workspace_scope_component_keeps_common_keys_readable() {
        assert_eq!(
            workspace_scope_component("pr-for-code-448"),
            "pr-for-code-448"
        );
        assert_eq!(
            workspace_scope_component("coord_for_code_448"),
            "coord_for_code_448"
        );
    }

    #[test]
    fn workspace_scope_component_encodes_path_syntax_as_one_component() {
        assert_eq!(
            workspace_scope_component("../agent/pr-for-code-448"),
            "%2E%2E%2Fagent%2Fpr-for-code-448"
        );
        assert_eq!(workspace_scope_component("/absolute"), "%2Fabsolute");
        assert_eq!(workspace_scope_component("windows\\path"), "windows%5Cpath");
        assert_eq!(workspace_scope_component(""), "%EMPTY");
    }

    #[test]
    fn workspace_scope_component_is_collision_resistant_for_encoded_bytes() {
        assert_ne!(
            workspace_scope_component("a/b"),
            workspace_scope_component("a%2Fb")
        );
        assert_ne!(
            workspace_scope_component("a.b"),
            workspace_scope_component("a%2Eb")
        );
    }

    #[test]
    fn scoped_workspace_root_keeps_escaped_correlation_under_workspace_root() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path =
            scoped_workspace_root(temp.path(), "engineer", "../escape").expect("scoped path");

        assert!(path.starts_with(temp.path()));
        let relative = path.strip_prefix(temp.path()).expect("under root");
        assert_eq!(relative.components().count(), 2);
        assert_eq!(relative, Path::new("engineer").join("%2E%2E%2Fescape"));
    }

    #[test]
    fn scoped_workspace_root_rejects_role_escape() {
        let parent = tempfile::tempdir().expect("tempdir");
        let root = parent.path().join("root");
        let outside = parent.path().join("outside");
        std::fs::create_dir_all(&root).expect("root dir");
        std::fs::create_dir_all(&outside).expect("outside dir");

        let error = scoped_workspace_root(&root, "../outside", "pr-for-code-7")
            .expect_err("role escape rejected");

        assert!(matches!(error, ScopedWorkspacePathError::InvalidRole(_)));
        assert!(outside.exists());
    }

    #[test]
    fn cleanup_scoped_workspace_removes_inactive_workstream() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path =
            scoped_workspace_root(temp.path(), "engineer", "pr-for-code-7").expect("scoped path");
        std::fs::create_dir_all(path.join("temper")).expect("workspace dir");
        std::fs::write(path.join("temper/README.md"), "product").expect("workspace file");

        let outcome =
            cleanup_scoped_workspace_sync(temp.path(), "engineer", "pr-for-code-7", false)
                .expect("cleanup");

        assert_eq!(
            outcome,
            ScopedWorkspaceCleanupOutcome::Removed { path: path.clone() }
        );
        assert!(!path.exists());
    }

    #[test]
    fn cleanup_scoped_workspace_preserves_active_workstream() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path =
            scoped_workspace_root(temp.path(), "engineer", "pr-for-code-7").expect("scoped path");
        std::fs::create_dir_all(&path).expect("workspace dir");

        let outcome = cleanup_scoped_workspace_sync(temp.path(), "engineer", "pr-for-code-7", true)
            .expect("cleanup");

        assert_eq!(
            outcome,
            ScopedWorkspaceCleanupOutcome::SkippedActive { path: path.clone() }
        );
        assert!(path.exists());
    }

    #[test]
    fn cleanup_scoped_workspace_skips_empty_correlation_key() {
        let temp = tempfile::tempdir().expect("tempdir");

        let outcome =
            cleanup_scoped_workspace_sync(temp.path(), "engineer", "  ", false).expect("cleanup");

        assert_eq!(
            outcome,
            ScopedWorkspaceCleanupOutcome::SkippedEmptyCorrelationKey
        );
    }
}
