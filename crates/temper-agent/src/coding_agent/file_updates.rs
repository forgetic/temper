//! Snapshot, stage, and recheck explicit file replacements for mutation tools.

use std::fs::{File, Permissions};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use tempfile::NamedTempFile;

use crate::workspace_files::{MAX_FILE_BYTES, existing_target};

pub(super) struct FileUpdate {
    relative: String,
    path: PathBuf,
    original: Vec<u8>,
    permissions: Permissions,
    staged: Option<NamedTempFile>,
}

impl FileUpdate {
    pub(super) fn original(&self) -> &[u8] {
        &self.original
    }

    pub(super) fn parent(&self) -> Result<&Path, String> {
        self.path
            .parent()
            .ok_or_else(|| "target has no parent".into())
    }

    pub(super) fn stage(&mut self, output: &[u8]) -> Result<(), String> {
        if output == self.original {
            return Ok(());
        }
        let mut staged =
            NamedTempFile::new_in(self.parent()?).map_err(|error| error.to_string())?;
        staged
            .write_all(output)
            .map_err(|error| error.to_string())?;
        staged
            .as_file()
            .set_permissions(self.permissions.clone())
            .map_err(|error| error.to_string())?;
        staged.flush().map_err(|error| error.to_string())?;
        self.staged = Some(staged);
        Ok(())
    }
}

pub(super) fn load<'a>(
    root: &Path,
    paths: impl IntoIterator<Item = &'a str>,
) -> Result<Vec<FileUpdate>, String> {
    let mut files = Vec::new();
    let mut input_bytes = 0;
    for relative in paths {
        let path = existing_target(root, relative)?;
        let original = read_bounded(&path)?;
        input_bytes += original.len();
        if input_bytes > MAX_FILE_BYTES {
            return Err(format!("input exceeds {MAX_FILE_BYTES} bytes"));
        }
        std::str::from_utf8(&original).map_err(|error| error.to_string())?;
        let permissions = std::fs::metadata(&path)
            .map_err(|error| error.to_string())?
            .permissions();
        files.push(FileUpdate {
            relative: relative.to_owned(),
            path,
            original,
            permissions,
            staged: None,
        });
    }
    Ok(files)
}

pub(super) fn commit(root: &Path, files: Vec<FileUpdate>, action: &str) -> Result<usize, String> {
    commit_checked(root, files, action, |_, _| {})
}

fn commit_checked(
    root: &Path,
    files: Vec<FileUpdate>,
    action: &str,
    mut before_each: impl FnMut(usize, &str),
) -> Result<usize, String> {
    for file in &files {
        verify_preimage(root, file)?;
    }
    let mut changed = 0;
    for mut file in files {
        if file.staged.is_some() {
            before_each(changed, &file.relative);
            // Recheck all three properties immediately before each replacement.
            // External I/O failures cannot promise transactionality across files.
            verify_preimage(root, &file)
                .map_err(|error| format!("{error} after {changed} {action} files"))?;
            let staged = file.staged.take().expect("changed file has staged output");
            staged.persist(&file.path).map_err(|error| {
                format!(
                    "cannot replace {} after {changed} {action} files: {error}",
                    file.relative
                )
            })?;
            changed += 1;
        }
    }
    Ok(changed)
}

fn verify_preimage(root: &Path, file: &FileUpdate) -> Result<(), String> {
    if existing_target(root, &file.relative)? != file.path
        || read_bounded(&file.path)? != file.original
        || std::fs::metadata(&file.path)
            .map_err(|error| error.to_string())?
            .permissions()
            != file.permissions
    {
        return Err(format!(
            "target changed during file update: {}",
            file.relative
        ));
    }
    Ok(())
}

fn read_bounded(path: &Path) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|error| error.to_string())?
        .take((MAX_FILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(format!("target exceeds {MAX_FILE_BYTES} bytes"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests;
