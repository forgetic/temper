//! Path-agnostic helpers for invoking a workspace executor from an action.
//!
//! Workspace-backed queue automation and daemon worker-result application both
//! need deterministic correlation keys, branch hints, and pull-request inputs
//! projected from workspace heads. Those path-agnostic helpers live here so each
//! caller produces identical, idempotent Forge writes for the same work item.

use temper_forge::{BranchRef, CreatePullRequest, ItemNumber, RepositoryId};
use temper_workflow::{
    ArtifactKindId, ArtifactRef, ArtifactSource, TransitionId, WorkflowMetadata,
    render_metadata_block,
};

use crate::CodingWorkspaceOutput;

/// Deterministic correlation key for the pull request a workspace head opens,
/// scoped to the work item's artifact kind and number so retries dedupe.
pub fn pr_correlation_key(kind: &ArtifactKindId, number: ItemNumber) -> String {
    format!("pr-for-{}-{}", safe_fragment(kind.as_str()), number.get())
}

/// Deterministic branch suggestion for a workspace head.
pub fn pr_branch_hint(kind: &ArtifactKindId, number: ItemNumber) -> String {
    format!("agent/{}", pr_correlation_key(kind, number))
}

/// Deterministic correlation key for content-bearing effects on a routed
/// transition, scoped to the work item and routed transition so retries dedupe.
///
/// This is the shared content-effect correlation key projection used by
/// workspace-backed runner paths and daemon verdict application.
pub fn workspace_content_key(
    kind: &ArtifactKindId,
    routed: &TransitionId,
    number: ItemNumber,
) -> String {
    format!(
        "content-{}-{}-{}",
        safe_fragment(kind.as_str()),
        safe_fragment(routed.as_str()),
        number.get()
    )
}

/// Returns the artifact item number a work-item target points at.
pub(crate) fn target_number(target: ArtifactSource) -> ItemNumber {
    match target {
        ArtifactSource::Issue { number } | ArtifactSource::PullRequest { number } => number,
    }
}

/// Sanitizes an id fragment for use inside a deterministic key or branch name.
pub(crate) fn safe_fragment(value: &str) -> String {
    let safe: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect();
    if safe.is_empty() {
        "work".to_string()
    } else {
        safe
    }
}

/// Projects implementation-PR fields into the pull-request creation input used
/// by workspace-backed runner paths and daemon worker-result application.
pub fn implementation_pr_pull_request_input(
    repo: RepositoryId,
    code_number: ItemNumber,
    issue_title: &str,
    head_branch: String,
    base_branch: String,
    summary: &str,
    labels: Vec<String>,
) -> CreatePullRequest {
    implementation_pr_pull_request_input_with_handoff(
        repo,
        code_number,
        issue_title,
        head_branch,
        base_branch,
        summary,
        labels,
        None,
        None,
    )
}

/// Projects implementation-PR fields with an optional agent-authored title and
/// report body. The metadata block remains Temper-owned and is appended to the
/// report body so classification/correlation stays parseable.
#[allow(clippy::too_many_arguments)]
pub fn implementation_pr_pull_request_input_with_handoff(
    repo: RepositoryId,
    code_number: ItemNumber,
    issue_title: &str,
    head_branch: String,
    base_branch: String,
    summary: &str,
    labels: Vec<String>,
    title: Option<&str>,
    report_body: Option<&str>,
) -> CreatePullRequest {
    let metadata = WorkflowMetadata {
        kind: Some(ArtifactKindId::new("implementation_pr")),
        parents: vec![ArtifactRef::same_repo(code_number)],
        ..WorkflowMetadata::default()
    };
    let intro = format!("Workspace-produced implementation for issue #{code_number}.");
    let body =
        implementation_pr_body_from_report_or_summary(report_body, &intro, summary, &metadata);
    CreatePullRequest {
        title: implementation_pr_title(title, code_number, issue_title),
        body,
        source: BranchRef {
            repository_id: repo.clone(),
            branch: head_branch,
        },
        target: BranchRef {
            repository_id: repo,
            branch: base_branch,
        },
        labels,
        assignees: Vec::new(),
    }
}

/// Renders the shared fallback implementation PR body for older agents that
/// only provide a summary.
pub fn implementation_pr_body(intro: &str, summary: &str, metadata: &WorkflowMetadata) -> String {
    let summary = summary.trim();
    format!(
        "{}\n\nSummary: {}\n\n{}",
        intro.trim(),
        if summary.is_empty() {
            "(none)"
        } else {
            summary
        },
        render_metadata_block(metadata)
    )
}

/// Renders an agent-authored implementation PR report body plus the Temper
/// workflow metadata block.
pub fn implementation_pr_report_body(report: &str, metadata: &WorkflowMetadata) -> String {
    let report = report.trim();
    if report.is_empty() {
        render_metadata_block(metadata)
    } else {
        format!("{}\n\n{}", report, render_metadata_block(metadata))
    }
}

