//! Verify a missing destination without following a symlink ancestor.

use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use temper_agent_core::TargetAdmissionStatus;

pub(crate) fn verify_missing_target(
    workspace: &Path,
    relative: &str,
) -> Result<PathBuf, TargetAdmissionStatus> {
    if relative.is_empty()
        || relative.trim() != relative
        || relative.contains(['\\', '\0'])
        || relative
            .split('/')
            .any(|part| matches!(part, "" | "." | ".." | ".git"))
    {
        return Err(TargetAdmissionStatus::MalformedTarget);
    }
    let parts = Path::new(relative).components().collect::<Vec<_>>();
    if parts.is_empty()
        || parts
            .iter()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(TargetAdmissionStatus::MalformedTarget);
    }
    let root = workspace
        .canonicalize()
        .map_err(|_| TargetAdmissionStatus::UnknownTarget)?;
    if root != workspace {
        return Err(TargetAdmissionStatus::OutsideWorkspace);
    }
    let mut candidate = root.clone();
    for (index, part) in parts.iter().enumerate() {
        candidate.push(part.as_os_str());
        match std::fs::symlink_metadata(&candidate) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(TargetAdmissionStatus::OutsideWorkspace);
                }
                if index + 1 == parts.len() || !metadata.is_dir() {
                    return Err(TargetAdmissionStatus::CompetingTargets);
                }
                let canonical = candidate
                    .canonicalize()
                    .map_err(|_| TargetAdmissionStatus::UnknownTarget)?;
                if !canonical.starts_with(&root) {
                    return Err(TargetAdmissionStatus::OutsideWorkspace);
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(_) => return Err(TargetAdmissionStatus::UnknownTarget),
        }
    }
    Ok(candidate)
}
