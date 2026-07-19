// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::path::Path;

use super::{BenchmarkComparisonV1, ComparisonError, render_comparison_markdown};

/// Writes `comparison.json` and `comparison.md` to an output directory without
/// following caller-provided symlinks.
pub fn write_comparison_artifacts(
    comparison: &BenchmarkComparisonV1,
    output_dir: impl AsRef<Path>,
) -> Result<(), ComparisonError> {
    let output_dir = output_dir.as_ref();
    match fs::symlink_metadata(output_dir) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(ComparisonError::UnsafeOutput(output_dir.to_path_buf()));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(output_dir).map_err(|source| ComparisonError::CreateOutput {
                path: output_dir.to_path_buf(),
                source,
            })?;
        }
        Err(source) => {
            return Err(ComparisonError::CreateOutput {
                path: output_dir.to_path_buf(),
                source,
            });
        }
    }
    let mut json = serde_json::to_vec_pretty(comparison)?;
    json.push(b'\n');
    write_regular(&output_dir.join("comparison.json"), &json)?;
    write_regular(
        &output_dir.join("comparison.md"),
        render_comparison_markdown(comparison).as_bytes(),
    )
}

fn write_regular(path: &Path, contents: &[u8]) -> Result<(), ComparisonError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(ComparisonError::UnsafeOutput(path.to_path_buf()));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(ComparisonError::Write {
                path: path.to_path_buf(),
                source,
            });
        }
    }
    fs::write(path, contents).map_err(|source| ComparisonError::Write {
        path: path.to_path_buf(),
        source,
    })
}
