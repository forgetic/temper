// SPDX-License-Identifier: MPL-2.0

//! Stable benchmark artifact paths and input snapshots.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::{PreparedBenchmarkWorkspace, ResolvedBenchmarkManifest};

/// Current schema version for a retained repository-baseline snapshot.
pub const BASELINE_SNAPSHOT_VERSION: u32 = 1;

/// Top-level paths shared by all repetitions in one invocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BenchmarkArtifactLayout {
    root: PathBuf,
    repetitions_dir: PathBuf,
    repetitions: u32,
    pub aggregate_json: PathBuf,
    pub aggregate_markdown: PathBuf,
}

/// Complete deterministic path set for one repetition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepetitionArtifactPaths {
    pub root: PathBuf,
    pub manifest_snapshot: PathBuf,
    pub workspace_context_snapshot: PathBuf,
    pub baselines: PathBuf,
    pub canonical_trace: PathBuf,
    pub workspace_result: PathBuf,
    pub run_json: PathBuf,
    pub run_markdown: PathBuf,
    pub validation_evidence: PathBuf,
    pub diff_statistics: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum ArtifactLayoutError {
    #[error("artifact layout requires at least one repetition")]
    NoRepetitions,
    #[error("repetition {requested} is outside 1..={available}")]
    RepetitionOutOfRange { requested: u32, available: u32 },
    #[error("cannot {operation} `{path}`: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("unsafe artifact path `{path}`: {reason}")]
    UnsafePath { path: PathBuf, reason: String },
    #[error("cannot serialize `{artifact}` snapshot: {source}")]
    Json {
        artifact: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error(
        "prepared workspace repetition {prepared} does not match artifact repetition {requested}"
    )]
    RepetitionMismatch { prepared: u32, requested: u32 },
}

#[derive(Serialize)]
struct BaselineSnapshot<'a> {
    version: u32,
    repetition: u32,
    repositories: &'a [crate::RepositoryBaselineV1],
}

