//! One strict interpretation of patch targets for admission and execution.

use std::collections::{BTreeMap, BTreeSet};

use temper_agent_core::TargetAdmissionStatus;

mod creation;
mod framing;
#[cfg(test)]
mod framing_tests;
mod hunks;
pub(crate) use creation::verify_missing_target;

pub(crate) const MAX_PATCH_BYTES: usize = 2 * 1024 * 1024;

pub(crate) struct PreparedPatch {
    pub(crate) text: String,
    pub(crate) targets: BTreeMap<String, PatchOperation>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PatchOperation {
    Existing,
    Create,
}

#[derive(Default)]
struct PatchSection {
    old: String,
    new: String,
    old_marker: Option<String>,
    new_marker: Option<String>,
    body_started: bool,
}

pub(crate) fn patch_targets(
    patch: &str,
) -> Result<BTreeMap<String, PatchOperation>, TargetAdmissionStatus> {
    Ok(prepare_patch(patch)?.targets)
}

pub(crate) fn prepare_patch(patch: &str) -> Result<PreparedPatch, TargetAdmissionStatus> {
    // Explicit contradictory declarations remain contradictions even when
    // another part of the input needs its framing or hunk lengths repaired.
    if matches!(
        canonical_targets(patch),
        Err(TargetAdmissionStatus::CompetingTargets)
    ) {
        return Err(TargetAdmissionStatus::CompetingTargets);
    }
    let text = framing::canonicalize(patch)?;
    let targets = canonical_targets(&text)?;
    Ok(PreparedPatch { text, targets })
}

fn canonical_targets(
    patch: &str,
) -> Result<BTreeMap<String, PatchOperation>, TargetAdmissionStatus> {
    let mut paths = BTreeMap::new();
    let mut current: Option<PatchSection> = None;
    for line in patch.lines() {
        if line.starts_with("diff --git ") {
            if let Some(section) = current.take() {
                finish_section(section, &mut paths)?;
            }
            let fields = line.split_ascii_whitespace().collect::<Vec<_>>();
            if fields.len() != 4 || fields[..2] != ["diff", "--git"] {
                return Err(TargetAdmissionStatus::MalformedTarget);
            }
            current = Some(PatchSection {
                old: git_path(fields[2], "a/")?,
                new: git_path(fields[3], "b/")?,
                ..PatchSection::default()
            });
        } else if line.starts_with("@@ ") || line == "GIT binary patch" {
            current
                .as_mut()
                .ok_or(TargetAdmissionStatus::MalformedTarget)?
                .body_started = true;
        } else if let Some(marker) = line.strip_prefix("--- ") {
            let section = current
                .as_mut()
                .ok_or(TargetAdmissionStatus::MalformedTarget)?;
            if !section.body_started && section.old_marker.replace(marker.to_string()).is_some() {
                return Err(TargetAdmissionStatus::CompetingTargets);
            }
        } else if let Some(marker) = line.strip_prefix("+++ ") {
            let section = current
                .as_mut()
                .ok_or(TargetAdmissionStatus::MalformedTarget)?;
            if !section.body_started && section.new_marker.replace(marker.to_string()).is_some() {
                return Err(TargetAdmissionStatus::CompetingTargets);
            }
        } else if let Some(mode) = line.strip_prefix("new file mode ") {
            if !matches!(mode, "100644" | "100755") {
                return Err(TargetAdmissionStatus::MalformedTarget);
            }
        }
    }
    finish_section(
        current.ok_or(TargetAdmissionStatus::MalformedTarget)?,
        &mut paths,
    )?;
    if paths.is_empty() || paths.len() > 64 {
        return Err(TargetAdmissionStatus::MalformedTarget);
    }
    Ok(paths)
}

fn finish_section(
    section: PatchSection,
    paths: &mut BTreeMap<String, PatchOperation>,
) -> Result<(), TargetAdmissionStatus> {
    let old = section
        .old_marker
        .ok_or(TargetAdmissionStatus::MalformedTarget)?;
    let new = section
        .new_marker
        .ok_or(TargetAdmissionStatus::MalformedTarget)?;
    if old != "/dev/null" && old != format!("a/{}", section.old)
        || new != "/dev/null" && new != format!("b/{}", section.new)
    {
        return Err(TargetAdmissionStatus::CompetingTargets);
    }
    if !section.body_started || old == "/dev/null" && new == "/dev/null" {
        return Err(TargetAdmissionStatus::MalformedTarget);
    }
    if old == "/dev/null" {
        if section.old != section.new {
            return Err(TargetAdmissionStatus::CompetingTargets);
        }
        insert_target(paths, section.new, PatchOperation::Create)?;
    } else {
        let mut section_paths = BTreeSet::from([section.old]);
        if new != "/dev/null" {
            section_paths.insert(section.new);
        }
        for path in section_paths {
            insert_target(paths, path, PatchOperation::Existing)?;
        }
    }
    Ok(())
}

fn insert_target(
    paths: &mut BTreeMap<String, PatchOperation>,
    path: String,
    operation: PatchOperation,
) -> Result<(), TargetAdmissionStatus> {
    if paths.insert(path, operation).is_some() {
        return Err(TargetAdmissionStatus::CompetingTargets);
    }
    Ok(())
}

fn git_path(value: &str, prefix: &str) -> Result<String, TargetAdmissionStatus> {
    let path = value
        .strip_prefix(prefix)
        .ok_or(TargetAdmissionStatus::MalformedTarget)?;
    if path.contains(['\\', '"', '\0'])
        || path
            .split('/')
            .any(|part| matches!(part, "" | "." | ".." | ".git"))
    {
        return Err(TargetAdmissionStatus::MalformedTarget);
    }
    Ok(path.to_string())
}
