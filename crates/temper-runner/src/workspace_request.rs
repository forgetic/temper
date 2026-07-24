//! Path-agnostic helpers for invoking a workspace executor from an action.
//!
//! Workspace-backed queue automation and daemon worker-result application both
//! need deterministic correlation keys, branch hints, and pull-request inputs
//! projected from workspace heads. Those path-agnostic helpers live here so each
//! caller produces identical, idempotent Forge writes for the same work item.

use temper_forge::{BranchRef, CreatePullRequest, ItemNumber, RepositoryId};
use temper_workflow::{
    ArtifactKindId, ArtifactRef, ArtifactSource, METADATA_END, MetadataError, TransitionId,
    WorkflowMetadata, inspect_metadata_blocks, render_metadata_block,
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
    let prose = implementation_pr_fallback_prose(intro, summary);
    implementation_pr_body_from_sanitized_prose(&prose, metadata)
}

/// Renders an agent-authored implementation PR report body plus the Temper
/// workflow metadata block.
pub fn implementation_pr_report_body(report: &str, metadata: &WorkflowMetadata) -> String {
    let report = sanitize_authored_report(report);
    implementation_pr_body_from_sanitized_prose(&report, metadata)
}

/// Returns only the sanitized, agent-authored portion of an implementation PR
/// handoff. Managed workflow metadata is intentionally a separate input so
/// callers updating an existing artifact can derive authority from a fresh
/// Forge snapshot instead of a worker result.
pub fn implementation_pr_prose_from_report_or_summary(
    report_body: Option<&str>,
    fallback_intro: &str,
    fallback_summary: &str,
) -> String {
    let report = report_body.map(sanitize_authored_report);
    match report.as_deref().and_then(non_blank) {
        Some(report) => report.to_string(),
        None => implementation_pr_fallback_prose(fallback_intro, fallback_summary),
    }
}

pub fn implementation_pr_body_from_report_or_summary(
    report_body: Option<&str>,
    fallback_intro: &str,
    fallback_summary: &str,
    metadata: &WorkflowMetadata,
) -> String {
    let prose = implementation_pr_prose_from_report_or_summary(
        report_body,
        fallback_intro,
        fallback_summary,
    );
    implementation_pr_body_from_sanitized_prose(&prose, metadata)
}

fn implementation_pr_fallback_prose(intro: &str, summary: &str) -> String {
    let intro = sanitize_authored_report(intro);
    let summary = sanitize_authored_report(summary);
    let summary = summary.trim();
    format!(
        "{}\n\nSummary: {}",
        intro.trim(),
        if summary.is_empty() {
            "(none)"
        } else {
            summary
        }
    )
}

fn implementation_pr_body_from_sanitized_prose(prose: &str, metadata: &WorkflowMetadata) -> String {
    let prose = prose.trim();
    if prose.is_empty() {
        render_metadata_block(metadata)
    } else {
        format!("{}\n\n{}", prose, render_metadata_block(metadata))
    }
}

