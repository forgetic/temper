// SPDX-License-Identifier: MPL-2.0

use crate::assertions::*;
use crate::support::*;

use std::collections::BTreeMap;
use std::sync::{Arc as StdArc, Mutex};

use temper_forge_memory::FaultOp;
use temper_protocol_worker::{
    ArtifactContextBundle, ArtifactContextTruncation, ArtifactReference, ArtifactRelationType,
    ArtifactRepository, ArtifactSnapshot, ArtifactSummary, ArtifactType,
};
use tracing::field::{Field, Visit};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry;

const VALIDATION_WORKFLOW: &str =
    include_str!("../../../../scenarios/plan-centric-feature-branch/config/workflow.json");

fn validation_workflow() -> ValidatedWorkflow {
    let spec: RawWorkflowSpec =
        serde_json::from_str(VALIDATION_WORKFLOW).expect("validation workflow parses");
    spec.validate().expect("validation workflow validates")
}

async fn create_plan(forge: &MemoryForge, repo: &RepositoryId) -> ItemNumber {
    let metadata = WorkflowMetadata {
        kind: Some(ArtifactKindId::new("plan")),
        target_branch: Some("feature/validation-audit".to_string()),
        ..WorkflowMetadata::default()
    };
    forge
        .create_issue(
            repo,
            CreateIssue {
                title: "Validate the feature plan".to_string(),
                body: format!(
                    "Private source body must stay private.\n\n{}",
                    render_metadata_block(&metadata)
                ),
                labels: vec!["plan".to_string(), "needs-validation".to_string()],
                assignees: Vec::new(),
            },
        )
        .await
        .expect("plan issue is created")
        .number
}

fn validation_job(number: ItemNumber, scope: Vec<ArtifactSummary>) -> InFlightJob {
    let primary_repo = ArtifactRepository {
        id: "forgejo:acme/service".to_string(),
        path: "acme/service".to_string(),
    };
    let primary_ref = ArtifactReference {
        repository: primary_repo.clone(),
        artifact_type: ArtifactType::Issue,
        number: number.get(),
    };
    let mut bundle = ArtifactContextBundle::new(ArtifactSnapshot {
        artifact: primary_ref,
        title: "Validate the feature plan".to_string(),
        body: "artifact-context primary body must stay private".to_string(),
        labels: vec!["plan".to_string(), "needs-validation".to_string()],
        state: "open".to_string(),
        workflow_kind: Some("plan".to_string()),
        workflow: None,
    });
    bundle.validation_scope = scope;
    bundle.truncation = ArtifactContextTruncation::default();

    job_for_context(
        "acme/service",
        number,
        "issue",
        JobContext {
            trace_context: None,
            artifact_context: Some(bundle),
            role: "tester".to_string(),
            repo: "acme/service".to_string(),
            queue: "plan_needs_validation".to_string(),
            artifact_kind: "plan".to_string(),
            artifact: None,
            workspace: Some(WorkspaceManifest {
                coordination_key: "feature-plan:validation-round-7".to_string(),
                repos: Vec::new(),
            }),
            action: Some("validate_plan".to_string()),
            checkout_capability: Some("read_only".to_string()),
            allowed_verdicts: vec!["validated".to_string(), "needs_followup".to_string()],
            verdict_contracts: Default::default(),
            source_metadata: BTreeMap::from([(
                "target_branch".to_string(),
                "feature/validation-audit".to_string(),
            )]),
            guidance: None,
            pull_request_freshness: None,
        },
    )
}

fn scope_artifact(repo: &str, number: u64, artifact_type: ArtifactType) -> ArtifactSummary {
    let repository = ArtifactRepository {
        id: format!("forgejo:{repo}"),
        path: repo.to_string(),
    };
    let artifact = ArtifactReference {
        repository: repository.clone(),
        artifact_type,
        number,
    };
    ArtifactSummary {
        artifact,
        title: "This title is deliberately not copied into the audit".to_string(),
        labels: vec!["implementation".to_string()],
        state: "closed".to_string(),
        workflow_kind: Some("implementation_pr".to_string()),
        relation_type: ArtifactRelationType::Dependency,
        source: ArtifactReference {
            repository,
            artifact_type: ArtifactType::Issue,
            number: 1,
        },
    }
}

