//! Tests for workflow metadata block render/parse (Phase 3).

use chrono::{DateTime, Utc};
use temper_forge::{ItemNumber, RepositoryId};
use temper_workflow::{
    ArtifactKindId, ArtifactRef, DurableAssignment, Lease, MetadataError, RoleId, WorkflowMetadata,
    global_child_correlation_key, is_heartbeat_only_body_change, parse_metadata_block,
    render_metadata_block, replace_metadata_block, split_metadata_block,
};

fn ts(value: &str) -> DateTime<Utc> {
    value.parse().expect("valid RFC 3339 timestamp")
}

fn metadata_fixture(json: &str) -> String {
    format!(
        "{}\n{json}\n{}",
        temper_workflow::METADATA_BEGIN,
        temper_workflow::METADATA_END
    )
}

fn authored_metadata_examples() -> String {
    format!(
        "Inline example: `{} {{}} {}`.\n\n```text\n{}\n{{}}\n{}\n```\n",
        temper_workflow::METADATA_BEGIN,
        temper_workflow::METADATA_END,
        temper_workflow::METADATA_BEGIN,
        temper_workflow::METADATA_END
    )
}

fn full_metadata() -> WorkflowMetadata {
    WorkflowMetadata {
        kind: Some(ArtifactKindId::new("code")),
        parents: vec![ArtifactRef::same_repo(ItemNumber::new(12))],
        dependencies: vec![
            ArtifactRef::same_repo(ItemNumber::new(34)),
            ArtifactRef::same_repo(ItemNumber::new(56)),
        ],
        correlation_key: Some("code-issue-42".to_string()),
        target_branch: Some("feature/144-plan-branch".to_string()),
        lease: Some(Lease {
            role: RoleId::new("engineer"),
            worker: "run-abc".to_string(),
            claimed_at: ts("2026-05-29T00:00:00Z"),
            heartbeat_at: ts("2026-05-29T00:05:00Z"),
            expires_at: ts("2026-05-29T00:30:00Z"),
        }),
        assignment: None,
        ..WorkflowMetadata::default()
    }
}

#[test]
fn full_metadata_round_trips_through_render_and_parse() {
    let metadata = full_metadata();
    let rendered = render_metadata_block(&metadata);
    let parsed = parse_metadata_block(&rendered)
        .expect("renders to parseable block")
        .expect("block is present");
    assert_eq!(parsed, metadata);
}

#[test]
fn render_is_deterministic() {
    let metadata = full_metadata();
    assert_eq!(
        render_metadata_block(&metadata),
        render_metadata_block(&metadata)
    );
}

#[test]
fn repo_qualified_metadata_projection_round_trips() {
    let metadata = WorkflowMetadata {
        parents: vec![ArtifactRef::in_repo(
            RepositoryId::new("repo-service"),
            ItemNumber::new(12),
        )],
        dependencies: vec![ArtifactRef::same_repo(ItemNumber::new(34))],
        ..WorkflowMetadata::default()
    };

    let rendered = render_metadata_block(&metadata);
    assert!(rendered.contains("\"repository_id\": \"repo-service\""));
    let parsed = parse_metadata_block(&rendered)
        .expect("renders to parseable block")
        .expect("block is present");
    assert_eq!(parsed, metadata);
}

#[test]
fn target_branch_metadata_round_trips() {
    let metadata = WorkflowMetadata {
        target_branch: Some("feature/144-plan-branch".to_string()),
        ..WorkflowMetadata::default()
    };

    let rendered = render_metadata_block(&metadata);
    assert!(rendered.contains("\"target_branch\": \"feature/144-plan-branch\""));
    let parsed = parse_metadata_block(&rendered)
        .expect("renders to parseable block")
        .expect("block is present");
    assert_eq!(parsed, metadata);
}

#[test]
fn global_child_correlation_key_is_stable_and_delimiter_safe() {
    let repo = RepositoryId::new("forgejo:acme/service#one");
    let first = global_child_correlation_key(&repo, ItemNumber::new(7), "api/client");
    let second = global_child_correlation_key(&repo, ItemNumber::new(7), "api/client");
    let different_repo = global_child_correlation_key(
        &RepositoryId::new("forgejo:acme/service"),
        ItemNumber::new(7),
        "api/client",
    );
    let different_slug = global_child_correlation_key(&repo, ItemNumber::new(7), "api#client");

    assert_eq!(first, second);
    assert_ne!(first, different_repo);
    assert_ne!(first, different_slug);
    assert_eq!(
        first,
        "parent-repo:24:forgejo:acme/service#one#parent:7/child:10:api/client"
    );
}

