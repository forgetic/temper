//! Tests for workflow metadata block render/parse (Phase 3).

use chrono::{DateTime, Utc};
use temper_forge::{ItemNumber, RepositoryId};
use temper_workflow::{
    ArtifactKindId, ArtifactRef, DurableAssignment, Lease, MetadataError, RoleId, WorkflowMetadata,
    global_child_correlation_key, parse_metadata_block, render_metadata_block,
};

fn ts(value: &str) -> DateTime<Utc> {
    value.parse().expect("valid RFC 3339 timestamp")
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
    let body = "<!-- temper:workflow\n{ not valid json }\n-->";
    let error = parse_metadata_block(body).expect_err("invalid json must fail");
    assert!(matches!(error, MetadataError::InvalidJson(_)));
}

#[test]
fn unterminated_metadata_block_is_reported() {
    let body = "<!-- temper:workflow\n{}";
    assert_eq!(parse_metadata_block(body), Err(MetadataError::Unterminated));
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
fn legacy_metadata_and_optional_assignment_fields_are_compatible() {
    let legacy = r#"<!-- temper:workflow
{"kind":"code","lease":{"role":"engineer","worker":"old","claimed_at":"2026-05-29T00:00:00Z","heartbeat_at":"2026-05-29T00:00:00Z","expires_at":"2026-05-29T00:30:00Z"}}
-->"#;
    let parsed = parse_metadata_block(legacy).unwrap().unwrap();
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
