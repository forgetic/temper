// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::path::Path;

use serde::Deserialize;

use super::evaluation::validation_summary_matches;
use super::{AcceptanceError, BenchmarkAcceptancePolicyV1};
use crate::{
    BASELINE_SNAPSHOT_VERSION, BenchmarkAggregateV1, DIFF_ARTIFACT_VERSION, DiffArtifactV1,
    RepositoryBaselineV1, ResolvedBenchmarkManifest, RunSummaryV1, VALIDATION_ARTIFACT_VERSION,
    ValidationArtifactV1,
};

pub(super) struct TrialSet {
    pub(super) aggregate: BenchmarkAggregateV1,
    pub(super) validations: Vec<ValidationArtifactV1>,
    pub(super) artifact_integrity: bool,
    pub(super) privacy_safe: bool,
    pub(super) baseline_snapshot: Vec<RepositoryBaselineV1>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BaselineSnapshotV1 {
    version: u32,
    repetition: u32,
    repositories: Vec<RepositoryBaselineV1>,
}

impl TrialSet {
    pub(super) fn summaries(&self) -> impl Iterator<Item = &RunSummaryV1> {
        self.aggregate.runs.iter().map(|run| &run.summary)
    }

    pub(super) fn enabled_pairs(
        &self,
    ) -> impl Iterator<Item = (&RunSummaryV1, &ValidationArtifactV1)> {
        self.aggregate
            .runs
            .iter()
            .map(|run| &run.summary)
            .zip(&self.validations)
    }
}

pub(super) fn load_trial_set(
    root: &Path,
    manifest: &ResolvedBenchmarkManifest,
    policy: &BenchmarkAcceptancePolicyV1,
) -> Result<TrialSet, AcceptanceError> {
    require_directory(root)?;
    let aggregate = read_json::<BenchmarkAggregateV1>(&root.join("aggregate.json"))?;
    let _aggregate_markdown = read_bytes(&root.join("aggregate.md"))?;
    let expected_manifest = manifest.source().as_bytes();
    let expected_context = read_json_value(manifest.workspace_context_path())?;
    let expected_patch = manifest.expected_patch_path().map(read_bytes).transpose()?;
    let mut validations = Vec::with_capacity(aggregate.runs.len());
    let mut artifact_integrity = true;
    let mut baseline_snapshot = None;
    for run in &aggregate.runs {
        let repetition = root
            .join("repetitions")
            .join(format!("{:03}", run.repetition));
        require_directory(&repetition)?;
        let summary = read_json::<RunSummaryV1>(&repetition.join("run.json"))?;
        let _run_markdown = read_bytes(&repetition.join("run.md"))?;
        let _canonical_trace = read_bytes(&repetition.join("trace.export.jsonl"))?;
        artifact_integrity &= summary == run.summary;
        if let Some(expected_result) = &summary.workspace_result {
            let result = read_json::<temper_protocol_agent::WorkspaceResult>(
                &repetition.join("workspace-result.json"),
            )?;
            artifact_integrity &= &result == expected_result;
        }
        let diff = read_json::<DiffArtifactV1>(&repetition.join("diff.json"))?;
        artifact_integrity &= diff.version == DIFF_ARTIFACT_VERSION;
        artifact_integrity &= summary.diff.as_ref() == Some(&diff.statistics);
        artifact_integrity &= read_bytes(&repetition.join("manifest.toml"))? == expected_manifest;
        artifact_integrity &=
            read_json_value(&repetition.join("workspace-context.json"))? == expected_context;
        if let Some(expected_patch) = &expected_patch {
            artifact_integrity &=
                read_bytes(&repetition.join("expected.patch"))? == *expected_patch;
        }
        let baseline = read_json::<BaselineSnapshotV1>(&repetition.join("baselines.json"))?;
        artifact_integrity &= baseline.version == BASELINE_SNAPSHOT_VERSION;
        artifact_integrity &= baseline.repetition == run.repetition;
        if let Some(first) = &baseline_snapshot {
            artifact_integrity &= first == &baseline.repositories;
        } else {
            baseline_snapshot = Some(baseline.repositories);
        }
        let validation = read_json::<ValidationArtifactV1>(&repetition.join("validation.json"))?;
        artifact_integrity &= validation.version == VALIDATION_ARTIFACT_VERSION;
        artifact_integrity &=
            validation.post_run_commands.len() == manifest.manifest().post_run_commands.len();
        artifact_integrity &= validation
            .post_run_commands
            .iter()
            .zip(&manifest.manifest().post_run_commands)
            .all(|(evidence, declared)| evidence.argv == *declared);
        artifact_integrity &= validation.exact_patch.as_ref().is_some_and(|patch| {
            manifest
                .manifest()
                .expected_patch
                .as_ref()
                .is_some_and(|declared| patch.expected_patch == declared.display().to_string())
        });
        artifact_integrity &= validation_summary_matches(&summary, &validation);
        validations.push(validation);
    }
    let baseline_snapshot = baseline_snapshot.ok_or_else(|| {
        AcceptanceError::Invalid("acceptance input contains no typed repetitions".to_string())
    })?;
    let privacy_safe = scan_privacy(root, &policy.privacy_forbidden_fragments)?
        && scan_aggregate_privacy(root, &policy.aggregate_privacy_forbidden_fragments)?;
    Ok(TrialSet {
        aggregate,
        validations,
        artifact_integrity,
        privacy_safe,
        baseline_snapshot,
    })
}

fn require_directory(path: &Path) -> Result<(), AcceptanceError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| AcceptanceError::Inspect {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(AcceptanceError::UnsafePath(path.to_path_buf()));
    }
    Ok(())
}

