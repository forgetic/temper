// SPDX-License-Identifier: MPL-2.0

use super::*;
use std::sync::Arc;
use temper_forge::{CreateIssue, CreateRepository, ItemNumber, UpdateIssue, UserId};
use temper_forge_memory::MemoryForge;
use temper_protocol_worker::JobContext;
use temper_workflow::{
    ArtifactKindId, Lease, RawWorkflowSpec, RoleId, WorkflowMetadata, render_metadata_block,
};

#[test]
fn startup_quarantine_records_one_idempotent_audit_comment() {
    temper_engine_io::block_on(async move {
        let forge = MemoryForge::new();
        let repo = forge
            .create_repository(CreateRepository {
                owner: "ai".to_string(),
                name: "temper".to_string(),
                default_branch: "main".to_string(),
                description: None,
            })
            .await
            .expect("repository is created")
            .id;
        let issue = forge
            .create_issue(
                &repo,
                CreateIssue {
                    title: "Malformed recovery assignment".to_string(),
                    body: "<!-- temper:workflow\n{not-json}\n-->".to_string(),
                    labels: vec!["code".to_string(), "in-progress".to_string()],
                    assignees: Vec::new(),
                },
            )
            .await
            .expect("issue is created");
        let target = ArtifactSource::Issue {
            number: issue.number,
        };

        let workflow = temper_workflow::parse_workflow_spec(
            "reference-delivery.json",
            include_str!("../../../temper-workflow/fixtures/reference-delivery.json"),
        )
        .expect("workflow parses");
        let workflow = workflow.validate().expect("workflow validates");
        let converger = AssignmentConverger::new(
            &workflow,
            &forge,
            LeasePolicy::new(chrono::Duration::minutes(5)),
        );
        converger
            .quarantine_target(&repo, target, "malformed assignment")
            .await
            .expect("first quarantine succeeds");
        converger
            .quarantine_target(&repo, target, "malformed assignment")
            .await
            .expect("replayed quarantine succeeds");

        let issue = forge
            .get_issue_by_number(&repo, issue.number)
            .await
            .unwrap()
            .unwrap();
        assert!(issue.labels.contains(&"needs-human".to_string()));
        let comments = forge.list_issue_comments(&issue.id).await.unwrap();
        assert_eq!(comments.len(), 1);
        assert!(
            comments[0]
                .body
                .contains(temper_workflow::ASSIGNMENT_RECOVERY_AUDIT_MARKER)
        );
    });
}

#[test]
fn missing_pull_collection_is_empty_during_startup_inventory() {
    let repo = RepositoryId::new("forgejo:acme/empty");
    let result = startup_pull_inventory(
        &repo,
        Err(ForgeError::NotFound(
            "pull collection unavailable".to_string(),
        )),
    )
    .expect("an absent PR collection is empty for an existing repository");

    assert!(result.is_empty());
}

const REFERENCE_DELIVERY_FIXTURE: &str =
    include_str!("../../../temper-workflow/fixtures/reference-delivery.json");

fn timestamp(value: &str) -> chrono::DateTime<Utc> {
    value.parse().expect("valid timestamp")
}

fn reference_workflow() -> (ValidatedWorkflow, CompiledWorkflow) {
    let raw: RawWorkflowSpec =
        serde_json::from_str(REFERENCE_DELIVERY_FIXTURE).expect("reference workflow parses");
    let workflow = raw.validate().expect("reference workflow validates");
    let compiled = workflow.compile();
    (workflow, compiled)
}