fn validated_result(job: &InFlightJob, summary: &str) -> JobResult {
    let mut result = verdict_result(
        "worker-a",
        &job.job_id,
        "validated",
        Some("forbidden-result-body Authorization: Bearer body-secret"),
    );
    result.title = Some("Land the validated feature".to_string());
    result.summary = Some(summary.to_string());
    result.details = Some(json!({
        "reasoning": "forbidden-details",
        "tool_output": "Authorization: Bearer details-secret"
    }));
    result
}

fn needs_followup_result(job: &InFlightJob, summary: &str) -> JobResult {
    let mut same_repo = job_child(
        "repair-api",
        "Repair API validation gap",
        "forbidden-child-body-api",
        &[],
    );
    same_repo.kind = Some("code".to_string());
    let mut cross_repo = job_child(
        "repair-client",
        "Repair client validation gap",
        "forbidden-child-body-client",
        &[],
    );
    cross_repo.kind = Some("code".to_string());
    cross_repo.depends_on = vec!["repair-api".to_string()];
    cross_repo.target_repo = Some("acme/followups".to_string());

    let mut result = verdict_result_with_children(
        "worker-a",
        &job.job_id,
        "needs_followup",
        vec![same_repo, cross_repo],
    );
    result.summary = Some(summary.to_string());
    result.body = Some("forbidden-negative-result-body".to_string());
    result.details = Some(json!({"reasoning":"forbidden-negative-details"}));
    result
}

fn actor() -> User {
    User {
        id: UserId::new("forge-user-9"),
        handle: "architect".to_string(),
        display_name: Some("Configured Architect".to_string()),
        email: Some("private@example.invalid".to_string()),
    }
}

async fn issue_comments(
    forge: &MemoryForge,
    repo: &RepositoryId,
    number: ItemNumber,
) -> Vec<temper_forge::Comment> {
    let issue = forge
        .get_issue_by_number(repo, number)
        .await
        .unwrap()
        .unwrap();
    forge.list_issue_comments(&issue.id).await.unwrap()
}

#[derive(Clone, Debug, Default)]
struct CapturedEvent {
    fields: BTreeMap<String, String>,
}

#[derive(Default)]
struct CapturedVisitor {
    fields: BTreeMap<String, String>,
}

impl Visit for CapturedVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.fields
            .insert(field.name().to_string(), format!("{value:?}"));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }
}

#[derive(Clone, Default)]
struct CaptureLayer {
    events: StdArc<Mutex<Vec<CapturedEvent>>>,
}

impl<S: tracing::Subscriber> Layer<S> for CaptureLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = CapturedVisitor::default();
        event.record(&mut visitor);
        self.events.lock().unwrap().push(CapturedEvent {
            fields: visitor.fields,
        });
    }
}

fn validation_events(events: &StdArc<Mutex<Vec<CapturedEvent>>>) -> Vec<CapturedEvent> {
    events
        .lock()
        .unwrap()
        .iter()
        .filter(|event| event.fields.get("event").map(String::as_str) == Some("validation.outcome"))
        .cloned()
        .collect()
}

