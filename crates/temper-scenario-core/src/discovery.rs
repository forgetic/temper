// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::path::{Path, PathBuf};

use toml::Value;

use crate::parse::parse_manifest_value;
use crate::{CheckReport, Diagnostic, DiscoverError, ScenarioEntry};

/// Checks a single scenario directory or manifest file.
pub fn check_scenario(path: impl AsRef<Path>) -> CheckReport {
    let path = path.as_ref();
    let (scenario_path, manifest_path) = match resolve_manifest_path(path) {
        Ok(resolved) => resolved,
        Err(diagnostic) => {
            return CheckReport {
                scenario_path: path.to_path_buf(),
                manifest_path: None,
                manifest: None,
                diagnostics: vec![diagnostic],
            };
        }
    };

    let source = match fs::read_to_string(&manifest_path) {
        Ok(source) => source,
        Err(error) => {
            return CheckReport {
                scenario_path,
                manifest_path: Some(manifest_path.clone()),
                manifest: None,
                diagnostics: vec![Diagnostic::document_error(format!(
                    "failed to read manifest: {error}"
                ))],
            };
        }
    };

    let value = match source.parse::<Value>() {
        Ok(value) => value,
        Err(error) => {
            return CheckReport {
                scenario_path,
                manifest_path: Some(manifest_path.clone()),
                manifest: None,
                diagnostics: vec![Diagnostic::document_error(format!("invalid TOML: {error}"))],
            };
        }
    };

    let base_dir = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let (manifest, diagnostics) = parse_manifest_value(&value, base_dir, Some(&manifest_path));
    CheckReport {
        scenario_path,
        manifest_path: Some(manifest_path),
        manifest: if diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == crate::Severity::Error)
        {
            None
        } else {
            manifest
        },
        diagnostics,
    }
}

/// Discovers scenario directories under `root`.
///
/// Missing roots are treated as empty so the tooling can land before a
/// repository has checked in its first `scenarios/` tree.
pub fn discover_scenarios(root: impl AsRef<Path>) -> Result<Vec<ScenarioEntry>, DiscoverError> {
    let root = root.as_ref();
    if !root.exists() {
        return Ok(Vec::new());
    }
    if !root.is_dir() {
        return Err(DiscoverError::NotDirectory {
            path: root.to_path_buf(),
        });
    }

    let mut entries = Vec::new();
    if let Some(manifest_path) = manifest_in_dir(root) {
        entries.push(ScenarioEntry {
            scenario_path: root.to_path_buf(),
            manifest_path,
        });
    }

    let read_dir = fs::read_dir(root).map_err(|source| DiscoverError::ReadDir {
        path: root.to_path_buf(),
        source,
    })?;
    for entry in read_dir {
        let entry = entry.map_err(|source| DiscoverError::ReadDir {
            path: root.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if let Some(manifest_path) = manifest_in_dir(&path) {
            entries.push(ScenarioEntry {
                scenario_path: path,
                manifest_path,
            });
        }
    }

    entries.sort_by(|left, right| left.scenario_path.cmp(&right.scenario_path));
    entries.dedup_by(|left, right| left.scenario_path == right.scenario_path);
    Ok(entries)
}

/// Checks every discovered scenario under `root`.
pub fn check_scenarios(root: impl AsRef<Path>) -> Result<Vec<CheckReport>, DiscoverError> {
    let entries = discover_scenarios(root)?;
    Ok(entries
        .into_iter()
        .map(|entry| check_scenario(entry.scenario_path))
        .collect())
}

/// Resolves a user-supplied scenario directory or manifest file.
pub fn resolve_manifest_path(path: &Path) -> Result<(PathBuf, PathBuf), Diagnostic> {
    if path.is_file() {
        let scenario_path = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        return Ok((scenario_path, path.to_path_buf()));
    }
    if path.is_dir() {
        if let Some(manifest_path) = manifest_in_dir(path) {
            return Ok((path.to_path_buf(), manifest_path));
        }
        return Err(Diagnostic::document_error(format!(
            "no scenario manifest found in {} (expected one of: {})",
            path.display(),
            crate::MANIFEST_FILE_NAMES.join(", ")
        )));
    }
    Err(Diagnostic::document_error(format!(
        "scenario path does not exist: {}",
        path.display()
    )))
}

fn manifest_in_dir(dir: &Path) -> Option<PathBuf> {
    crate::MANIFEST_FILE_NAMES
        .iter()
        .map(|name| dir.join(name))
        .find(|path| path.is_file())
}