#[test]
fn empty_metadata_round_trips() {
    let metadata = WorkflowMetadata::default();
    assert!(metadata.is_empty());
    let rendered = render_metadata_block(&metadata);
    let parsed = parse_metadata_block(&rendered)
        .expect("renders to parseable block")
        .expect("block is present");
    assert_eq!(parsed, metadata);
    assert!(parsed.is_empty());
}

#[test]
fn metadata_block_is_found_within_surrounding_prose() {
    let metadata = WorkflowMetadata {
        correlation_key: Some("key".to_string()),
        ..WorkflowMetadata::default()
    };
    let body = format!(
        "Human-facing summary.\n\n{}\n\nMore prose below.",
        render_metadata_block(&metadata)
    );
    let parsed = parse_metadata_block(&body)
        .expect("block parses among prose")
        .expect("block is present");
    assert_eq!(parsed.correlation_key.as_deref(), Some("key"));
}

#[test]
fn missing_metadata_block_returns_none() {
    assert_eq!(
        parse_metadata_block("a body with no workflow metadata"),
        Ok(None)
    );
}

#[test]
fn malformed_metadata_json_is_reported() {
    let body = metadata_fixture("{ not valid json }");
    let error = parse_metadata_block(&body).expect_err("invalid json must fail");
    assert!(matches!(error, MetadataError::InvalidJson(_)));
}

#[test]
fn unterminated_metadata_block_is_reported() {
    let body = format!("{}\n{{}}", temper_workflow::METADATA_BEGIN);
    assert_eq!(
        parse_metadata_block(&body),
        Err(MetadataError::Unterminated)
    );
}

#[test]
fn split_preserves_exact_prefix_and_suffix_bytes() {
    let metadata = WorkflowMetadata {
        correlation_key: Some("split-key".to_string()),
        ..WorkflowMetadata::default()
    };
    let prefix = "\u{feff}Authored\r\n\r\n<!-- ordinary comment -->\n";
    let suffix = "\n  trailing spaces stay  \r\n";
    let body = format!("{prefix}{}{suffix}", render_metadata_block(&metadata));

    let (authored, parsed) = split_metadata_block(&body).expect("valid block splits");

    assert_eq!(authored.as_bytes(), format!("{prefix}{suffix}").as_bytes());
    assert_eq!(parsed, Some(metadata));
}

#[test]
fn split_without_managed_metadata_returns_body_unchanged() {
    let body = format!(
        "Authored.\n<!-- ordinary comment -->\n{}-example\n<!-- workflow:temper -->\n",
        temper_workflow::METADATA_BEGIN
    );

    let (authored, metadata) = split_metadata_block(&body).expect("no managed block");

    assert_eq!(authored.as_bytes(), body.as_bytes());
    assert_eq!(metadata, None);
    assert_eq!(parse_metadata_block(&body), Ok(None));
}

#[test]
fn inline_and_fenced_examples_remain_authored_visible() {
    let body = authored_metadata_examples();

    let (authored, metadata) = split_metadata_block(&body).expect("examples are not managed");

    assert_eq!(authored.as_bytes(), body.as_bytes());
    assert_eq!(metadata, None);
    assert_eq!(parse_metadata_block(&body), Ok(None));
}

#[test]
fn examples_before_real_metadata_are_preserved_and_real_block_is_split() {
    let examples = authored_metadata_examples();
    let metadata = WorkflowMetadata {
        kind: Some(ArtifactKindId::new("code")),
        correlation_key: Some("real-block".to_string()),
        ..WorkflowMetadata::default()
    };
    let body = format!(
        "{examples}\nProse before.\n{}\nProse after.\n",
        render_metadata_block(&metadata)
    );
    let expected_authored = format!("{examples}\nProse before.\n\nProse after.\n");

    let (authored, parsed) = split_metadata_block(&body).expect("real block splits");

    assert_eq!(authored.as_bytes(), expected_authored.as_bytes());
    assert_eq!(parsed, Some(metadata));
}

#[test]
fn replacement_ignores_examples_and_replaces_the_real_block() {
    let examples = authored_metadata_examples();
    let original = WorkflowMetadata {
        correlation_key: Some("old".to_string()),
        ..WorkflowMetadata::default()
    };
    let replacement = WorkflowMetadata {
        correlation_key: Some("new".to_string()),
        ..WorkflowMetadata::default()
    };
    let body = format!(
        "{examples}\nBefore.\n{}\nAfter.",
        render_metadata_block(&original)
    );

    let replaced = replace_metadata_block(&body, &replacement).expect("replacement succeeds");
    let (authored, parsed) = split_metadata_block(&replaced).expect("replacement stays valid");

    assert_eq!(authored, format!("{examples}\nBefore.\n\nAfter."));
    assert_eq!(parsed, Some(replacement));
}

