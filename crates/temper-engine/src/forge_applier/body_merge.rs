// SPDX-License-Identifier: MPL-2.0

//! Pull-request body merging for implementation PR finalization.

use temper_workflow::{
    MetadataError, WorkflowMetadata, render_metadata_block, split_metadata_block,
};

pub(super) fn merge_implementation_pr_body(
    current: &str,
    desired: &str,
) -> Result<Option<String>, MetadataError> {
    let (desired_prose, desired_metadata) = split_metadata_block(desired)?;
    let (_, current_metadata) = split_metadata_block(current)?;
    let metadata = current_metadata.or(desired_metadata).unwrap_or_default();

    let updated = join_body(&desired_prose, &metadata);
    if updated == current {
        Ok(None)
    } else {
        Ok(Some(updated))
    }
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
        ArtifactKindId, WorkflowMetadata, parse_metadata_block, render_metadata_block,
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
        let desired = format!(
            "# Implementation report\n\nImplemented the final fix.\n\n{}",
            render_metadata_block(&metadata())
        );

        let updated = merge_implementation_pr_body(&current, &desired)
            .expect("merge succeeds")
            .expect("body changes");

        assert!(updated.contains("# Implementation report"));
        assert!(updated.contains("Implemented the final fix."));
        assert!(!updated.contains("Old report"));
        assert!(!updated.contains("Implementation plan"));
        assert_eq!(parse_metadata_block(&updated).unwrap(), Some(metadata()));
    }

    #[test]
    fn canonical_split_preserves_examples_before_desired_metadata() {
        let current = format!("Old report.\n\n{}", render_metadata_block(&metadata()));
        let desired = format!(
            "Inline example: `{}`.\n\n# Implementation report\n\nNew report.\n\n{}",
            temper_workflow::METADATA_BEGIN,
            render_metadata_block(&metadata())
        );

        let updated = merge_implementation_pr_body(&current, &desired)
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
    fn keeps_current_metadata_when_desired_metadata_differs() {
        let current_metadata = metadata();
        let desired_metadata = WorkflowMetadata {
            kind: Some(ArtifactKindId::new("implementation_pr")),
            correlation_key: Some("wrong-key".to_string()),
            ..WorkflowMetadata::default()
        };
        let current = format!("Old.\n\n{}", render_metadata_block(&current_metadata));
        let desired = format!("New.\n\n{}", render_metadata_block(&desired_metadata));

        let updated = merge_implementation_pr_body(&current, &desired)
            .expect("merge succeeds")
            .expect("body changes");

        assert!(updated.starts_with("New."));
        assert_eq!(
            parse_metadata_block(&updated).unwrap(),
            Some(current_metadata)
        );
    }
}