/// Removes every real Temper-managed region at the authored-report boundary.
///
/// Structural inspection intentionally ignores inline and fenced examples and
/// identifies complete blocks without parsing their JSON. An unterminated real
/// block owns the remainder of the authored input; temporarily closing it lets
/// the same inspection API identify that final span without maintaining a
/// second Markdown parser here.
fn sanitize_authored_report(authored: &str) -> String {
    let original_len = authored.len();
    let inspection = match inspect_metadata_blocks(authored) {
        Ok(inspection) => inspection,
        Err(MetadataError::Unterminated) => {
            let mut closed = String::with_capacity(original_len + METADATA_END.len() + 1);
            closed.push_str(authored);
            closed.push('\n');
            closed.push_str(METADATA_END);
            let Ok(inspection) = inspect_metadata_blocks(&closed) else {
                // Inspection errors are authority-boundary failures. Dropping
                // authored input is safer than publishing a managed region.
                return String::new();
            };
            inspection
        }
        Err(_) => {
            // Complete malformed and duplicate blocks are successful structural
            // inspections. Any other error must fail closed at this boundary.
            return String::new();
        }
    };

    let mut sanitized = authored.to_string();
    for span in inspection.blocks().iter().rev() {
        if span.start() < original_len {
            sanitized.replace_range(span.start()..span.end().min(original_len), "");
        }
    }
    sanitized
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
    use temper_workflow::{
        DurableAssignment, Lease, RoleId, inspect_metadata_blocks, parse_metadata_block,
    };

    fn trusted_metadata() -> WorkflowMetadata {
        WorkflowMetadata {
            kind: Some(ArtifactKindId::new("implementation_pr")),
            correlation_key: Some("trusted-correlation".to_string()),
            parents: vec![ArtifactRef::same_repo(ItemNumber::new(120))],
            ..WorkflowMetadata::default()
        }
    }

    fn stale_metadata() -> WorkflowMetadata {
        WorkflowMetadata {
            kind: Some(ArtifactKindId::new("stale-kind")),
            correlation_key: Some("stale-correlation-id".to_string()),
            repaired_head: Some("stale-repaired-head".to_string()),
            parents: vec![ArtifactRef::same_repo(ItemNumber::new(999))],
            lease: Some(Lease {
                role: RoleId::new("engineer"),
                worker: "stale-lease-worker".to_string(),
                claimed_at: "2026-07-23T00:00:00Z".parse().expect("timestamp"),
                heartbeat_at: "2026-07-23T00:01:00Z".parse().expect("timestamp"),
                expires_at: "2026-07-23T00:30:00Z".parse().expect("timestamp"),
            }),
            assignment: Some(DurableAssignment {
                job_id: Some("stale-job-id".to_string()),
                attempt_id: Some("stale-attempt-id".to_string()),
                daemon_boot_id: Some("stale-daemon-boot-id".to_string()),
                ..DurableAssignment::default()
            }),
            ..WorkflowMetadata::default()
        }
    }

    fn assert_only_trusted_metadata(body: &str, trusted: &WorkflowMetadata) {
        let inspection = inspect_metadata_blocks(body).expect("composed body is inspectable");
        assert_eq!(inspection.block_count(), 1);
        assert_eq!(
            parse_metadata_block(body).expect("composed metadata parses"),
            Some(trusted.clone())
        );
        for stale in [
            "stale-kind",
            "stale-correlation-id",
            "stale-repaired-head",
            "stale-lease-worker",
            "stale-job-id",
            "stale-attempt-id",
            "stale-daemon-boot-id",
            "\"number\": 999",
        ] {
            assert!(!body.contains(stale), "stale authority leaked: {stale}");
        }
    }

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

    #[test]
    fn authored_report_removes_duplicate_blocks_before_and_after_prose() {
        let trusted = trusted_metadata();
        let stale_block = render_metadata_block(&stale_metadata());
        let report =
            format!("{stale_block}\n\nAuthored prose stays byte-for-byte.\n\n{stale_block}");

        let body = implementation_pr_report_body(&report, &trusted);

        assert_eq!(
            body,
            format!(
                "Authored prose stays byte-for-byte.\n\n{}",
                render_metadata_block(&trusted)
            )
        );
        assert_only_trusted_metadata(&body, &trusted);
    }

    #[test]
    fn authored_report_removes_malformed_complete_block_without_reformatting_prose() {
        let trusted = trusted_metadata();
        let malformed = format!(
            "{}\n{{ definitely not JSON: stale-attempt-id }}\n{}",
            temper_workflow::METADATA_BEGIN,
            temper_workflow::METADATA_END
        );
        let report = format!("Before.\n\n{malformed}\n\nAfter.");

        let body = implementation_pr_report_body(&report, &trusted);

        assert_eq!(
            body,
            format!(
                "Before.\n\n\n\nAfter.\n\n{}",
                render_metadata_block(&trusted)
            )
        );
        assert_only_trusted_metadata(&body, &trusted);
    }

    #[test]
    fn authored_report_drops_unterminated_region_through_end_of_input() {
        let trusted = trusted_metadata();
        let report = format!(
            "Kept before.\n\n{}\n{{\"attempt_id\":\"stale-attempt-id\"}}\nprose inside the unterminated region",
            temper_workflow::METADATA_BEGIN
        );

        let body = implementation_pr_report_body(&report, &trusted);

        assert_eq!(
            body,
            format!("Kept before.\n\n{}", render_metadata_block(&trusted))
        );
        assert!(!body.contains("prose inside the unterminated region"));
        assert_only_trusted_metadata(&body, &trusted);
    }

    #[test]
    fn authored_report_preserves_inline_and_fenced_examples_byte_for_byte() {
        let trusted = trusted_metadata();
        let examples = format!(
            "Inline example: `{} {{}} {}`.\n\n```text\n{}\n{{}}\n{}\n```",
            temper_workflow::METADATA_BEGIN,
            temper_workflow::METADATA_END,
            temper_workflow::METADATA_BEGIN,
            temper_workflow::METADATA_END
        );

        let body = implementation_pr_report_body(&examples, &trusted);

        assert_eq!(&body[..examples.len()], examples);
        assert_eq!(
            body,
            format!("{examples}\n\n{}", render_metadata_block(&trusted))
        );
        assert_only_trusted_metadata(&body, &trusted);
    }

    #[test]
    fn metadata_only_report_uses_sanitized_summary_fallback() {
        let trusted = trusted_metadata();
        let stale_block = render_metadata_block(&stale_metadata());
        let fallback_summary = format!("Kept summary.\n\n{stale_block}\n\nSummary tail stays.");

        let body = implementation_pr_body_from_report_or_summary(
            Some(&stale_block),
            "Fallback intro",
            &fallback_summary,
            &trusted,
        );

        assert_eq!(
            body,
            format!(
                "Fallback intro\n\nSummary: Kept summary.\n\n\n\nSummary tail stays.\n\n{}",
                render_metadata_block(&trusted)
            )
        );
        assert_only_trusted_metadata(&body, &trusted);
    }
}
