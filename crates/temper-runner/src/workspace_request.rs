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

/// Minimum number of structured implementation phases that should produce PR
/// checklist ceremony. Zero or one phase remains a plain PR body.
pub const IMPLEMENTATION_PLAN_CHECKLIST_PHASE_COUNT: usize = 2;

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
    implementation_pr_pull_request_input_with_plan(
        repo,
        code_number,
        issue_title,
        head_branch,
        base_branch,
        summary,
        labels,
        &[],
    )
}

/// Projects implementation-PR fields with optional structured plan phases into
/// the pull-request creation input.
pub fn implementation_pr_pull_request_input_with_plan(
    repo: RepositoryId,
    code_number: ItemNumber,
    issue_title: &str,
    head_branch: String,
    base_branch: String,
    summary: &str,
    labels: Vec<String>,
    plan_phases: &[String],
) -> CreatePullRequest {
    let metadata = WorkflowMetadata {
        kind: Some(ArtifactKindId::new("implementation_pr")),
        parents: vec![ArtifactRef::same_repo(code_number)],
        ..WorkflowMetadata::default()
    };
    let intro = format!("Workspace-produced implementation for issue #{code_number}.");
    let body = implementation_pr_body(&intro, summary, plan_phases, &metadata);
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

/// Renders the shared implementation PR body, adding a plan checklist only when
/// structured plan phases are non-trivial.
pub fn implementation_pr_body(
    intro: &str,
    summary: &str,
    plan_phases: &[String],
    metadata: &WorkflowMetadata,
) -> String {
    let summary = summary.trim();
    let plan_section = render_implementation_plan_checklist(plan_phases)
        .map(|section| format!("\n\n{section}"))
        .unwrap_or_default();
    format!(
        "{}\n\nSummary: {}{}\n\n{}",
        intro.trim(),
        if summary.is_empty() {
            "(none)"
        } else {
            summary
        },
        plan_section,
        render_metadata_block(metadata)
    )
}

/// Renders a Markdown checklist for non-trivial implementation plans.
pub fn render_implementation_plan_checklist(plan_phases: &[String]) -> Option<String> {
    let phases = checklist_phases(plan_phases);
    if phases.len() < IMPLEMENTATION_PLAN_CHECKLIST_PHASE_COUNT {
        return None;
    }

    let mut checklist = String::from("Implementation plan:\n\n");
    for phase in phases {
        checklist.push_str("- [ ] ");
        checklist.push_str(&phase);
        checklist.push('\n');
    }
    checklist.pop();
    Some(checklist)
}

fn checklist_phases(plan_phases: &[String]) -> Vec<String> {
    plan_phases
        .iter()
        .map(|phase| phase.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|phase| !phase.is_empty())
        .collect()
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
    implementation_pr_pull_request_input_with_plan(
        repo,
        code_number,
        issue_title,
        output.branch,
        base_branch,
        &output.summary,
        output.labels,
        &output.plan_phases,
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
    fn implementation_pr_body_renders_multi_phase_plan_checklist() {
        let repo = RepositoryId::new("repo-1");
        let number = ItemNumber::new(120);
        let phases = vec![
            "Write failing test".to_string(),
            "Implement fix\nwith docs".to_string(),
        ];

        let input = implementation_pr_pull_request_input_with_plan(
            repo,
            number,
            "daemon worker apply",
            "agent/pr-for-code-120".to_string(),
            "main".to_string(),
            "implemented the thing",
            vec!["implementation".to_string()],
            &phases,
        );

        assert!(input.body.contains("Summary: implemented the thing"));
        assert!(input.body.contains(
            "Implementation plan:\n\n- [ ] Write failing test\n- [ ] Implement fix with docs"
        ));
        assert!(input.body.contains("<!-- temper:workflow-metadata"));
    }

    #[test]
    fn implementation_pr_body_keeps_trivial_or_absent_plan_plain() {
        for phases in [Vec::new(), vec!["Single obvious edit".to_string()]] {
            let input = implementation_pr_pull_request_input_with_plan(
                RepositoryId::new("repo-1"),
                ItemNumber::new(120),
                "daemon worker apply",
                "agent/pr-for-code-120".to_string(),
                "main".to_string(),
                "implemented the thing",
                vec!["implementation".to_string()],
                &phases,
            );

            assert!(input.body.contains("Summary: implemented the thing"));
            assert!(!input.body.contains("Implementation plan"));
            assert!(!input.body.contains("- [ ]"));
        }
    }

    #[test]
    fn workspace_pull_request_input_carries_output_plan_phases() {
        let repo = RepositoryId::new("repo-1");
        let input = workspace_pull_request_input(
            repo,
            ItemNumber::new(120),
            "daemon worker apply",
            output("main").with_plan_phases(["Design API", "Wire caller"]),
            "main".to_string(),
        );

        assert!(input.body.contains("- [ ] Design API"));
        assert!(input.body.contains("- [ ] Wire caller"));
    }
}
