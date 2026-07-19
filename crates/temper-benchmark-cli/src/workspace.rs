// SPDX-License-Identifier: MPL-2.0

//! Fresh, isolated benchmark workspace preparation.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde::{Deserialize, Serialize};
use temper_protocol_agent::WorkspaceContext;
use tempfile::{Builder, TempDir};

use crate::manifest::{
    BenchmarkManifestError, ResolvedBenchmarkManifest, validate_fixture_tree,
    validate_relative_path,
};

const BASELINE_COMMIT_MESSAGE: &str = "chore: initialize benchmark fixture";
const BASELINE_GIT_DATE: &str = "2000-01-01T00:00:00 +0000";

/// A repository baseline retained for diff calculation after the agent run.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryBaselineV1 {
    pub id: String,
    pub dir: String,
    pub sha: String,
}

/// One repetition's temporary workspace. Dropping this value removes it.
pub struct PreparedBenchmarkWorkspace {
    temporary: TempDir,
    workspace_root: PathBuf,
    context: WorkspaceContext,
    baselines: Vec<RepositoryBaselineV1>,
    repetition: u32,
}

impl PreparedBenchmarkWorkspace {
    pub fn repetition(&self) -> u32 {
        self.repetition
    }

    pub fn root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn context(&self) -> &WorkspaceContext {
        &self.context
    }

    pub fn baselines(&self) -> &[RepositoryBaselineV1] {
        &self.baselines
    }

    /// The owner directory is exposed for process configuration which must stay
    /// outside the agent-visible workspace, such as an isolated Git HOME.
    pub fn temporary_root(&self) -> &Path {
        self.temporary.path()
    }

