//! Behavior tests for the deterministic reference-delivery fake agents.

use std::sync::Arc;
use temper_forge_model::{
    CreatePullRequestReview, CreateRepository, Forge, IssueQuery, IssueState, ItemNumber,
    PullRequestState, RepositoryId, RequestReviewers, ReviewDecision, UserId,
};
use temper_forge_memory::MemoryForge;
use temper_runner::{Agent, CiSink, Progress, RoleTools, RoleWorker, WorkItem, Worker};
use temper_workflow::{
    ArtifactKindId, ArtifactRef, ArtifactSource, QueueId, RoleId, WorkflowMetadata,
    parse_metadata_block, render_metadata_block,
};

use temper_testing::agents::{
    ARCHITECT_PLAN_BEGIN, ClosingArchitect, FakeArchitect, FakeEngineer, FakeHuman, FakeOwner,
    FakeReviewer,
};
use temper_testing::ci::MemoryCiSink;
use temper_testing::{
    actor_user, block_on, create_issue, create_pr, labels, new_repo, runner_config, ts,
};

#[test]
fn architect_fake_triages_intake_and_reconciles_landed_pr() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let intake = create_issue(&forge, &repo, &["untriaged"], "new request", "");
    let landed = create_pr(&forge, &repo, &["implementation", "landed"], "impl", "");
    let workflow = temper_testing::workflow();
    let compiled = workflow.compile();
    let architect_forge = forge.as_user(actor_user("architect"));
    let worker = RoleWorker::new(
        &workflow,
        &compiled,
        &architect_forge,
        &repo,
        RoleId::new("architect"),
        Arc::new(FakeArchitect),
        runner_config().execution_context(&RoleId::new("architect")),
    );

    assert_eq!(
        tick(&worker),
        Progress {
            changed: true,
            actions: 2
        }
    );

    let issue = block_on(forge.get_issue_by_number(&repo, intake))
        .expect("lookup succeeds")
        .expect("issue exists");
    assert_eq!(labels(issue.labels), vec!["code", "ready"]);
    let pr = block_on(forge.get_pull_request_by_number(&repo, landed))
        .expect("lookup succeeds")
        .expect("pull request exists");
    assert_eq!(labels(pr.labels), vec!["implementation"]);
}

#[test]
fn closing_architect_clears_in_progress_when_closing_produced_issue() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let parent = create_issue(&forge, &repo, &["code", "in-progress"], "build thing", "");
    let pr_body = render_metadata_block(&WorkflowMetadata {
        kind: Some(ArtifactKindId::new("implementation_pr")),
        parents: vec![ArtifactRef::same_repo(parent)],
        ..WorkflowMetadata::default()
    });
    create_pr(
        &forge,
        &repo,
        &["implementation", "landed"],
        "impl",
        &pr_body,
    );
    let workflow = temper_testing::workflow();
    let compiled = workflow.compile();
    let architect_forge = forge.as_user(actor_user("architect"));
    let worker = RoleWorker::new(
        &workflow,
        &compiled,
        &architect_forge,
        &repo,
        RoleId::new("architect"),
        Arc::new(ClosingArchitect),
        runner_config().execution_context(&RoleId::new("architect")),
    );

    assert_eq!(
        tick(&worker),
        Progress {
            changed: true,
            actions: 1
        }
    );

    let issue = block_on(forge.get_issue_by_number(&repo, parent))
        .expect("lookup succeeds")
        .expect("parent issue exists");
    assert_eq!(issue.state, IssueState::Closed);
    assert_eq!(labels(issue.labels), vec!["code"]);
}

