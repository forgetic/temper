//! Path-agnostic helpers for invoking a workspace executor from an action.
//!
//! Both the LLM role-decision path ([`crate::role_process_tools`]) and the
//! queue-automation path ([`crate::worker::automation`]) build a
//! [`CodingWorkspaceRequest`], invoke a bound workspace, and route on the
//! returned verdict through the action's `outcomes` map. The deterministic
//! correlation keys, branch hints, and the pull-request input projected from a
//! workspace head are shared here so both paths produce identical, idempotent
//! Forge writes for the same work item.

use temper_forge::{BranchRef, CreatePullRequest, ItemNumber, RepositoryId};
use temper_workflow::{
    render_metadata_block, ArtifactKindId, ArtifactRef, ArtifactSource, TransitionId,
    WorkflowMetadata,
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
    let metadata = WorkflowMetadata {
        kind: Some(ArtifactKindId::new("implementation_pr")),
        parents: vec![ArtifactRef::same_repo(code_number)],
        ..WorkflowMetadata::default()
    };
    let summary = summary.trim();
    let body = format!(
        "Workspace-produced implementation for issue #{code_number}.\n\nSummary: {}\n\n{}",
        if summary.is_empty() {
            "(none)"
        } else {
            summary
        },
        render_metadata_block(&metadata)
    );
    CreatePullRequest {
        title: format!("Implement #{code_number}: {issue_title}"),
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

/// Projects a workspace head into the pull-request creation input for an
/// implementation PR parented to the originating code issue.
pub(crate) fn workspace_pull_request_input(
    repo: RepositoryId,
    code_number: ItemNumber,
    issue_title: &str,
    output: CodingWorkspaceOutput,
    default_base_branch: String,
) -> CreatePullRequest {
    let base_branch = if output.base_branch.trim().is_empty() {
        default_base_branch
    } else {
        output.base_branch
    };
    implementation_pr_pull_request_input(
        repo,
        code_number,
        issue_title,
        output.branch,
        base_branch,
        &output.summary,
        output.labels,
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
}