fn ambiguous_workflow() -> (ValidatedWorkflow, CompiledWorkflow) {
    let mut value: serde_json::Value =
        serde_json::from_str(REFERENCE_DELIVERY_FIXTURE).expect("reference workflow parses");
    value["labels"]
        .as_array_mut()
        .expect("labels are an array")
        .extend([
            serde_json::json!({"id": "variant-a"}),
            serde_json::json!({"id": "variant-b"}),
        ]);
    value["artifact_kinds"]
        .as_array_mut()
        .expect("artifact kinds are an array")
        .extend([
            serde_json::json!({
                "id": "code_variant_a",
                "target": "issue",
                "identifying_labels": ["code", "variant-a"]
            }),
            serde_json::json!({
                "id": "code_variant_b",
                "target": "issue",
                "identifying_labels": ["code", "variant-b"]
            }),
        ]);
    let raw: RawWorkflowSpec =
        serde_json::from_value(value).expect("ambiguous workflow still parses");
    let workflow = raw.validate().expect("ambiguous workflow validates");
    let compiled = workflow.compile();
    (workflow, compiled)
}

fn active_assignment(number: ItemNumber, suffix: &str) -> DurableAssignment {
    DurableAssignment {
        job_id: Some(format!(
            "ai/temper/issue-{}/engineer/code_ready",
            number.get()
        )),
        attempt_id: Some(format!("attempt-{suffix}")),
        role: Some(RoleId::new("engineer")),
        queue: Some("code_ready".to_string()),
        action: Some("open_pr".to_string()),
        worker_id: Some(format!("worker-{suffix}")),
        coordination_key: Some(format!("pr-for-code-{}", number.get())),
        daemon_boot_id: Some(format!("daemon-{suffix}")),
        pre_claim_labels: vec!["code".to_string(), "ready".to_string()],
        pre_claim_assignees: Vec::new(),
        assigned_at: Some(timestamp("2026-07-21T16:00:00Z")),
        expires_at: Some(timestamp("2026-07-21T16:10:00Z")),
        ..DurableAssignment::default()
    }
}

async fn create_assigned_issue(
    forge: &MemoryForge,
    repo: &RepositoryId,
    title: &str,
    labels: &[&str],
    metadata_kind: Option<ArtifactKindId>,
) -> (temper_forge::Issue, DurableAssignment) {
    let issue = forge
        .create_issue(
            repo,
            CreateIssue {
                title: title.to_string(),
                body: format!("Operator-authored context for {title}."),
                labels: labels.iter().map(|label| (*label).to_string()).collect(),
                assignees: vec![UserId::new("engineer")],
            },
        )
        .await
        .expect("assigned issue is created");
    let assignment = active_assignment(issue.number, &issue.number.get().to_string());
    let metadata = WorkflowMetadata {
        kind: metadata_kind,
        lease: Some(Lease {
            role: RoleId::new("engineer"),
            worker: assignment.worker_id.clone().unwrap(),
            claimed_at: timestamp("2026-07-21T16:00:00Z"),
            heartbeat_at: timestamp("2026-07-21T16:05:00Z"),
            expires_at: timestamp("2026-07-21T16:10:00Z"),
        }),
        assignment: Some(assignment.clone()),
        ..WorkflowMetadata::default()
    };
    let issue = forge
        .update_issue(
            &issue.id,
            UpdateIssue {
                body: Some(format!(
                    "Operator-authored context for {title}.\n\n{}",
                    render_metadata_block(&metadata)
                )),
                ..UpdateIssue::default()
            },
        )
        .await
        .expect("assignment metadata is stored");
    (issue, assignment)
}