#[test]
fn validated_audit_retries_after_landing_pr_and_emits_only_after_convergence() {
    let layer = CaptureLayer::default();
    let events = layer.events.clone();
    let subscriber = registry().with(layer);

    temper_engine_io::block_on(async move {
        let _subscriber_guard = tracing::subscriber::set_default(subscriber);
        let forge = Arc::new(MemoryForge::with_current_user(actor()));
        let repo = new_repo(&forge, "main").await;
        let plan = create_plan(&forge, &repo).await;
        let workflow = Arc::new(validation_workflow());
        let applier = ForgeApplier::new(forge.clone(), workflow);
        let job = validation_job(
            plan,
            vec![
                scope_artifact("acme/service", 41, ArtifactType::PullRequest),
                scope_artifact("acme/widgets", 12, ArtifactType::Issue),
            ],
        );
        let unsafe_summary =
            "Checks pass. Authorization: Bearer summary-secret-that-must-not-escape";
        let expected_summary = temper_log::validation_summary_preview(unsafe_summary);
        let result = validated_result(&job, unsafe_summary);

        forge.fail_next(FaultOp::AddIssueComment, "audit publication unavailable");
        let first = applier.apply(job.clone(), result.clone()).await;
        assert!(matches!(
            first,
            temper_engine::ApplyOutcome::ConvergencePending { .. }
        ));
        assert_eq!(
            forge
                .list_pull_requests(&repo, PullRequestQuery::default())
                .await
                .unwrap()
                .len(),
            1,
            "the landing PR is durable before audit publication"
        );
        assert!(issue_comments(&forge, &repo, plan).await.is_empty());
        assert!(has_label(
            &issue_labels(&forge, &repo, plan).await,
            "needs-validation"
        ));
        assert!(validation_events(&events).is_empty());

        assert_eq!(
            applier.apply(job.clone(), result).await,
            temper_engine::ApplyOutcome::Applied
        );
        assert_eq!(
            forge
                .list_pull_requests(&repo, PullRequestQuery::default())
                .await
                .unwrap()
                .len(),
            1,
            "exact replay reuses the landing PR"
        );
        let comments = issue_comments(&forge, &repo, plan).await;
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].author_id, UserId::new("forge-user-9"));
        let audit = &comments[0].body;
        assert!(audit.contains("**Outcome:** `validated`"));
        assert!(audit.contains("Workflow role: `tester`"));
        assert!(audit.contains("Forge actor: `architect` (`forge-user-9`)"));
        assert!(audit.contains("Routed transition: `plan_validated_create_landing`"));
        assert!(audit.contains("Workspace coordination key: `feature-plan:validation-round-7`"));
        assert!(audit.contains("acme/service#41 (pull request)"));
        assert!(audit.contains("acme/widgets#12 (issue)"));
        assert!(audit.contains("&lt;redacted&gt;"));
        for forbidden in [
            "summary-secret-that-must-not-escape",
            "forbidden-result-body",
            "body-secret",
            "forbidden-details",
            "details-secret",
            "artifact-context primary body",
            "This title is deliberately not copied",
            "private@example.invalid",
        ] {
            assert!(!audit.contains(forbidden), "audit leaked {forbidden:?}");
        }
        assert_eq!(
            audit.matches("temper:comment-key=plan-validation:").count(),
            1
        );

        let captured = validation_events(&events);
        assert_eq!(captured.len(), 1);
        let fields = &captured[0].fields;
        assert_eq!(fields.get("outcome").map(String::as_str), Some("validated"));
        assert_eq!(fields.get("role").map(String::as_str), Some("tester"));
        assert_eq!(
            fields.get("forge.actor.handle").map(String::as_str),
            Some("architect")
        );
        assert_eq!(
            fields.get("forge.actor.id").map(String::as_str),
            Some("forge-user-9")
        );
        assert_eq!(
            fields.get("validation.scope_count").map(String::as_str),
            Some("2")
        );
        assert_eq!(fields.get("follow_up.count").map(String::as_str), Some("0"));
        assert_eq!(
            fields.get("summary.preview").map(String::as_str),
            Some(expected_summary.as_str())
        );
    });
}

#[test]
fn negative_audit_replay_links_each_final_child_once_and_bounds_summary() {
    temper_engine_io::block_on(async move {
        let forge = Arc::new(MemoryForge::with_current_user(actor()));
        let repo = new_repo(&forge, "main").await;
        let followup_repo = create_repo(&forge, "acme", "followups", "main").await;
        let plan = create_plan(&forge, &repo).await;
        let workflow = Arc::new(validation_workflow());
        let applier = ForgeApplier::new(forge.clone(), workflow);
        let job = validation_job(
            plan,
            vec![scope_artifact(
                "acme/service",
                77,
                ArtifactType::PullRequest,
            )],
        );
        let oversized_summary = "é".repeat(500);
        let result = needs_followup_result(&job, &oversized_summary);

        forge.fail_next(FaultOp::AddIssueComment, "negative audit unavailable");
        assert!(matches!(
            applier.apply(job.clone(), result.clone()).await,
            temper_engine::ApplyOutcome::ConvergencePending { .. }
        ));
        assert_eq!(list_issues(&forge, &repo).await.len(), 2);
        assert_eq!(list_issues(&forge, &followup_repo).await.len(), 1);
        assert!(issue_comments(&forge, &repo, plan).await.is_empty());

        assert_eq!(
            applier.apply(job.clone(), result.clone()).await,
            temper_engine::ApplyOutcome::Applied
        );
        let parent_issues = list_issues(&forge, &repo).await;
        let target_issues = list_issues(&forge, &followup_repo).await;
        assert_eq!(parent_issues.len(), 2);
        assert_eq!(target_issues.len(), 1);
        let same_repo_child = parent_issues
            .iter()
            .find(|issue| issue.number != plan)
            .expect("same-repository child exists");
        let cross_repo_child = &target_issues[0];
        let comments = issue_comments(&forge, &repo, plan).await;
        assert_eq!(comments.len(), 1);
        let audit = &comments[0].body;
        assert!(audit.contains("**Outcome:** `needs_followup`"));
        assert!(audit.contains(&format!("#{}", same_repo_child.number.get())));
        assert!(audit.contains(&format!("acme/followups#{}", cross_repo_child.number.get())));
        assert_eq!(
            audit
                .matches(&format!("#{}", same_repo_child.number.get()))
                .count(),
            1
        );
        assert_eq!(
            audit
                .matches(&format!("acme/followups#{}", cross_repo_child.number.get()))
                .count(),
            1
        );
        let preview = temper_log::validation_summary_preview(&oversized_summary);
        assert_eq!(preview.chars().count(), 240);
        assert!(audit.contains(&preview));
        assert!(!audit.contains(&oversized_summary));
        for forbidden in [
            "forbidden-child-body-api",
            "forbidden-child-body-client",
            "forbidden-negative-result-body",
            "forbidden-negative-details",
        ] {
            assert!(!audit.contains(forbidden), "audit leaked {forbidden:?}");
        }

        // A replay after convergence is a quiet stale result and cannot create
        // another product, relation, or comment.
        assert_eq!(
            applier.apply(job, result).await,
            temper_engine::ApplyOutcome::Stale
        );
        assert_eq!(list_issues(&forge, &repo).await.len(), 2);
        assert_eq!(list_issues(&forge, &followup_repo).await.len(), 1);
        assert_eq!(issue_comments(&forge, &repo, plan).await.len(), 1);
    });
}

