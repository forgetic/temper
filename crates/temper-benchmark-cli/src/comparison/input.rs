// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::path::Path;

use super::{ComparisonError, ComparisonInput};
use crate::{BenchmarkAggregateV1, RunSummaryV1};

/// Resolves either a summary JSON file or an artifact directory. Aggregate
/// roots use `aggregate.json`; repetition roots use `run.json`.
pub fn load_comparison_input(path: impl AsRef<Path>) -> Result<ComparisonInput, ComparisonError> {
    let requested = path.as_ref();
    let metadata = fs::symlink_metadata(requested).map_err(|source| ComparisonError::Inspect {
        path: requested.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() {
        return Err(ComparisonError::UnsafeInput(requested.to_path_buf()));
    }
    let resolved = if metadata.is_dir() {
        let aggregate = requested.join("aggregate.json");
        let run = requested.join("run.json");
        if regular_file(&aggregate)? {
            aggregate
        } else if regular_file(&run)? {
            run
        } else {
            return Err(ComparisonError::MissingArtifact(requested.to_path_buf()));
        }
    } else if metadata.is_file() {
        requested.to_path_buf()
    } else {
        return Err(ComparisonError::UnsafeInput(requested.to_path_buf()));
    };
    parse_comparison_input(&resolved)
}

fn regular_file(path: &Path) -> Result<bool, ComparisonError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(ComparisonError::UnsafeInput(path.to_path_buf()))
        }
        Ok(metadata) => Ok(metadata.is_file()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(ComparisonError::Inspect {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn parse_comparison_input(path: &Path) -> Result<ComparisonInput, ComparisonError> {
    let bytes = fs::read(path).map_err(|source| ComparisonError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let value = serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|source| {
        ComparisonError::Json {
            path: path.to_path_buf(),
            source,
        }
    })?;
    let object = value
        .as_object()
        .ok_or_else(|| ComparisonError::Unrecognized(path.to_path_buf()))?;
    match (object.contains_key("identity"), object.contains_key("runs")) {
        (true, false) => serde_json::from_value::<RunSummaryV1>(value)
            .map(ComparisonInput::Run)
            .map_err(|source| ComparisonError::Json {
                path: path.to_path_buf(),
                source,
            }),
        (false, true) => serde_json::from_value::<BenchmarkAggregateV1>(value)
            .map(ComparisonInput::Aggregate)
            .map_err(|source| ComparisonError::Json {
                path: path.to_path_buf(),
                source,
            }),
        _ => Err(ComparisonError::Unrecognized(path.to_path_buf())),
    }
}
