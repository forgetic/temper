// SPDX-License-Identifier: MPL-2.0

//! Benchmark manifest loading and path validation.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use temper_protocol_activity::{
    CaptureModeV1, GraphCorrelationTargetKindV1, GraphCorrelationToolV1, GraphCorrelationV1,
};
use temper_protocol_agent::WorkspaceContext;

mod security;

use security::{InputKind, resolve_declared_path, validate_context_repositories};
pub(crate) use security::{validate_fixture_tree, validate_relative_path};

use crate::{BenchmarkAcceptancePolicyV1, GraphConsumptionModeV1, GraphDecisionKindV1};

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
    /// Shell command prefixes explicitly classified as conventional discovery
    /// by this benchmark's decision-relevance rubric.
    #[serde(default)]
    pub discovery_command_prefixes: Vec<Vec<String>>,
    /// Expected decision targets used to classify graph results as consumed or
    /// irrelevant without treating RPC success as usefulness.
    #[serde(default)]
    pub graph_decision_targets: Vec<GraphDecisionTargetV1>,
    /// Commands run by the benchmark host after the measured agent session.
    #[serde(default)]
    pub post_run_commands: Vec<Vec<String>>,
    /// Deterministic Jig provider script used by harness mode.
    pub jig_script: PathBuf,
    /// Optional controlled runner profile. Profiled benchmarks require an
    /// explicit condition at execution time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition_profile: Option<BenchmarkConditionProfileV1>,
    /// Optional exact final patch used as a host-owned correctness gate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_patch: Option<PathBuf>,
    /// Number of repetitions used when the CLI does not supply an override.
    #[serde(default = "default_repetitions")]
    pub repetitions: u32,
    /// Explicit non-secret annotations useful when interpreting a run.
    #[serde(default)]
    pub annotations: BenchmarkAnnotationsV1,
    /// Optional fail-closed policy evaluated only by the acceptance verifier.
    /// Ordinary runs and report-only comparisons do not apply these gates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance: Option<BenchmarkAcceptancePolicyV1>,
}

/// Runner-owned settings for a controlled benchmark condition family.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BenchmarkConditionProfileV1 {
    pub kind: BenchmarkConditionProfileKindV1,
    /// Hermetic MCP provider used by harness runs and by the forced-unavailable
    /// live condition. The enabled live condition keeps the production profile.
    pub fixture_provider: PathBuf,
    /// Harness script that begins with conventional discovery because disabled
    /// runs do not expose a graph tool for the model to call.
    pub disabled_jig_script: PathBuf,
    /// Harness script that performs exactly one failing graph call before
    /// conventional fallback, so unavailable runs never model an immediate retry.
    pub unavailable_jig_script: PathBuf,
}

/// Condition families with runner-enforced availability changes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BenchmarkConditionProfileKindV1 {
    CodebaseMemory,
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

/// One fixture-owned target which a typed graph call may inform.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphDecisionTargetV1 {
    pub target: String,
    pub kind: GraphDecisionKindV1,
    /// The exact, wrapper-fingerprinted producer that may inform this target.
    pub producer: GraphDecisionCorrelationV1,
    /// Exact, fixture-owned graph/source consumers which may use this producer.
    /// Direct reads and mutations retain their existing exact target matching
    /// and therefore do not need an entry here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub consumption: Vec<GraphDecisionCorrelationV1>,
}

/// One exact provider-shaped target declared in a benchmark manifest.
///
/// The analyzer derives the same closed [`GraphCorrelationV1`] record as the
/// trusted wrapper and compares only that record. The raw manifest target is
/// never copied from a trace or rendered in decision evidence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphDecisionCorrelationV1 {
    pub tool: GraphCorrelationToolV1,
    pub target_kind: GraphCorrelationTargetKindV1,
    pub target: String,
}

impl GraphDecisionCorrelationV1 {
    pub(crate) fn correlation(&self) -> Option<GraphCorrelationV1> {
        GraphCorrelationV1::new(self.tool, self.target_kind, &self.target)
    }
}

/// A manifest after all declared inputs have been securely resolved.
#[derive(Clone, Debug)]
pub struct ResolvedBenchmarkManifest {
    manifest_path: PathBuf,
    manifest_root: PathBuf,
    fixture_dir: PathBuf,
    workspace_context_path: PathBuf,
    jig_script_path: PathBuf,
    condition_fixture_provider_path: Option<PathBuf>,
    condition_disabled_jig_script_path: Option<PathBuf>,
    condition_unavailable_jig_script_path: Option<PathBuf>,
    expected_patch_path: Option<PathBuf>,
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

    pub fn condition_fixture_provider_path(&self) -> Option<&Path> {
        self.condition_fixture_provider_path.as_deref()
    }

    pub fn condition_disabled_jig_script_path(&self) -> Option<&Path> {
        self.condition_disabled_jig_script_path.as_deref()
    }

    pub fn condition_unavailable_jig_script_path(&self) -> Option<&Path> {
        self.condition_unavailable_jig_script_path.as_deref()
    }

