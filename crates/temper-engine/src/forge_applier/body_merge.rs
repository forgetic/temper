// SPDX-License-Identifier: MPL-2.0

//! Pull-request body merging for implementation PR finalization.

use std::error::Error;
use std::fmt;

use temper_workflow::{
    MetadataError, WorkflowMetadata, inspect_metadata_blocks, parse_metadata_block,
    render_metadata_block,
};

/// Authored prose and the sole authoritative metadata record derived from one
/// Forge snapshot.
pub(super) struct CanonicalSnapshotBody {
    pub(super) prose: String,
    pub(super) metadata: Option<WorkflowMetadata>,
    pub(super) block_count: usize,
}

#[derive(Debug)]
pub(super) enum BodyMergeError {
    Metadata(MetadataError),
    MissingSnapshotMetadata,
}

impl fmt::Display for BodyMergeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Metadata(error) => error.fmt(formatter),
            Self::MissingSnapshotMetadata => formatter.write_str(
                "fresh pull-request snapshot has no canonical workflow metadata block; refusing to source workflow identity from handoff prose",
            ),
        }
    }
}

impl Error for BodyMergeError {}

impl From<MetadataError> for BodyMergeError {
    fn from(error: MetadataError) -> Self {
        Self::Metadata(error)
    }
}

/// Extracts authority from the first complete real block in a fresh snapshot
/// and removes every real block from the returned prose.
///
/// Later complete blocks are deliberately not parsed: once the first record is
/// known to be valid, they are duplicate managed state and are removed by the
/// next CAS write. A malformed or unterminated first record fails closed.
pub(super) fn canonical_snapshot_body(body: &str) -> Result<CanonicalSnapshotBody, MetadataError> {
    let inspection = inspect_metadata_blocks(body)?;
    let metadata = inspection
        .blocks()
        .first()
        .map(|span| {
            parse_metadata_block(&body[span.start()..span.end()])?.ok_or_else(|| {
                MetadataError::InvalidJson(
                    "managed metadata span did not contain a parseable block".to_string(),
                )
            })
        })
        .transpose()?;
    let prose = remove_blocks(body, inspection.blocks());
    Ok(CanonicalSnapshotBody {
        prose,
        metadata,
        block_count: inspection.block_count(),
    })
}

pub(super) fn body_with_canonical_metadata(
    snapshot: &CanonicalSnapshotBody,
    metadata: &WorkflowMetadata,
) -> String {
    join_body(&snapshot.prose, metadata)
}

pub(super) fn merge_implementation_pr_body(
    current: &str,
    desired_prose: &str,
    fallback_metadata: Option<&WorkflowMetadata>,
) -> Result<Option<String>, BodyMergeError> {
    // This is a second authority boundary in addition to runner sanitization.
    // Complete real blocks are stripped structurally and never parsed as a
    // metadata source; inline and fenced examples remain authored prose.
    let desired_prose = authored_prose(desired_prose)?;
    let snapshot = canonical_snapshot_body(current)?;
    let metadata = snapshot
        .metadata
        .as_ref()
        .or(fallback_metadata)
        .ok_or(BodyMergeError::MissingSnapshotMetadata)?;

    let updated = join_body(&desired_prose, metadata);
    if updated == current {
        Ok(None)
    } else {
        Ok(Some(updated))
    }
}

fn authored_prose(body: &str) -> Result<String, MetadataError> {
    let inspection = inspect_metadata_blocks(body)?;
    Ok(remove_blocks(body, inspection.blocks()))
}

fn remove_blocks(body: &str, blocks: &[temper_workflow::MetadataBlockSpan]) -> String {
    let mut prose = body.to_string();
    for span in blocks.iter().rev() {
        prose.replace_range(span.start()..span.end(), "");
    }
    prose
}

