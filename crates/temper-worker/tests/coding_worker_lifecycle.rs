//! Hermetic daemon/worker lifecycle regression for engineer session resume.
//!
//! This test composes the real daemon transport, real worker loop, real
//! `CodingExecutor`, local `file://` git, `MemoryForge`, and the basic-delivery
//! workflow. It is deliberately in-process (no Forgejo service): MemoryForge is
//! the Forge state, while local bare git proves the worker pushes the PR head
//! branch the daemon assigned.

use std::sync::Arc;
use std::time::Duration;

use temper_forge::{CiJobConclusion, Forge, IssueState, PullRequestState, RepositoryPath};
use temper_forge_memory::MemoryForge;
use temper_protocol_worker::ResultStatus;
use temper_worker::{
    CodingExecutor, CodingExecutorConfig, ScopedWorkspaceCleanupOutcome, run_worker_with_transport,
};
use temper_workflow::{InMemoryJournal, LeasePolicy, RoleId};

#[path = "support/real_daemon.rs"]
mod real_daemon;
use real_daemon::DaemonHarness;

#[path = "coding_worker_lifecycle/support.rs"]
mod support;
use support::*;

const BASIC_DELIVERY: &str = include_str!("../../temper-workflow/fixtures/basic-delivery.json");
const ENGINEER: &str = "engineer";
const REPO: &str = "acme/service";

