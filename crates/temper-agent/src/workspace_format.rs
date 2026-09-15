//! Shared admission and execution contract for explicit Rust formatting targets.

use std::collections::BTreeSet;
use std::path::Path;

use serde::Deserialize;

use crate::workspace_files::validate_relative;
pub(crate) use crate::workspace_files::{
    MAX_FILE_BYTES as MAX_FORMAT_BYTES, MAX_FILE_PATHS as MAX_FORMAT_PATHS,
    existing_target as existing_format_target,
};

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
            if validate_relative(raw).is_err()
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
