// SPDX-License-Identifier: MPL-2.0

use crate::assertions::*;
use crate::support::*;
use temper_forge::{CiJob, CiJobConclusion, CiJobId, CiJobStatus, PullRequestState, UpdateIssue};
use temper_scenario_core::{
    EvidenceKind, StructuredEvidenceEntry, ValidationAssertion, ValidationStatus,
    ValidationVerdict, ValidatorBinaryIdentity, ValidatorResult, ValidatorResultTarget,
};
use temper_workflow::{ExecutionError, RoleId, TransitionId};

const HEAD_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HEAD_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const SOURCE_BRANCH: &str = "feature/778-exact-head-validation";
const WORKFLOW: &str =
    include_str!("../../../../scenarios/plan-centric-feature-branch/config/workflow.json");

fn exact_workflow() -> ValidatedWorkflow {
    serde_json::from_str::<RawWorkflowSpec>(WORKFLOW)
        .expect("workflow parses")
        .validate()
        .expect("workflow validates")
}

async fn create_exact_plan(forge: &MemoryForge, repo: &RepositoryId) -> (ItemNumber, ItemNumber) {
    let feature = forge
        .create_issue(
            repo,
            CreateIssue {
                title: "Exact-head feature".to_string(),
                body: "Feature".to_string(),
                labels: vec!["feature".to_string()],
                assignees: Vec::new(),
            },
        )
        .await
        .expect("feature")
        .number;
    let plan = forge
        .create_issue(
            repo,
            CreateIssue {
                title: "Exact-head plan".to_string(),
                body: render_metadata_block(&WorkflowMetadata {
                    kind: Some(ArtifactKindId::new("plan")),
                    parents: vec![ArtifactRef::same_repo(feature)],
                    target_branch: Some(SOURCE_BRANCH.to_string()),
                    ..WorkflowMetadata::default()
                }),
                labels: vec!["plan".to_string(), "needs-validation".to_string()],
                assignees: Vec::new(),
            },
        )
        .await
        .expect("plan")
        .number;
    (feature, plan)
}

fn exact_job(plan: ItemNumber, feature: ItemNumber, attempt: &str) -> InFlightJob {
    let mut job = job_for_context(
        "acme/service",
        plan,
        "issue",
        JobContext {
            trace_context: None,
            artifact_context: None,
            role: "tester".to_string(),
            repo: "acme/service".to_string(),
            queue: "plan_needs_validation".to_string(),
            artifact_kind: "plan".to_string(),
            artifact: Some(temper_protocol_worker::JobArtifactSnapshot {
                number: plan.get(),
                title: "Exact-head plan".to_string(),
                body: String::new(),
                labels: vec!["plan".to_string(), "needs-validation".to_string()],
                state: "Open".to_string(),
            }),
            workspace: None,
            action: Some("validate_plan".to_string()),
            checkout_capability: Some("read_only".to_string()),
            allowed_verdicts: vec!["validated".to_string(), "needs_followup".to_string()],
            verdict_contracts: Default::default(),
            source_metadata: BTreeMap::from([
                ("target_branch".to_string(), SOURCE_BRANCH.to_string()),
                (
                    "validation_binding_id".to_string(),
                    "validate_exact_feature_head".to_string(),
                ),
                (
                    "validation_feature".to_string(),
                    format!("acme/service#{}", feature.get()),
                ),
                (
                    "validation_plan".to_string(),
                    format!("acme/service#{}", plan.get()),
                ),
                (
                    "validation_source_branch".to_string(),
                    SOURCE_BRANCH.to_string(),
                ),
            ]),
            guidance: None,
            structured_guidance: None,
            pull_request_freshness: None,
        },
    );
    job.attempt_id = Some(attempt.to_string());
    job
}

