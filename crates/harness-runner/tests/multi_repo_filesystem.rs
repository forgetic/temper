//! Filesystem integration coverage for multi-repository runner wrappers.
//!
//! These tests use one durable filesystem store, distinct backend handles, and
//! per-role `as_user` identities to mirror the process-split assumptions without
//! spawning OS processes.

use async_trait::async_trait;
use chrono::Duration;
use harness_forge::{
    CreateIssue, Forge, IssueState, ItemNumber, RepositoryId, RepositoryPath, UpdateIssue,
};
use harness_forge_filesystem::FilesystemForge;
use harness_runner::{
    Agent, AgentError, MultiRepoMechanicalWorker, MultiRepoRoleWorker, PollLoop, Progress,
    RepositoryJournal, RepositorySet, RepositoryTarget, RoleTools, WakeTarget, WakeablePollLoop,
    WorkItem,
};
use harness_testing::{block_on, runner_config, ts, user, workflow};
use harness_workflow::{
    parse_metadata_block, ArtifactKindId, ArtifactSource, CommandId, CommandJournal, CommandRecord,
    CommandState, ExecutionContext, InMemoryJournal, LeaseManager, LeasePolicy, QueueId, RoleId,
    TransitionId, WorkflowEffect,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration as StdDuration, Instant};

static NEXT_TEMP_ROOT: AtomicU64 = AtomicU64::new(0);

struct TempRoot {
    path: PathBuf,
}

impl TempRoot {
    fn new(suite: &str) -> Self {
        let id = NEXT_TEMP_ROOT.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "harness-runner-multi-repo-fs-{suite}-{}-{id}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        Self { path }
    }

