// SPDX-License-Identifier: MPL-2.0

//! Host-owned implementation-PR run section.
//!
//! Source issues keep only the concise handoff once an implementation PR exists;
//! this managed PR-body section is the human-facing coarse run history. It is
//! keyed by the same correlation key as the workflow metadata, but kept outside
//! the workflow metadata block so human edits and machine metadata remain
//! independently mergeable.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use serde_json::Value;
use temper_protocol_worker::JobProgress;
use temper_workflow::{METADATA_BEGIN, METADATA_END};

const PR_RUN_END: &str = "<!-- /temper-pr-run -->";
const CHECKPOINT_MARKER_PREFIX: &str = "<!-- temper-pr-run-checkpoint step=";
const CHECKPOINT_MARKER_END: &str = " -->";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PullRequestRunStatus {
    ImplementationInProgress,
    ReadyForReview,
}

impl PullRequestRunStatus {
    fn render(self) -> &'static str {
        match self {
            Self::ImplementationInProgress => "implementation in progress",
            Self::ReadyForReview => "ready for review",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PullRequestRunCheckpoint {
    pub(super) step: u32,
    pub(super) sha: String,
    pub(super) label: String,
}

impl PullRequestRunCheckpoint {
    pub(super) fn from_progress(progress: &JobProgress, pushed_sha: &str) -> Self {
        Self {
            step: progress.step,
            sha: short_sha(pushed_sha),
            label: one_line_or(&progress.status, "checkpoint"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PullRequestRunUpdate {
    pub(super) correlation_key: String,
    pub(super) status: PullRequestRunStatus,
    pub(super) work_branch: String,
    pub(super) checkpoint: Option<PullRequestRunCheckpoint>,
    /// `None` means keep any existing validation lines. `Some` replaces the
    /// managed validation section (and an empty vector removes it).
    pub(super) validation: Option<Vec<String>>,
}

impl PullRequestRunUpdate {
    pub(super) fn checkpoint_progress(
        correlation_key: &str,
        work_branch: &str,
        progress: &JobProgress,
        pushed_sha: &str,
    ) -> Self {
        Self {
            correlation_key: correlation_key.to_string(),
            status: PullRequestRunStatus::ImplementationInProgress,
            work_branch: one_line_or(work_branch, "-"),
            checkpoint: Some(PullRequestRunCheckpoint::from_progress(progress, pushed_sha)),
            validation: None,
        }
    }

    pub(super) fn ready_for_review(
        correlation_key: &str,
        work_branch: &str,
        validation: Option<Vec<String>>,
    ) -> Self {
        Self {
            correlation_key: correlation_key.to_string(),
            status: PullRequestRunStatus::ReadyForReview,
            work_branch: one_line_or(work_branch, "-"),
            checkpoint: None,
            validation,
        }
    }
}

pub(super) fn merge_pull_request_run_section(
    body: &str,
    update: &PullRequestRunUpdate,
) -> Result<Option<String>, PullRequestRunMergeError> {
    let mut section = if let Some((start, end)) = pr_run_span(body, &update.correlation_key)? {
        RunSection::parse(&body[start..end])
    } else {
        RunSection::default()
    };
    section.apply(update);
    let block = section.render(&update.correlation_key);

    if let Some((start, end)) = pr_run_span(body, &update.correlation_key)? {
        if &body[start..end] == block {
            return Ok(None);
        }
        let updated = format!("{}{}{}", &body[..start], block, &body[end..]);
        return if updated == body {
            Ok(None)
        } else {
            Ok(Some(updated))
        };
    }

    insert_pr_run_block(body, &block).map(Some)
}

pub(super) fn validation_lines_from_details(details: Option<&Value>) -> Option<Vec<String>> {
    let details = details?;
    let validation = details
        .get("validation")
        .or_else(|| details.get("validations"))?;
    Some(validation_lines(validation))
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RunSection {
    status: PullRequestRunStatus,
    work_branch: String,
    checkpoints: BTreeMap<u32, PullRequestRunCheckpoint>,
    validation: Vec<String>,
}

impl Default for RunSection {
    fn default() -> Self {
        Self {
            status: PullRequestRunStatus::ImplementationInProgress,
            work_branch: "-".to_string(),
            checkpoints: BTreeMap::new(),
            validation: Vec::new(),
        }
    }
}

impl RunSection {
    fn parse(block: &str) -> Self {
        let mut section = Self::default();
        let mut in_validation = false;

        for line in block.lines() {
            let trimmed = line.trim();
            if trimmed == PR_RUN_END || trimmed.starts_with("<!-- temper-pr-run ") {
                in_validation = false;
                continue;
            }
            if let Some(status) = trimmed.strip_prefix("Status:") {
                in_validation = false;
                if status.trim() == PullRequestRunStatus::ReadyForReview.render() {
                    section.status = PullRequestRunStatus::ReadyForReview;
                } else {
                    section.status = PullRequestRunStatus::ImplementationInProgress;
                }
                continue;
            }
            if let Some(branch) = trimmed.strip_prefix("Work branch:") {
                in_validation = false;
                section.work_branch = parse_backticked(branch).unwrap_or_else(|| {
                    let collapsed = one_line(branch.trim());
                    if collapsed.is_empty() {
                        "-".to_string()
                    } else {
                        collapsed
                    }
                });
                continue;
            }
            if let Some(checkpoint) = parse_checkpoint_line(trimmed) {
                in_validation = false;
                section.checkpoints.insert(checkpoint.step, checkpoint);
                continue;
            }
            if trimmed == "Validation:" {
                in_validation = true;
                continue;
            }
            if in_validation {
                if let Some(line) = trimmed.strip_prefix("- ") {
                    let line = one_line(line);
                    if !line.is_empty() {
                        section.validation.push(line);
                    }
                } else if !trimmed.is_empty() {
                    in_validation = false;
                }
            }
        }

        section
    }

    fn apply(&mut self, update: &PullRequestRunUpdate) {
        self.status = match (self.status, update.status) {
            (_, PullRequestRunStatus::ReadyForReview) => PullRequestRunStatus::ReadyForReview,
            (PullRequestRunStatus::ReadyForReview, PullRequestRunStatus::ImplementationInProgress) => {
                PullRequestRunStatus::ReadyForReview
            }
            (_, PullRequestRunStatus::ImplementationInProgress) => {
                PullRequestRunStatus::ImplementationInProgress
            }
        };

        let work_branch = one_line(&update.work_branch);
        if !work_branch.is_empty() {
            self.work_branch = work_branch;
        }

        if let Some(checkpoint) = &update.checkpoint {
            self.checkpoints.insert(checkpoint.step, checkpoint.clone());
        }

        if let Some(validation) = &update.validation {
            self.validation = validation
                .iter()
                .map(|line| one_line(line))
                .filter(|line| !line.is_empty())
                .collect();
        }
    }

    fn render(&self, correlation_key: &str) -> String {
        let mut lines = vec![
            pr_run_marker(correlation_key),
            "### Temper run".to_string(),
            format!("Status: {}", self.status.render()),
            format!("Work branch: `{}`", one_line_or(&self.work_branch, "-")),
        ];

        if let Some((_, checkpoint)) = self.checkpoints.iter().next_back() {
            lines.push(format!(
                "Last checkpoint: `{}` — {}",
                one_line_or(&checkpoint.sha, "-"),
                one_line_or(&checkpoint.label, "checkpoint")
            ));
        } else {
            lines.push("Last checkpoint: none recorded".to_string());
        }

        lines.push(String::new());
        lines.push("Checkpoints:".to_string());
        if self.checkpoints.is_empty() {
            lines.push("- none recorded".to_string());
        } else {
            for checkpoint in self.checkpoints.values() {
                lines.push(format!(
                    "- `{}` — {} {}",
                    one_line_or(&checkpoint.sha, "-"),
                    one_line_or(&checkpoint.label, "checkpoint"),
                    checkpoint_marker(checkpoint.step)
                ));
            }
        }

        if !self.validation.is_empty() {
            lines.push(String::new());
            lines.push("Validation:".to_string());
            for line in &self.validation {
                lines.push(format!("- {}", one_line_or(line, "validation reported")));
            }
        }

        lines.push(PR_RUN_END.to_string());
        lines.join("\n")
    }
}

fn parse_checkpoint_line(line: &str) -> Option<PullRequestRunCheckpoint> {
    let marker_start = line.find(CHECKPOINT_MARKER_PREFIX)?;
    let step_start = marker_start + CHECKPOINT_MARKER_PREFIX.len();
    let step_end = line[step_start..].find(CHECKPOINT_MARKER_END)? + step_start;
    let step = line[step_start..step_end].trim().parse::<u32>().ok()?;

    let rendered = line[..marker_start].trim_end();
    let rendered = rendered.strip_prefix("- ")?.trim_start();
    let (sha, rest) = parse_leading_backticked(rendered)
        .unwrap_or_else(|| ("-".to_string(), rendered.to_string()));
    let label = rest
        .trim_start()
        .strip_prefix('—')
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .unwrap_or("checkpoint");

    Some(PullRequestRunCheckpoint {
        step,
        sha: one_line_or(&sha, "-"),
        label: one_line_or(label, "checkpoint"),
    })
}

fn parse_leading_backticked(value: &str) -> Option<(String, String)> {
    let rest = value.strip_prefix('`')?;
    let end = rest.find('`')?;
    Some((rest[..end].to_string(), rest[end + 1..].to_string()))
}

fn parse_backticked(value: &str) -> Option<String> {
    let start = value.find('`')? + 1;
    let end = value[start..].find('`')? + start;
    Some(value[start..end].to_string())
}

fn validation_lines(value: &Value) -> Vec<String> {
    match value {
        Value::Array(entries) => entries.iter().filter_map(validation_line).collect(),
        Value::Object(map) => map
            .get("commands")
            .and_then(|commands| match commands {
                Value::Array(entries) => Some(entries.iter().filter_map(validation_line).collect()),
                _ => None,
            })
            .unwrap_or_else(|| validation_line(value).into_iter().collect()),
        _ => validation_line(value).into_iter().collect(),
    }
}

fn validation_line(value: &Value) -> Option<String> {
    match value {
        Value::String(line) => non_empty_line(line),
        Value::Object(map) => {
            if let Some(line) = map.get("line").and_then(Value::as_str).and_then(non_empty_line) {
                return Some(line);
            }
            let command = ["command", "cmd"]
                .into_iter()
                .find_map(|key| map.get(key).and_then(Value::as_str).and_then(non_empty_line));
            let result = ["result", "status", "outcome"]
                .into_iter()
                .find_map(|key| map.get(key).and_then(Value::as_str).and_then(non_empty_line));
            match (command, result) {
                (Some(command), Some(result)) => {
                    Some(format!("`{}` {}", inline_code(&command), result))
                }
                (Some(command), None) => Some(format!("`{}`", inline_code(&command))),
                (None, Some(result)) => Some(result),
                (None, None) => None,
            }
        }
        _ => None,
    }
}

fn non_empty_line(value: &str) -> Option<String> {
    let line = one_line(value);
    if line.is_empty() { None } else { Some(line) }
}

fn inline_code(value: &str) -> String {
    value.replace('`', "'")
}

fn pr_run_span(
    body: &str,
    correlation_key: &str,
) -> Result<Option<(usize, usize)>, PullRequestRunMergeError> {
    let marker = pr_run_marker(correlation_key);
    let Some(start) = body.find(&marker) else {
        return Ok(None);
    };
    let after_marker = start + marker.len();
    let Some(end_relative) = body[after_marker..].find(PR_RUN_END) else {
        return Err(PullRequestRunMergeError::UnterminatedRunSection);
    };
    let end = after_marker + end_relative + PR_RUN_END.len();
    Ok(Some((start, end)))
}

fn insert_pr_run_block(body: &str, block: &str) -> Result<String, PullRequestRunMergeError> {
    if let Some(index) = workflow_metadata_start(body)? {
        let before = body[..index].trim_end();
        let after = body[index..].trim_start_matches('\n');
        return if before.is_empty() {
            Ok(format!("{block}\n\n{after}"))
        } else {
            Ok(format!("{before}\n\n{block}\n\n{after}"))
        };
    }

    if body.trim().is_empty() {
        Ok(block.to_string())
    } else {
        Ok(format!("{}\n\n{block}", body.trim_end()))
    }
}

fn workflow_metadata_start(body: &str) -> Result<Option<usize>, PullRequestRunMergeError> {
    let Some(start) = body.find(METADATA_BEGIN) else {
        return Ok(None);
    };
    let after_begin = start + METADATA_BEGIN.len();
    if body[after_begin..].find(METADATA_END).is_none() {
        return Err(PullRequestRunMergeError::UnterminatedWorkflowMetadata);
    }
    Ok(Some(start))
}

fn checkpoint_marker(step: u32) -> String {
    format!("{CHECKPOINT_MARKER_PREFIX}{step}{CHECKPOINT_MARKER_END}")
}

fn pr_run_marker(correlation_key: &str) -> String {
    format!(
        "<!-- temper-pr-run correlation_key={} -->",
        one_line_or(correlation_key, "unknown")
    )
}

fn short_sha(sha: &str) -> String {
    sha.trim().chars().take(12).collect::<String>()
}

fn one_line_or(value: &str, fallback: &str) -> String {
    let collapsed = one_line(value);
    if collapsed.is_empty() {
        fallback.to_string()
    } else {
        collapsed
    }
}

fn one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum PullRequestRunMergeError {
    UnterminatedRunSection,
    UnterminatedWorkflowMetadata,
}

impl fmt::Display for PullRequestRunMergeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnterminatedRunSection => {
                formatter.write_str("pull-request run section was not terminated")
            }
            Self::UnterminatedWorkflowMetadata => {
                formatter.write_str("workflow metadata block was not terminated")
            }
        }
    }
}

impl Error for PullRequestRunMergeError {}

#[cfg(test)]
mod tests {
    use super::*;
    use temper_workflow::{ArtifactKindId, WorkflowMetadata, parse_metadata_block, render_metadata_block};

    fn metadata() -> WorkflowMetadata {
        WorkflowMetadata {
            kind: Some(ArtifactKindId::new("implementation_pr")),
            correlation_key: Some("pr-for-code-7".to_string()),
            ..WorkflowMetadata::default()
        }
    }

    fn update(step: u32, sha: &str, label: &str) -> PullRequestRunUpdate {
        PullRequestRunUpdate {
            correlation_key: "pr-for-code-7".to_string(),
            status: PullRequestRunStatus::ImplementationInProgress,
            work_branch: "agent/pr-for-code-7".to_string(),
            checkpoint: Some(PullRequestRunCheckpoint {
                step,
                sha: sha.to_string(),
                label: label.to_string(),
            }),
            validation: None,
        }
    }

    #[test]
    fn inserts_run_section_before_workflow_metadata() {
        let body = format!("Summary: pending\n\n{}", render_metadata_block(&metadata()));

        let updated = merge_pull_request_run_section(&body, &update(2, "abc123456789", "build it"))
            .expect("merge succeeds")
            .expect("body changes");

        assert!(updated.contains("### Temper run"));
        assert!(updated.contains("Status: implementation in progress"));
        assert!(updated.contains("- `abc123456789` — build it"));
        assert!(updated.find("<!-- temper-pr-run").unwrap() < updated.find("<!-- temper:workflow").unwrap());
        assert_eq!(parse_metadata_block(&updated).unwrap(), Some(metadata()));
    }

    #[test]
    fn checkpoint_replay_replaces_same_step_without_duplicates() {
        let body = format!("Summary: pending\n\n{}", render_metadata_block(&metadata()));
        let with_step = merge_pull_request_run_section(&body, &update(2, "abc123456789", "build it"))
            .unwrap()
            .unwrap();
        let replayed = merge_pull_request_run_section(&with_step, &update(2, "def123456789", "build it again"))
            .unwrap()
            .unwrap();

        assert_eq!(replayed.matches("temper-pr-run-checkpoint step=2").count(), 1);
        assert!(!replayed.contains("abc123456789"));
        assert!(replayed.contains("def123456789"));
        assert!(replayed.contains("build it again"));
    }

    #[test]
    fn ready_update_preserves_checkpoint_and_renders_validation() {
        let body = format!("Human note.\n\n{}", render_metadata_block(&metadata()));
        let with_step = merge_pull_request_run_section(&body, &update(2, "abc123456789", "build it"))
            .unwrap()
            .unwrap();
        let ready = PullRequestRunUpdate::ready_for_review(
            "pr-for-code-7",
            "agent/pr-for-code-7",
            Some(vec!["`cargo test -p temper-engine` passed".to_string()]),
        );
        let updated = merge_pull_request_run_section(&with_step, &ready)
            .unwrap()
            .unwrap();

        assert!(updated.contains("Human note."));
        assert!(updated.contains("Status: ready for review"));
        assert!(updated.contains("- `abc123456789` — build it"));
        assert!(updated.contains("Validation:\n- `cargo test -p temper-engine` passed"));
        assert_eq!(parse_metadata_block(&updated).unwrap(), Some(metadata()));
    }
}
