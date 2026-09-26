//! Stage bounded formatter output and validate every preimage before replacing files.

use std::fs::{File, Permissions};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use temper_agent_core::AgentContainmentContext;
use tempfile::NamedTempFile;

use crate::workspace_format::{FormatRustInput, MAX_FORMAT_BYTES, existing_format_target};

use super::process::format_source;

pub(super) struct FormattedFile {
    relative: String,
    path: PathBuf,
    original: Vec<u8>,
    permissions: Permissions,
    staged: Option<NamedTempFile>,
}

pub(super) fn prepare(
    root: &Path,
    input: &FormatRustInput,
    containment: &AgentContainmentContext,
) -> Result<Vec<FormattedFile>, String> {
    let mut files = Vec::new();
    let mut input_bytes = 0;
    // Resolve and read the entire explicit set before invoking any formatter.
    for relative in &input.paths {
        let path = existing_format_target(root, relative)?;
        let original = read_bounded(&path)?;
        input_bytes += original.len();
        if input_bytes > MAX_FORMAT_BYTES {
            return Err(format!("input exceeds {MAX_FORMAT_BYTES} bytes"));
        }
        std::str::from_utf8(&original).map_err(|error| error.to_string())?;
        let permissions = std::fs::metadata(&path)
            .map_err(|error| error.to_string())?
            .permissions();
        files.push(FormattedFile {
            relative: relative.clone(),
            path,
            original,
            permissions,
            staged: None,
        });
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut output_bytes = 0;
    for file in &mut files {
        let parent = file.path.parent().ok_or("format target has no parent")?;
        let output = format_source(parent, &file.original, input.edition, deadline, containment)?;
        output_bytes += output.len();
        if output_bytes > MAX_FORMAT_BYTES * 2 {
            return Err("aggregate formatter output exceeds limit".into());
        }
        if output != file.original {
            let mut staged = NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
            staged
                .write_all(&output)
                .map_err(|error| error.to_string())?;
            staged
                .as_file()
                .set_permissions(file.permissions.clone())
                .map_err(|error| error.to_string())?;
            staged.flush().map_err(|error| error.to_string())?;
            file.staged = Some(staged);
        }
    }
    Ok(files)
}

pub(super) fn commit(root: &Path, files: Vec<FormattedFile>) -> Result<usize, String> {
    for file in &files {
        verify_preimage(root, file)?;
    }
    let mut changed = 0;
    for mut file in files {
        if file.staged.is_some() {
            // Recheck the path before each atomic replacement as well. Multiple
            // replacements cannot promise transactionality under external I/O failure.
            verify_preimage(root, &file)
                .map_err(|error| format!("{error} after {changed} formatted files"))?;
            let staged = file.staged.take().expect("changed file has staged output");
            staged.persist(&file.path).map_err(|error| {
                format!(
                    "cannot replace {} after {changed} formatted files: {error}",
                    file.relative
                )
            })?;
            changed += 1;
        }
    }
    Ok(changed)
}

fn verify_preimage(root: &Path, file: &FormattedFile) -> Result<(), String> {
    if existing_format_target(root, &file.relative)? != file.path
        || read_bounded(&file.path)? != file.original
        || std::fs::metadata(&file.path)
            .map_err(|error| error.to_string())?
            .permissions()
            != file.permissions
    {
        return Err(format!(
            "target changed during formatting: {}",
            file.relative
        ));
    }
    Ok(())
}

fn read_bounded(path: &Path) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|error| error.to_string())?
        .take((MAX_FORMAT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > MAX_FORMAT_BYTES {
        return Err(format!("format target exceeds {MAX_FORMAT_BYTES} bytes"));
    }
    Ok(bytes)
}