#[test]
fn label_classified_startup_assignment_stages_and_orphan_requeues() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let forge = MemoryForge::new();
        let repo = forge
            .create_repository(CreateRepository {
                owner: "ai".to_string(),
                name: "temper".to_string(),
                default_branch: "main".to_string(),
                description: None,
            })
            .await
            .expect("repository is created")
            .id;
        let (issue, assignment) = create_assigned_issue(
            &forge,
            &repo,
            "Label-only active assignment",
            &["code", "in-progress"],
            None,
        )
        .await;
        let (workflow, compiled) = reference_workflow();
        let policy = LeasePolicy::new(chrono::Duration::minutes(5));
        let daemon = Daemon::new(Arc::new(handle)).begin_startup_recovery();

        let recovered = stage_startup_assignments(
            &daemon,
            &forge,
            std::slice::from_ref(&repo),
            &workflow,
            &compiled,
            policy,
            timestamp("2026-07-21T16:06:00Z"),
        )
        .await
        .expect("startup assignment stages");
        assert_eq!(recovered.len(), 1);
        assert!(recovered.contains_key(assignment.job_id.as_deref().unwrap()));

        let orphaned = daemon.collect_startup_orphans().await;
        assert_eq!(orphaned.len(), 1);
        assert_eq!(orphaned[0].job_id, assignment.job_id.as_deref().unwrap());
        assert_eq!(orphaned[0].attempt_id, assignment.attempt_id);
        assert_eq!(
            orphaned[0].worker_id,
            assignment.worker_id.as_deref().unwrap()
        );
        let context: JobContext = serde_json::from_value(orphaned[0].job_payload.clone())
            .expect("staged job context parses");
        assert_eq!(context.artifact_kind, "code");
        assert_eq!(context.action.as_deref(), Some("open_pr"));

        converge_startup_orphans(&forge, policy, &workflow, &recovered, &orphaned)
            .await
            .expect("unreattached assignment requeues");
        let requeued = forge
            .get_issue_by_number(&repo, issue.number)
            .await
            .expect("issue reload succeeds")
            .expect("issue still exists");
        assert!(requeued.labels.contains(&"code".to_string()));
        assert!(requeued.labels.contains(&"ready".to_string()));
        assert!(!requeued.labels.contains(&"in-progress".to_string()));
        assert!(!requeued.labels.contains(&"needs-human".to_string()));
        let metadata = parse_metadata_block(&requeued.body)
            .expect("metadata parses")
            .expect("metadata is retained");
        assert!(metadata.kind.is_none());
        assert!(metadata.assignment.is_none());
        assert!(metadata.lease.is_none());
        assert!(
            forge
                .list_issue_comments(&requeued.id)
                .await
                .expect("comments load")
                .is_empty()
        );
    });
}

#[test]
fn startup_kind_conflict_and_ambiguity_quarantine_once() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let forge = MemoryForge::new();
        let repo = forge
            .create_repository(CreateRepository {
                owner: "ai".to_string(),
                name: "temper".to_string(),
                default_branch: "main".to_string(),
                description: None,
            })
            .await
            .expect("repository is created")
            .id;
        let (conflicting, _) = create_assigned_issue(
            &forge,
            &repo,
            "Conflicting active assignment",
            &["design", "in-progress"],
            Some(ArtifactKindId::new("code")),
        )
        .await;
        let (ambiguous, _) = create_assigned_issue(
            &forge,
            &repo,
            "Ambiguous active assignment",
            &["code", "variant-b", "in-progress", "variant-a"],
            None,
        )
        .await;
        let (workflow, compiled) = ambiguous_workflow();
        let policy = LeasePolicy::new(chrono::Duration::minutes(5));
        let daemon = Daemon::new(Arc::new(handle)).begin_startup_recovery();

        for _ in 0..2 {
            let recovered = stage_startup_assignments(
                &daemon,
                &forge,
                std::slice::from_ref(&repo),
                &workflow,
                &compiled,
                policy,
                timestamp("2026-07-21T16:06:00Z"),
            )
            .await
            .expect("fail-closed inventory completes");
            assert!(recovered.is_empty());
        }

        for issue in [conflicting, ambiguous] {
            let quarantined = forge
                .get_issue_by_number(&repo, issue.number)
                .await
                .expect("issue reload succeeds")
                .expect("issue still exists");
            assert!(quarantined.labels.contains(&"needs-human".to_string()));
            assert!(!quarantined.labels.contains(&"in-progress".to_string()));
            let metadata = parse_metadata_block(&quarantined.body)
                .expect("metadata parses")
                .expect("metadata is retained");
            assert!(metadata.assignment.is_none());
            assert!(metadata.lease.is_none());
            let comments = forge
                .list_issue_comments(&quarantined.id)
                .await
                .expect("comments load");
            assert_eq!(comments.len(), 1);
            assert!(
                comments[0]
                    .body
                    .contains(temper_workflow::ASSIGNMENT_RECOVERY_AUDIT_MARKER)
            );
        }
    });
}
