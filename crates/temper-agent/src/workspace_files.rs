//! Shared containment for explicit existing workspace files.

use std::path::{Component, Path, PathBuf};

pub(crate) const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const MAX_FILE_PATHS: usize = 64;

pub(crate) fn validate_relative(raw: &str) -> Result<(), String> {
    if raw.is_empty()
        || raw.len() > 4096
        || raw.trim() != raw
        || raw.contains(['\0', '\\'])
        || raw
            .split('/')
            .any(|part| matches!(part, "" | "." | ".." | ".git"))
        || Path::new(raw)
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(format!(
            "expected a canonical workspace-relative file path: {raw:?}"
        ));
    }
    Ok(())
}

pub(crate) fn existing_target(root: &Path, relative: &str) -> Result<PathBuf, String> {
    validate_relative(relative)?;
    let mut candidate = root.canonicalize().map_err(|error| error.to_string())?;
    if candidate != root {
        return Err("workspace root changed during file update".into());
    }
    for component in Path::new(relative).components() {
        candidate.push(component.as_os_str());
        let metadata = std::fs::symlink_metadata(&candidate)
            .map_err(|error| format!("cannot inspect {relative}: {error}"))?;
        if metadata.file_type().is_symlink() {
            return Err(format!("target contains a symlink: {relative}"));
        }
    }
    if !std::fs::metadata(&candidate).is_ok_and(|metadata| metadata.is_file()) {
        return Err(format!(
            "target must be an existing regular file: {relative}"
        ));
    }
    Ok(candidate)
}
