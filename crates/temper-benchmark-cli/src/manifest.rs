// SPDX-License-Identifier: MPL-2.0

//! Benchmark manifest loading and path validation.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use temper_protocol_activity::CaptureModeV1;
use temper_protocol_agent::WorkspaceContext;

mod security;

use security::{InputKind, resolve_declared_path, validate_context_repositories};
pub(crate) use security::{validate_fixture_tree, validate_relative_path};

/// Schema identifier for an agent-session benchmark manifest.
pub const BENCHMARK_MANIFEST_SCHEMA: &str = "temper.benchmark.v1";

fn default_repetitions() -> u32 {
    1
}

/// The checked-in, provider-independent benchmark declaration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BenchmarkManifestV1 {
    pub schema: String,
    pub name: String,
    /// Directory whose contents become the fresh workspace for each repetition.
    pub fixture: PathBuf,
    /// JSON-encoded [`WorkspaceContext`] handed to the agent.
    #[serde(alias = "context")]
    pub workspace_context: PathBuf,
    pub capture: CaptureModeV1,
    /// Prefixes which identify successful agent validation commands in traces.
    #[serde(default)]
    pub validation_command_prefixes: Vec<Vec<String>>,
    /// Commands run by the benchmark host after the measured agent session.
    #[serde(default)]
    pub post_run_commands: Vec<Vec<String>>,
    /// Deterministic Jig provider script used by harness mode.
    pub jig_script: PathBuf,
    /// Number of repetitions used when the CLI does not supply an override.
    #[serde(default = "default_repetitions")]
    pub repetitions: u32,
    /// Explicit non-secret annotations useful when interpreting a run.
    #[serde(default)]
    pub annotations: BenchmarkAnnotationsV1,
}

/// Deliberately narrow metadata: credentials and arbitrary environment values
/// have no manifest field and therefore cannot be copied into snapshots.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BenchmarkAnnotationsV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_region: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_warmth: Option<String>,
}

/// A manifest after all declared inputs have been securely resolved.
#[derive(Clone, Debug)]
pub struct ResolvedBenchmarkManifest {
    manifest_path: PathBuf,
    manifest_root: PathBuf,
    fixture_dir: PathBuf,
    workspace_context_path: PathBuf,
    jig_script_path: PathBuf,
    source: String,
    manifest: BenchmarkManifestV1,
    workspace_context: WorkspaceContext,
}

impl ResolvedBenchmarkManifest {
    pub fn manifest(&self) -> &BenchmarkManifestV1 {
        &self.manifest
    }

    pub fn manifest_path(&self) -> &Path {
        &self.manifest_path
    }

    pub fn manifest_root(&self) -> &Path {
        &self.manifest_root
    }

    pub fn fixture_dir(&self) -> &Path {
        &self.fixture_dir
    }

    pub fn workspace_context_path(&self) -> &Path {
        &self.workspace_context_path
    }

    pub fn jig_script_path(&self) -> &Path {
        &self.jig_script_path
    }

    pub fn workspace_context(&self) -> &WorkspaceContext {
        &self.workspace_context
    }