#[test]
fn engineer_session_resumes_after_ci_failure_then_lands_and_cleans_workstream() {
    temper_engine_io::block_on_with(|cx, handle| async move {
        let git = GitFixture::new();
        let forge = Arc::new(MemoryForge::new());
        let repo = create_repo(&forge).await;
        let issue = create_ready_issue(&forge, &repo).await;
        let workflow = Arc::new(workflow());
        let compiled = workflow.compile();
        let role = RoleId::new(ENGINEER);
        let coordination_key = format!("pr-for-code-{}", issue.get());
        let branch = format!("agent/{coordination_key}");

        let applier = Arc::new(temper_engine::LeaseApplier::new(
            forge.clone(),
            LeasePolicy::new(chrono::Duration::seconds(300)),
            "daemon-1",
            Arc::new(temper_engine::ForgeApplier::new(
                forge.clone(),
                workflow.clone(),
            )),
            temper_engine::system_clock(),
        ));
        let mut harness = DaemonHarness::start_with_applier(&handle, applier);

        assert_eq!(
            harness
                .daemon
                .enqueue_scanned_role_work(
                    forge.as_ref(),
                    &repo,
                    workflow.as_ref(),
                    &compiled,
                    ts("2026-05-29T00:00:00Z"),
                    &role,
                    temper_engine::RoleFeedMode::Normal,
                )
                .await
                .expect("code_ready feed succeeds"),
            1,
            "ready code issue should enqueue one engineer implementation job"
        );

        let agent = Arc::new(RecordingAgent::default());
        let executor = Arc::new(
            CodingExecutor::new(
                CodingExecutorConfig {
                    workspace_root: git.workspace_root.clone(),
                    git_base_url: git.git_base_url(),
                    role_identities: role_identities(),
                },
                agent.clone(),
            )
            .with_pr_freshness_guard(Arc::new(DaemonPrFreshnessGuard::new(
                harness.daemon.as_ref().clone(),
            ))),
        );
        let transport = harness.transport();
        let worker_handle = handle.clone();
        handle.spawn(async move {
            let _ = run_worker_with_transport(worker_handle, worker_config(), executor, transport)
                .await;
        });

        let implementation_result = harness.await_result().await;
        assert_eq!(implementation_result.status, ResultStatus::Success);
        assert_eq!(implementation_result.repos.len(), 1);
        assert_eq!(implementation_result.repos[0].branch.name, branch);
        let implementation_head = implementation_result.repos[0].branch.head_sha.clone();
        assert_eq!(git.origin_rev(&branch), implementation_head);

        let mut pulls = wait_for_pull_request_count(&cx, forge.as_ref(), &repo, 1).await;
        let mut pull = pulls.pop().expect("implementation PR exists");
        assert_eq!(pull.source.branch, branch);
        assert_eq!(pull.state, PullRequestState::Open);
        assert!(pull.labels.iter().any(|label| label == "implementation"));
        assert!(pull.labels.iter().any(|label| label == "landing"));
        pull = forge
            .set_pull_request_head(&pull.id, Some(implementation_head.clone()))
            .expect("memory forge observes implementation branch head");

        let runs = agent.runs();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].queue, "code_ready");
        assert_eq!(runs[0].action, "open_pr");
        assert_eq!(runs[0].correlation_key, coordination_key);
        let initial_session = runs[0].session.clone();
        let store = temper_worker::AgentSessionStore::for_workspace_root(
            &git.workspace_root,
            ENGINEER,
            &coordination_key,
        )
        .expect("session store path");
        assert_eq!(
            store.load_sync().expect("saved session loads"),
            Some(initial_session.clone()),
            "successful implementation run should save the engineer session while the PR waits"
        );

        forge.seed_ci_jobs(
            &repo,
            vec![ci_job(
                &repo,
                &pull.id,
                &implementation_head,
                CiJobConclusion::Failure,
                "ci-failed-initial-head",
            )],
        );

        assert_eq!(
            harness
                .daemon
                .enqueue_scanned_role_work(
                    forge.as_ref(),
                    &repo,
                    workflow.as_ref(),
                    &compiled,
                    ts("2026-05-29T00:01:00Z"),
                    &role,
                    temper_engine::RoleFeedMode::Normal,
                )
                .await
                .expect("pr_ci_failed feed succeeds"),
            1,
            "failed CI on the implementation PR should enqueue one PR-feedback job"
        );

        let feedback_result = harness.await_result().await;
        assert_eq!(feedback_result.status, ResultStatus::Success);
        assert!(
            feedback_result.job_id.contains(&format!(
                "/pull_request-{}/engineer/pr_ci_failed",
                pull.number.get()
            )),
            "feedback job id should target the failed implementation PR: {}",
            feedback_result.job_id
        );
        assert_eq!(feedback_result.repos.len(), 1);
        assert_eq!(
            feedback_result.repos[0].branch.name, branch,
            "PR feedback must report the existing PR head branch, not a new branch"
        );
        let feedback_head = feedback_result.repos[0].branch.head_sha.clone();
        assert_ne!(feedback_head, implementation_head);
        assert_eq!(git.origin_rev(&branch), feedback_head);
        assert_eq!(
            git.origin_rev(&format!("{branch}^")),
            implementation_head,
            "the CI fix should be a new commit on top of the implementation head"
        );
        assert_eq!(
            git.origin_log_format(&branch, "%s"),
            format!("Fix CI for {coordination_key}")
        );
        assert_eq!(
            git.origin_show(&format!("{branch}:ci-fix.txt")),
            "fixed failing CI"
        );

        pulls = wait_for_pull_request_count(&cx, forge.as_ref(), &repo, 1).await;
        assert_eq!(pulls[0].number, pull.number);
        assert_eq!(pulls[0].source.branch, branch);
        pull = forge
            .set_pull_request_head(&pull.id, Some(feedback_head.clone()))
            .expect("memory forge observes feedback branch head");

        let runs = agent.runs();
        assert_eq!(
            runs.len(),
            2,
            "only implementation + PR feedback should run"
        );
        let feedback_run = &runs[1];
        assert_eq!(feedback_run.queue, "pr_ci_failed");
        assert_eq!(feedback_run.action, "address_ci_failure");
        assert_eq!(feedback_run.correlation_key, coordination_key);
        assert_eq!(
            feedback_run.branch_hint.as_deref(),
            Some(branch.as_str()),
            "PR feedback checkout should use the existing PR head branch"
        );
        assert_eq!(
            feedback_run.observed_head_sha, implementation_head,
            "PR feedback agent should start from the assigned PR head"
        );
        let freshness = feedback_run
            .pull_request_freshness
            .as_ref()
            .expect("PR feedback context carries freshness facts");
        assert_eq!(freshness.queue_condition.as_deref(), Some("ci_failed"));
        assert_eq!(
            freshness.head_sha.as_deref(),
            Some(implementation_head.as_str()),
            "PR feedback should be assigned against the failed head"
        );
        assert_eq!(freshness.pull_request_id, pull.id.as_str());
        assert_eq!(
            feedback_run.session, initial_session,
            "PR feedback should resume the same engineer agent_session as implementation"
        );
        assert_eq!(
            wait_for_pull_request_count(&cx, forge.as_ref(), &repo, 1)
                .await
                .len(),
            1,
            "feedback success must not open a second pull request"
        );

        wait_for_workstream_inactive(&cx, harness.daemon.as_ref(), &coordination_key).await;
        forge.seed_ci_jobs(
            &repo,
            vec![ci_job(
                &repo,
                &pull.id,
                &feedback_head,
                CiJobConclusion::Success,
                "ci-passed-feedback-head",
            )],
        );

        let cleanup = Arc::new(TestWorkstreamCleaner::new(
            harness.daemon.as_ref().clone(),
            git.workspace_root.clone(),
        ));
        let mechanical_config = temper_engine::MechanicalBackstopConfig {
            repositories: temper_engine::RepositorySet::new(vec![
                temper_engine::RepositoryTarget::new(
                    repo.clone(),
                    RepositoryPath::new("acme", "service"),
                ),
            ]),
            cadence: Duration::from_secs(60),
            lease_policy: LeasePolicy::new(chrono::Duration::seconds(300)),
            pull_request_merge_observer: Some(cleanup.clone()),
        };
        let journals = vec![InMemoryJournal::new()];
        let progress = temper_engine::run_mechanical_backstop_tick(
            forge.as_ref(),
            workflow.as_ref(),
            ts("2026-05-29T00:02:00Z"),
            &mechanical_config,
            &journals,
            &temper_engine::MechanicalScope::All,
        )
        .await
        .expect("mechanical landing tick succeeds");
        assert!(progress.changed, "mechanical tick should land the green PR");

        let landed = forge
            .get_pull_request_by_number(&repo, pull.number)
            .await
            .expect("pull request reload succeeds")
            .expect("pull request still exists");
        assert_eq!(landed.state, PullRequestState::Merged);
        assert!(landed.merge.is_some(), "landing should record a merge");
        assert!(
            !landed.labels.iter().any(|label| label == "landing"),
            "landing label should be removed after merge"
        );
        let source_issue = forge
            .get_issue_by_number(&repo, issue)
            .await
            .expect("issue reload succeeds")
            .expect("source issue exists");
        assert_eq!(source_issue.state, IssueState::Closed);

        let workstream_root =
            temper_worker::scoped_workspace_root(&git.workspace_root, ENGINEER, &coordination_key)
                .expect("scoped workspace path");
        assert_eq!(cleanup.outcomes().len(), 1);
        assert!(
            matches!(
                cleanup.outcomes().first(),
                Some(ScopedWorkspaceCleanupOutcome::Removed { path }) if path == &workstream_root
            ),
            "landing cleanup should remove the inactive engineer workstream: {:?}",
            cleanup.outcomes()
        );
        assert!(
            !workstream_root.exists(),
            "merged PR cleanup should remove the scoped checkout root"
        );
        assert_eq!(
            store.load_sync().expect("session state after cleanup"),
            None,
            "merged PR cleanup should remove the saved agent session state"
        );
    });
}