fn exact_result(job: &InFlightJob, plan: ItemNumber, feature: ItemNumber, head: &str) -> JobResult {
    let scenario = "exact-head-feature-validation";
    let mut evidence = ValidatorResult::new(
        ValidatorResultTarget::new(
            "plan",
            "acme/service",
            temper_scenario_core::ArtifactReference::issue(plan.get()),
        ),
        ValidationVerdict::Passed,
    );
    evidence.feature = Some(format!("acme/service#{}", feature.get()));
    evidence.plan = Some(format!("acme/service#{}", plan.get()));
    evidence.mapping_id = Some(format!("acme/service#{}:{scenario}", feature.get()));
    evidence.scenario_name = Some(scenario.to_string());
    evidence.scenario_path = Some(format!("scenarios/{scenario}"));
    evidence.source_branch = Some(SOURCE_BRANCH.to_string());
    evidence.exact_head_sha = Some(head.to_string());
    evidence.resolved_content_digest = Some(format!("sha256:{}", "c".repeat(64)));
    evidence.standalone_binary = Some(ValidatorBinaryIdentity {
        path: "target/debug/temper".to_string(),
        sha256: "d".repeat(64),
        size_bytes: 1024,
    });
    evidence.duration_ms = Some(1234);
    evidence.retained_paths = vec!["artifacts/exact-head.json".to_string()];
    evidence.evidence.push(StructuredEvidenceEntry::new(
        "scenario-run",
        EvidenceKind::ScenarioRun,
        "The exact mapped live scenario passed.",
    ));
    evidence.acceptance_criteria.push(
        ValidationAssertion::new("Landing is exact-head gated.", ValidationStatus::Satisfied)
            .with_evidence_ref("scenario-run"),
    );
    assert!(evidence.validate_contract().is_empty());

    let mut result = verdict_result("worker-a", &job.job_id, "validated", Some("typed evidence"));
    result.attempt_id = job.attempt_id.clone();
    result.title = Some("Land exact validated head".to_string());
    result.body = Some(evidence.render_markdown());
    result.summary = Some(format!("validated {head}"));
    result.details = Some(json!({"validator_result": evidence}));
    result
}

#[test]
fn stale_head_before_aggregate_create_requeues_without_landing_artifact() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "main").await;
        let (feature, plan) = create_exact_plan(&forge, &repo).await;
        forge
            .set_branch_head(&repo, SOURCE_BRANCH, HEAD_B)
            .expect("feature head");
        let applier = ForgeApplier::new(forge.clone(), Arc::new(exact_workflow()));
        let job = exact_job(plan, feature, "attempt-head-a");

        assert_eq!(
            applier
                .apply(job.clone(), exact_result(&job, plan, feature, HEAD_A))
                .await,
            temper_engine::ApplyOutcome::Stale
        );
        assert_pull_request_count_stays(&cx, &forge, &repo, 0).await;
        let labels = issue_labels(&forge, &repo, plan).await;
        assert!(has_label(&labels, "needs-validation"), "{labels:?}");
        assert!(!has_label(&labels, "validated"), "{labels:?}");
    });
}

#[test]
fn missing_structured_evidence_cannot_create_aggregate_pr() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "main").await;
        let (feature, plan) = create_exact_plan(&forge, &repo).await;
        forge
            .set_branch_head(&repo, SOURCE_BRANCH, HEAD_A)
            .expect("feature head");
        let applier = ForgeApplier::new(forge.clone(), Arc::new(exact_workflow()));
        let job = exact_job(plan, feature, "attempt-head-a");
        let mut result = verdict_result("worker-a", &job.job_id, "validated", Some("prose only"));
        result.attempt_id = job.attempt_id.clone();
        result.title = Some("Prose must not authorize landing".to_string());

        let outcome = applier.apply(job, result).await;
        assert!(
            matches!(outcome, temper_engine::ApplyOutcome::Rejected { .. }),
            "{outcome:?}"
        );
        assert_pull_request_count_stays(&cx, &forge, &repo, 0).await;
        let labels = issue_labels(&forge, &repo, plan).await;
        assert!(has_label(&labels, "needs-validation"), "{labels:?}");
    });
}

