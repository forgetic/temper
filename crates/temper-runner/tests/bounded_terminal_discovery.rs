//! Production-shaped bounded terminal discovery and recovery regressions.

mod support;

use chrono::{DateTime, Utc};
use support::{CountedForgeOp, CountingForge};
use temper_forge::{
    BranchRef, CreateIssue, CreatePullRequest, Forge, IssueState, ItemNumber, MergeMethod,
    MergePullRequest, PullRequestState, RepositoryId, UpdateIssue, UserId,
};
use temper_forge_memory::MemoryForge;
use temper_runner::{
    MechanicalWorker, Progress, TerminalDiscoveryRead, TerminalDiscoveryState, Worker,
    prepare_terminal_discovery_generation, retain_terminal_discovery_target,
    scan_roles_wake_with_discovery,
};
use temper_workflow::{
    ArtifactKindId, ArtifactSource, CommandId, CommandJournal, CommandRecord, CommandState,
    InMemoryJournal, Lease, LeasePolicy, RawWorkflowSpec, RoleId, TransitionId, WorkflowEffect,
    WorkflowMetadata, parse_metadata_block, render_metadata_block,
};

const FIXTURE: &str = include_str!("../../temper-workflow/fixtures/reference-delivery.json");

fn timestamp(value: &str) -> DateTime<Utc> {
    value.parse().expect("timestamp")
}

fn workflow() -> temper_workflow::ValidatedWorkflow {
    let mut raw: serde_json::Value = serde_json::from_str(FIXTURE).expect("workflow parses");
    raw["labels"]
        .as_array_mut()
        .expect("labels")
        .push(serde_json::json!({
            "id": "actionable",
            "description": "test-only terminal action evidence"
        }));
    let queue = raw["queues"]
        .as_array_mut()
        .expect("queues")
        .iter_mut()
        .find(|queue| queue["id"] == "landed_inbox")
        .expect("landed inbox");
    queue["labels"] = serde_json::json!(["landed", "actionable"]);
    serde_json::from_value::<RawWorkflowSpec>(raw)
        .expect("workflow shape")
        .validate()
        .expect("workflow validates")
}

fn repository(forge: &MemoryForge) -> RepositoryId {
    temper_testing::block_on(forge.create_repository(temper_forge::CreateRepository {
        owner: "acme".into(),
        name: "history".into(),
        default_branch: "main".into(),
        description: None,
    }))
    .expect("repository")
    .id
}

fn pull_request(
    forge: &MemoryForge,
    repo: &RepositoryId,
    labels: &[&str],
    body: String,
) -> ItemNumber {
    let pull_request = temper_testing::block_on(forge.create_pull_request(
        repo,
        CreatePullRequest {
            title: "historical PR".into(),
            body,
            source: BranchRef {
                repository_id: repo.clone(),
                branch: format!("history-{}", labels.join("-")),
            },
            target: BranchRef {
                repository_id: repo.clone(),
                branch: "main".into(),
            },
            labels: labels.iter().map(|label| (*label).to_string()).collect(),
            assignees: Vec::<UserId>::new(),
        },
    ))
    .expect("pull request");
    temper_testing::block_on(forge.merge_pull_request(
        &pull_request.id,
        MergePullRequest {
            method: MergeMethod::Squash,
            commit_title: None,
            commit_body: None,
            delete_source_branch: false,
        },
    ))
    .expect("merge");
    pull_request.number
}

fn closed_issue(forge: &MemoryForge, repo: &RepositoryId) {
    let issue = temper_testing::block_on(forge.create_issue(
        repo,
        CreateIssue {
            title: "historical issue".into(),
            body: String::new(),
            labels: vec!["landed".into()],
            assignees: Vec::new(),
        },
    ))
    .expect("issue");
    temper_testing::block_on(forge.update_issue(
        &issue.id,
        UpdateIssue {
            state: Some(IssueState::Closed),
            ..UpdateIssue::default()
        },
    ))
    .expect("close issue");
}