    /// Re-checks repository containment after an agent session. This detects a
    /// repository directory replaced by a symlink before any host command runs.
    pub fn verify_context_directories(&self) -> Result<(), WorkspacePreparationError> {
        verify_context_directories(&self.workspace_root, &self.context).map(drop)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WorkspacePreparationError {
    #[error(transparent)]
    Manifest(#[from] BenchmarkManifestError),
    #[error("invalid repetition {0}; repetitions are numbered from one")]
    InvalidRepetition(u32),
    #[error("cannot {operation} `{path}`: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("unsafe workspace repository `{id}` at `{path}`: {reason}")]
    UnsafeRepository {
        id: String,
        path: PathBuf,
        reason: String,
    },
    #[error("git command `{command}` failed in `{cwd}` ({status}): {stderr}")]
    Git {
        command: String,
        cwd: PathBuf,
        status: String,
        stderr: String,
    },
    #[error("git command `{command}` returned invalid UTF-8: {source}")]
    GitUtf8 {
        command: String,
        #[source]
        source: std::string::FromUtf8Error,
    },
}

/// Copies the fixture into a brand-new temporary directory and creates a
/// deterministic baseline commit in every context repository.
pub fn prepare_benchmark_workspace(
    manifest: &ResolvedBenchmarkManifest,
    repetition: u32,
) -> Result<PreparedBenchmarkWorkspace, WorkspacePreparationError> {
    if repetition == 0 {
        return Err(WorkspacePreparationError::InvalidRepetition(repetition));
    }

    // Validate again immediately before copying so a fixture changed after
    // manifest loading cannot smuggle a link or special file into the run.
    validate_fixture_tree(manifest.fixture_dir())?;
    let temporary = Builder::new()
        .prefix(&format!("temper-benchmark-{repetition:03}-"))
        .tempdir()
        .map_err(|source| WorkspacePreparationError::Io {
            operation: "create repetition directory",
            path: std::env::temp_dir(),
            source,
        })?;
    let workspace_root = temporary.path().join("workspace");
    fs::create_dir(&workspace_root).map_err(|source| WorkspacePreparationError::Io {
        operation: "create workspace",
        path: workspace_root.clone(),
        source,
    })?;
    copy_fixture_directory(manifest.fixture_dir(), &workspace_root)?;

    let context = manifest.workspace_context().clone();
    let repositories = verify_context_directories(&workspace_root, &context)?;
    let git_home = temporary.path().join("git-home");
    fs::create_dir(&git_home).map_err(|source| WorkspacePreparationError::Io {
        operation: "create isolated Git home",
        path: git_home.clone(),
        source,
    })?;

    let mut baselines = Vec::with_capacity(context.repos.len());
    for (repository, path) in context.repos.iter().zip(repositories) {
        let sha = initialize_baseline(&path, &repository.default_branch, &git_home)?;
        baselines.push(RepositoryBaselineV1 {
            id: repository.id.clone(),
            dir: repository.dir.clone(),
            sha,
        });
    }

    Ok(PreparedBenchmarkWorkspace {
        temporary,
        workspace_root,
        context,
        baselines,
        repetition,
    })
}

fn copy_fixture_directory(
    source: &Path,
    destination: &Path,
) -> Result<(), WorkspacePreparationError> {
    let mut entries = fs::read_dir(source)
        .map_err(|source_error| WorkspacePreparationError::Io {
            operation: "read fixture directory",
            path: source.to_path_buf(),
            source: source_error,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source_error| WorkspacePreparationError::Io {
            operation: "read fixture entry",
            path: source.to_path_buf(),
            source: source_error,
        })?;
    entries.sort_by_key(std::fs::DirEntry::file_name);

    for entry in entries {
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let metadata = fs::symlink_metadata(&source_path).map_err(|source_error| {
            WorkspacePreparationError::Io {
                operation: "inspect fixture entry",
                path: source_path.clone(),
                source: source_error,
            }
        })?;
        let file_type = metadata.file_type();
        if file_type.is_symlink() || (!file_type.is_dir() && !file_type.is_file()) {
            return Err(WorkspacePreparationError::Manifest(
                BenchmarkManifestError::UnsafeFixture {
                    path: source_path,
                    reason: "fixture changed after validation".to_string(),
                },
            ));
        }
        if file_type.is_dir() {
            fs::create_dir(&destination_path).map_err(|source_error| {
                WorkspacePreparationError::Io {
                    operation: "create fixture directory",
                    path: destination_path.clone(),
                    source: source_error,
                }
            })?;
            copy_fixture_directory(&source_path, &destination_path)?;
        } else {
            fs::copy(&source_path, &destination_path).map_err(|source_error| {
                WorkspacePreparationError::Io {
                    operation: "copy fixture file",
                    path: source_path.clone(),
                    source: source_error,
                }
            })?;
        }
    }
    Ok(())
}

fn verify_context_directories(
    workspace_root: &Path,
    context: &WorkspaceContext,
) -> Result<Vec<PathBuf>, WorkspacePreparationError> {
    let canonical_root =
        fs::canonicalize(workspace_root).map_err(|source| WorkspacePreparationError::Io {
            operation: "resolve workspace root",
            path: workspace_root.to_path_buf(),
            source,
        })?;
    let mut repositories = Vec::with_capacity(context.repos.len());
    for repository in &context.repos {
        let declared = Path::new(&repository.dir);
        validate_relative_path("workspace_context.repos[].dir", declared)?;
        let joined = canonical_root.join(declared);
        let metadata =
            fs::symlink_metadata(&joined).map_err(|source| WorkspacePreparationError::Io {
                operation: "inspect workspace repository",
                path: joined.clone(),
                source,
            })?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(WorkspacePreparationError::UnsafeRepository {
                id: repository.id.clone(),
                path: joined,
                reason: "must remain a real directory".to_string(),
            });
        }
        let canonical =
            fs::canonicalize(&joined).map_err(|source| WorkspacePreparationError::Io {
                operation: "resolve workspace repository",
                path: joined.clone(),
                source,
            })?;
        if !canonical.starts_with(&canonical_root) {
            return Err(WorkspacePreparationError::UnsafeRepository {
                id: repository.id.clone(),
                path: canonical,
                reason: "directory escapes the repetition workspace".to_string(),
            });
        }
        repositories.push((repository.id.as_str(), canonical));
    }
    for (index, (id, path)) in repositories.iter().enumerate() {
        for (other_id, other_path) in repositories.iter().skip(index + 1) {
            if path == other_path || path.starts_with(other_path) || other_path.starts_with(path) {
                return Err(WorkspacePreparationError::UnsafeRepository {
                    id: (*id).to_string(),
                    path: path.clone(),
                    reason: format!("overlaps context repository `{other_id}`"),
                });
            }
        }
    }
    Ok(repositories.into_iter().map(|(_, path)| path).collect())
}

fn initialize_baseline(
    repository: &Path,
    default_branch: &str,
    git_home: &Path,
) -> Result<String, WorkspacePreparationError> {
    run_git(
        repository,
        git_home,
        &[
            "init",
            "--quiet",
            &format!("--initial-branch={default_branch}"),
        ],
    )?;
    run_git(
        repository,
        git_home,
        &["config", "user.name", "Temper Benchmark"],
    )?;
    run_git(
        repository,
        git_home,
        &["config", "user.email", "benchmark@temper.invalid"],
    )?;
    run_git(repository, git_home, &["config", "commit.gpgSign", "false"])?;
    run_git(repository, git_home, &["config", "core.autocrlf", "false"])?;
    run_git(repository, git_home, &["add", "--all", "--", "."])?;
    run_git(
        repository,
        git_home,
        &[
            "commit",
            "--quiet",
            "--allow-empty",
            "--no-gpg-sign",
            "-m",
            BASELINE_COMMIT_MESSAGE,
        ],
    )?;
    let output = run_git(repository, git_home, &["rev-parse", "HEAD"])?;
    let sha =
        String::from_utf8(output.stdout).map_err(|source| WorkspacePreparationError::GitUtf8 {
            command: "git rev-parse HEAD".to_string(),
            source,
        })?;
    Ok(sha.trim().to_string())
}

fn run_git(
    cwd: &Path,
    git_home: &Path,
    arguments: &[&str],
) -> Result<Output, WorkspacePreparationError> {
    let command = format!("git {}", arguments.join(" "));
    let output = Command::new("git")
        .args(arguments)
        .current_dir(cwd)
        .env("HOME", git_home)
        .env("XDG_CONFIG_HOME", git_home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", git_home.join("global.gitconfig"))
        .env("GIT_AUTHOR_NAME", "Temper Benchmark")
        .env("GIT_AUTHOR_EMAIL", "benchmark@temper.invalid")
        .env("GIT_AUTHOR_DATE", BASELINE_GIT_DATE)
        .env("GIT_COMMITTER_NAME", "Temper Benchmark")
        .env("GIT_COMMITTER_EMAIL", "benchmark@temper.invalid")
        .env("GIT_COMMITTER_DATE", BASELINE_GIT_DATE)
        .output()
        .map_err(|source| WorkspacePreparationError::Io {
            operation: "run Git",
            path: cwd.to_path_buf(),
            source,
        })?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(WorkspacePreparationError::Git {
            command,
            cwd: cwd.to_path_buf(),
            status: output.status.to_string(),
            stderr: bounded_stderr(&output.stderr),
        })
    }
}

fn bounded_stderr(stderr: &[u8]) -> String {
    const LIMIT: usize = 4096;
    let start = stderr.len().saturating_sub(LIMIT);
    String::from_utf8_lossy(&stderr[start..]).trim().to_string()
}
