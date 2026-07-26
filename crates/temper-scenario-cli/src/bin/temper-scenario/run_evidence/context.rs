// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::path::Path;
use std::process::Command;

use sha2::{Digest, Sha256};
use temper_scenario_core::{CheckReport, ScenarioTopology, load_resolved_manifest_toml};

use crate::run_context::ScenarioRunFacts;
use crate::runner_registry::SelectedRunner;

use super::model::{
    ArtifactCollections, FinalStateEvidence, FixtureEvidence, RUN_EVIDENCE_SCHEMA,
    RUN_EVIDENCE_VERSION, RunEvidenceArtifact, ScenarioEvidence, TopologyEvidence,
};

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct RunEvidenceContext {
    pub(crate) scenario: ScenarioEvidence,
    pub(crate) fixtures: Vec<FixtureEvidence>,
}

impl RunEvidenceContext {
    pub(crate) fn from_check_report(
        check_report: &CheckReport,
        facts: &ScenarioRunFacts,
        selected_runner: &SelectedRunner,
    ) -> Self {
        let manifest = check_report.manifest.as_ref();
        let scenario_name = manifest
            .map(|manifest| manifest.name.clone())
            .unwrap_or_else(|| "unknown".to_string());
        let manifest_path = check_report
            .manifest_path
            .as_deref()
            .unwrap_or(check_report.scenario_path.as_path());
        let fixtures = manifest
            .map(|manifest| {
                manifest
                    .path_references
                    .iter()
                    .map(|reference| FixtureEvidence {
                        field: reference.field.clone(),
                        value: reference.value.clone(),
                        resolved_path: reference.resolved_path.display().to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let identity = scenario_identity(manifest_path, check_report);
        Self {
            scenario: ScenarioEvidence {
                name: scenario_name.clone(),
                source: facts.source.evidence_value().to_string(),
                source_description: facts.source.as_str().to_string(),
                scenario_path: check_report.scenario_path.display().to_string(),
                manifest_path: manifest_path.display().to_string(),
                feature: identity.feature,
                plan: identity.plan,
                mapped_scenario: Some(scenario_name),
                source_branch: identity.source_branch,
                checkout_head_sha: identity.checkout_head_sha,
                resolved_content_digest: identity.resolved_content_digest,
                runner_id: selected_runner.id().to_string(),
                runner_selector: selected_runner.selector_key().to_string(),
                runner_selection: selected_runner.selection_detail(),
                tier: facts.tier.as_str().to_string(),
                tier_description: facts.tier.description().to_string(),
                topology: TopologyEvidence::from_topology(&facts.topology),
            },
            fixtures,
        }
    }

    pub(crate) fn artifact(&self, final_state: FinalStateEvidence) -> RunEvidenceArtifact {
        RunEvidenceArtifact {
            schema: RUN_EVIDENCE_SCHEMA.to_string(),
            version: RUN_EVIDENCE_VERSION,
            verdict: super::model::RunEvidenceVerdict::Passed,
            scenario: self.scenario.clone(),
            binary: None,
            execution: None,
            fixtures: self.fixtures.clone(),
            final_state,
            convergence: None,
            provider: None,
            observability: None,
            artifacts: ArtifactCollections::default(),
            evidence_lines: Vec::new(),
            stimuli: Vec::new(),
            limitations: Vec::new(),
            follow_up_intent: None,
            assertions: None,
        }
    }

    pub(crate) fn failure_artifact(
        &self,
        message: impl Into<String>,
        total_duration_ms: u64,
    ) -> RunEvidenceArtifact {
        let message = message.into();
        let mut artifact = self.artifact(FinalStateEvidence::default());
        artifact.verdict = super::model::RunEvidenceVerdict::Failed;
        artifact.execution = Some(super::model::ExecutionEvidence {
            status: "failed".to_string(),
            total_duration_ms,
            failure: Some(message.clone()),
        });
        artifact.limitations.push(
            "The live executor failed before complete final-state collection; retained diagnostics describe the last observable state."
                .to_string(),
        );
        artifact.follow_up_intent = Some(
            "Repair the live execution failure and rerun this exact scenario at the same checkout head."
                .to_string(),
        );
        artifact
            .evidence_lines
            .push(format!("execution failure: {message}"));
        for path in retained_failure_paths(&message) {
            if path.is_dir() {
                push_unique_path(
                    &mut artifact.artifacts.artifact_paths,
                    path.display().to_string(),
                );
            } else {
                push_unique_path(
                    &mut artifact.artifacts.log_paths,
                    path.display().to_string(),
                );
                if let Some(workspace) = path.parent().and_then(Path::parent) {
                    push_unique_path(
                        &mut artifact.artifacts.artifact_paths,
                        workspace.display().to_string(),
                    );
                }
            }
        }
        artifact
    }
}

fn retained_failure_paths(message: &str) -> Vec<std::path::PathBuf> {
    message
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            let raw = trimmed
                .strip_prefix("retained workspace: ")
                .or_else(|| trimmed.strip_prefix("standalone log: "))
                .or_else(|| trimmed.strip_prefix("CI diagnostics: "))
                .or_else(|| {
                    trimmed
                        .strip_prefix("--- ")
                        .and_then(|line| line.strip_suffix(" ---"))
                        .and_then(|line| line.rsplit_once('('))
                        .map(|(_, path)| path.trim_end_matches(')'))
                })?;
            let path = std::path::PathBuf::from(raw);
            path.exists().then_some(path)
        })
        .collect()
}

fn push_unique_path(paths: &mut Vec<String>, path: String) {
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

#[derive(Default)]
struct ScenarioIdentity {
    feature: Option<String>,
    plan: Option<String>,
    source_branch: Option<String>,
    checkout_head_sha: Option<String>,
    resolved_content_digest: Option<String>,
}

fn scenario_identity(manifest_path: &Path, check_report: &CheckReport) -> ScenarioIdentity {
    let resolved = load_resolved_manifest_toml(manifest_path).ok();
    let validation = resolved
        .as_ref()
        .and_then(|manifest| manifest.get("validation"))
        .and_then(toml::Value::as_table);
    let feature = validation
        .and_then(|table| table.get("feature"))
        .and_then(toml::Value::as_str)
        .map(str::to_string);
    let plan = validation
        .and_then(|table| table.get("plan"))
        .and_then(toml::Value::as_str)
        .map(str::to_string);
    let resolved_content_digest = resolved_content_digest(
        manifest_path,
        resolved.as_ref(),
        check_report.manifest.as_ref(),
    );
    let checkout = find_checkout_root(manifest_path);
    ScenarioIdentity {
        feature,
        plan,
        source_branch: checkout
            .as_deref()
            .and_then(|root| git_output(root, &["rev-parse", "--abbrev-ref", "HEAD"])),
        checkout_head_sha: checkout
            .as_deref()
            .and_then(|root| git_output(root, &["rev-parse", "HEAD"])),
        resolved_content_digest,
    }
}

fn resolved_content_digest(
    manifest_path: &Path,
    resolved: Option<&toml::Value>,
    manifest: Option<&temper_scenario_core::ScenarioManifest>,
) -> Option<String> {
    let mut source = resolved
        .and_then(|manifest| toml::to_string(manifest).ok())
        .or_else(|| fs::read_to_string(manifest_path).ok())?;
    let mut references = manifest
        .map(|manifest| manifest.path_references.clone())
        .unwrap_or_default();
    references.sort_by(|left, right| {
        left.field
            .cmp(&right.field)
            .then(left.value.cmp(&right.value))
            .then(left.resolved_path.cmp(&right.resolved_path))
    });

    // Resolved manifests carry absolute checkout paths so executors can open
    // inherited fixtures. Replace those machine-local strings before hashing;
    // fixture bytes are added separately below.
    for reference in &references {
        source = source.replace(
            &reference.resolved_path.display().to_string(),
            &format!("$resolved:{}:{}", reference.field, reference.value),
        );
    }

    let mut digest = Sha256::new();
    digest.update(b"temper.scenario.resolved-content.v1\0");
    digest.update(source.as_bytes());
    for reference in &references {
        digest.update(b"\0reference\0");
        digest.update(reference.field.as_bytes());
        digest.update(b"\0");
        digest.update(reference.value.as_bytes());
        if hash_fixture_tree(
            &mut digest,
            &reference.resolved_path,
            &reference.resolved_path,
        )
        .is_err()
        {
            return None;
        }
    }
    Some(format!("sha256:{:x}", digest.finalize()))
}

fn hash_fixture_tree(digest: &mut Sha256, root: &Path, path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("stat resolved fixture {}: {error}", path.display()))?;
    let relative = path.strip_prefix(root).unwrap_or(path);
    if metadata.is_dir() {
        let mut entries = fs::read_dir(path)
            .map_err(|error| format!("read resolved fixture dir {}: {error}", path.display()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("read resolved fixture dir entry: {error}"))?;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            hash_fixture_tree(digest, root, &entry.path())?;
        }
    } else if metadata.is_file() {
        digest.update(b"\0file\0");
        digest.update(relative.to_string_lossy().as_bytes());
        digest.update(b"\0");
        digest.update(
            fs::read(path)
                .map_err(|error| format!("read resolved fixture {}: {error}", path.display()))?,
        );
    } else {
        return Err(format!(
            "resolved fixture is not a regular file or directory: {}",
            path.display()
        ));
    }
    Ok(())
}

fn find_checkout_root(path: &Path) -> Option<std::path::PathBuf> {
    let mut current = if path.is_file() { path.parent()? } else { path };
    loop {
        if current.join(".git").exists() {
            return Some(current.to_path_buf());
        }
        current = current.parent()?;
    }
}

fn git_output(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

impl TopologyEvidence {
    fn from_topology(topology: &ScenarioTopology) -> Self {
        Self {
            kind: topology.kind.clone(),
            forge: topology.forge.clone(),
            runner: topology.runner.clone(),
            temper: topology.temper.clone(),
            agent_model: topology.agent_model.clone(),
        }
    }
}