fn scan_to_authority(
    forge: &CountingForge<MemoryForge>,
    repo: &RepositoryId,
    workflow: &temper_workflow::ValidatedWorkflow,
    state: &TerminalDiscoveryState,
) -> Vec<temper_runner::WorkItem> {
    let compiled = workflow.compile();
    let roles = [RoleId::new("architect")];
    let mut selected = Vec::new();
    for generation in 0..10 {
        selected.extend(
            temper_testing::block_on(scan_roles_wake_with_discovery(
                forge,
                repo,
                workflow,
                &compiled,
                timestamp("2026-08-01T00:00:00Z"),
                &roles,
                state,
                TerminalDiscoveryRead::Advance,
            ))
            .expect("bounded scan"),
        );
        if state
            .snapshot(repo)
            .is_some_and(|snapshot| snapshot.authoritative)
        {
            return selected;
        }
        assert!(
            generation < 9,
            "continuation must finish within the test bound"
        );
    }
    unreachable!()
}

#[test]
fn irrelevant_terminal_history_is_page_bounded_and_never_hydrated() {
    let inner = MemoryForge::new();
    let repo = repository(&inner);
    for _ in 0..250 {
        closed_issue(&inner, &repo);
        pull_request(&inner, &repo, &["implementation", "landed"], String::new());
    }
    let forge = CountingForge::new(inner);
    let workflow = workflow();
    let state = TerminalDiscoveryState::default();

    assert!(scan_to_authority(&forge, &repo, &workflow, &state).is_empty());
    assert_eq!(
        forge
            .pull_request_candidate_queries()
            .iter()
            .filter(|query| query.lifecycle == temper_forge::CandidateLifecycle::Terminal)
            .count(),
        3,
        "250 terminal rows are consumed through three bounded pages"
    );
    assert!(forge.exact_issue_reads().is_empty());
    assert!(forge.exact_pull_request_reads().is_empty());
    assert_eq!(forge.count(CountedForgeOp::ListCiJobs), 0);
    assert_eq!(forge.count(CountedForgeOp::ListPullRequestReviews), 0);
    assert_eq!(forge.count(CountedForgeOp::ListIssueComments), 0);
    assert_eq!(forge.count(CountedForgeOp::ListPullRequestComments), 0);

    // Restart is cold and rebuilds the same authority instead of treating a
    // newest-only page as complete.
    let restarted = TerminalDiscoveryState::default();
    assert!(scan_to_authority(&forge, &repo, &workflow, &restarted).is_empty());
    assert!(restarted.snapshot(&repo).unwrap().authoritative);

    // No webhook is delivered for this newer terminal transition. The next
    // poll sweep still walks past all older history and selects it.
    let changed = pull_request(
        forge.inner(),
        &repo,
        &["implementation", "landed", "actionable"],
        String::new(),
    );
    assert!(prepare_terminal_discovery_generation(&state, &repo));
    let selected = scan_to_authority(&forge, &repo, &workflow, &state);
    assert!(selected.iter().any(|item| {
        item.target == temper_workflow::ArtifactSource::PullRequest { number: changed }
    }));
}

#[test]
fn mechanical_and_role_consumers_share_each_bounded_page() {
    let inner = MemoryForge::new();
    let repo = repository(&inner);
    for _ in 0..150 {
        pull_request(&inner, &repo, &["implementation", "landed"], String::new());
    }
    let forge = CountingForge::new(inner);
    let workflow = workflow();
    let compiled = workflow.compile();
    let journal = InMemoryJournal::new();
    let state = TerminalDiscoveryState::default();
    let worker = MechanicalWorker::new(
        &workflow,
        &forge,
        &repo,
        &journal,
        LeasePolicy::new(chrono::Duration::minutes(30)),
    )
    .with_terminal_discovery_state(state.clone());

    assert_eq!(
        temper_testing::block_on(worker.tick(timestamp("2026-08-01T00:00:00Z")))
            .expect("first mechanical page"),
        Progress::unchanged()
    );
    let terminal_pages = || {
        forge
            .pull_request_candidate_queries()
            .iter()
            .filter(|query| query.lifecycle == temper_forge::CandidateLifecycle::Terminal)
            .count()
    };
    assert_eq!(terminal_pages(), 1);
    assert!(
        temper_testing::block_on(scan_roles_wake_with_discovery(
            &forge,
            &repo,
            &workflow,
            &compiled,
            timestamp("2026-08-01T00:00:01Z"),
            &[RoleId::new("architect")],
            &state,
            TerminalDiscoveryRead::RetainedOnly,
        ))
        .expect("role consumes retained page")
        .is_empty()
    );
    assert_eq!(
        terminal_pages(),
        1,
        "the role lane must not restart or duplicate the mechanical page"
    );

    assert_eq!(
        temper_testing::block_on(worker.tick(timestamp("2026-08-01T00:00:02Z")))
            .expect("continuation page"),
        Progress::unchanged()
    );
    assert_eq!(terminal_pages(), 2);
    assert!(state.snapshot(&repo).unwrap().authoritative);
}