#[test]
fn malformed_and_unterminated_real_blocks_are_never_removed_or_replaced() {
    let malformed = metadata_fixture("{ not valid json }");
    let unterminated = format!("{}\n{{}}", temper_workflow::METADATA_BEGIN);
    let replacement = WorkflowMetadata::default();

    assert!(matches!(
        split_metadata_block(&malformed),
        Err(MetadataError::InvalidJson(_))
    ));
    assert!(matches!(
        replace_metadata_block(&malformed, &replacement),
        Err(MetadataError::InvalidJson(_))
    ));
    assert_eq!(
        split_metadata_block(&unterminated),
        Err(MetadataError::Unterminated)
    );
    assert_eq!(
        replace_metadata_block(&unterminated, &replacement),
        Err(MetadataError::Unterminated)
    );
}

#[test]
fn lease_expiry_is_detected() {
    let lease = Lease {
        role: RoleId::new("engineer"),
        worker: "run-abc".to_string(),
        claimed_at: ts("2026-05-29T00:00:00Z"),
        heartbeat_at: ts("2026-05-29T00:00:00Z"),
        expires_at: ts("2026-05-29T01:00:00Z"),
    };
    assert!(!lease.is_expired(ts("2026-05-29T00:30:00Z")));
    assert!(lease.is_expired(ts("2026-05-29T01:00:00Z")));
    assert!(lease.is_expired(ts("2026-05-29T02:00:00Z")));
}

#[test]
fn repaired_head_marker_round_trips_and_legacy_metadata_defaults_to_none() {
    let legacy = metadata_fixture(r#"{"kind":"implementation_pr"}"#);
    assert!(
        parse_metadata_block(&legacy)
            .unwrap()
            .unwrap()
            .repaired_head
            .is_none()
    );

    let metadata = WorkflowMetadata {
        kind: Some(ArtifactKindId::new("implementation_pr")),
        repaired_head: Some("repaired-sha".to_string()),
        ..WorkflowMetadata::default()
    };
    let reparsed = parse_metadata_block(&render_metadata_block(&metadata))
        .unwrap()
        .unwrap();
    assert_eq!(reparsed, metadata);
}

#[test]
fn legacy_metadata_and_optional_assignment_fields_are_compatible() {
    let legacy = metadata_fixture(
        r#"{"kind":"code","lease":{"role":"engineer","worker":"old","claimed_at":"2026-05-29T00:00:00Z","heartbeat_at":"2026-05-29T00:00:00Z","expires_at":"2026-05-29T00:30:00Z"}}"#,
    );
    let parsed = parse_metadata_block(&legacy).unwrap().unwrap();
    assert!(parsed.assignment.is_none());

    let metadata = WorkflowMetadata {
        assignment: Some(DurableAssignment {
            job_id: Some("job-257".to_string()),
            daemon_boot_id: Some("boot-a".to_string()),
            ..DurableAssignment::default()
        }),
        ..WorkflowMetadata::default()
    };
    let reparsed = parse_metadata_block(&render_metadata_block(&metadata))
        .unwrap()
        .unwrap();
    assert_eq!(reparsed, metadata);
}

#[test]
fn heartbeat_only_body_change_requires_exact_structural_delta() {
    let mut old = full_metadata();
    old.assignment = Some(DurableAssignment {
        job_id: Some("job-257".to_string()),
        expires_at: Some(ts("2026-05-29T00:30:00Z")),
        ..DurableAssignment::default()
    });
    let old_body = format!("Human prose.\n\n{}", render_metadata_block(&old));

    let mut heartbeat = old.clone();
    heartbeat.lease.as_mut().unwrap().heartbeat_at = ts("2026-05-29T00:06:00Z");
    heartbeat.lease.as_mut().unwrap().expires_at = ts("2026-05-29T00:31:00Z");
    heartbeat.assignment.as_mut().unwrap().expires_at = Some(ts("2026-05-29T00:31:00Z"));
    let heartbeat_body = format!("Human prose.\n\n{}", render_metadata_block(&heartbeat));
    assert!(is_heartbeat_only_body_change(&old_body, &heartbeat_body));

    let mut extra_metadata = heartbeat.clone();
    extra_metadata.target_branch = Some("feature/changed".to_string());
    assert!(!is_heartbeat_only_body_change(
        &old_body,
        &format!("Human prose.\n\n{}", render_metadata_block(&extra_metadata))
    ));
    assert!(!is_heartbeat_only_body_change(
        &old_body,
        &format!("Edited prose.\n\n{}", render_metadata_block(&heartbeat))
    ));
    assert!(!is_heartbeat_only_body_change(&old_body, &old_body));
    assert!(!is_heartbeat_only_body_change(
        "no metadata",
        &heartbeat_body
    ));
    assert!(!is_heartbeat_only_body_change(
        &metadata_fixture("{bad}"),
        &heartbeat_body
    ));
}