#[test]
fn architect_fake_fans_out_cross_repo_children_idempotently() {
    let forge = MemoryForge::new();
    let source_repo = new_repo(&forge);
    let target_repo = memory_repo(&forge, "service-canary");
    let body = architect_plan_body(&source_repo, &target_repo);
    let parent = create_issue(
        &forge,
        &source_repo,
        &["untriaged"],
        "cross repo work",
        &body,
    );
    let workflow = temper_testing::workflow();
    let compiled = workflow.compile();
    let architect_forge = forge.as_user(actor_user("architect"));
    let worker = RoleWorker::new(
        &workflow,
        &compiled,
        &architect_forge,
        &source_repo,
        RoleId::new("architect"),
        Arc::new(FakeArchitect),
        runner_config().execution_context(&RoleId::new("architect")),
    );

    assert_eq!(
        tick(&worker),
        Progress {
            changed: true,
            actions: 1
        }
    );

    let parent_issue = block_on(forge.get_issue_by_number(&source_repo, parent))
        .expect("lookup succeeds")
        .expect("parent exists");
    assert_eq!(labels(parent_issue.labels), vec!["blocked", "code"]);
    let parent_metadata = parse_metadata_block(&parent_issue.body)
        .expect("parent metadata parses")
        .expect("parent metadata exists");
    assert_eq!(parent_metadata.dependencies.len(), 2);

    let source_child = assert_child(&forge, &source_repo, "service child", &source_repo, parent);
    let target_child = assert_child(&forge, &target_repo, "canary child", &source_repo, parent);
    assert!(
        parent_metadata
            .dependencies
            .contains(&ArtifactRef::in_repo(source_repo.clone(), source_child))
    );
    assert!(
        parent_metadata
            .dependencies
            .contains(&ArtifactRef::in_repo(target_repo.clone(), target_child))
    );

    assert_eq!(
        tick(&worker),
        Progress {
            changed: false,
            actions: 0
        }
    );
    assert_eq!(issue_count(&forge, &source_repo), 2);
    assert_eq!(issue_count(&forge, &target_repo), 1);
}

#[test]
fn architect_fake_same_repo_without_plan_matches_plain_triage() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let intake = create_issue(&forge, &repo, &["untriaged"], "plain request", "");
    let workflow = temper_testing::workflow();
    let compiled = workflow.compile();
    let architect_forge = forge.as_user(actor_user("architect"));
    let worker = RoleWorker::new(
        &workflow,
        &compiled,
        &architect_forge,
        &repo,
        RoleId::new("architect"),
        Arc::new(FakeArchitect),
        runner_config().execution_context(&RoleId::new("architect")),
    );

    assert_eq!(
        tick(&worker),
        Progress {
            changed: true,
            actions: 1
        }
    );

    let issue = block_on(forge.get_issue_by_number(&repo, intake))
        .expect("lookup succeeds")
        .expect("issue exists");
    assert_eq!(labels(issue.labels), vec!["code", "ready"]);
    assert!(
        parse_metadata_block(&issue.body)
            .expect("metadata parses")
            .is_none()
    );
    assert_eq!(issue_count(&forge, &repo), 1);
}

#[test]
fn engineer_fake_claims_code_opens_pr_and_requests_review() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let code = create_issue(&forge, &repo, &["code", "ready"], "build thing", "");
    let workflow = temper_testing::workflow();
    let compiled = workflow.compile();
    let engineer_forge = forge.as_user(actor_user("engineer"));
    let worker = RoleWorker::new(
        &workflow,
        &compiled,
        &engineer_forge,
        &repo,
        RoleId::new("engineer"),
        Arc::new(FakeEngineer),
        runner_config().execution_context(&RoleId::new("engineer")),
    );

    assert_eq!(
        tick(&worker),
        Progress {
            changed: true,
            actions: 1
        }
    );

    let issue = block_on(forge.get_issue_by_number(&repo, code))
        .expect("lookup succeeds")
        .expect("issue exists");
    assert_eq!(labels(issue.labels), vec!["code", "in-progress"]);
    assert_eq!(issue.assignees, vec![UserId::new("engineer")]);

    let pull_requests =
        block_on(forge.list_pull_requests(&repo, Default::default())).expect("list succeeds");
    assert_eq!(pull_requests.len(), 1);
    let pr = &pull_requests[0];
    assert_eq!(
        labels(pr.labels.clone()),
        vec!["implementation", "needs-reviewer"]
    );
    assert_eq!(pr.requested_reviewers, vec![UserId::new("reviewer")]);
    assert!(pr.body.contains("pr-for-code-"));
    assert!(pr.body.contains("\"parents\": ["));
}

