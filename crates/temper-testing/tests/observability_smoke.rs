//! Hermetic observability smoke proof for a small reference-delivery movement.
//!
//! The test drives real workflow scan/execution/reconciliation code against the
//! in-memory Forge and asserts that the structured observability events carry
//! enough shared identity to diagnose both movement and the original stuck
//! zero-dependency shape.

use std::time::Duration;

use chrono::{DateTime, Utc};
use temper_forge_memory::MemoryForge;
use temper_forge_model::{CreateIssue, CreateRepository, Forge, IssueQuery, RepositoryId};
use temper_runner::{
    ActionDispatchEvent, MechanicalReconciliationEvent, RoleDecisionReplyEvent,
    RoleDecisionRequestEvent, ScanSummaryEvent, TransitionExecutionEvent, WorkItemIdentity,
    WorkItemSelectedEvent, render_action_dispatch_event, render_mechanical_reconciliation_event,
    render_role_decision_reply_event, render_role_decision_request_event,
    render_scan_summary_event, render_transition_execution_event, render_work_item_selected_event,
    scan_role,
};
use temper_testing::{block_on, workflow};
use temper_workflow::{
    ArtifactKindId, ArtifactSource, DefaultRecoveryPolicy, DependencyStatus, ReconcileFinding,
    RoleId, TransitionId, WorkflowMetadata, render_metadata_block,
};

#[test]
fn moving_and_stuck_workflow_emit_correlatable_observability_events() {
    let forge = MemoryForge::new();
    let repo = create_repo(&forge);
    let workflow = workflow();
    let compiled = workflow.compile();
    let role = RoleId::new("architect");
    let now = DateTime::<Utc>::from_timestamp(0, 0).expect("epoch is valid");

    let intake = block_on(forge.create_issue(
        &repo,
        CreateIssue {
            title: "Coordinate greeting".to_string(),
            body: String::new(),
            labels: vec!["untriaged".to_string()],
            assignees: Vec::new(),
        },
    ))
    .expect("intake issue is created");

    let items = block_on(scan_role(&forge, &repo, &workflow, &compiled, now, &role))
        .expect("scan succeeds");
    assert_eq!(items.len(), 1);
    let item = &items[0];
    assert_eq!(
        item.target,
        ArtifactSource::Issue {
            number: intake.number
        }
    );

    let identity = WorkItemIdentity::new(&repo, &role, &item.queue, item.target, &item.kind)
        .with_tick_id("tick/smoke/1");
    let manifest = compiled.role(&role).expect("role manifest exists");
    let authorized_actions = manifest
        .tools
        .iter()
        .map(|tool| tool.name.clone())
        .collect::<Vec<_>>();
    let transition = TransitionId::new("triage_to_code");

    let mut events = vec![
        render_scan_summary_event(&ScanSummaryEvent {
            tick_id: identity.tick_id.as_deref(),
            worker_kind: "role",
            worker: "multi-role:architect",
            repo: &repo,
            workflow_id: workflow.name(),
            role: Some(role.as_str()),
            work_item_count: items.len(),
        }),
        render_work_item_selected_event(&WorkItemSelectedEvent {
            identity: &identity,
            workflow_id: workflow.name(),
            worker: "multi-role:architect",
        }),
        render_role_decision_request_event(&RoleDecisionRequestEvent {
            identity: &identity,
            workflow_id: workflow.name(),
            authorized_actions: &authorized_actions,
            available_external_tools: &[],
        }),
        render_role_decision_reply_event(&RoleDecisionReplyEvent {
            identity: &identity,
            selected_action: Some("triage_to_code"),
            validation_outcome: "valid",
            action_kind: "authorized_action",
            reason: Some("triage intake to code"),
            latency: Duration::from_millis(3),
            error: None,
        }),
        render_action_dispatch_event(&ActionDispatchEvent {
            identity: &identity,
            selected_action: "triage_to_code",
            transition: &transition,
            external_executor_required: false,
            external_executor_id: None,
            external_executor_available: None,
            outcome: "dispatching",
            no_op_reason: None,
        }),
    ];

    let report = block_on(workflow.executor(&forge).execute(
        &repo,
        item.target,
        &transition,
        &role,
    ))
    .expect("transition executes");
    events.push(render_transition_execution_event(
        &TransitionExecutionEvent {
            identity: &identity,
            transition: &report.transition,
            outcome: "mutated",
            stale_work: false,
            effects: &report.applied,
            failure_class: None,
            diagnostic_classes: Vec::new(),
            postcondition_outcome: "passed",
        },
    ));

    let blocked = block_on(forge.create_issue(
        &repo,
        CreateIssue {
            title: "Blocked without fan-out".to_string(),
            body: render_metadata_block(&WorkflowMetadata {
                kind: Some(ArtifactKindId::new("code")),
                ..WorkflowMetadata::default()
            }),
            labels: vec!["code".to_string(), "blocked".to_string()],
            assignees: Vec::new(),
        },
    ))
    .expect("blocked issue is created");
    let reconciler = workflow.reconciler(&DefaultRecoveryPolicy);
    let reconcile = reconciler.scan(
        &[temper_workflow::ArtifactSnapshot::from_issue(&blocked)],
        &[],
        &DependencyStatus::default(),
        now,
    );
    assert!(matches!(
        reconcile.findings.as_slice(),
        [ReconcileFinding::BlockedWithoutDependencies { .. }]
    ));
    events.push(render_mechanical_reconciliation_event(
        &MechanicalReconciliationEvent {
            worker: "mechanical",
            repo: &repo,
            finding: &reconcile.findings[0],
            action: &reconcile.actions[0],
        },
    ));

    let joined = events.join("\n");
    assert!(joined.contains(r#""event":"scan_summary""#));
    assert!(joined.contains(r#""event":"work_item_selected""#));
    assert!(joined.contains(r#""event":"role_decision_request""#));
    assert!(joined.contains(r#""event":"role_decision_reply""#));
    assert!(joined.contains(r#""event":"action_dispatch""#));
    assert!(joined.contains(r#""event":"transition_execution""#));
    assert!(joined.contains(r#""event":"mechanical_reconciliation""#));
    assert!(joined.contains(&format!(r#""decision_id":"{}""#, identity.decision_id)));
    assert!(joined.contains(r#""transition":"triage_to_code""#));
    assert!(joined.contains(r#""diagnostic":"blocked_artifact_without_dependencies""#));
    assert!(joined.contains(r#""dependency_count":0"#));
    assert!(!joined.contains("Blocked without fan-out"));

    let issues =
        block_on(forge.list_issues(&repo, IssueQuery::default())).expect("issues are readable");
    assert!(issues.iter().any(|issue| {
        issue.number == intake.number
            && issue.labels.iter().any(|label| label == "code")
            && issue.labels.iter().any(|label| label == "ready")
    }));
}

fn create_repo(forge: &MemoryForge) -> RepositoryId {
    block_on(forge.create_repository(CreateRepository {
        owner: "acme".to_string(),
        name: "service".to_string(),
        default_branch: "main".to_string(),
        description: None,
    }))
    .expect("repo is created")
    .id
}
