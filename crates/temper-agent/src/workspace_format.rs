//! Shared admission and execution contract for explicit Rust formatting targets.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

pub(crate) const MAX_FORMAT_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const MAX_FORMAT_PATHS: usize = 64;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FormatRustInput {
    pub(crate) paths: Vec<String>,
    pub(crate) edition: RustEdition,
}

#[derive(Clone, Copy, Deserialize)]
pub(crate) enum RustEdition {
    #[serde(rename = "2015")]
    E2015,
    #[serde(rename = "2018")]
    E2018,
    #[serde(rename = "2021")]
    E2021,
    #[serde(rename = "2024")]
    E2024,
}

impl RustEdition {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::E2015 => "2015",
            Self::E2018 => "2018",
            Self::E2021 => "2021",
            Self::E2024 => "2024",
        }
    }
}

impl FormatRustInput {
    pub(crate) fn parse(value: &serde_json::Value) -> Result<Self, String> {
        let input: Self = serde_json::from_value(value.clone())
            .map_err(|error| format!("invalid format_rust input: {error}"))?;
        if input.paths.is_empty() || input.paths.len() > MAX_FORMAT_PATHS {
            return Err(format!(
                "paths must contain 1 to {MAX_FORMAT_PATHS} Rust files"
            ));
        }
        let mut unique = BTreeSet::new();
        for raw in &input.paths {
            let path = Path::new(raw);
            if raw.is_empty()
                || raw.len() > 4096
                || raw.trim() != raw
                || raw.contains(['\0', '\\'])
                || raw.split('/').any(|part| matches!(part, "" | "." | ".."))
                || path
                    .components()
                    .any(|part| !matches!(part, Component::Normal(_)))
                || path.extension().and_then(|value| value.to_str()) != Some("rs")
                || !unique.insert(raw)
            {
                return Err(format!(
                    "expected unique workspace-relative .rs paths: {raw:?}"
                ));
            }
        }
        Ok(input)
    }
}

pub(crate) fn existing_format_target(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let mut candidate = root.canonicalize().map_err(|error| error.to_string())?;
    if candidate != root {
        return Err("workspace root changed during formatting".into());
    }
    for component in Path::new(relative).components() {
        let Component::Normal(component) = component else {
            return Err("format target must remain inside the workspace".into());
        };
        candidate.push(component);
        let metadata = std::fs::symlink_metadata(&candidate)
            .map_err(|error| format!("cannot inspect {relative}: {error}"))?;
        if metadata.file_type().is_symlink() {
            return Err(format!("format target contains a symlink: {relative}"));
        }
    }
    if !std::fs::metadata(&candidate).is_ok_and(|metadata| metadata.is_file()) {
        return Err(format!(
            "format target must be an existing regular file: {relative}"
        ));
    }
    Ok(candidate)
}