    pub fn expected_patch_path(&self) -> Option<&Path> {
        self.expected_patch_path.as_deref()
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
    let condition_fixture_provider_path = manifest
        .condition_profile
        .as_ref()
        .map(|profile| {
            resolve_declared_path(
                &manifest_root,
                "condition_profile.fixture_provider",
                &profile.fixture_provider,
                InputKind::File,
            )
        })
        .transpose()?;
    let condition_disabled_jig_script_path = manifest
        .condition_profile
        .as_ref()
        .map(|profile| {
            resolve_declared_path(
                &manifest_root,
                "condition_profile.disabled_jig_script",
                &profile.disabled_jig_script,
                InputKind::File,
            )
        })
        .transpose()?;
    let condition_unavailable_jig_script_path = manifest
        .condition_profile
        .as_ref()
        .map(|profile| {
            resolve_declared_path(
                &manifest_root,
                "condition_profile.unavailable_jig_script",
                &profile.unavailable_jig_script,
                InputKind::File,
            )
        })
        .transpose()?;
    let expected_patch_path = manifest
        .expected_patch
        .as_ref()
        .map(|path| resolve_declared_path(&manifest_root, "expected_patch", path, InputKind::File))
        .transpose()?;

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
        condition_fixture_provider_path,
        condition_disabled_jig_script_path,
        condition_unavailable_jig_script_path,
        expected_patch_path,
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
    validate_argv_lists(
        "discovery_command_prefixes",
        &manifest.discovery_command_prefixes,
    )?;
    validate_graph_targets(&manifest.graph_decision_targets)?;
    validate_argv_lists("post_run_commands", &manifest.post_run_commands)?;
    validate_annotation(
        "provider_region",
        manifest.annotations.provider_region.as_deref(),
    )?;
    validate_annotation("cache_warmth", manifest.annotations.cache_warmth.as_deref())?;
    validate_acceptance_policy(manifest)?;
    if manifest.condition_profile.is_some() {
        for kind in [
            GraphDecisionKindV1::Implementation,
            GraphDecisionKindV1::Caller,
            GraphDecisionKindV1::FocusedTest,
        ] {
            if !manifest
                .graph_decision_targets
                .iter()
                .any(|target| target.kind == kind)
            {
                return Err(BenchmarkManifestError::Invalid(format!(
                    "a condition profile requires a `{kind:?}` graph decision target"
                )));
            }
        }
    }
    Ok(())
}

fn validate_acceptance_policy(
    manifest: &BenchmarkManifestV1,
) -> Result<(), BenchmarkManifestError> {
    let Some(policy) = &manifest.acceptance else {
        return Ok(());
    };
    if manifest.condition_profile.is_none() || manifest.expected_patch.is_none() {
        return Err(BenchmarkManifestError::Invalid(
            "`acceptance` requires `condition_profile` and `expected_patch`".to_string(),
        ));
    }
    if manifest.annotations.cache_warmth.is_none() {
        return Err(BenchmarkManifestError::Invalid(
            "`acceptance` requires an explicit `annotations.cache_warmth`".to_string(),
        ));
    }
    if policy.smoke_repetitions == 0 || policy.matrix_repetitions == 0 {
        return Err(BenchmarkManifestError::Invalid(
            "acceptance repetition counts must be positive".to_string(),
        ));
    }
    if policy.provider.trim().is_empty()
        || policy.model.trim().is_empty()
        || !(1..=100).contains(&policy.minimum_relevance_percent)
        || !(1..=100).contains(&policy.minimum_improvement_percent)
    {
        return Err(BenchmarkManifestError::Invalid(
            "acceptance identity and percentages must be non-empty and valid".to_string(),
        ));
    }
    if !manifest
        .graph_decision_targets
        .iter()
        .any(|target| target.target == policy.exact_source_selection_target)
    {
        return Err(BenchmarkManifestError::Invalid(
            "acceptance exact source selection must name a declared target".to_string(),
        ));
    }
    for required in [
        GraphDecisionKindV1::Implementation,
        GraphDecisionKindV1::Caller,
        GraphDecisionKindV1::FocusedTest,
    ] {
        if !policy.required_decision_kinds.contains(&required) {
            return Err(BenchmarkManifestError::Invalid(
                "acceptance must require implementation, caller, and focused-test evidence"
                    .to_string(),
            ));
        }
    }
    for required in [
        GraphConsumptionModeV1::Source,
        GraphConsumptionModeV1::Selection,
    ] {
        if !policy.required_consumption_modes.contains(&required) {
            return Err(BenchmarkManifestError::Invalid(
                "acceptance must require typed source and exact selection consumption".to_string(),
            ));
        }
    }
    if policy.privacy_forbidden_fragments.is_empty()
        || policy
            .privacy_forbidden_fragments
            .iter()
            .any(|fragment| fragment.len() < 8)
    {
        return Err(BenchmarkManifestError::Invalid(
            "acceptance privacy fragments must each contain at least eight bytes".to_string(),
        ));
    }
    Ok(())
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

fn validate_graph_targets(targets: &[GraphDecisionTargetV1]) -> Result<(), BenchmarkManifestError> {
    for (index, target) in targets.iter().enumerate() {
        if target.target.trim().is_empty() || target.target.contains('\0') {
            return Err(BenchmarkManifestError::Invalid(format!(
                "`graph_decision_targets[{index}].target` must not be empty or contain a NUL byte"
            )));
        }
        validate_graph_correlation(
            &format!("graph_decision_targets[{index}].producer"),
            &target.producer,
        )?;
        for (consumption_index, consumption) in target.consumption.iter().enumerate() {
            validate_graph_correlation(
                &format!("graph_decision_targets[{index}].consumption[{consumption_index}]"),
                consumption,
            )?;
        }
    }
    Ok(())
}

fn validate_graph_correlation(
    field: &str,
    correlation: &GraphDecisionCorrelationV1,
) -> Result<(), BenchmarkManifestError> {
    if correlation.correlation().is_none() {
        return Err(BenchmarkManifestError::Invalid(format!(
            "`{field}` must declare one complete supported normalized graph correlation target"
        )));
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