fn join_body(prose: &str, metadata: &WorkflowMetadata) -> String {
    let block = render_metadata_block(metadata);
    if prose.trim().is_empty() {
        block
    } else {
        format!("{}\n\n{block}", prose.trim_end())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use temper_workflow::{
        ArtifactKindId, DurableAssignment, RoleId, WorkflowMetadata, inspect_metadata_blocks,
        parse_metadata_block, render_metadata_block,
    };

    fn metadata() -> WorkflowMetadata {
        WorkflowMetadata {
            kind: Some(ArtifactKindId::new("implementation_pr")),
            correlation_key: Some("pr-for-code-7".to_string()),
            ..WorkflowMetadata::default()
        }
    }

    #[test]
    fn replaces_legacy_report_with_desired_report_and_preserves_metadata() {
        let current = format!(
            "Old report.\n\nSummary: planned\n\nImplementation plan:\n\n- [ ] Test\n\n{}",
            render_metadata_block(&metadata())
        );

        let updated = merge_implementation_pr_body(
            &current,
            "# Implementation report\n\nImplemented the final fix.",
            None,
        )
        .expect("merge succeeds")
        .expect("body changes");

        assert!(updated.contains("# Implementation report"));
        assert!(updated.contains("Implemented the final fix."));
        assert!(!updated.contains("Old report"));
        assert!(!updated.contains("Implementation plan"));
        assert_eq!(parse_metadata_block(&updated).unwrap(), Some(metadata()));
    }

    #[test]
    fn canonical_split_preserves_authored_examples() {
        let current = format!("Old report.\n\n{}", render_metadata_block(&metadata()));
        let desired_prose = format!(
            "Inline example: `{}`.\n\n# Implementation report\n\nNew report.",
            temper_workflow::METADATA_BEGIN,
        );

        let updated = merge_implementation_pr_body(&current, &desired_prose, None)
            .expect("merge succeeds")
            .expect("body changes");

        assert!(updated.contains(&format!(
            "Inline example: `{}`.",
            temper_workflow::METADATA_BEGIN
        )));
        assert!(updated.contains("New report."));
        assert_eq!(parse_metadata_block(&updated).unwrap(), Some(metadata()));
    }

    #[test]
    fn uses_fallback_metadata_only_when_snapshot_has_no_record() {
        let fallback = metadata();
        let updated = merge_implementation_pr_body("Old.", "New.", Some(&fallback))
            .expect("merge succeeds")
            .expect("body changes");

        assert!(updated.starts_with("New."));
        assert_eq!(parse_metadata_block(&updated).unwrap(), Some(fallback));
    }

    #[test]
    fn repair_merge_fails_closed_without_snapshot_metadata() {
        let stale_result = format!("Report.\n\n{}", render_metadata_block(&metadata()));

        let error = merge_implementation_pr_body("Legacy PR body.", &stale_result, None)
            .expect_err("result-supplied metadata is not a fallback authority");

        assert!(matches!(error, BodyMergeError::MissingSnapshotMetadata));
    }

    #[test]
    fn removes_result_supplied_real_blocks_without_reading_their_authority() {
        let current_metadata = metadata();
        let stale_metadata = WorkflowMetadata {
            kind: Some(ArtifactKindId::new("stale-kind")),
            correlation_key: Some("stale-correlation".to_string()),
            repaired_head: Some("stale-head".to_string()),
            assignment: Some(DurableAssignment {
                role: Some(RoleId::new("stale-owner")),
                ..DurableAssignment::default()
            }),
            ..WorkflowMetadata::default()
        };
        let current = format!("Old.\n\n{}", render_metadata_block(&current_metadata));
        let desired = format!(
            "New report.\n\n{}\n\nTrailing prose.",
            render_metadata_block(&stale_metadata)
        );

        let updated = merge_implementation_pr_body(&current, &desired, None)
            .expect("merge succeeds")
            .expect("body changes");

        assert!(updated.contains("New report."));
        assert!(updated.contains("Trailing prose."));
        assert!(!updated.contains("stale-owner"));
        assert_eq!(
            parse_metadata_block(&updated).unwrap(),
            Some(current_metadata)
        );
    }

    #[test]
    fn cleans_complete_duplicate_snapshot_blocks_using_first_record() {
        let canonical = metadata();
        let duplicate = WorkflowMetadata {
            correlation_key: Some("stale-correlation".to_string()),
            ..WorkflowMetadata::default()
        };
        let current = format!(
            "Old.\n\n{}\n\nStale separator.\n\n{}",
            render_metadata_block(&canonical),
            render_metadata_block(&duplicate)
        );

        let updated = merge_implementation_pr_body(&current, "New.", None)
            .expect("first snapshot record is canonical")
            .expect("duplicates are cleaned");

        assert_eq!(inspect_metadata_blocks(&updated).unwrap().block_count(), 1);
        assert_eq!(parse_metadata_block(&updated).unwrap(), Some(canonical));
        assert!(!updated.contains("stale-correlation"));
    }

    #[test]
    fn malformed_first_snapshot_record_fails_closed() {
        let current = format!(
            "Old.\n\n{}\nnot-json\n{}\n\n{}",
            temper_workflow::METADATA_BEGIN,
            temper_workflow::METADATA_END,
            render_metadata_block(&metadata())
        );

        let error = merge_implementation_pr_body(&current, "New.", None)
            .expect_err("malformed first record is not safe authority");

        assert!(matches!(
            error,
            BodyMergeError::Metadata(MetadataError::InvalidJson(_))
        ));
    }

    #[test]
    fn retry_merge_recomputes_metadata_from_reloaded_snapshot() {
        let initial = WorkflowMetadata {
            repaired_head: Some("head-before-conflict".to_string()),
            ..metadata()
        };
        let reloaded = WorkflowMetadata {
            repaired_head: Some("head-after-conflict".to_string()),
            assignment: Some(DurableAssignment {
                job_id: Some("new-assignment".to_string()),
                ..DurableAssignment::default()
            }),
            ..metadata()
        };
        let initial_body = format!("Old.\n\n{}", render_metadata_block(&initial));
        let reloaded_body = format!("Concurrent edit.\n\n{}", render_metadata_block(&reloaded));

        let first_attempt = merge_implementation_pr_body(&initial_body, "Repair report.", None)
            .unwrap()
            .unwrap();
        let retry_attempt = merge_implementation_pr_body(&reloaded_body, "Repair report.", None)
            .unwrap()
            .unwrap();

        assert_eq!(parse_metadata_block(&first_attempt).unwrap(), Some(initial));
        assert_eq!(
            parse_metadata_block(&retry_attempt).unwrap(),
            Some(reloaded)
        );
    }
}
