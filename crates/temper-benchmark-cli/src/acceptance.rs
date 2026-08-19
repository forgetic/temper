// SPDX-License-Identifier: MPL-2.0

//! Fail-closed acceptance verification for exact-head benchmark evidence.
//!
//! Unlike the report-only comparison path, this module evaluates one
//! manifest-declared smoke and frozen condition matrix. It consumes only typed
//! runner artifacts and emits a content-free gate summary.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    BenchmarkConditionV1, BenchmarkModeV1, GraphConsumptionModeV1, GraphDecisionKindV1,
    ResolvedBenchmarkManifest, load_benchmark_manifest,
};

mod evaluation;
mod input;

use evaluation::*;
use input::load_trial_set;

pub const BENCHMARK_ACCEPTANCE_VERSION: u32 = 1;
pub const BENCHMARK_ACCEPTANCE_FILE: &str = "acceptance.json";

/// Manifest-owned policy for a smoke plus a three-condition matrix.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BenchmarkAcceptancePolicyV1 {
    pub mode: BenchmarkModeV1,
    pub smoke_repetitions: u32,
    pub matrix_repetitions: u32,
    pub provider: String,
    pub model: String,
    pub minimum_relevance_percent: u8,
    pub minimum_improvement_percent: u8,
    pub improvement_measure: AcceptanceImprovementMeasureV1,
    pub exact_source_selection_target: String,
    pub required_decision_kinds: Vec<GraphDecisionKindV1>,
    pub required_consumption_modes: Vec<GraphConsumptionModeV1>,
    /// Literal fragments which must not occur in generated durable artifacts.
    /// The snapshotted manifest itself is an input and is excluded from this
    /// output scan so that declarations do not match themselves.
    pub privacy_forbidden_fragments: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcceptanceImprovementMeasureV1 {
    ConventionalDiscoveryCalls,
    WallTimeMs,
    InputTokens,
    OutputTokens,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BenchmarkAcceptanceOptions {
    pub benchmark: PathBuf,
    pub candidate_commit: String,
    pub smoke: PathBuf,
    pub enabled: PathBuf,
    pub disabled: PathBuf,
    pub unavailable: PathBuf,
    pub output_dir: PathBuf,
}

/// Privacy-safe result. Gate names and aggregate integers are a closed schema;
/// no input diagnostics, commands, paths, targets, or trace content are copied.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "BenchmarkAcceptanceWireV1")]
pub struct BenchmarkAcceptanceV1 {
    pub version: u32,
    pub benchmark: String,
    pub candidate_commit: String,
    pub passed: bool,
    pub gates: Vec<AcceptanceGateResultV1>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BenchmarkAcceptanceWireV1 {
    version: u32,
    benchmark: String,
    candidate_commit: String,
    passed: bool,
    gates: Vec<AcceptanceGateResultV1>,
}

impl TryFrom<BenchmarkAcceptanceWireV1> for BenchmarkAcceptanceV1 {
    type Error = String;

    fn try_from(value: BenchmarkAcceptanceWireV1) -> Result<Self, Self::Error> {
        if value.version != BENCHMARK_ACCEPTANCE_VERSION {
            return Err(format!(
                "unsupported benchmark acceptance version {}; expected {BENCHMARK_ACCEPTANCE_VERSION}",
                value.version
            ));
        }
        if value.benchmark.trim().is_empty()
            || value.candidate_commit.len() != 40
            || !value
                .candidate_commit
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("benchmark acceptance identity is malformed".to_string());
        }
        let expected = [
            AcceptanceGateV1::ArtifactIntegrity,
            AcceptanceGateV1::ExecutionIdentity,
            AcceptanceGateV1::UniqueTrials,
            AcceptanceGateV1::EnabledTaskCorrectness,
            AcceptanceGateV1::EnabledHostValidation,
            AcceptanceGateV1::EnabledExactPatch,
            AcceptanceGateV1::EnabledDecisionEvidence,
            AcceptanceGateV1::SmokeRelevance,
            AcceptanceGateV1::MatrixAggregateRelevance,
            AcceptanceGateV1::UnavailableRetry,
            AcceptanceGateV1::ControlClassification,
            AcceptanceGateV1::Improvement,
            AcceptanceGateV1::DurablePrivacy,
        ];
        if value.gates.len() != expected.len()
            || expected.iter().any(|expected| {
                value
                    .gates
                    .iter()
                    .filter(|gate| gate.gate == *expected)
                    .count()
                    != 1
            })
            || value.passed != value.gates.iter().all(|gate| gate.passed)
        {
            return Err("benchmark acceptance gates are incomplete or inconsistent".to_string());
        }
        Ok(Self {
            version: value.version,
            benchmark: value.benchmark,
            candidate_commit: value.candidate_commit,
            passed: value.passed,
            gates: value.gates,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptanceGateResultV1 {
    pub gate: AcceptanceGateV1,
    pub passed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation: Option<AcceptanceObservationV1>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcceptanceGateV1 {
    ArtifactIntegrity,
    ExecutionIdentity,
    UniqueTrials,
    EnabledTaskCorrectness,
    EnabledHostValidation,
    EnabledExactPatch,
    EnabledDecisionEvidence,
    SmokeRelevance,
    MatrixAggregateRelevance,
    UnavailableRetry,
    ControlClassification,
    Improvement,
    DurablePrivacy,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptanceObservationV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub numerator: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub denominator: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_percent: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled_median: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled_median: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum AcceptanceError {
    #[error(transparent)]
    Manifest(#[from] crate::BenchmarkManifestError),
    #[error("benchmark manifest does not declare an acceptance policy")]
    MissingPolicy,
    #[error("invalid benchmark acceptance configuration: {0}")]
    Invalid(String),
    #[error("cannot inspect benchmark acceptance artifact `{path}`: {source}")]
    Inspect {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("unsafe benchmark acceptance artifact path `{0}`")]
    UnsafePath(PathBuf),
    #[error("cannot read benchmark acceptance artifact `{path}`: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("benchmark acceptance artifact `{path}` is malformed JSON: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("cannot create benchmark acceptance output `{path}`: {source}")]
    CreateOutput {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot serialize benchmark acceptance: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("cannot write benchmark acceptance output `{path}`: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Verifies a typed smoke and frozen matrix. A failed gate is a successful
/// evaluation represented by `passed: false`; malformed or unsafe inputs are
/// errors and can never become passing evidence.
pub fn verify_benchmark_acceptance(
    options: &BenchmarkAcceptanceOptions,
) -> Result<BenchmarkAcceptanceV1, AcceptanceError> {
    let manifest = load_benchmark_manifest(&options.benchmark)?;
    let policy = manifest
        .manifest()
        .acceptance
        .as_ref()
        .ok_or(AcceptanceError::MissingPolicy)?;
    validate_policy(&manifest, policy, &options.candidate_commit)?;

    let smoke = load_trial_set(&options.smoke, &manifest, policy)?;
    let enabled = load_trial_set(&options.enabled, &manifest, policy)?;
    let disabled = load_trial_set(&options.disabled, &manifest, policy)?;
    let unavailable = load_trial_set(&options.unavailable, &manifest, policy)?;
    let sets = [&smoke, &enabled, &disabled, &unavailable];

    let artifact_integrity = sets.iter().all(|set| set.artifact_integrity)
        && sets
            .iter()
            .skip(1)
            .all(|set| set.baseline_snapshot == smoke.baseline_snapshot);
    let execution_identity = identity_matches(
        &smoke,
        &manifest,
        policy,
        &options.candidate_commit,
        BenchmarkConditionV1::CodebaseMemoryEnabled,
        policy.smoke_repetitions,
    ) && identity_matches(
        &enabled,
        &manifest,
        policy,
        &options.candidate_commit,
        BenchmarkConditionV1::CodebaseMemoryEnabled,
        policy.matrix_repetitions,
    ) && identity_matches(
        &disabled,
        &manifest,
        policy,
        &options.candidate_commit,
        BenchmarkConditionV1::CodebaseMemoryDisabled,
        policy.matrix_repetitions,
    ) && identity_matches(
        &unavailable,
        &manifest,
        policy,
        &options.candidate_commit,
        BenchmarkConditionV1::CodebaseMemoryUnavailable,
        policy.matrix_repetitions,
    );
    let unique_trials = unique_trials(&sets);

    let enabled_pairs = smoke.enabled_pairs().chain(enabled.enabled_pairs());
    let enabled_pairs = enabled_pairs.collect::<Vec<_>>();
    let enabled_task_correctness = enabled_pairs
        .iter()
        .all(|(summary, _)| task_correct(summary));
    let enabled_host_validation = enabled_pairs
        .iter()
        .all(|(summary, validation)| host_validation_passed(summary, validation));
    let enabled_exact_patch = enabled_pairs
        .iter()
        .all(|(_, validation)| exact_patch_passed(validation));
    let enabled_decision_evidence = enabled_pairs
        .iter()
        .all(|(summary, _)| decision_evidence_complete(summary, policy));

    let smoke_relevance = summed_relevance(&smoke);
    let matrix_relevance = summed_relevance(&enabled);
    let smoke_relevance_passed = relevance_passes(smoke_relevance, policy);
    let matrix_relevance_passed = relevance_passes(matrix_relevance, policy);
    let unavailable_retry = unavailable.summaries().all(unavailable_retry_passed);
    let control_classification = enabled
        .summaries()
        .chain(disabled.summaries())
        .all(classification_complete);
    let improvement = improvement_observation(&enabled, &disabled, policy);
    let improvement_passed = improvement.as_ref().is_some_and(|observation| {
        let (Some(enabled), Some(disabled)) =
            (observation.enabled_median, observation.disabled_median)
        else {
            return false;
        };
        disabled > 0
            && u128::from(enabled) * 100
                <= u128::from(disabled)
                    * u128::from(100_u8.saturating_sub(policy.minimum_improvement_percent))
    });
    let durable_privacy = sets.iter().all(|set| set.privacy_safe);

    let gates = vec![
        gate(AcceptanceGateV1::ArtifactIntegrity, artifact_integrity),
        gate(AcceptanceGateV1::ExecutionIdentity, execution_identity),
        gate(AcceptanceGateV1::UniqueTrials, unique_trials),
        gate(
            AcceptanceGateV1::EnabledTaskCorrectness,
            enabled_task_correctness,
        ),
        gate(
            AcceptanceGateV1::EnabledHostValidation,
            enabled_host_validation,
        ),
        gate(AcceptanceGateV1::EnabledExactPatch, enabled_exact_patch),
        gate(
            AcceptanceGateV1::EnabledDecisionEvidence,
            enabled_decision_evidence,
        ),
        relevance_gate(
            AcceptanceGateV1::SmokeRelevance,
            smoke_relevance_passed,
            smoke_relevance,
            policy.minimum_relevance_percent,
        ),
        relevance_gate(
            AcceptanceGateV1::MatrixAggregateRelevance,
            matrix_relevance_passed,
            matrix_relevance,
            policy.minimum_relevance_percent,
        ),
        gate(AcceptanceGateV1::UnavailableRetry, unavailable_retry),
        gate(
            AcceptanceGateV1::ControlClassification,
            control_classification,
        ),
        AcceptanceGateResultV1 {
            gate: AcceptanceGateV1::Improvement,
            passed: improvement_passed,
            observation: improvement,
        },
        gate(AcceptanceGateV1::DurablePrivacy, durable_privacy),
    ];
    let passed = gates.iter().all(|gate| gate.passed);
    Ok(BenchmarkAcceptanceV1 {
        version: BENCHMARK_ACCEPTANCE_VERSION,
        benchmark: manifest.manifest().name.clone(),
        candidate_commit: options.candidate_commit.to_ascii_lowercase(),
        passed,
        gates,
    })
}

pub fn write_benchmark_acceptance(
    acceptance: &BenchmarkAcceptanceV1,
    output_dir: impl AsRef<Path>,
) -> Result<(), AcceptanceError> {
    let output_dir = output_dir.as_ref();
    match fs::symlink_metadata(output_dir) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(AcceptanceError::UnsafePath(output_dir.to_path_buf()));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(output_dir).map_err(|source| AcceptanceError::CreateOutput {
                path: output_dir.to_path_buf(),
                source,
            })?;
        }
        Err(source) => {
            return Err(AcceptanceError::CreateOutput {
                path: output_dir.to_path_buf(),
                source,
            });
        }
    }
    let path = output_dir.join(BENCHMARK_ACCEPTANCE_FILE);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(AcceptanceError::UnsafePath(path));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(AcceptanceError::Write { path, source });
        }
    }
    let mut bytes = serde_json::to_vec_pretty(acceptance)?;
    bytes.push(b'\n');
    fs::write(&path, bytes).map_err(|source| AcceptanceError::Write { path, source })
}

fn validate_policy(
    manifest: &ResolvedBenchmarkManifest,
    policy: &BenchmarkAcceptancePolicyV1,
    candidate_commit: &str,
) -> Result<(), AcceptanceError> {
    if candidate_commit.len() != 40
        || !candidate_commit
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(AcceptanceError::Invalid(
            "candidate commit must be a full 40-character hexadecimal commit".to_string(),
        ));
    }
    if manifest.manifest().condition_profile.is_none() {
        return Err(AcceptanceError::Invalid(
            "acceptance requires a controlled condition profile".to_string(),
        ));
    }
    if manifest.expected_patch_path().is_none() {
        return Err(AcceptanceError::Invalid(
            "acceptance requires a checked-in expected patch".to_string(),
        ));
    }
    if policy.smoke_repetitions == 0 || policy.matrix_repetitions == 0 {
        return Err(AcceptanceError::Invalid(
            "acceptance repetition counts must be positive".to_string(),
        ));
    }
    if policy.provider.trim().is_empty() || policy.model.trim().is_empty() {
        return Err(AcceptanceError::Invalid(
            "acceptance provider and model must be non-empty".to_string(),
        ));
    }
    if !(1..=100).contains(&policy.minimum_relevance_percent)
        || !(1..=100).contains(&policy.minimum_improvement_percent)
    {
        return Err(AcceptanceError::Invalid(
            "acceptance percentages must be in 1..=100".to_string(),
        ));
    }
    if manifest.manifest().annotations.cache_warmth.is_none() {
        return Err(AcceptanceError::Invalid(
            "acceptance requires an explicit cache_warmth annotation".to_string(),
        ));
    }
    if policy.exact_source_selection_target.trim().is_empty()
        || !manifest
            .manifest()
            .graph_decision_targets
            .iter()
            .any(|target| target.target == policy.exact_source_selection_target)
    {
        return Err(AcceptanceError::Invalid(
            "exact source selection must name a declared decision target".to_string(),
        ));
    }
    for required in [
        GraphDecisionKindV1::Implementation,
        GraphDecisionKindV1::Caller,
        GraphDecisionKindV1::FocusedTest,
    ] {
        if !policy.required_decision_kinds.contains(&required) {
            return Err(AcceptanceError::Invalid(
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
            return Err(AcceptanceError::Invalid(
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
        return Err(AcceptanceError::Invalid(
            "privacy fragments must contain at least one value of eight or more bytes".to_string(),
        ));
    }
    Ok(())
}
