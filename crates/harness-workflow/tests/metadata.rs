//! Tests for workflow metadata block render/parse (Phase 3).

use chrono::{DateTime, Utc};
use harness_forge::{ItemNumber, RepositoryId};
use harness_workflow::{
    parse_metadata_block, render_metadata_block, ArtifactKindId, ArtifactRef, Lease, MetadataError,
    RoleId, WorkflowMetadata,
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
        lease: Some(Lease {
            role: RoleId::new("engineer"),
            worker: "run-abc".to_string(),
            claimed_at: ts("2026-05-29T00:00:00Z"),
            heartbeat_at: ts("2026-05-29T00:05:00Z"),
            expires_at: ts("2026-05-29T00:30:00Z"),
        }),
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
    let body = "<!-- harness:workflow\n{ not valid json }\n-->";
    let error = parse_metadata_block(body).expect_err("invalid json must fail");
    assert!(matches!(error, MetadataError::InvalidJson(_)));
}

#[test]
fn unterminated_metadata_block_is_reported() {
    let body = "<!-- harness:workflow\n{}";
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