    pub(crate) fn source(&self) -> &str {
        &self.source
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BenchmarkManifestError {
    #[error("cannot {operation} `{path}`: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("benchmark manifest `{path}` is not valid TOML: {source}")]
    Toml {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("benchmark workspace context `{path}` is not valid JSON: {source}")]
    Context {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("unsupported benchmark schema `{actual}`; expected `{BENCHMARK_MANIFEST_SCHEMA}`")]
    UnsupportedSchema { actual: String },
    #[error("invalid benchmark manifest: {0}")]
    Invalid(String),
    #[error("invalid `{field}` path `{path}`: {reason}")]
    Path {
        field: &'static str,
        path: PathBuf,
        reason: String,
    },
    #[error("unsafe fixture entry `{path}`: {reason}")]
    UnsafeFixture { path: PathBuf, reason: String },
    #[error("fixture directory cycle at `{path}`")]
    DirectoryCycle { path: PathBuf },
}

/// Loads a benchmark manifest and validates all of its filesystem inputs.
///
/// Declared paths are interpreted only relative to the directory containing the
/// manifest. Absolute paths, parent traversal, missing inputs, symlinked path
/// components, fixture symlinks, special files, and directory cycles fail
/// before a workspace is created.
pub fn load_benchmark_manifest(
    path: impl AsRef<Path>,
) -> Result<ResolvedBenchmarkManifest, BenchmarkManifestError> {
    let requested_path = path.as_ref();
    let metadata =
        fs::symlink_metadata(requested_path).map_err(|source| BenchmarkManifestError::Io {
            operation: "inspect manifest",
            path: requested_path.to_path_buf(),
            source,
        })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(BenchmarkManifestError::Path {
            field: "manifest",
            path: requested_path.to_path_buf(),
            reason: "must be a regular file, not a link".to_string(),
        });
    }
    let manifest_path =
        fs::canonicalize(requested_path).map_err(|source| BenchmarkManifestError::Io {
            operation: "resolve manifest",
            path: requested_path.to_path_buf(),
            source,
        })?;
    let manifest_root = manifest_path
        .parent()
        .ok_or_else(|| BenchmarkManifestError::Invalid("manifest has no parent directory".into()))?
        .to_path_buf();
    let source =
        fs::read_to_string(&manifest_path).map_err(|source| BenchmarkManifestError::Io {
            operation: "read manifest",
            path: manifest_path.clone(),
            source,
        })?;
    let manifest = toml::from_str::<BenchmarkManifestV1>(&source).map_err(|source| {
        BenchmarkManifestError::Toml {
            path: manifest_path.clone(),
            source,
        }
    })?;
    validate_manifest_values(&manifest)?;

    let fixture_dir = resolve_declared_path(
        &manifest_root,
        "fixture",
        &manifest.fixture,
        InputKind::Directory,
    )?;
    let workspace_context_path = resolve_declared_path(
        &manifest_root,
        "workspace_context",
        &manifest.workspace_context,
        InputKind::File,
    )?;
    let jig_script_path = resolve_declared_path(
        &manifest_root,
        "jig_script",
        &manifest.jig_script,
        InputKind::File,
    )?;

    validate_fixture_tree(&fixture_dir)?;
    let context_source = fs::read_to_string(&workspace_context_path).map_err(|source| {
        BenchmarkManifestError::Io {
            operation: "read workspace context",
            path: workspace_context_path.clone(),
            source,
        }
    })?;
    let workspace_context =
        serde_json::from_str::<WorkspaceContext>(&context_source).map_err(|source| {
            BenchmarkManifestError::Context {
                path: workspace_context_path.clone(),
                source,
            }
        })?;
    validate_context_repositories(&fixture_dir, &workspace_context)?;

    Ok(ResolvedBenchmarkManifest {
        manifest_path,
        manifest_root,
        fixture_dir,
        workspace_context_path,
        jig_script_path,
        source,
        manifest,
        workspace_context,
    })
}

fn validate_manifest_values(manifest: &BenchmarkManifestV1) -> Result<(), BenchmarkManifestError> {
    if manifest.schema != BENCHMARK_MANIFEST_SCHEMA {
        return Err(BenchmarkManifestError::UnsupportedSchema {
            actual: manifest.schema.clone(),
        });
    }
    if manifest.name.trim().is_empty() {
        return Err(BenchmarkManifestError::Invalid(
            "`name` must not be empty".to_string(),
        ));
    }
    if manifest.repetitions == 0 {
        return Err(BenchmarkManifestError::Invalid(
            "`repetitions` must be at least one".to_string(),
        ));
    }
    validate_argv_lists(
        "validation_command_prefixes",
        &manifest.validation_command_prefixes,
    )?;
    validate_argv_lists("post_run_commands", &manifest.post_run_commands)?;
    validate_annotation(
        "provider_region",
        manifest.annotations.provider_region.as_deref(),
    )?;
    validate_annotation("cache_warmth", manifest.annotations.cache_warmth.as_deref())
}

fn validate_argv_lists(
    field: &str,
    commands: &[Vec<String>],
) -> Result<(), BenchmarkManifestError> {
    for (index, argv) in commands.iter().enumerate() {
        if argv.is_empty() || argv[0].trim().is_empty() {
            return Err(BenchmarkManifestError::Invalid(format!(
                "`{field}[{index}]` must contain a non-empty executable"
            )));
        }
        if argv.iter().any(|argument| argument.contains('\0')) {
            return Err(BenchmarkManifestError::Invalid(format!(
                "`{field}[{index}]` contains a NUL byte"
            )));
        }
    }
    Ok(())
}

fn validate_annotation(field: &str, value: Option<&str>) -> Result<(), BenchmarkManifestError> {
    if value.is_some_and(|value| value.trim().is_empty()) {
        return Err(BenchmarkManifestError::Invalid(format!(
            "annotation `{field}` must not be empty"
        )));
    }
    Ok(())
}
