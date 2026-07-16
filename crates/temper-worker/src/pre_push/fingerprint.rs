use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Output};

use crate::executor::JobCancellation;
use crate::managed_effect::ManagedCommand;

use serde::{Deserialize, Serialize};
use temper_protocol_agent::WorkspaceContext;

/// Host-owned snapshot of every writable checkout's push-relevant git state.
///
/// The representation is intentionally worker-side only: agents never supply or
/// edit it. Equality is the contract the final push path needs — if HEAD,
/// staged/unstaged tracked changes, or untracked file contents differ from the
/// accepted `submit_for_pr` moment, the proof is stale.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceFingerprint {
    pub repos: Vec<RepoFingerprint>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RepoFingerprint {
    pub repo: String,
    pub dir: String,
    pub head: Vec<u8>,
    pub status_porcelain_z: Vec<u8>,
    pub staged_diff_binary: Vec<u8>,
    pub unstaged_diff_binary: Vec<u8>,
    pub untracked_files: Vec<UntrackedFileFingerprint>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UntrackedFileFingerprint {
    pub path: String,
    pub git_object_id: String,
}

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceFingerprintError {
    #[error("run git fingerprint command `{command}` in `{repo}`: {source}")]
    Io {
        repo: PathBuf,
        command: String,
        #[source]
        source: io::Error,
    },
    #[error(
        "git fingerprint command `{command}` failed in `{repo}` with status {status}: {stderr}"
    )]
    Git {
        repo: PathBuf,
        command: String,
        status: String,
        stderr: String,
    },
    #[error("workspace fingerprint cancelled by the worker watchdog")]
    Cancelled,
    #[error("git reported a non-utf8 untracked path in `{repo}`")]
    NonUtf8Path { repo: PathBuf },
}

pub async fn fingerprint_writable_repos(
    context: &WorkspaceContext,
    workspace_root: impl AsRef<Path>,
) -> Result<WorkspaceFingerprint, WorkspaceFingerprintError> {
    fingerprint_writable_repos_controlled(
        context,
        workspace_root.as_ref(),
        &JobCancellation::default(),
    )
    .await
}

pub(crate) async fn fingerprint_writable_repos_controlled(
    context: &WorkspaceContext,
    workspace_root: &Path,
    cancellation: &JobCancellation,
) -> Result<WorkspaceFingerprint, WorkspaceFingerprintError> {
    let mut repos = Vec::new();
    for repo in context.repos.iter().filter(|repo| repo.is_writable()) {
        repos.push(
            fingerprint_repo(
                repo.id.clone(),
                repo.dir.clone(),
                workspace_root.join(&repo.dir),
                cancellation,
            )
            .await?,
        );
    }
    Ok(WorkspaceFingerprint { repos })
}

pub fn fingerprint_writable_repos_blocking(
    context: &WorkspaceContext,
    workspace_root: &Path,
) -> Result<WorkspaceFingerprint, WorkspaceFingerprintError> {
    let context = context.clone();
    let workspace_root = workspace_root.to_path_buf();
    temper_worker_io::block_on(
        async move { fingerprint_writable_repos(&context, workspace_root).await },
    )
}

async fn fingerprint_repo(
    repo: String,
    dir: String,
    repo_root: PathBuf,
    cancellation: &JobCancellation,
) -> Result<RepoFingerprint, WorkspaceFingerprintError> {
    let head = git_stdout(&repo_root, &["rev-parse", "HEAD"], cancellation).await?;
    let status_porcelain_z = git_stdout(
        &repo_root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        cancellation,
    )
    .await?;
    let staged_diff_binary = git_stdout(
        &repo_root,
        &["diff", "--cached", "--binary", "--no-ext-diff", "--"],
        cancellation,
    )
    .await?;
    let unstaged_diff_binary = git_stdout(
        &repo_root,
        &["diff", "--binary", "--no-ext-diff", "--"],
        cancellation,
    )
    .await?;
    let untracked_files = untracked_files(&repo_root, cancellation).await?;

    Ok(RepoFingerprint {
        repo,
        dir,
        head,
        status_porcelain_z,
        staged_diff_binary,
        unstaged_diff_binary,
        untracked_files,
    })
}

async fn untracked_files(
    repo_root: &Path,
    cancellation: &JobCancellation,
) -> Result<Vec<UntrackedFileFingerprint>, WorkspaceFingerprintError> {
    let output = git_stdout(
        repo_root,
        &["ls-files", "--others", "--exclude-standard", "-z"],
        cancellation,
    )
    .await?;
    let mut paths = split_nul(&output)
        .into_iter()
        .map(|bytes| {
            String::from_utf8(bytes.to_vec()).map_err(|_| WorkspaceFingerprintError::NonUtf8Path {
                repo: repo_root.to_path_buf(),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort();

    let mut fingerprints = Vec::new();
    for path in paths {
        let object_id = git_stdout(
            repo_root,
            &["hash-object", "--no-filters", "--", &path],
            cancellation,
        )
        .await?;
        fingerprints.push(UntrackedFileFingerprint {
            path,
            git_object_id: trim_ascii_newline(object_id),
        });
    }
    Ok(fingerprints)
}

fn split_nul(bytes: &[u8]) -> Vec<&[u8]> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .collect()
}

fn trim_ascii_newline(mut bytes: Vec<u8>) -> String {
    while matches!(bytes.last(), Some(b'\n' | b'\r')) {
        bytes.pop();
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

async fn git_stdout(
    repo: &Path,
    args: &[&str],
    cancellation: &JobCancellation,
) -> Result<Vec<u8>, WorkspaceFingerprintError> {
    let output = run_git(repo, args, cancellation).await?;
    Ok(output.stdout)
}

async fn run_git(
    repo: &Path,
    args: &[&str],
    cancellation: &JobCancellation,
) -> Result<Output, WorkspaceFingerprintError> {
    let repo = repo.to_path_buf();
    let args = args
        .iter()
        .map(|arg| (*arg).to_string())
        .collect::<Vec<_>>();
    let command = format!("git -C {} {}", repo.display(), args.join(" "));
    let mut git = Command::new("git");
    git.env("GIT_TERMINAL_PROMPT", "0")
        .arg("-C")
        .arg(&repo)
        .args(&args);
    let output = cancellation
        .run(ManagedCommand::spawn(git))
        .await
        .ok_or(WorkspaceFingerprintError::Cancelled)?
        .map_err(|source| WorkspaceFingerprintError::Io {
            repo: repo.clone(),
            command: command.clone(),
            source,
        })?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(WorkspaceFingerprintError::Git {
            repo,
            command,
            status: status_string(output.status),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

fn status_string(status: ExitStatus) -> String {
    status
        .code()
        .map_or_else(|| status.to_string(), |code| code.to_string())
}