#[test]
fn current_evidence_creates_one_authorized_pr_and_changed_attempt_refreshes_it() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "main").await;
        let (feature, plan) = create_exact_plan(&forge, &repo).await;
        forge
            .set_branch_head(&repo, SOURCE_BRANCH, HEAD_A)
            .expect("feature head A");
        let applier = ForgeApplier::new(forge.clone(), Arc::new(exact_workflow()));
        let job_a = exact_job(plan, feature, "attempt-head-a");
        let result_a = exact_result(&job_a, plan, feature, HEAD_A);

        assert_eq!(
            applier.apply(job_a.clone(), result_a.clone()).await,
            temper_engine::ApplyOutcome::Applied
        );
        let pulls = wait_for_pull_request_count(&cx, &forge, &repo, 1).await;
        let first = &pulls[0];
        let metadata = parse_metadata_block(&first.body)
            .expect("landing metadata")
            .expect("landing metadata block");
        let authority = metadata
            .exact_head_validation
            .expect("exact-head authority");
        assert_eq!(authority.exact_head_sha, HEAD_A);
        assert_eq!(authority.attempt_id, "attempt-head-a");
        assert_eq!(
            authority.mapping_id,
            format!("acme/service#{feature}:exact-head-feature-validation")
        );
        let comments = issue_comment_bodies(&forge, &repo, plan).await;
        assert_eq!(comments.len(), 1);
        assert!(comments[0].contains("Structured exact-head evidence"));
        assert!(comments[0].contains(HEAD_A));

        assert_eq!(
            applier.apply(job_a, result_a).await,
            temper_engine::ApplyOutcome::Stale
        );
        assert_pull_request_count_stays(&cx, &forge, &repo, 1).await;

        forge
            .set_branch_head(&repo, SOURCE_BRANCH, HEAD_B)
            .expect("feature head B");
        forge
            .set_pull_request_head(&first.id, Some(HEAD_B.to_string()))
            .expect("landing PR observes B");
        let issue = forge
            .get_issue_by_number(&repo, plan)
            .await
            .unwrap()
            .unwrap();
        forge
            .update_issue(
                &issue.id,
                UpdateIssue {
                    add_labels: vec!["needs-validation".to_string()],
                    remove_labels: vec!["validated".to_string()],
                    ..UpdateIssue::default()
                },
            )
            .await
            .expect("requeue validation");
        let job_b = exact_job(plan, feature, "attempt-head-b");
        assert_eq!(
            applier
                .apply(job_b.clone(), exact_result(&job_b, plan, feature, HEAD_B))
                .await,
            temper_engine::ApplyOutcome::Applied
        );

        let pulls = forge
            .list_pull_requests(&repo, PullRequestQuery::default())
            .await
            .unwrap();
        assert_eq!(pulls.len(), 1, "changed head must reuse the aggregate PR");
        let authority = parse_metadata_block(&pulls[0].body)
            .unwrap()
            .unwrap()
            .exact_head_validation
            .expect("refreshed authority");
        assert_eq!(authority.exact_head_sha, HEAD_B);
        assert_eq!(authority.attempt_id, "attempt-head-b");
    });
}