impl BenchmarkArtifactLayout {
    /// Creates the fixed output tree. Existing symlinks in layout-owned path
    /// components are rejected rather than followed.
    pub fn create(
        output_dir: impl AsRef<Path>,
        repetitions: u32,
    ) -> Result<Self, ArtifactLayoutError> {
        if repetitions == 0 {
            return Err(ArtifactLayoutError::NoRepetitions);
        }
        let requested = output_dir.as_ref();
        reject_existing_link(requested)?;
        fs::create_dir_all(requested).map_err(|source| ArtifactLayoutError::Io {
            operation: "create artifact root",
            path: requested.to_path_buf(),
            source,
        })?;
        let root = fs::canonicalize(requested).map_err(|source| ArtifactLayoutError::Io {
            operation: "resolve artifact root",
            path: requested.to_path_buf(),
            source,
        })?;
        let repetitions_dir = root.join("repetitions");
        create_owned_directory(&repetitions_dir)?;
        Ok(Self {
            aggregate_json: root.join("aggregate.json"),
            aggregate_markdown: root.join("aggregate.md"),
            root,
            repetitions_dir,
            repetitions,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn repetitions(&self) -> u32 {
        self.repetitions
    }

    /// Returns paths without creating the repetition directory.
    pub fn repetition(
        &self,
        repetition: u32,
    ) -> Result<RepetitionArtifactPaths, ArtifactLayoutError> {
        if repetition == 0 || repetition > self.repetitions {
            return Err(ArtifactLayoutError::RepetitionOutOfRange {
                requested: repetition,
                available: self.repetitions,
            });
        }
        let root = self.repetitions_dir.join(format!("{repetition:03}"));
        Ok(RepetitionArtifactPaths {
            manifest_snapshot: root.join("manifest.toml"),
            workspace_context_snapshot: root.join("workspace-context.json"),
            baselines: root.join("baselines.json"),
            canonical_trace: root.join("trace.export.jsonl"),
            workspace_result: root.join("workspace-result.json"),
            run_json: root.join("run.json"),
            run_markdown: root.join("run.md"),
            validation_evidence: root.join("validation.json"),
            diff_statistics: root.join("diff.json"),
            root,
        })
    }

    /// Creates one repetition directory and writes exactly the manifest/context
    /// used for the run plus the deterministic baseline SHAs. No environment or
    /// credential source is consulted while producing these snapshots.
    pub fn snapshot_inputs(
        &self,
        repetition: u32,
        manifest: &ResolvedBenchmarkManifest,
        workspace: &PreparedBenchmarkWorkspace,
    ) -> Result<RepetitionArtifactPaths, ArtifactLayoutError> {
        if workspace.repetition() != repetition {
            return Err(ArtifactLayoutError::RepetitionMismatch {
                prepared: workspace.repetition(),
                requested: repetition,
            });
        }
        let paths = self.repetition(repetition)?;
        create_owned_directory(&paths.root)?;

        write_owned_file(
            &paths.manifest_snapshot,
            manifest.source().as_bytes(),
            "write manifest snapshot",
        )?;
        let context = pretty_json(workspace.context(), "workspace context")?;
        write_owned_file(
            &paths.workspace_context_snapshot,
            &context,
            "write workspace context snapshot",
        )?;
        let baselines = pretty_json(
            &BaselineSnapshot {
                version: BASELINE_SNAPSHOT_VERSION,
                repetition,
                repositories: workspace.baselines(),
            },
            "repository baselines",
        )?;
        write_owned_file(
            &paths.baselines,
            &baselines,
            "write repository baseline snapshot",
        )?;
        Ok(paths)
    }
}

fn pretty_json<T: Serialize>(
    value: &T,
    artifact: &'static str,
) -> Result<Vec<u8>, ArtifactLayoutError> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|source| ArtifactLayoutError::Json { artifact, source })?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn create_owned_directory(path: &Path) -> Result<(), ArtifactLayoutError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(ArtifactLayoutError::UnsafePath {
                path: path.to_path_buf(),
                reason: "layout component is a symlink".to_string(),
            });
        }
        Ok(metadata) if !metadata.is_dir() => {
            return Err(ArtifactLayoutError::UnsafePath {
                path: path.to_path_buf(),
                reason: "layout component is not a directory".to_string(),
            });
        }
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(ArtifactLayoutError::Io {
                operation: "inspect artifact directory",
                path: path.to_path_buf(),
                source,
            });
        }
    }
    fs::create_dir(path).map_err(|source| ArtifactLayoutError::Io {
        operation: "create artifact directory",
        path: path.to_path_buf(),
        source,
    })
}

fn reject_existing_link(path: &Path) -> Result<(), ArtifactLayoutError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(ArtifactLayoutError::UnsafePath {
            path: path.to_path_buf(),
            reason: "artifact root is a symlink".to_string(),
        }),
        Ok(metadata) if !metadata.is_dir() => Err(ArtifactLayoutError::UnsafePath {
            path: path.to_path_buf(),
            reason: "artifact root is not a directory".to_string(),
        }),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(ArtifactLayoutError::Io {
            operation: "inspect artifact root",
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn write_owned_file(
    path: &Path,
    contents: &[u8],
    operation: &'static str,
) -> Result<(), ArtifactLayoutError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(ArtifactLayoutError::UnsafePath {
                path: path.to_path_buf(),
                reason: "artifact file is not a regular file".to_string(),
            });
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(ArtifactLayoutError::Io {
                operation: "inspect artifact file",
                path: path.to_path_buf(),
                source,
            });
        }
    }
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)
        .map_err(|source| ArtifactLayoutError::Io {
            operation,
            path: path.to_path_buf(),
            source,
        })?;
    file.write_all(contents)
        .map_err(|source| ArtifactLayoutError::Io {
            operation,
            path: path.to_path_buf(),
            source,
        })
}
