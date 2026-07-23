// SPDX-License-Identifier: MPL-2.0

use super::*;
use chrono::{DateTime, Utc};
use temper_forge::{CreateIssue, Issue, UserId};
use temper_workflow::{Lease, parse_metadata_block};

fn recovery_workflow(fixture: &str) -> (ValidatedWorkflow, CompiledWorkflow) {
    let raw: RawWorkflowSpec = serde_json::from_str(fixture).expect("workflow fixture parses");
    let workflow = raw.validate().expect("workflow fixture validates");
    let compiled = workflow.compile();
    (workflow, compiled)
}

fn ambiguous_recovery_workflow() -> (ValidatedWorkflow, CompiledWorkflow) {
    let mut value: serde_json::Value =
        serde_json::from_str(REFERENCE_DELIVERY_FIXTURE).expect("reference workflow parses");
    value["labels"]
        .as_array_mut()
        .expect("labels are an array")
        .extend([json!({"id": "variant-a"}), json!({"id": "variant-b"})]);
    value["artifact_kinds"]
        .as_array_mut()
        .expect("artifact kinds are an array")
        .extend([
            json!({
                "id": "code_variant_a",
                "target": "issue",
                "identifying_labels": ["code", "variant-a"]
            }),
            json!({
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

fn timestamp(value: &str) -> DateTime<Utc> {
    value.parse().expect("valid timestamp")
}

fn active_assignment(number: ItemNumber) -> DurableAssignment {
    DurableAssignment {
        job_id: Some(format!(
            "ai/temper/issue-{}/engineer/code_ready",
            number.get()
        )),
        attempt_id: Some("attempt-before-restart".to_string()),
        role: Some(RoleId::new("engineer")),
        queue: Some("code_ready".to_string()),
        action: Some("open_pr".to_string()),
        worker_id: Some("worker-before-restart".to_string()),
        coordination_key: Some(format!("pr-for-code-{}", number.get())),
        daemon_boot_id: Some("daemon-before-restart".to_string()),
        pre_claim_labels: vec!["code".to_string(), "ready".to_string()],
        pre_claim_assignees: Vec::new(),
        assigned_at: Some(timestamp("2026-07-21T16:00:00Z")),
        expires_at: Some(timestamp("2026-07-21T16:10:00Z")),
        ..DurableAssignment::default()
    }
}

async fn create_recovery_issue(
    forge: &MemoryForge,
    repo: &RepositoryId,
    labels: &[&str],
    metadata_kind: Option<ArtifactKindId>,
) -> (Issue, DurableAssignment) {
    let issue = forge
        .create_issue(
            repo,
            CreateIssue {
                title: "Recover label-classified work".to_string(),
                body: "Operator-authored recovery context.".to_string(),
                labels: labels.iter().map(|label| (*label).to_string()).collect(),
                assignees: vec![UserId::new("engineer")],
            },
        )
        .await
        .expect("recovery issue is created");
    let assignment = active_assignment(issue.number);
    let metadata = WorkflowMetadata {
        kind: metadata_kind,
        lease: Some(Lease {
            role: RoleId::new("engineer"),
            worker: "worker-before-restart".to_string(),
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
                    "Operator-authored recovery context.\n\n{}",
                    render_metadata_block(&metadata)
                )),
                ..UpdateIssue::default()
            },
        )
        .await
        .expect("active assignment metadata is stored");
    (issue, assignment)
}

#[test]
fn recovered_job_uses_label_resolved_kind_without_metadata_kind() {
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
        let (issue, assignment) =
            create_recovery_issue(&forge, &repo, &["code", "in-progress"], None).await;
        let (workflow, compiled) = recovery_workflow(BASIC_DELIVERY_FIXTURE);

        let job = recovered_job_from_assignment(
            &forge,
            &repo,
            ArtifactSource::Issue {
                number: issue.number,
            },
            &assignment,
            ArtifactKindId::new("code"),
            &workflow,
            &compiled,
        )
        .await
        .expect("label-classified assignment reconstructs");

        assert_eq!(job.job_id, assignment.job_id.as_deref().unwrap());
        let context: JobContext =
            serde_json::from_value(job.job_payload).expect("recovered job context parses");
        assert_eq!(context.artifact_kind, "code");
        assert_eq!(context.queue, "code_ready");
        assert_eq!(context.action.as_deref(), Some("open_pr"));
        assert_eq!(
            context.artifact.as_ref().expect("fresh snapshot").labels,
            vec!["code".to_string(), "in-progress".to_string()]
        );
        assert_eq!(
            context
                .workspace
                .as_ref()
                .expect("workspace is reconstructed")
                .coordination_key,
            assignment.coordination_key.as_deref().unwrap()
        );
        assert!(
            parse_metadata_block(&issue.body)
                .expect("metadata parses")
                .expect("metadata exists")
                .kind
                .is_none()
        );
    });
}

#[test]
fn recovered_job_rejects_conflicting_and_ambiguous_kind_evidence() {
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

        let (conflicting, conflicting_assignment) = create_recovery_issue(
            &forge,
            &repo,
            &["design", "in-progress"],
            Some(ArtifactKindId::new("code")),
        )
        .await;
        let (workflow, compiled) = recovery_workflow(REFERENCE_DELIVERY_FIXTURE);
        let conflict = recovered_job_from_assignment(
            &forge,
            &repo,
            ArtifactSource::Issue {
                number: conflicting.number,
            },
            &conflicting_assignment,
            ArtifactKindId::new("code"),
            &workflow,
            &compiled,
        )
        .await
        .expect_err("conflicting metadata and labels fail closed");
        assert!(conflict.contains("metadata names artifact kind `code`"));
        assert!(conflict.contains("labels resolve to `design`"));

        let (ambiguous, ambiguous_assignment) = create_recovery_issue(
            &forge,
            &repo,
            &["code", "variant-b", "in-progress", "variant-a"],
            None,
        )
        .await;
        let (workflow, compiled) = ambiguous_recovery_workflow();
        let ambiguity = recovered_job_from_assignment(
            &forge,
            &repo,
            ArtifactSource::Issue {
                number: ambiguous.number,
            },
            &ambiguous_assignment,
            ArtifactKindId::new("code"),
            &workflow,
            &compiled,
        )
        .await
        .expect_err("ambiguous identifying labels fail closed");
        assert!(ambiguity.contains("matches several artifact kinds"));
        assert!(ambiguity.contains("`code_variant_a`, `code_variant_b`"));
    });
}