pub fn implementation_pr_body_from_report_or_summary(
    report_body: Option<&str>,
    fallback_intro: &str,
    fallback_summary: &str,
    metadata: &WorkflowMetadata,
) -> String {
    match report_body.and_then(non_blank) {
        Some(report) => implementation_pr_report_body(report, metadata),
        None => implementation_pr_body(fallback_intro, fallback_summary, metadata),
    }
}

pub fn implementation_pr_title(
    title: Option<&str>,
    code_number: ItemNumber,
    issue_title: &str,
) -> String {
    title
        .and_then(non_blank)
        .map(str::to_string)
        .unwrap_or_else(|| format!("Implement #{code_number}: {issue_title}"))
}

fn non_blank(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

/// Projects a workspace head into the pull-request creation input for an
/// implementation PR parented to the originating code issue.
pub(crate) fn workspace_pull_request_input(
    repo: RepositoryId,
    code_number: ItemNumber,
    issue_title: &str,
    output: CodingWorkspaceOutput,
    default_base_branch: String,
) -> CreatePullRequest {
    let CodingWorkspaceOutput {
        branch,
        base_branch: output_base_branch,
        summary,
        labels,
        title,
        body,
        ..
    } = output;
    let base_branch = if output_base_branch.trim().is_empty() {
        default_base_branch
    } else {
        output_base_branch
    };
    implementation_pr_pull_request_input_with_handoff(
        repo,
        code_number,
        issue_title,
        branch,
        base_branch,
        &summary,
        labels,
        title.as_deref(),
        body.as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(base_branch: &str) -> CodingWorkspaceOutput {
        CodingWorkspaceOutput::new(
            "agent/pr-for-code-120",
            base_branch,
            "  implemented the thing  ",
            vec!["src/lib.rs".to_string()],
            vec!["implementation".to_string()],
        )
    }

    fn assert_parity(base_branch: &str, expected_base_branch: &str) {
        let repo = RepositoryId::new("repo-1");
        let number = ItemNumber::new(120);
        let delegated = workspace_pull_request_input(
            repo.clone(),
            number,
            "daemon worker apply",
            output(base_branch),
            "main".to_string(),
        );
        let direct = implementation_pr_pull_request_input(
            repo,
            number,
            "daemon worker apply",
            "agent/pr-for-code-120".to_string(),
            expected_base_branch.to_string(),
            "  implemented the thing  ",
            vec!["implementation".to_string()],
        );

        assert_eq!(delegated.title, direct.title);
        assert_eq!(delegated.body, direct.body);
        assert_eq!(delegated.source, direct.source);
        assert_eq!(delegated.target, direct.target);
        assert_eq!(delegated.labels, direct.labels);
        assert!(delegated.assignees.is_empty());
        assert_eq!(delegated.assignees, direct.assignees);
        assert_eq!(delegated, direct);
    }

    #[test]
    fn workspace_pull_request_input_delegates_with_default_base_branch() {
        assert_parity("   ", "main");
    }

    #[test]
    fn workspace_pull_request_input_delegates_with_explicit_base_branch() {
        assert_parity("release/1.2", "release/1.2");
    }

    #[test]
    fn implementation_pr_input_uses_agent_authored_title_and_report_body() {
        let input = implementation_pr_pull_request_input_with_handoff(
            RepositoryId::new("repo-1"),
            ItemNumber::new(120),
            "daemon worker apply",
            "agent/pr-for-code-120".to_string(),
            "main".to_string(),
            "short log summary",
            vec!["implementation".to_string()],
            Some("Implement agent-authored handoff"),
            Some("# Implementation report\n\nChanged the handoff path."),
        );

        assert_eq!(input.title, "Implement agent-authored handoff");
        assert!(input.body.starts_with("# Implementation report"));
        assert!(input.body.contains("Changed the handoff path."));
        assert!(!input.body.contains("Summary: short log summary"));
        assert!(input.body.contains("<!-- temper:workflow"));
    }

    #[test]
    fn implementation_pr_body_stays_progress_checklist_free() {
        let input = implementation_pr_pull_request_input(
            RepositoryId::new("repo-1"),
            ItemNumber::new(120),
            "daemon worker apply",
            "agent/pr-for-code-120".to_string(),
            "main".to_string(),
            "implemented the thing",
            vec!["implementation".to_string()],
        );

        assert!(input.body.contains("Summary: implemented the thing"));
        assert!(!input.body.contains("Implementation plan"));
        assert!(!input.body.contains("- [ ]"));
        assert!(input.body.contains("<!-- temper:workflow"));
    }
}
