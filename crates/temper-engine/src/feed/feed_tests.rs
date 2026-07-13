// SPDX-License-Identifier: MPL-2.0

//! Unit tests for the work-item feed: `WorkItem` mapping and enrichment.

use super::*;
use serde_json::json;
use std::sync::Arc;
use temper_forge::{
    BranchRef, CreatePullRequest, CreateRepository, Forge, IssueState, ItemNumber, RepositoryId,
    UpdateIssue, UpdatePullRequest,
};
use temper_forge_memory::MemoryForge;
use temper_protocol_worker::{Artifact, JobContext};
use temper_workflow::{
    ArtifactKindId, ArtifactSource, CompiledWorkflow, DurableAssignment, QueueId, RawWorkflowSpec,
    RoleId, ValidatedWorkflow, WorkflowMetadata, render_metadata_block,
};

use crate::Daemon;

const BASIC_DELIVERY_FIXTURE: &str =
    include_str!("../../../temper-workflow/fixtures/basic-delivery.json");
const REFERENCE_DELIVERY_FIXTURE: &str =
    include_str!("../../../temper-workflow/fixtures/reference-delivery.json");

#[path = "feed_tests/action_assignment.rs"]
mod action_assignment;
#[path = "feed_tests/artifact_context_dispatch.rs"]
mod artifact_context_dispatch;
#[path = "feed_tests/attention.rs"]
mod attention;
#[path = "feed_tests/reconciliation.rs"]
mod reconciliation;
#[path = "feed_tests/target_branch.rs"]
mod target_branch;
#[path = "feed_tests/targeted.rs"]
mod targeted;

fn work_item(target: ArtifactSource) -> WorkItem {
    WorkItem {
        queue: QueueId::new("code_ready"),
        role: RoleId::new("engineer"),
        target,
        kind: ArtifactKindId::new("code"),
    }
}

#[test]
fn skip_log_reason_names_existing_pull_request_concisely() {
    assert_eq!(
        skip_log_reason(EnrichOutcome::SkipExistingPullRequest),
        "existing-pr"
    );
}

#[test]
fn skip_log_line_includes_existing_pull_request_reason() {
    let item = work_item(ArtifactSource::Issue {
        number: ItemNumber::new(153),
    });

    assert_eq!(
        skip_log_line(
            "ai/temper",
            &RoleId::new("engineer"),
            &item,
            EnrichOutcome::SkipExistingPullRequest
        ),
        "engine: skip existing-pr ai/temper#153 role=engineer queue=code_ready kind=code"
    );
}

#[test]
fn enrich_work_item_job_skips_merged_correlated_implementation_pr() {
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
                temper_forge::CreateIssue {
                    title: "ready".to_string(),
                    body: "needs implementation".to_string(),
                    labels: vec!["code".to_string(), "ready".to_string()],
                    assignees: Vec::new(),
                },
            )
            .await
            .expect("issue is created");
        let correlation_key = format!("pr-for-code-{}", issue.number.get());
        let pull_request = forge
            .create_pull_request(
                &repo,
                CreatePullRequest {
                    title: "Implement ready issue".to_string(),
                    body: format!(
                        "Implementation PR.\n\n{}",
                        render_metadata_block(&WorkflowMetadata {
                            correlation_key: Some(correlation_key.clone()),
                            ..WorkflowMetadata::default()
                        })
                    ),
                    source: BranchRef {
                        repository_id: repo.clone(),
                        branch: format!("agent/{correlation_key}"),
                    },
                    target: BranchRef {
                        repository_id: repo.clone(),
                        branch: "main".to_string(),
                    },
                    labels: vec!["implementation".to_string()],
                    assignees: Vec::new(),
                },
            )
            .await
            .expect("pull request is created");
        forge
            .merge_pull_request(
                &pull_request.id,
                temper_forge::MergePullRequest {
                    method: temper_forge::MergeMethod::Squash,
                    commit_title: None,
                    commit_body: None,
                    delete_source_branch: false,
                },
            )
            .await
            .expect("pull request is merged");
        let item = work_item(ArtifactSource::Issue {
            number: issue.number,
        });
        let mut job = job_from_work_item("ai/temper", &item);
        let workflow: RawWorkflowSpec =
            serde_json::from_str(BASIC_DELIVERY_FIXTURE).expect("basic-delivery workflow parses");
        let workflow = workflow
            .validate()
            .expect("basic-delivery workflow validates");
        let compiled = workflow.compile();

        assert_eq!(
            enrich_work_item_job(&forge, &repo, &item, &mut job, &workflow, &compiled)
                .await
                .expect("enrichment skip succeeds"),
            EnrichOutcome::SkipExistingPullRequest
        );
    })
}