    fn forge(&self) -> FilesystemForge {
        FilesystemForge::new(&self.path)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

struct TriageToCode;

#[async_trait]
impl Agent<FilesystemForge> for TriageToCode {
    async fn service(
        &self,
        item: &WorkItem,
        tools: &RoleTools<'_, FilesystemForge>,
    ) -> Result<bool, AgentError> {
        if item.queue == QueueId::new("design_triage") && item.kind == ArtifactKindId::new("intake")
        {
            tools
                .run(item.target, &TransitionId::new("triage_to_code"))
                .await?;
            return Ok(true);
        }
        Ok(false)
    }
}

struct ClaimReady {
    claimed: Option<Arc<AtomicBool>>,
}

#[async_trait]
impl Agent<FilesystemForge> for ClaimReady {
    async fn service(
        &self,
        item: &WorkItem,
        tools: &RoleTools<'_, FilesystemForge>,
    ) -> Result<bool, AgentError> {
        if item.queue == QueueId::new("code_ready") && item.kind == ArtifactKindId::new("code") {
            tools
                .run(item.target, &TransitionId::new("claim_code"))
                .await?;
            if let Some(claimed) = &self.claimed {
                claimed.store(true, Ordering::SeqCst);
            }
            return Ok(true);
        }
        Ok(false)
    }
}

fn create_repo(forge: &FilesystemForge, name: &str) -> RepositoryTarget {
    let repo = block_on(forge.create_repository(harness_forge::CreateRepository {
        owner: "acme".into(),
        name: name.into(),
        default_branch: "main".into(),
        description: None,
    }))
    .expect("repository created");
    RepositoryTarget::new(repo.id, RepositoryPath::new(repo.owner, repo.name))
}

fn create_issue(
    forge: &FilesystemForge,
    repo: &RepositoryId,
    title: &str,
    labels: &[&str],
) -> ItemNumber {
    block_on(forge.create_issue(
        repo,
        CreateIssue {
            title: title.into(),
            body: String::new(),
            labels: labels.iter().map(|label| (*label).to_string()).collect(),
            assignees: Vec::new(),
        },
    ))
    .expect("issue created")
    .number
}

fn close_issue(forge: &FilesystemForge, repo: &RepositoryId, number: ItemNumber) {
    let issue = block_on(forge.get_issue_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("issue exists");
    block_on(forge.update_issue(
        &issue.id,
        UpdateIssue {
            state: Some(IssueState::Closed),
            ..UpdateIssue::default()
        },
    ))
    .expect("issue closes");
}

fn add_issue_dependency(
    forge: &FilesystemForge,
    repo: &RepositoryId,
    source: ItemNumber,
    target: ItemNumber,
) {
    let issue = block_on(forge.get_issue_by_number(repo, source))
        .expect("lookup succeeds")
        .expect("issue exists");
    block_on(forge.add_issue_dependency(&issue.id, target)).expect("dependency link added");
}

fn issue_labels(forge: &FilesystemForge, repo: &RepositoryId, number: ItemNumber) -> Vec<String> {
    let mut labels = block_on(forge.get_issue_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("issue exists")
        .labels;
    labels.sort();
    labels
}

fn issue_body(forge: &FilesystemForge, repo: &RepositoryId, number: ItemNumber) -> String {
    block_on(forge.get_issue_by_number(repo, number))
        .expect("lookup succeeds")
        .expect("issue exists")
        .body
}

fn command_state(journal: &InMemoryJournal, id: &str) -> CommandState {
    block_on(journal.get(&CommandId::new(id)))
        .expect("journal get succeeds")
        .expect("command exists")
        .state
}

fn lease_policy() -> LeasePolicy {
    LeasePolicy::new(Duration::minutes(30))
}

fn engineer_context() -> ExecutionContext {
    runner_config().execution_context(&RoleId::new("engineer"))
}

fn claim_record(id: &str, number: ItemNumber) -> CommandRecord {
    CommandRecord::planned(
        CommandId::new(id),
        ArtifactSource::Issue { number },
        TransitionId::new("claim_code"),
        RoleId::new("engineer"),
        vec![
            WorkflowEffect::RemoveLabel("ready".into()),
            WorkflowEffect::AddLabel("in-progress".into()),
        ],
        ts("2026-05-29T00:00:00Z"),
    )
}

fn mark_applying(journal: &InMemoryJournal, id: &str) {
    block_on(journal.transition_state(
        &CommandId::new(id),
        CommandState::Applying,
        None,
        ts("2026-05-29T00:00:30Z"),
    ))
    .expect("command moves to applying");
}

fn append_applying_claim(journal: &InMemoryJournal, id: &str, number: ItemNumber) {
    block_on(journal.append(claim_record(id, number))).expect("command appends");
    mark_applying(journal, id);
}

#[test]
fn role_workers_converge_two_repos_after_filesystem_handle_restart() {
    let root = TempRoot::new("role-restart");
    let producer = root.forge();
    let repo_a = create_repo(&producer, "alpha");
    let repo_b = create_repo(&producer, "bravo");
    let issue_a = create_issue(&producer, &repo_a.id, "alpha work", &["untriaged"]);
    let issue_b = create_issue(&producer, &repo_b.id, "bravo work", &["untriaged"]);
    let repositories = RepositorySet::new(vec![repo_b.clone(), repo_a.clone()]);
    let workflow = workflow();
    let compiled = workflow.compile();

    let architect_forge = root.forge().as_user(user("architect", "architect"));
    let architect = MultiRepoRoleWorker::new(
        &workflow,
        &compiled,
        &architect_forge,
        repositories.clone(),
        RoleId::new("architect"),
        Arc::new(TriageToCode),
        ExecutionContext::new(),
    );
    let triage = block_on(architect.tick_report(ts("2026-05-29T00:00:00Z")));
    assert!(triage.failures.is_empty());
    assert_eq!(
        triage.progress,
        Progress {
            changed: true,
            actions: 2
        }
    );

    // Simulate a process restart: the next role receives fresh process-local
    // worker state and a fresh handle, but reads the durable triage result.
    let engineer_forge = root.forge().as_user(user("engineer", "engineer"));
    let engineer = MultiRepoRoleWorker::new(
        &workflow,
        &compiled,
        &engineer_forge,
        repositories,
        RoleId::new("engineer"),
        Arc::new(ClaimReady { claimed: None }),
        engineer_context(),
    );
    let claim = block_on(engineer.tick_report(ts("2026-05-29T00:00:01Z")));
    assert!(claim.failures.is_empty());
    assert_eq!(
        claim.progress,
        Progress {
            changed: true,
            actions: 2
        }
    );

    let observer = root.forge();
    assert_eq!(
        issue_labels(&observer, &repo_a.id, issue_a),
        vec!["code".to_string(), "in-progress".to_string()]
    );
    assert_eq!(
        issue_labels(&observer, &repo_b.id, issue_b),
        vec!["code".to_string(), "in-progress".to_string()]
    );
}

#[test]
fn mechanical_recovery_keeps_leases_and_journals_repository_scoped() {
    let root = TempRoot::new("mechanical-recovery");
    let writer = root.forge();
    let repo_a = create_repo(&writer, "alpha");
    let repo_b = create_repo(&writer, "bravo");
    let workflow = workflow();

    let leased_a = create_issue(&writer, &repo_a.id, "leased", &["code", "in-progress"]);
    let manager = LeaseManager::new(&writer, lease_policy());
    block_on(manager.acquire(
        &repo_a.id,
        ArtifactSource::Issue { number: leased_a },
        RoleId::new("engineer"),
        "run-a",
        ts("2026-05-29T00:00:00Z"),
    ))
    .expect("lease is written");

    // The same repo-scoped issue number and command id are deliberately used in
    // both journals. Binding one journal per repository prevents aliasing.
    let partial_a = create_issue(&writer, &repo_a.id, "partial a", &["code", "ready"]);
    let partial_b = create_issue(&writer, &repo_b.id, "partial b", &["code", "ready"]);
    let journal_a = InMemoryJournal::new();
    let journal_b = InMemoryJournal::new();
    append_applying_claim(&journal_a, "claim-shared", partial_a);
    append_applying_claim(&journal_b, "claim-shared", partial_b);

    let mechanical_forge = root.forge();
    let worker = MultiRepoMechanicalWorker::new(
        &workflow,
        &mechanical_forge,
        RepositorySet::new(vec![repo_b.clone(), repo_a.clone()]),
        vec![
            RepositoryJournal {
                repository: &repo_a.id,
                journal: &journal_a,
            },
            RepositoryJournal {
                repository: &repo_b.id,
                journal: &journal_b,
            },
        ],
        lease_policy(),
    )
    .expect("worker builds");

    let report = block_on(worker.tick_report(ts("2026-05-29T01:00:00Z")));
    assert!(report.failures.is_empty());
    assert_eq!(
        report.progress,
        Progress {
            changed: true,
            actions: 3
        }
    );

    let observer = root.forge();
    let metadata = parse_metadata_block(&issue_body(&observer, &repo_a.id, leased_a))
        .expect("metadata parses")
        .expect("metadata exists");
    assert!(metadata.lease.is_none(), "repo A lease was cleared");
    assert_eq!(
        issue_labels(&observer, &repo_a.id, partial_a),
        vec!["code".to_string(), "in-progress".to_string()]
    );
    assert_eq!(
        issue_labels(&observer, &repo_b.id, partial_b),
        vec!["code".to_string(), "in-progress".to_string()]
    );
    assert_eq!(
        command_state(&journal_a, "claim-shared"),
        CommandState::Reconciled
    );
    assert_eq!(
        command_state(&journal_b, "claim-shared"),
        CommandState::Reconciled
    );
}

#[test]
fn fresh_mechanical_wrapper_recovers_one_repo_without_mutating_another() {
    let root = TempRoot::new("mechanical-restart");
    let writer = root.forge();
    let repo_a = create_repo(&writer, "alpha");
    let repo_b = create_repo(&writer, "bravo");
    let workflow = workflow();

    let dependency = create_issue(&writer, &repo_a.id, "dependency", &["code", "ready"]);
    close_issue(&writer, &repo_a.id, dependency);
    let blocked = create_issue(&writer, &repo_a.id, "blocked", &["code", "blocked"]);
    add_issue_dependency(&writer, &repo_a.id, blocked, dependency);
    let untouched_b = create_issue(&writer, &repo_b.id, "unrelated", &["code", "ready"]);

    // Simulate a restarted controller process: fresh forge handle, fresh worker,
    // and fresh process-local journals. The unblock is derived from Forge state.
    let mechanical_forge = root.forge();
    let journal_a = InMemoryJournal::new();
    let journal_b = InMemoryJournal::new();
    let restarted = MultiRepoMechanicalWorker::new(
        &workflow,
        &mechanical_forge,
        RepositorySet::new(vec![repo_a.clone(), repo_b.clone()]),
        vec![
            RepositoryJournal {
                repository: &repo_a.id,
                journal: &journal_a,
            },
            RepositoryJournal {
                repository: &repo_b.id,
                journal: &journal_b,
            },
        ],
        lease_policy(),
    )
    .expect("worker builds");

    let report = block_on(restarted.tick_report(ts("2026-05-29T00:00:00Z")));
    assert!(report.failures.is_empty());
    assert_eq!(
        report.progress,
        Progress {
            changed: true,
            actions: 1
        }
    );

    let observer = root.forge();
    assert_eq!(
        issue_labels(&observer, &repo_a.id, blocked),
        vec!["code".to_string(), "ready".to_string()]
    );
    assert_eq!(
        issue_labels(&observer, &repo_b.id, untouched_b),
        vec!["code".to_string(), "ready".to_string()]
    );
    assert!(block_on(journal_b.list())
        .expect("journal lists")
        .is_empty());
}

#[test]
fn filesystem_hint_from_one_repo_wakes_shared_multi_repo_worker() {
    let root = TempRoot::new("hint-wake");
    let producer = root.forge();
    let repo_a = create_repo(&producer, "alpha");
    let repo_b = create_repo(&producer, "bravo");
    create_issue(&producer, &repo_a.id, "not ready", &["code"]);
    let issue_b = block_on(producer.create_issue(
        &repo_b.id,
        CreateIssue {
            title: "becomes ready".into(),
            body: String::new(),
            labels: vec!["code".into()],
            assignees: Vec::new(),
        },
    ))
    .expect("issue created before it is ready");

    let workflow = workflow();
    let compiled = workflow.compile();
    let role = RoleId::new("engineer");
    let worker_forge = root.forge().as_user(user("engineer", "engineer"));
    let mut hints = worker_forge.subscribe_hints();
    let claimed = Arc::new(AtomicBool::new(false));
    let worker = MultiRepoRoleWorker::new(
        &workflow,
        &compiled,
        &worker_forge,
        RepositorySet::new(vec![repo_a, repo_b.clone()]),
        role.clone(),
        Arc::new(ClaimReady {
            claimed: Some(Arc::clone(&claimed)),
        }),
        engineer_context(),
    );
    let target = WakeTarget::Role(role.clone());
    let loop_ = WakeablePollLoop::new(&worker, target.clone(), Duration::seconds(30));
    let start = Instant::now();

    std::thread::scope(|scope| {
        let handle = scope.spawn(|| {
            block_on(loop_.run_until(
                &mut hints,
                |_| vec![target.clone()],
                || claimed.load(Ordering::SeqCst),
            ))
        });

        std::thread::sleep(StdDuration::from_millis(50));
        block_on(producer.update_issue(
            &issue_b.id,
            UpdateIssue {
                add_labels: vec!["ready".into()],
                ..UpdateIssue::default()
            },
        ))
        .expect("repo B issue becomes ready");

        let report = handle
            .join()
            .expect("worker thread joins")
            .expect("wake loop runs");
        assert!(
            report.ticks >= 2,
            "initial scan plus hint-triggered scan ran"
        );
    });

    assert!(
        start.elapsed() < StdDuration::from_secs(1),
        "hint from repo B should wake the shared worker before the 30s poll interval"
    );
}

#[test]
fn missed_filesystem_hint_still_converges_on_poll_backstop_tick() {
    let root = TempRoot::new("hint-missed");
    let producer = root.forge();
    let repo_a = create_repo(&producer, "alpha");
    let repo_b = create_repo(&producer, "bravo");
    let issue_b = create_issue(&producer, &repo_b.id, "already ready", &["code", "ready"]);

    // Subscribe only after the issue was created, so the old hint is missed.
    let worker_forge = root.forge().as_user(user("engineer", "engineer"));
    let _missed_hints = worker_forge.subscribe_hints();
    let workflow = workflow();
    let compiled = workflow.compile();
    let worker = MultiRepoRoleWorker::new(
        &workflow,
        &compiled,
        &worker_forge,
        RepositorySet::new(vec![repo_a, repo_b.clone()]),
        RoleId::new("engineer"),
        Arc::new(ClaimReady { claimed: None }),
        engineer_context(),
    );
    let poll = PollLoop::new(&worker, Duration::seconds(30));

    let report = block_on(poll.run_bounded(1)).expect("poll tick succeeds");
    assert_eq!(report.ticks, 1);
    assert_eq!(
        issue_labels(&root.forge(), &repo_b.id, issue_b),
        vec!["code".to_string(), "in-progress".to_string()]
    );
}
