// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::path::{Path, PathBuf};

use super::model::{DEFAULT_RUN_EVIDENCE_FILE, LoadedRunEvidence, RunEvidenceArtifact};

impl RunEvidenceArtifact {
    pub(crate) fn write_to_path(&self, path: &Path) -> Result<PathBuf, String> {
        let output_path = if path.is_dir() {
            path.join(DEFAULT_RUN_EVIDENCE_FILE)
        } else {
            path.to_path_buf()
        };
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "create run evidence output directory {}: {error}",
                    parent.display()
                )
            })?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|error| format!("serialize run evidence artifact: {error}"))?;
        fs::write(&output_path, format!("{json}\n")).map_err(|error| {
            format!(
                "write run evidence artifact {}: {error}",
                output_path.display()
            )
        })?;
        Ok(output_path)
    }
}

pub(crate) fn load_run_evidence(path: &Path) -> Result<LoadedRunEvidence, String> {
    let path = resolve_run_evidence_path(path)?;
    let source = fs::read_to_string(&path)
        .map_err(|error| format!("read run evidence artifact {}: {error}", path.display()))?;
    let artifact = serde_json::from_str::<RunEvidenceArtifact>(&source)
        .map_err(|error| format!("parse run evidence artifact {}: {error}", path.display()))?;
    Ok(LoadedRunEvidence { path, artifact })
}

fn resolve_run_evidence_path(path: &Path) -> Result<PathBuf, String> {
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    if path.is_dir() {
        let default = path.join(DEFAULT_RUN_EVIDENCE_FILE);
        if default.is_file() {
            return Ok(default);
        }
        let mut candidates = Vec::new();
        for entry in fs::read_dir(path)
            .map_err(|error| format!("read run evidence directory {}: {error}", path.display()))?
        {
            let entry = entry.map_err(|error| {
                format!(
                    "read run evidence directory entry {}: {error}",
                    path.display()
                )
            })?;
            let candidate = entry.path();
            if candidate.is_file()
                && candidate
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(".run-evidence.json"))
            {
                candidates.push(candidate);
            }
        }
        candidates.sort();
        return match candidates.as_slice() {
            [candidate] => Ok(candidate.clone()),
            [] => Err(format!(
                "run evidence directory {} does not contain {DEFAULT_RUN_EVIDENCE_FILE} or a *.run-evidence.json file",
                path.display()
            )),
            _ => Err(format!(
                "run evidence directory {} contains multiple *.run-evidence.json files; pass one file path explicitly",
                path.display()
            )),
        };
    }
    Err(format!(
        "run evidence path does not exist or is not readable: {}",
        path.display()
    ))
}