#[test]
fn maps_issue_work_item_to_daemon_job() {
    let item = work_item(ArtifactSource::Issue {
        number: ItemNumber::new(103),
    });

    let job = job_from_work_item("ai/temper", &item);

    assert_eq!(job.job_id, "ai/temper/issue-103/engineer/code_ready");
    assert_eq!(job.role, "engineer");
    assert_eq!(job.repo, "ai/temper");
    assert_eq!(
        job.artifact,
        Artifact {
            item: json!(103),
            kind: "issue".to_string(),
        }
    );
    assert_eq!(
        job.job_payload,
        json!({
            "role": "engineer",
            "repo": "ai/temper",
            "queue": "code_ready",
            "artifact_kind": "code"
        })
    );
    assert_eq!(
        serde_json::from_value::<JobContext>(job.job_payload).expect("valid JobContext"),
        JobContext {
            artifact_context: None,
            role: "engineer".to_string(),
            repo: "ai/temper".to_string(),
            queue: "code_ready".to_string(),
            artifact_kind: "code".to_string(),
            artifact: None,
            workspace: None,
            action: None,
            checkout_capability: None,
            allowed_verdicts: Vec::new(),
            verdict_contracts: Default::default(),
            source_metadata: Default::default(),
            guidance: None,
            pull_request_freshness: None,
        }
    );
}

#[test]
fn maps_pull_request_work_item_to_daemon_job() {
    let item = work_item(ArtifactSource::PullRequest {
        number: ItemNumber::new(42),
    });

    let job = job_from_work_item("ai/temper", &item);

    assert_eq!(job.artifact.kind, "pull_request");
    assert!(job.job_id.contains("/pull_request-42/"));
    assert_eq!(job.artifact.item, json!(42));
}

#[test]
fn work_item_job_mapping_is_deterministic() {
    let item = work_item(ArtifactSource::Issue {
        number: ItemNumber::new(103),
    });

    assert_eq!(
        job_from_work_item("ai/temper", &item),
        job_from_work_item("ai/temper", &item)
    );
}

#[test]
fn enqueue_work_item_stores_mapped_job() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let daemon = Daemon::new(Arc::new(handle));
        let item = work_item(ArtifactSource::Issue {
            number: ItemNumber::new(103),
        });
        let expected = job_from_work_item("ai/temper", &item);

        daemon.enqueue_work_item("ai/temper", &item).await;

        assert_eq!(daemon.queued_jobs().await, vec![expected]);
    })
}

#[test]
fn enrich_work_item_job_skips_closed_issue() {
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
                temper_forge::CreateIssue {
                    title: "closed".to_string(),
                    body: "done".to_string(),
                    labels: vec!["code".to_string(), "ready".to_string()],
                    assignees: Vec::new(),
                },
            )
            .await
            .expect("issue is created");
        forge
            .update_issue(
                &issue.id,
                UpdateIssue {
                    state: Some(IssueState::Closed),
                    ..UpdateIssue::default()
                },
            )
            .await
            .expect("issue is closed");
        let item = work_item(ArtifactSource::Issue {
            number: issue.number,
        });
        let mut job = job_from_work_item("ai/temper", &item);
        let workflow: RawWorkflowSpec =
            serde_json::from_str(BASIC_DELIVERY_FIXTURE).expect("basic-delivery workflow parses");
        let workflow = workflow
            .validate()
            .expect("basic-delivery workflow validates");
        let compiled = workflow.compile();

        assert_eq!(
            enrich_work_item_job(&forge, &repo, &item, &mut job, &workflow, &compiled)
                .await
                .expect("enrichment skip succeeds"),
            EnrichOutcome::SkipTerminalArtifact
        );
    })
}

