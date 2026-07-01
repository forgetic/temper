// SPDX-License-Identifier: MPL-2.0

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use temper_scenario_core::{CheckReport, DEFAULT_SCENARIOS_DIR, ScenarioTopology};

/// Operator-selected confidence tier for `temper-scenario run`.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) enum ScenarioTier {
    Hermetic,
    Live,
}

impl ScenarioTier {
    pub(super) fn parse(value: &str) -> Option<Self> {
        match value {
            "hermetic" => Some(Self::Hermetic),
            "live" => Some(Self::Live),
            _ => None,
        }
    }

    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Hermetic => "hermetic",
            Self::Live => "live",
        }
    }

    pub(super) fn description(self) -> &'static str {
        match self {
            Self::Hermetic => {
                "fast in-process/memory runner; lower confidence than live; not a live Forgejo proof"
            }
            Self::Live => "live Forgejo/topology proof",
        }
    }
}

/// Classification of the scenario bundle path supplied by the operator.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) enum ScenarioSource {
    CheckedIn,
    Ephemeral,
}

impl ScenarioSource {
    pub(super) fn classify(scenario_path: &Path) -> Self {
        let scenario_path = canonical_or_absolute(scenario_path);
        for repo_root in candidate_repo_roots(&scenario_path) {
            let corpus = repo_root.join(DEFAULT_SCENARIOS_DIR);
            let Ok(corpus) = fs::canonicalize(&corpus) else {
                continue;
            };
            if scenario_path.starts_with(corpus) {
                return Self::CheckedIn;
            }
        }
        Self::Ephemeral
    }

    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::CheckedIn => "checked-in scenario",
            Self::Ephemeral => "ephemeral validation bundle",
        }
    }
}

/// Source/tier/topology facts attached to a run and to validation evidence.
#[derive(Debug, Clone, Eq, PartialEq)]
pub(super) struct ScenarioRunFacts {
    pub(super) source: ScenarioSource,
    pub(super) tier: ScenarioTier,
    pub(super) topology: ScenarioTopology,
}

impl ScenarioRunFacts {
    pub(super) fn from_check_report(check_report: &CheckReport, tier: ScenarioTier) -> Self {
        let topology = check_report
            .manifest
            .as_ref()
            .map(|manifest| manifest.topology.clone())
            .unwrap_or_default();
        Self {
            source: ScenarioSource::classify(&check_report.scenario_path),
            tier,
            topology,
        }
    }

    pub(super) fn print_stdout(&self) {
        println!("source: {}", self.source.as_str());
        println!(
            "confidence tier: {} ({})",
            self.tier.as_str(),
            self.tier.description()
        );
        println!("manifest topology:");
        if self.topology.is_empty() {
            println!("  not declared");
        } else {
            for (field, value) in self.topology.field_values() {
                println!("  {field}: {value}");
            }
        }
    }

    pub(super) fn evidence_details(&self) -> Vec<String> {
        let mut details = vec![
            format!("source: {}", self.source.as_str()),
            format!(
                "confidence tier: {} ({})",
                self.tier.as_str(),
                self.tier.description()
            ),
        ];
        if self.topology.is_empty() {
            details.push("manifest topology: not declared".to_string());
        } else {
            details.extend(
                self.topology
                    .field_values()
                    .into_iter()
                    .map(|(field, value)| format!("manifest topology.{field}: `{value}`")),
            );
        }
        details
    }

    pub(super) fn unsupported_live_message(&self, scenario_name: &str) -> String {
        let topology = if self.topology.is_empty() {
            "manifest topology: not declared".to_string()
        } else {
            self.topology
                .field_values()
                .into_iter()
                .map(|(field, value)| format!("{field}={value}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        format!(
            "unsupported-live-topology: live tier is not implemented yet for scenario `{scenario_name}` ({topology}); refusing to substitute the hermetic memory runner, use --tier hermetic instead"
        )
    }
}

fn candidate_repo_roots(scenario_path: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(current_dir) = env::current_dir() {
        if let Some(root) = find_repo_root(&current_dir) {
            push_unique(&mut candidates, root);
        }
    }
    push_unique(
        &mut candidates,
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(".")),
    );
    if let Some(root) = find_repo_root(scenario_path) {
        push_unique(&mut candidates, root);
    }
    candidates
}

fn find_repo_root(start: &Path) -> Option<PathBuf> {
    let mut current = if start.is_file() {
        start.parent().unwrap_or(start)
    } else {
        start
    };
    loop {
        if current.join(".git").exists() && current.join(DEFAULT_SCENARIOS_DIR).is_dir() {
            return Some(current.to_path_buf());
        }
        let parent = current.parent()?;
        current = parent;
    }
}

fn push_unique(candidates: &mut Vec<PathBuf>, path: PathBuf) {
    let path = fs::canonicalize(&path).unwrap_or(path);
    if !candidates.iter().any(|candidate| candidate == &path) {
        candidates.push(path);
    }
}

fn canonical_or_absolute(path: &Path) -> PathBuf {
    if let Ok(canonical) = fs::canonicalize(path) {
        return canonical;
    }
    if path.is_absolute() {
        return path.to_path_buf();
    }
    env::current_dir()
        .map(|current_dir| current_dir.join(path))
        .unwrap_or_else(|_| path.to_path_buf())
}