#[test]
fn audit_convergence_returns_503_and_retains_durable_assignment_for_exact_replay() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let forge = Arc::new(MemoryForge::with_current_user(actor()));
        let repo = new_repo(&forge, "main").await;
        let plan = create_plan(&forge, &repo).await;
        let workflow = Arc::new(validation_workflow());
        let inner = Arc::new(ForgeApplier::new(forge.clone(), workflow));
        let applier = Arc::new(LeaseApplier::new(
            forge.clone(),
            policy(),
            "daemon-1",
            inner,
            temper_engine::system_clock(),
        ));
        let daemon = Daemon::with_applier(Arc::new(handle.clone()), applier);
        let url = spawn(&handle, &daemon).await;
        let client = temper_engine_io::http::JsonClient::new();
        assert_eq!(
            post(
                &client,
                &url,
                &register("worker-a", "tester", "acme/service")
            )
            .await
            .status,
            204
        );

        let job = validation_job(plan, Vec::new());
        daemon
            .enqueue_job(
                job.job_id.clone(),
                job.role.clone(),
                job.repo.clone(),
                job.artifact.clone(),
                job.job_payload.clone(),
            )
            .await;
        let assignment =
            poll_assignment_for_role(&client, &url, "worker-a", "tester", "issue", plan).await;
        let result = validated_result(&job, "All validation checks pass.");

        forge.fail_next(FaultOp::AddIssueComment, "comment transport failed");
        let first = post(
            &client,
            &url,
            &WorkerProtocolMessage::Result(result.clone()),
        )
        .await;
        assert_eq!(first.status, 503);
        let interrupted = forge
            .get_issue_by_number(&repo, plan)
            .await
            .unwrap()
            .unwrap();
        let durable = parse_metadata_block(&interrupted.body)
            .unwrap()
            .unwrap()
            .assignment
            .expect("audit failure retains durable assignment");
        assert_eq!(durable.job_id.as_deref(), Some(assignment.job_id.as_str()));
        assert_eq!(durable.attempt_id, assignment.attempt_id);
        assert_eq!(durable.worker_id.as_deref(), Some("worker-a"));

        assert_release(
            post_json(&client, &url, &WorkerProtocolMessage::Result(result)).await,
            "worker-a",
            &assignment.job_id,
        );
        assert_eq!(
            forge
                .list_pull_requests(&repo, PullRequestQuery::default())
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(issue_comments(&forge, &repo, plan).await.len(), 1);
        let completed = forge
            .get_issue_by_number(&repo, plan)
            .await
            .unwrap()
            .unwrap();
        assert!(
            parse_metadata_block(&completed.body)
                .unwrap()
                .unwrap_or_default()
                .assignment
                .is_none()
        );
    });
}