#[test]
fn enrich_work_item_job_enriches_open_pull_request_artifact_snapshot() {
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
        let pull_request = forge
            .create_pull_request(
                &repo,
                CreatePullRequest {
                    title: "Fix failing CI".to_string(),
                    body: "Address the failing PR.".to_string(),
                    source: BranchRef {
                        repository_id: repo.clone(),
                        branch: "agent/pr-for-code-42".to_string(),
                    },
                    target: BranchRef {
                        repository_id: repo.clone(),
                        branch: "main".to_string(),
                    },
                    labels: vec!["implementation".to_string()],
                    assignees: Vec::new(),
                },
            )
            .await
            .expect("pull request is created");
        let item = work_item(ArtifactSource::PullRequest {
            number: pull_request.number,
        });
        let mut job = job_from_work_item("ai/temper", &item);
        let workflow: RawWorkflowSpec =
            serde_json::from_str(BASIC_DELIVERY_FIXTURE).expect("basic-delivery workflow parses");
        let workflow = workflow
            .validate()
            .expect("basic-delivery workflow validates");
        let compiled = workflow.compile();

        assert_eq!(
            enrich_work_item_job(&forge, &repo, &item, &mut job, &workflow, &compiled)
                .await
                .expect("enrichment succeeds for pull request targets"),
            EnrichOutcome::Enriched
        );

        let context: JobContext =
            serde_json::from_value(job.job_payload).expect("enriched JobContext parses");
        // A non-coordinated PR job gets a degenerate single-repo manifest:
        // one writable primary on the coordination branch.
        let workspace = context
            .workspace
            .as_ref()
            .expect("enriched job carries a workspace manifest");
        assert_eq!(
            workspace.coordination_key,
            format!("pr-for-code-{}", pull_request.number.get())
        );
        assert_eq!(workspace.repos.len(), 1);
        let primary = workspace.primary().expect("primary repo present");
        assert_eq!(primary.repo, "ai/temper");
        assert_eq!(primary.dir, "temper");
        assert!(primary.is_writable());
        assert_eq!(primary.default_branch, "main");
        assert_eq!(primary.base_branch, "main");
        assert_eq!(
            primary.branch_hint.as_deref(),
            Some(format!("agent/pr-for-code-{}", pull_request.number.get()).as_str())
        );
        let artifact = context.artifact.expect("pull request snapshot is present");
        assert_eq!(artifact.number, pull_request.number.get());
        assert_eq!(artifact.title, "Fix failing CI");
        assert_eq!(artifact.body, "Address the failing PR.");
        assert_eq!(artifact.labels, vec!["implementation".to_string()]);
        assert_eq!(artifact.state, "Open");
        assert_eq!(context.action.as_deref(), Some("open_pr"));
        assert_eq!(context.checkout_capability.as_deref(), Some("writable"));
        assert!(context.allowed_verdicts.is_empty());
    })
}