#[test]
fn mechanical_merge_rereads_branch_head_and_fails_closed_on_race() {
    temper_engine_io::block_on(async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "main").await;
        let (feature, plan) = create_exact_plan(&forge, &repo).await;
        forge
            .set_branch_head(&repo, SOURCE_BRANCH, HEAD_A)
            .expect("head A");
        let workflow = Arc::new(exact_workflow());
        let applier = ForgeApplier::new(forge.clone(), workflow.clone());
        let job = exact_job(plan, feature, "attempt-head-a");
        assert_eq!(
            applier
                .apply(job.clone(), exact_result(&job, plan, feature, HEAD_A))
                .await,
            temper_engine::ApplyOutcome::Applied
        );
        let pull = forge
            .list_pull_requests(&repo, PullRequestQuery::default())
            .await
            .unwrap()
            .remove(0);
        seed_success_ci(&forge, &repo, &pull);

        // Simulate the branch advancing after the PR representation used by the
        // gate was loaded. The explicit branch read in apply_merge must catch it.
        forge
            .set_branch_head(&repo, SOURCE_BRANCH, HEAD_B)
            .expect("head B races merge");
        let error = workflow
            .executor(forge.as_ref())
            .execute(
                &repo,
                temper_workflow::ArtifactSource::PullRequest {
                    number: pull.number,
                },
                &TransitionId::new("land_feature_pr"),
                &RoleId::new("mechanical"),
            )
            .await
            .expect_err("stale exact-head authority must not merge");
        assert!(
            matches!(
                error,
                ExecutionError::TargetStale { .. } | ExecutionError::Precondition { .. }
            ),
            "{error}"
        );
        let current = forge
            .get_pull_request_by_number(&repo, pull.number)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(current.state, PullRequestState::Open);
        assert!(
            parse_metadata_block(&current.body)
                .unwrap()
                .unwrap()
                .exact_head_validation
                .is_none(),
            "stale landing authority must be removed"
        );
        let plan_labels = issue_labels(&forge, &repo, plan).await;
        assert!(
            has_label(&plan_labels, "needs-validation"),
            "{plan_labels:?}"
        );
        assert!(!has_label(&plan_labels, "validated"), "{plan_labels:?}");
    });
}

#[test]
fn mechanical_merge_accepts_only_complete_current_authority() {
    temper_engine_io::block_on(async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "main").await;
        let (feature, plan) = create_exact_plan(&forge, &repo).await;
        forge
            .set_branch_head(&repo, SOURCE_BRANCH, HEAD_A)
            .expect("head A");
        let workflow = Arc::new(exact_workflow());
        let applier = ForgeApplier::new(forge.clone(), workflow.clone());
        let job = exact_job(plan, feature, "attempt-head-a");
        assert_eq!(
            applier
                .apply(job.clone(), exact_result(&job, plan, feature, HEAD_A))
                .await,
            temper_engine::ApplyOutcome::Applied
        );
        let pull = forge
            .list_pull_requests(&repo, PullRequestQuery::default())
            .await
            .unwrap()
            .remove(0);
        seed_success_ci(&forge, &repo, &pull);

        let report = workflow
            .executor(forge.as_ref())
            .execute(
                &repo,
                temper_workflow::ArtifactSource::PullRequest {
                    number: pull.number,
                },
                &TransitionId::new("land_feature_pr"),
                &RoleId::new("mechanical"),
            )
            .await
            .expect("current exact-head authority lands");
        assert!(
            report
                .applied
                .contains(&temper_workflow::WorkflowEffect::MergePullRequest)
        );
        let merged = forge
            .get_pull_request_by_number(&repo, pull.number)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(merged.state, PullRequestState::Merged);
    });
}

fn seed_success_ci(forge: &MemoryForge, repo: &RepositoryId, pull: &PullRequest) {
    let timestamp = ts("2026-05-29T00:00:00Z");
    forge.seed_ci_jobs(
        repo,
        vec![CiJob {
            id: CiJobId::new("exact-head-ci"),
            repo_id: repo.clone(),
            pull_request_id: Some(pull.id.clone()),
            commit_sha: pull.head_sha.clone().expect("landing head"),
            name: "exact-head".to_string(),
            status: CiJobStatus::Completed,
            conclusion: Some(CiJobConclusion::Success),
            provider_conclusion: None,
            provider_reason: None,
            run_id: None,
            attempt: None,
            verified_failure: None,
            url: None,
            created_at: timestamp,
            started_at: Some(timestamp),
            completed_at: Some(timestamp),
            updated_at: timestamp,
        }],
    );
}