#[test]
fn reviewer_fake_approves_as_requested_reviewer() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let number = create_pr(
        &forge,
        &repo,
        &["implementation", "needs-reviewer"],
        "impl",
        "",
    );
    request_reviewer(&forge, &repo, number);
    let workflow = temper_testing::workflow();
    let compiled = workflow.compile();
    let reviewer_forge = forge.as_user(actor_user("reviewer"));
    let worker = RoleWorker::new(
        &workflow,
        &compiled,
        &reviewer_forge,
        &repo,
        RoleId::new("reviewer"),
        Arc::new(FakeReviewer),
        runner_config().execution_context(&RoleId::new("reviewer")),
    );

    assert_eq!(
        tick(&worker),
        Progress {
            changed: true,
            actions: 1
        }
    );

    let pr = block_on(forge.get_pull_request_by_number(&repo, number))
        .expect("lookup succeeds")
        .expect("pull request exists");
    assert_eq!(labels(pr.labels), vec!["implementation", "landing"]);
    let reviews = block_on(forge.list_pull_request_reviews(&pr.id)).expect("reviews list");
    assert_eq!(reviews.len(), 1);
    assert_eq!(reviews[0].reviewer_id, UserId::new("reviewer"));
    assert_eq!(reviews[0].decision, ReviewDecision::Approved);
}

#[test]
fn owner_fake_handles_owner_and_alignment_queues() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let design = create_issue(&forge, &repo, &["design", "needs-owner"], "design", "");
    let prs: Vec<_> = (0..5)
        .map(|index| {
            create_pr(
                &forge,
                &repo,
                &["implementation", "alignment"],
                &format!("impl {index}"),
                "",
            )
        })
        .collect();
    let workflow = temper_testing::workflow();
    let compiled = workflow.compile();
    let owner_forge = forge.as_user(actor_user("owner"));
    let worker = RoleWorker::new(
        &workflow,
        &compiled,
        &owner_forge,
        &repo,
        RoleId::new("owner"),
        Arc::new(FakeOwner),
        runner_config().execution_context(&RoleId::new("owner")),
    );

    assert_eq!(
        tick(&worker),
        Progress {
            changed: true,
            actions: 6
        }
    );

    let issue = block_on(forge.get_issue_by_number(&repo, design))
        .expect("lookup succeeds")
        .expect("issue exists");
    assert_eq!(labels(issue.labels), vec!["design", "needs-human"]);
    for number in prs {
        let pr = block_on(forge.get_pull_request_by_number(&repo, number))
            .expect("lookup succeeds")
            .expect("pull request exists");
        assert_eq!(labels(pr.labels), vec!["implementation"]);
    }
}

#[test]
fn human_fake_clears_human_flag() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let design = create_issue(&forge, &repo, &["design", "needs-human"], "design", "");
    let workflow = temper_testing::workflow();
    let compiled = workflow.compile();
    let human_forge = forge.as_user(actor_user("human"));
    let worker = RoleWorker::new(
        &workflow,
        &compiled,
        &human_forge,
        &repo,
        RoleId::new("human"),
        Arc::new(FakeHuman),
        runner_config().execution_context(&RoleId::new("human")),
    );

    assert_eq!(
        tick(&worker),
        Progress {
            changed: true,
            actions: 1
        }
    );

    let issue = block_on(forge.get_issue_by_number(&repo, design))
        .expect("lookup succeeds")
        .expect("issue exists");
    assert_eq!(labels(issue.labels), vec!["design"]);
}