#[test]
fn enrich_ci_failed_pull_request_becomes_writable_head_fix_with_guidance() {
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
        let source_coordination_key = "pr-for-code-226";
        let pull_request = forge
            .create_pull_request(
                &repo,
                CreatePullRequest {
                    title: "Implement #226".to_string(),
                    body: format!(
                        "Applied the change.\n\n{}",
                        render_metadata_block(&WorkflowMetadata {
                            correlation_key: Some(source_coordination_key.to_string()),
                            ..WorkflowMetadata::default()
                        })
                    ),
                    source: BranchRef {
                        repository_id: repo.clone(),
                        branch: "agent/pr-for-code-226".to_string(),
                    },
                    target: BranchRef {
                        repository_id: repo.clone(),
                        branch: "main".to_string(),
                    },
                    labels: vec!["implementation".to_string()],
                    assignees: Vec::new(),
                },
            )
            .await
            .expect("pull request is created");
        let pull_request = forge
            .set_pull_request_head(&pull_request.id, Some("abc123".to_string()))
            .expect("pull request head sha is set");

        // Seed a FAILED CI job on the PR so the feed reads it into guidance.
        let head_sha = pull_request.head_sha.clone().unwrap_or_default();
        forge.seed_ci_jobs(
            &repo,
            vec![temper_forge::CiJob {
                id: temper_forge::CiJobId::new("ci-validate-1"),
                repo_id: repo.clone(),
                pull_request_id: Some(pull_request.id.clone()),
                commit_sha: head_sha,
                name: "validate".to_string(),
                status: temper_forge::CiJobStatus::Completed,
                conclusion: Some(temper_forge::CiJobConclusion::Failure),
                url: Some("https://ci.example.test/jobs/validate".to_string()),
                created_at: chrono::DateTime::<chrono::Utc>::from_timestamp(1, 0).unwrap(),
                started_at: None,
                completed_at: None,
                updated_at: chrono::DateTime::<chrono::Utc>::from_timestamp(1, 0).unwrap(),
            }],
        );

        // A `pr_ci_failed`-queue member for the implementation PR.
        let item = WorkItem {
            queue: QueueId::new("pr_ci_failed"),
            role: RoleId::new("engineer"),
            target: ArtifactSource::PullRequest {
                number: pull_request.number,
            },
            kind: ArtifactKindId::new("implementation_pr"),
        };
        let mut job = job_from_work_item("ai/temper", &item);
        let workflow: RawWorkflowSpec =
            serde_json::from_str(BASIC_DELIVERY_FIXTURE).expect("basic-delivery workflow parses");
        let workflow = workflow
            .validate()
            .expect("basic-delivery workflow validates");
        let compiled = workflow.compile();

        assert_eq!(
            enrich_work_item_job(&forge, &repo, &item, &mut job, &workflow, &compiled)
                .await
                .expect("enrichment succeeds for ci-failed pull request"),
            EnrichOutcome::Enriched
        );

        let context: JobContext =
            serde_json::from_value(job.job_payload).expect("enriched JobContext parses");
        assert_eq!(context.action.as_deref(), Some("address_ci_failure"));
        // Writable checkout of the PR's REAL head branch (not a synthetic one).
        assert_eq!(
            context.checkout_capability.as_deref(),
            Some("pull_request_writable")
        );
        let freshness = context
            .pull_request_freshness
            .as_ref()
            .expect("PR-head freshness guard is present");
        assert_eq!(freshness.queue, "pr_ci_failed");
        assert_eq!(freshness.queue_condition.as_deref(), Some("ci_failed"));
        assert_eq!(freshness.pull_request_id, pull_request.id.as_str());
        assert_eq!(freshness.head_sha, pull_request.head_sha);
        let workspace = context.workspace.as_ref().expect("manifest present");
        assert_eq!(workspace.coordination_key, source_coordination_key);
        let primary = workspace.primary().expect("primary repo present");
        assert!(primary.is_writable());
        assert_eq!(
            primary.branch_hint.as_deref(),
            Some("agent/pr-for-code-226")
        );
        assert_eq!(primary.base_branch, "main");
        // Guidance surfaces the durable PR handoff plus fresh structured CI gate details.
        let guidance = context.guidance.expect("ci-failure guidance present");
        assert!(
            guidance.contains("Current implementation PR handoff from Forge"),
            "guidance: {guidance}"
        );
        assert!(guidance.contains("Implement #226"), "guidance: {guidance}");
        assert!(
            guidance.contains("Applied the change."),
            "guidance: {guidance}"
        );
        assert!(
            guidance.contains("head_branch: agent/pr-for-code-226"),
            "guidance: {guidance}"
        );
        assert!(
            guidance.contains("base_branch: main"),
            "guidance: {guidance}"
        );
        assert!(
            guidance.contains("head_sha: abc123"),
            "guidance: {guidance}"
        );
        assert!(
            guidance.contains("reason: ci_failed"),
            "guidance: {guidance}"
        );
        assert!(guidance.contains("name: validate"), "guidance: {guidance}");
        assert!(
            guidance.contains("status: completed"),
            "guidance: {guidance}"
        );
        assert!(
            guidance.contains("conclusion: failure"),
            "guidance: {guidance}"
        );
        assert!(
            guidance.contains("commit_sha: abc123"),
            "guidance: {guidance}"
        );
        assert!(
            guidance.contains("url: https://ci.example.test/jobs/validate"),
            "guidance: {guidance}"
        );
        assert!(
            guidance.contains("updated current PR `title`"),
            "guidance: {guidance}"
        );
        assert!(
            guidance.contains("implementation-report `body`"),
            "guidance: {guidance}"
        );
    })
}

#[test]
fn enrich_work_item_job_skips_closed_pull_request() {
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
        let pull_request = forge
            .create_pull_request(
                &repo,
                CreatePullRequest {
                    title: "closed PR".to_string(),
                    body: "closed".to_string(),
                    source: BranchRef {
                        repository_id: repo.clone(),
                        branch: "agent/pr-for-code-7".to_string(),
                    },
                    target: BranchRef {
                        repository_id: repo.clone(),
                        branch: "main".to_string(),
                    },
                    labels: vec!["implementation".to_string()],
                    assignees: Vec::new(),
                },
            )
            .await
            .expect("pull request is created");
        forge
            .update_pull_request(
                &pull_request.id,
                UpdatePullRequest {
                    state: Some(temper_forge::PullRequestUpdateState::Closed),
                    ..UpdatePullRequest::default()
                },
            )
            .await
            .expect("pull request is closed");
        let item = work_item(ArtifactSource::PullRequest {
            number: pull_request.number,
        });
        let mut job = job_from_work_item("ai/temper", &item);
        let workflow: RawWorkflowSpec =
            serde_json::from_str(BASIC_DELIVERY_FIXTURE).expect("basic-delivery workflow parses");
        let workflow = workflow
            .validate()
            .expect("basic-delivery workflow validates");
        let compiled = workflow.compile();

        assert_eq!(
            enrich_work_item_job(&forge, &repo, &item, &mut job, &workflow, &compiled)
                .await
                .expect("enrichment skip succeeds"),
            EnrichOutcome::SkipTerminalArtifact
        );
    })
}