#[test]
fn old_terminal_recovery_metadata_survives_newer_irrelevant_history() {
    let inner = MemoryForge::new();
    let repo = repository(&inner);
    let recovering = pull_request(
        &inner,
        &repo,
        &["implementation", "landed"],
        render_metadata_block(&WorkflowMetadata {
            kind: Some(ArtifactKindId::new("implementation_pr")),
            lease: Some(Lease {
                role: RoleId::new("architect"),
                worker: "lost-worker".into(),
                claimed_at: timestamp("2026-07-01T00:00:00Z"),
                heartbeat_at: timestamp("2026-07-01T00:01:00Z"),
                expires_at: timestamp("2026-07-01T00:02:00Z"),
            }),
            ..WorkflowMetadata::default()
        }),
    );
    for _ in 0..220 {
        pull_request(&inner, &repo, &["implementation", "landed"], String::new());
    }
    let forge = CountingForge::new(inner);
    let workflow = workflow();
    let journal = InMemoryJournal::new();
    let state = TerminalDiscoveryState::default();
    let worker = MechanicalWorker::new(
        &workflow,
        &forge,
        &repo,
        &journal,
        LeasePolicy::new(chrono::Duration::minutes(30)),
    )
    .with_terminal_discovery_state(state.clone());

    assert_eq!(
        temper_testing::block_on(worker.tick(timestamp("2026-08-01T00:00:00Z")))
            .expect("mechanical recovery"),
        Progress {
            changed: true,
            actions: 1,
        }
    );
    let current =
        temper_testing::block_on(forge.inner().get_pull_request_by_number(&repo, recovering))
            .expect("read")
            .expect("recovery target");
    assert_eq!(current.state, PullRequestState::Merged);
    assert!(
        parse_metadata_block(&current.body)
            .expect("metadata")
            .expect("metadata present")
            .lease
            .is_none()
    );
    assert!(state.snapshot(&repo).is_some());
}

#[test]
fn incomplete_journal_and_retained_recovery_target_share_one_exact_read() {
    let inner = MemoryForge::new();
    let repo = repository(&inner);
    let issue = temper_testing::block_on(inner.create_issue(
        &repo,
        CreateIssue {
            title: "interrupted transition".into(),
            body: String::new(),
            labels: vec!["code".into(), "ready".into()],
            assignees: Vec::new(),
        },
    ))
    .expect("issue");
    temper_testing::block_on(inner.update_issue(
        &issue.id,
        UpdateIssue {
            state: Some(IssueState::Closed),
            ..UpdateIssue::default()
        },
    ))
    .expect("close issue");

    let forge = CountingForge::new(inner);
    let workflow = workflow();
    let compiled = workflow.compile();
    let journal = InMemoryJournal::new();
    let command = CommandRecord::planned(
        CommandId::new("interrupted-claim"),
        ArtifactSource::Issue {
            number: issue.number,
        },
        TransitionId::new("claim_code"),
        RoleId::new("engineer"),
        vec![WorkflowEffect::AddLabel("in-progress".into())],
        timestamp("2026-07-01T00:00:00Z"),
    );
    temper_testing::block_on(journal.append(command)).expect("journal append");
    temper_testing::block_on(journal.transition_state(
        &CommandId::new("interrupted-claim"),
        CommandState::Applying,
        None,
        timestamp("2026-07-01T00:00:01Z"),
    ))
    .expect("journal applying");

    let state = TerminalDiscoveryState::default();
    retain_terminal_discovery_target(
        &state,
        &repo,
        &workflow,
        &compiled,
        temper_runner::ArtifactAddress::issue(issue.number),
    )
    .expect("retain target");
    let worker = MechanicalWorker::new(
        &workflow,
        &forge,
        &repo,
        &journal,
        LeasePolicy::new(chrono::Duration::minutes(30)),
    )
    .with_terminal_discovery_state(state);

    let progress = temper_testing::block_on(worker.tick(timestamp("2026-08-01T00:00:00Z")))
        .expect("journal recovery");
    assert!(progress.changed);
    assert_eq!(
        forge
            .exact_issue_reads()
            .iter()
            .filter(|read| read.details == temper_forge::ItemListDetails::summary())
            .count(),
        1,
        "the retained target and journal target are unioned before exact loading"
    );
}