fn read_bytes(path: &Path) -> Result<Vec<u8>, AcceptanceError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| AcceptanceError::Inspect {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(AcceptanceError::UnsafePath(path.to_path_buf()));
    }
    fs::read(path).map_err(|source| AcceptanceError::Read {
        path: path.to_path_buf(),
        source,
    })
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, AcceptanceError> {
    let bytes = read_bytes(path)?;
    serde_json::from_slice(&bytes).map_err(|source| AcceptanceError::Json {
        path: path.to_path_buf(),
        source,
    })
}

fn read_json_value(path: &Path) -> Result<serde_json::Value, AcceptanceError> {
    read_json(path)
}

fn scan_privacy(root: &Path, forbidden: &[String]) -> Result<bool, AcceptanceError> {
    let mut safe = true;
    scan_privacy_directory(root, forbidden, &mut safe)?;
    Ok(safe)
}

fn scan_privacy_directory(
    directory: &Path,
    forbidden: &[String],
    safe: &mut bool,
) -> Result<(), AcceptanceError> {
    for entry in fs::read_dir(directory).map_err(|source| AcceptanceError::Read {
        path: directory.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| AcceptanceError::Read {
            path: directory.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|source| AcceptanceError::Inspect {
            path: path.clone(),
            source,
        })?;
        if metadata.file_type().is_symlink() {
            return Err(AcceptanceError::UnsafePath(path));
        }
        if metadata.is_dir() {
            scan_privacy_directory(&path, forbidden, safe)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(AcceptanceError::UnsafePath(path));
        }
        if path.file_name().and_then(|name| name.to_str()) == Some("manifest.toml") {
            continue;
        }
        let bytes = read_bytes(&path)?;
        if forbidden.iter().any(|fragment| {
            bytes
                .windows(fragment.len())
                .any(|window| window == fragment.as_bytes())
        }) {
            *safe = false;
        }
    }
    Ok(())
}

fn scan_aggregate_privacy(root: &Path, forbidden: &[String]) -> Result<bool, AcceptanceError> {
    for name in ["aggregate.json", "aggregate.md"] {
        let bytes = read_bytes(&root.join(name))?;
        if forbidden.iter().any(|fragment| {
            bytes
                .windows(fragment.len())
                .any(|window| window == fragment.as_bytes())
        }) {
            return Ok(false);
        }
    }
    Ok(true)
}