#[test]
fn owner_fake_ignores_landing_prs_because_merges_are_mechanical() {
    let forge = MemoryForge::new();
    let repo = new_repo(&forge);
    let number = create_pr(&forge, &repo, &["implementation", "landing"], "impl", "");
    approve_as_requested_reviewer(&forge, &repo, number);
    block_on(MemoryCiSink::new(forge.clone()).record(
        &repo,
        number,
        temper_forge_model::CiJobConclusion::Success,
    ))
    .expect("CI recorded");
    let workflow = temper_testing::workflow();
    let owner_forge = forge.as_user(actor_user("owner"));
    let tools = RoleTools::new(
        &workflow,
        &owner_forge,
        &repo,
        RoleId::new("owner"),
        runner_config().execution_context(&RoleId::new("owner")),
    );
    let item = WorkItem {
        queue: QueueId::new("landing"),
        role: RoleId::new("owner"),
        target: ArtifactSource::PullRequest { number },
        kind: ArtifactKindId::new("implementation_pr"),
    };

    assert!(!block_on(FakeOwner.service(&item, &tools)).expect("owner ignores landing PR"));

    let pr = block_on(forge.get_pull_request_by_number(&repo, number))
        .expect("lookup succeeds")
        .expect("pull request exists");
    assert_eq!(pr.state, PullRequestState::Open);
    assert_eq!(labels(pr.labels), vec!["implementation", "landing"]);
}

fn tick(worker: &impl Worker) -> Progress {
    block_on(worker.tick(ts("2026-05-29T00:00:00Z"))).expect("tick succeeds")
}

fn memory_repo(forge: &MemoryForge, name: &str) -> RepositoryId {
    block_on(forge.create_repository(CreateRepository {
        owner: "acme".into(),
        name: name.into(),
        default_branch: "main".into(),
        description: None,
    }))
    .expect("repository created")
    .id
}

fn architect_plan_body(source_repo: &RepositoryId, target_repo: &RepositoryId) -> String {
    format!(
        "Split this across services.\n\n{ARCHITECT_PLAN_BEGIN}\n{{\n  \"children\": [\n    {{\n      \"slug\": \"service\",\n      \"target_repo\": \"{source_repo}\",\n      \"title\": \"service child\",\n      \"body\": \"Implement service side.\"\n    }},\n    {{\n      \"slug\": \"canary\",\n      \"target_repo\": \"{target_repo}\",\n      \"title\": \"canary child\",\n      \"body\": \"Implement canary side.\"\n    }}\n  ]\n}}\n-->"
    )
}

fn assert_child(
    forge: &MemoryForge,
    repo: &RepositoryId,
    title: &str,
    parent_repo: &RepositoryId,
    parent: ItemNumber,
) -> ItemNumber {
    let issues = block_on(forge.list_issues(repo, IssueQuery::default())).expect("issues list");
    let issue = issues
        .iter()
        .find(|issue| issue.title == title)
        .expect("child exists");
    assert_eq!(labels(issue.labels.clone()), vec!["code", "ready"]);
    let metadata = parse_metadata_block(&issue.body)
        .expect("child metadata parses")
        .expect("child metadata exists");
    assert!(
        metadata
            .parents
            .contains(&ArtifactRef::in_repo(parent_repo.clone(), parent))
    );
    assert!(metadata.correlation_key.is_some());
    issue.number
}

fn issue_count(forge: &MemoryForge, repo: &RepositoryId) -> usize {
    block_on(forge.list_issues(repo, IssueQuery::default()))
        .expect("issues list")
        .len()
}

fn request_reviewer(forge: &MemoryForge, repo: &temper_forge_model::RepositoryId, number: ItemNumber) {
    let pr = block_on(forge.get_pull_request_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("pull request exists");
    block_on(forge.request_pull_request_reviewers(
        &pr.id,
        RequestReviewers {
            reviewers: vec![UserId::new("reviewer")],
        },
    ))
    .expect("reviewer requested");
}

fn approve_as_requested_reviewer(
    forge: &MemoryForge,
    repo: &temper_forge_model::RepositoryId,
    number: ItemNumber,
) {
    request_reviewer(forge, repo, number);
    let reviewer_forge = forge.as_user(actor_user("reviewer"));
    let pr = block_on(forge.get_pull_request_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("pull request exists");
    block_on(reviewer_forge.submit_pull_request_review(
        &pr.id,
        CreatePullRequestReview {
            decision: ReviewDecision::Approved,
            body: None,
        },
    ))
    .expect("review submitted");
}
