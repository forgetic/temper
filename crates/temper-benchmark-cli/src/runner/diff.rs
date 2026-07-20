// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::path::Path;
use std::process::{Command, ExitStatus};

use serde::{Deserialize, Serialize};

use super::BenchmarkRunError;
use crate::{DiffStatisticsV1, PreparedBenchmarkWorkspace};

/// Version for the per-file diff artifact.
pub const DIFF_ARTIFACT_VERSION: u32 = 1;

/// Detailed final diff evidence for all context repositories.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiffArtifactV1 {
    pub version: u32,
    pub statistics: DiffStatisticsV1,
    pub repositories: Vec<RepositoryDiffEvidenceV1>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryDiffEvidenceV1 {
    pub id: String,
    pub dir: String,
    pub baseline: String,
    pub files: Vec<DiffFileEvidenceV1>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiffFileEvidenceV1 {
    pub path: String,
    pub tracked: bool,
    /// Binary files have no line count and therefore leave this absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub insertions: Option<u64>,
    /// Binary files have no line count and therefore leave this absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deletions: Option<u64>,
}

pub(super) fn collect_diff_artifact(
    workspace: &PreparedBenchmarkWorkspace,
) -> Result<DiffArtifactV1, BenchmarkRunError> {
    let empty = workspace.temporary_root().join("empty-for-untracked-diff");
    fs::write(&empty, []).map_err(|source| BenchmarkRunError::Io {
        operation: "create empty diff input",
        path: empty.clone(),
        source,
    })?;

    let mut repositories = Vec::new();
    let mut statistics = DiffStatisticsV1 {
        files_changed: 0,
        insertions: 0,
        deletions: 0,
        tracked_files: 0,
        untracked_files: 0,
    };
    for baseline in workspace.baselines() {
        let repository = workspace.root().join(&baseline.dir);
        let tracked = git_output(
            &repository,
            &[
                "diff",
                "--no-ext-diff",
                "--numstat",
                "--no-renames",
                "-z",
                &baseline.sha,
                "--",
                ".",
            ],
            &[0],
        )?;
        let mut files = parse_tracked_numstat(&tracked.stdout);
        statistics.tracked_files = statistics.tracked_files.saturating_add(files.len() as u64);

        let untracked = git_output(
            &repository,
            &["ls-files", "--others", "--exclude-standard", "-z"],
            &[0],
        )?;
        let mut untracked_paths = untracked
            .stdout
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(|path| String::from_utf8_lossy(path).into_owned())
            .collect::<Vec<_>>();
        untracked_paths.sort();
        statistics.untracked_files = statistics
            .untracked_files
            .saturating_add(untracked_paths.len() as u64);
        for path in untracked_paths {
            let absolute = repository.join(&path);
            let empty_arg = empty.to_string_lossy().into_owned();
            let file_arg = absolute.to_string_lossy().into_owned();
            let output = git_output(
                &repository,
                &[
                    "diff",
                    "--no-index",
                    "--no-ext-diff",
                    "--numstat",
                    "--no-renames",
                    "-z",
                    "--",
                    &empty_arg,
                    &file_arg,
                ],
                &[0, 1],
            )?;
            let (insertions, deletions) =
                parse_leading_numstat(&output.stdout).unwrap_or((Some(0), Some(0)));
            files.push(DiffFileEvidenceV1 {
                path,
                tracked: false,
                insertions,
                deletions,
            });
        }
        files.sort_by(|left, right| left.path.cmp(&right.path));
        for file in &files {
            statistics.insertions = statistics
                .insertions
                .saturating_add(file.insertions.unwrap_or(0));
            statistics.deletions = statistics
                .deletions
                .saturating_add(file.deletions.unwrap_or(0));
        }
        statistics.files_changed = statistics.files_changed.saturating_add(files.len() as u64);
        repositories.push(RepositoryDiffEvidenceV1 {
            id: baseline.id.clone(),
            dir: baseline.dir.clone(),
            baseline: baseline.sha.clone(),
            files,
        });
    }

    Ok(DiffArtifactV1 {
        version: DIFF_ARTIFACT_VERSION,
        statistics,
        repositories,
    })
}

fn parse_tracked_numstat(bytes: &[u8]) -> Vec<DiffFileEvidenceV1> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .filter_map(|record| {
            let first_tab = record.iter().position(|byte| *byte == b'\t')?;
            let second_tab = record[first_tab + 1..]
                .iter()
                .position(|byte| *byte == b'\t')?
                + first_tab
                + 1;
            let insertions = parse_numstat_count(&record[..first_tab]);
            let deletions = parse_numstat_count(&record[first_tab + 1..second_tab]);
            let path = String::from_utf8_lossy(&record[second_tab + 1..]).into_owned();
            Some(DiffFileEvidenceV1 {
                path,
                tracked: true,
                insertions,
                deletions,
            })
        })
        .collect()
}

fn parse_leading_numstat(bytes: &[u8]) -> Option<(Option<u64>, Option<u64>)> {
    let first_tab = bytes.iter().position(|byte| *byte == b'\t')?;
    let second_tab = bytes[first_tab + 1..]
        .iter()
        .position(|byte| *byte == b'\t')?
        + first_tab
        + 1;
    Some((
        parse_numstat_count(&bytes[..first_tab]),
        parse_numstat_count(&bytes[first_tab + 1..second_tab]),
    ))
}

fn parse_numstat_count(bytes: &[u8]) -> Option<u64> {
    (bytes != b"-")
        .then(|| std::str::from_utf8(bytes).ok()?.parse().ok())
        .flatten()
}

fn git_output(
    cwd: &Path,
    arguments: &[&str],
    accepted_codes: &[i32],
) -> Result<std::process::Output, BenchmarkRunError> {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|source| BenchmarkRunError::Io {
            operation: "run Git diff command",
            path: cwd.to_path_buf(),
            source,
        })?;
    if output
        .status
        .code()
        .is_some_and(|code| accepted_codes.contains(&code))
    {
        Ok(output)
    } else {
        Err(BenchmarkRunError::Git {
            command: format!("git {}", arguments.join(" ")),
            cwd: cwd.to_path_buf(),
            status: status_string(output.status),
            stderr: bounded_text(&output.stderr, 4096),
        })
    }
}

fn status_string(status: ExitStatus) -> String {
    status
        .code()
        .map_or_else(|| status.to_string(), |code| code.to_string())
}

fn bounded_text(bytes: &[u8], limit: usize) -> String {
    let start = bytes.len().saturating_sub(limit);
    String::from_utf8_lossy(&bytes[start..]).trim().to_string()
}
