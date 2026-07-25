use super::*;

#[cfg(unix)]
pub(in crate::trace) fn repair_private_dir(path: &Path) -> Result<(), TraceError> {
    repair_permissions(path, 0o700, true)
}

#[cfg(not(unix))]
pub(in crate::trace) fn repair_private_dir(_path: &Path) -> Result<(), TraceError> {
    Ok(())
}

#[cfg(unix)]
fn repair_private_file(path: &Path) -> Result<(), TraceError> {
    repair_permissions(path, 0o600, false)
}

#[cfg(not(unix))]
fn repair_private_file(_path: &Path) -> Result<(), TraceError> {
    Ok(())
}

#[cfg(unix)]
fn repair_permissions(path: &Path, expected_mode: u32, directory: bool) -> Result<(), TraceError> {
    use std::os::unix::fs::PermissionsExt as _;

    let metadata = fs::symlink_metadata(path)
        .map_err(|source| io_error("inspect trace spool permissions", path, source))?;
    let expected_type = if directory {
        metadata.file_type().is_dir()
    } else {
        metadata.file_type().is_file() && !metadata.file_type().is_symlink()
    };
    if !expected_type {
        return Err(TraceError::InvalidSpool(format!(
            "trace spool path has an unexpected file type: {}",
            path.display()
        )));
    }
    if metadata.permissions().mode() & 0o7777 != expected_mode {
        fs::set_permissions(path, fs::Permissions::from_mode(expected_mode))
            .map_err(|source| io_error("repair private trace permissions", path, source))?;
        record_permission_change();
    }
    Ok(())
}

pub(in crate::trace) fn repair_spool_root_permissions(root: &Path) -> Result<(), TraceError> {
    repair_private_dir(root)?;
    repair_private_file_if_present(&root.join(".spool-root.lock"))
}

pub(super) fn repair_run_permissions(run_dir: &Path) -> Result<(), TraceError> {
    repair_private_dir(run_dir)?;
    for name in [
        ".spool.lock",
        ".owner.lock",
        "manifest.json",
        "events.jsonl",
        "acknowledgement.json",
        "compacted.json",
        "terminalization.json",
        FORWARDING_INDEX_FILE,
    ] {
        repair_private_file_if_present(&run_dir.join(name))?;
    }

    let blobs_dir = run_dir.join("blobs");
    repair_private_dir(&blobs_dir)?;
    for entry in read_dir(&blobs_dir)? {
        let entry = entry.map_err(|source| {
            io_error("read trace blobs for permission repair", &blobs_dir, source)
        })?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|source| {
            io_error("inspect trace blob for permission repair", &path, source)
        })?;
        if metadata.is_file() && !metadata.file_type().is_symlink() {
            repair_private_file(&path)?;
        }
    }
    Ok(())
}

fn repair_private_file_if_present(path: &Path) -> Result<(), TraceError> {
    match fs::symlink_metadata(path) {
        Ok(_) => repair_private_file(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(io_error("inspect trace spool permissions", path, source)),
    }
}

pub(super) fn sync_file_all(file: &File) -> io::Result<()> {
    file.sync_all()?;
    record_file_sync();
    Ok(())
}

pub(in crate::trace) fn sync_file_data(file: &File) -> io::Result<()> {
    file.sync_data()?;
    record_file_sync();
    Ok(())
}

pub(super) fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()?;
    record_directory_sync();
    Ok(())
}
