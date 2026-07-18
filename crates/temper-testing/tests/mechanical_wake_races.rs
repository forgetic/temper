//! Deterministic integration evidence for lossless coordinated mechanical wakes.
//!
//! Every race is held at a Forge operation after the in-memory result has been
//! captured. The tests use only engine-runtime oneshots and pause permits: no
//! cadence loop, sleep, timeout, or probabilistic task race participates.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use temper_engine::{
    CoordinatedMechanical, Daemon, MechanicalBackstopConfig, MechanicalTrigger, RoleFeedMode,
    RoleFeedTarget, WebhookConfig, webhook_signature,
};
use temper_forge_memory::MemoryForge;
use temper_forge_model::{
    BranchRef, ChangeHint, ChangeKind, CiJob, CiJobConclusion, CiJobId, CiJobStatus,
    CreatePullRequest, CreateRepository, Forge, ItemNumber, PullRequest, PullRequestState,
    RepositoryId, RepositoryPath, UserId,
};
use temper_runner::{RepositorySet, RepositoryTarget};
use temper_testing::counting_forge::{CountedForgeOp, CountingForge};
use temper_workflow::{LeasePolicy, RawWorkflowSpec, RoleId, ValidatedWorkflow};

const WORKFLOW: &str = r#"
{
  "name": "mechanical-wake-races",
  "roles": [
    { "id": "mechanical", "queues": [] },
    { "id": "observer", "queues": [] }
  ],
  "labels": [
    { "id": "implementation" },
    { "id": "landing" },
    { "id": "landed" }
  ],
  "artifact_kinds": [
    {
      "id": "implementation_pr",
      "target": "pull_request",
      "identifying_labels": ["implementation"]
    }
  ],
  "queues": [
    {
      "id": "landing",
      "artifact": "implementation_pr",
      "labels": ["landing"],
      "automation": { "actor": "mechanical", "transition": "land_pr" }
    }
  ],
  "transitions": [
    {
      "id": "land_pr",
      "artifact": "implementation_pr",
      "roles": ["mechanical"],
      "requires_gates": ["ci_gate"],
      "effects": [
        { "kind": "merge_pull_request" },
        { "kind": "remove_label", "label": "landing" },
        { "kind": "add_label", "label": "landed" }
      ]
    }
  ],
  "gates": [
    { "id": "ci_gate", "condition": { "kind": "ci_passed" } }
  ]
}
"#;

const WEBHOOK_SECRET: &str = "mechanical-race-secret";
const HEARTBEAT_ISSUE: u64 = 777;
const DUPLICATE_ISSUE: u64 = 778;

type RaceForge = CountingForge<MemoryForge>;

struct RaceFixture {
    forge: Arc<RaceForge>,
    daemon: Daemon,
    repo: RepositoryId,
    path: RepositoryPath,
    pull_request: PullRequest,
}

fn ts(value: &str) -> DateTime<Utc> {
    value.parse().expect("valid RFC 3339 timestamp")
}

fn workflow() -> ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(WORKFLOW).expect("workflow parses");
    spec.validate().expect("workflow validates")
}

async fn fixture(handle: &skein::runtime::RuntimeHandle) -> RaceFixture {
    let memory = MemoryForge::new();
    let repository = memory
        .create_repository(CreateRepository {
            owner: "acme".to_string(),
            name: "service".to_string(),
            default_branch: "main".to_string(),
            description: None,
        })
        .await
        .expect("repository is created");
    let pull_request = memory
        .create_pull_request(
            &repository.id,
            CreatePullRequest {
                title: "land after fresh CI".to_string(),
                body: String::new(),
                source: BranchRef {
                    repository_id: repository.id.clone(),
                    branch: "agent/mechanical-race".to_string(),
                },
                target: BranchRef {
                    repository_id: repository.id.clone(),
                    branch: "main".to_string(),
                },
                labels: vec!["implementation".to_string(), "landing".to_string()],
                assignees: Vec::<UserId>::new(),
            },
        )
        .await
        .expect("pull request is created");
    let path = RepositoryPath::new(repository.owner, repository.name);
    let forge = Arc::new(CountingForge::new(memory));
    forge.enable_synthetic_pull_request_heads();

    let workflow = Arc::new(workflow());
    let compiled = Arc::new(workflow.compile());
    let clock: temper_engine::WallClock = Arc::new(|| ts("2026-07-13T12:00:00Z"));
    let target = RepositoryTarget::new(repository.id.clone(), path.clone());
    let mechanical: Arc<dyn CoordinatedMechanical> = Arc::new(MechanicalTrigger::new(
        Arc::clone(&forge),
        Arc::clone(&workflow),
        MechanicalBackstopConfig {
            repositories: RepositorySet::new(vec![target]),
            // No cadence loop is spawned in these tests. The production-sized
            // value makes accidental cadence dependence especially visible.
            cadence: Duration::from_secs(120),
            lease_policy: LeasePolicy::new(chrono::Duration::minutes(30)),
            pull_request_merge_observer: None,
        },
        clock.clone(),
    ));
    let webhook = Arc::new(WebhookConfig {
        secret: WEBHOOK_SECRET.to_string(),
        targets: vec![RoleFeedTarget {
            repo: repository.id.clone(),
            path: path.clone(),
            role: RoleId::new("observer"),
            mode: RoleFeedMode::Wake,
        }],
    });
    let spawner: Arc<dyn temper_engine_io::Spawner> = Arc::new(handle.clone());
    let daemon = Daemon::new(spawner)
        .with_wake_scheduling(Duration::ZERO, 1)
        .with_webhook_and_mechanical(
            Arc::clone(&forge),
            workflow,
            compiled,
            webhook,
            clock,
            Some(mechanical),
        );

    RaceFixture {
        forge,
        daemon,
        repo: repository.id,
        path,
        pull_request,
    }
}

fn successful_ci(fixture: &RaceFixture) -> CiJob {
    CiJob {
        id: CiJobId::new(format!("ci-{}", fixture.pull_request.number.get())),
        repo_id: fixture.repo.clone(),
        pull_request_id: Some(fixture.pull_request.id.clone()),
        commit_sha: format!("pr-{}-head", fixture.pull_request.number.get()),
        name: "required".to_string(),
        status: CiJobStatus::Completed,
        conclusion: Some(CiJobConclusion::Success),
        url: None,
        created_at: ts("2026-07-13T11:58:00Z"),
        started_at: Some(ts("2026-07-13T11:58:01Z")),
        completed_at: Some(ts("2026-07-13T11:59:00Z")),
        updated_at: ts("2026-07-13T11:59:00Z"),
    }
}

async fn pull_request_state(fixture: &RaceFixture) -> PullRequestState {
    fixture
        .forge
        .inner()
        .get_pull_request_by_number(&fixture.repo, fixture.pull_request.number)
        .await
        .expect("pull request lookup succeeds")
        .expect("pull request exists")
        .state
}

fn first_op(trace: &[CountedForgeOp], from: usize, op: CountedForgeOp) -> usize {
    trace[from..]
        .iter()
        .position(|candidate| *candidate == op)
        .map(|index| from + index)
        .unwrap_or_else(|| panic!("{op:?} does not appear after trace index {from}: {trace:?}"))
}

fn heartbeat_body() -> Vec<u8> {
    let old = r#"Mechanical race fixture

<!-- temper:workflow
{"lease":{"role":"engineer","worker":"worker","claimed_at":"2026-07-13T12:00:00Z","heartbeat_at":"2026-07-13T12:00:00Z","expires_at":"2026-07-13T12:05:00Z"}}
-->"#;
    let new = r#"Mechanical race fixture

<!-- temper:workflow
{"lease":{"role":"engineer","worker":"worker","claimed_at":"2026-07-13T12:00:00Z","heartbeat_at":"2026-07-13T12:01:00Z","expires_at":"2026-07-13T12:06:00Z"}}
-->"#;
    serde_json::to_vec(&serde_json::json!({
        "action": "edited",
        "repository": {"full_name": "acme/service"},
        "issue": {"number": HEARTBEAT_ISSUE, "body": new},
        "changes": {"body": {"from": old}}
    }))
    .expect("heartbeat body serializes")
}

fn ci_webhook_body(number: ItemNumber) -> Vec<u8> {
    let event_payload = serde_json::json!({
        "pull_request": {"number": number.get()}
    })
    .to_string();
    serde_json::to_vec(&serde_json::json!({
        "action": "success",
        "run": {
            "id": 706,
            "status": "success",
            "repository": {"full_name": "acme/service"},
            "event_payload": event_payload
        },
        "prior_status": "running"
    }))
    .expect("CI webhook body serializes")
}

async fn post_webhook(url: &str, event: &str, body: Vec<u8>) -> u16 {
    let signature = webhook_signature(WEBHOOK_SECRET, &body);
    let client = temper_engine_io::http::build_http_client();
    temper_engine_io::http::http_call(
        &client,
        temper_engine_io::http::HttpCall {
            method: "POST".to_string(),
            url: url.to_string(),
            headers: vec![
                ("x-forgejo-event".to_string(), event.to_string()),
                ("x-forgejo-signature".to_string(), signature),
            ],
            body,
        },
    )
    .await
    .expect("webhook request succeeds")
    .status
}

fn submit_duplicate_issue_and_broad_traffic(fixture: &RaceFixture) {
    for _ in 0..64 {
        fixture.daemon.submit_change_hint(ChangeHint::issue(
            fixture.path.clone(),
            ItemNumber::new(DUPLICATE_ISSUE),
            ChangeKind::Label,
        ));
    }
    fixture.daemon.submit_change_hint(ChangeHint::repository(
        fixture.path.clone(),
        ChangeKind::Label,
    ));
    fixture.daemon.submit_change_hint(ChangeHint::repository(
        fixture.path.clone(),
        ChangeKind::Unknown,
    ));
}

#[test]
fn ci_change_after_stale_active_read_lands_in_immediate_exact_follow_up() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let fixture = fixture(&handle).await;
        let mut stale_ci = fixture.forge.pause_after(CountedForgeOp::ListCiJobs, 1);

        // This is a real coordinator-admitted broad mechanical generation.
        fixture
            .daemon
            .schedule_mechanical_poll(fixture.path.clone());
        stale_ci.wait_until_paused().await;
        assert_eq!(pull_request_state(&fixture).await, PullRequestState::Open);
        assert_eq!(fixture.forge.count(CountedForgeOp::MergePullRequest), 0);

        // The active broad pass already owns the empty CI result. Change the
        // fixture only after that result is captured, then dirty the repository
        // with the exact CI address.
        fixture
            .forge
            .inner()
            .seed_ci_jobs(&fixture.repo, vec![successful_ci(&fixture)]);
        fixture.daemon.submit_change_hint(ChangeHint::pull_request(
            fixture.path.clone(),
            fixture.pull_request.number,
            ChangeKind::Ci,
        ));

        let mut exact_read = fixture.forge.pause_after(
            CountedForgeOp::GetPullRequestByNumber,
            fixture.forge.count(CountedForgeOp::GetPullRequestByNumber) + 1,
        );
        stale_ci.release();
        exact_read.wait_until_paused().await;
        assert_eq!(
            fixture.forge.count(CountedForgeOp::MergePullRequest),
            0,
            "the stale active response cannot land the pull request"
        );
        assert_eq!(pull_request_state(&fixture).await, PullRequestState::Open);
        let exact_start = fixture
            .forge
            .operation_trace()
            .iter()
            .rposition(|op| *op == CountedForgeOp::GetPullRequestByNumber)
            .expect("exact follow-up starts");

        let mut merge = fixture
            .forge
            .pause_after(CountedForgeOp::MergePullRequest, 1);
        exact_read.release();
        merge.wait_until_paused().await;

        let trace = fixture.forge.operation_trace();
        let merge_index = first_op(&trace, exact_start, CountedForgeOp::MergePullRequest);
        let exact_trace = &trace[exact_start..=merge_index];
        assert_eq!(
            exact_trace,
            &[
                CountedForgeOp::GetPullRequestByNumber,
                CountedForgeOp::GetPullRequestByNumber,
                CountedForgeOp::ListCiJobs,
                CountedForgeOp::MergePullRequest,
            ],
            "the dirty generation uses only the exact PR fetch and required CI signal before landing"
        );
        let ci_query = fixture
            .forge
            .ci_job_queries()
            .last()
            .cloned()
            .expect("targeted follow-up reads CI");
        assert_eq!(
            ci_query.pull_request_id.as_ref(),
            Some(&fixture.pull_request.id)
        );
        let expected_head = format!("pr-{}-head", fixture.pull_request.number.get());
        assert_eq!(ci_query.commit_sha.as_deref(), Some(expected_head.as_str()));
        assert_eq!(pull_request_state(&fixture).await, PullRequestState::Merged);
        merge.release();
    });
}

#[test]
fn heartbeat_burst_keeps_ci_target_bounded_and_ahead_of_broad_work() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let fixture = fixture(&handle).await;
        let server = temper_engine::serve(
            &handle,
            &fixture.daemon,
            "127.0.0.1:0".parse().expect("loopback address"),
        )
        .await
        .expect("webhook server binds");
        let webhook_url = format!("http://{}/forgejo/webhook", server.local_addr());
        let mut stale_ci = fixture.forge.pause_after(CountedForgeOp::ListCiJobs, 1);
        fixture
            .daemon
            .schedule_mechanical_poll(fixture.path.clone());
        stale_ci.wait_until_paused().await;
        let stale_boundary = fixture.forge.operation_trace().len();

        // Proven heartbeat-only deliveries are acknowledged while Forge work is
        // deliberately unable to finish. The final heartbeat in each half is
        // also a FIFO coordinator barrier for the direct hint submissions.
        for _ in 0..8 {
            assert_eq!(
                post_webhook(&webhook_url, "issues", heartbeat_body()).await,
                202
            );
        }
        submit_duplicate_issue_and_broad_traffic(&fixture);
        assert_eq!(
            post_webhook(&webhook_url, "issues", heartbeat_body()).await,
            202
        );

        fixture
            .forge
            .inner()
            .seed_ci_jobs(&fixture.repo, vec![successful_ci(&fixture)]);
        assert_eq!(
            post_webhook(
                &webhook_url,
                "action_run_success",
                ci_webhook_body(fixture.pull_request.number),
            )
            .await,
            202
        );

        submit_duplicate_issue_and_broad_traffic(&fixture);
        for _ in 0..8 {
            assert_eq!(
                post_webhook(&webhook_url, "issues", heartbeat_body()).await,
                202
            );
        }
        assert_eq!(
            fixture.forge.operation_trace().len(),
            stale_boundary,
            "accepted traffic remains bounded behind the paused repository pass"
        );

        let mut exact_read = fixture.forge.pause_after(
            CountedForgeOp::GetPullRequestByNumber,
            fixture.forge.count(CountedForgeOp::GetPullRequestByNumber) + 1,
        );
        stale_ci.release();
        exact_read.wait_until_paused().await;
        let mut merge = fixture
            .forge
            .pause_after(CountedForgeOp::MergePullRequest, 1);
        exact_read.release();
        merge.wait_until_paused().await;

        // Retained broad work is part of the same dirty follow-up, but it must
        // start only after priority targets and their serialized mutation.
        let mut broad_read = fixture.forge.pause_after(
            CountedForgeOp::ListPullRequests,
            fixture.forge.count(CountedForgeOp::ListPullRequests) + 1,
        );
        merge.release();
        broad_read.wait_until_paused().await;

        let trace = fixture.forge.operation_trace();
        let pr_index = first_op(
            &trace,
            stale_boundary,
            CountedForgeOp::GetPullRequestByNumber,
        );
        let merge_index = first_op(&trace, stale_boundary, CountedForgeOp::MergePullRequest);
        let issue_index = first_op(&trace, stale_boundary, CountedForgeOp::GetIssueByNumber);
        let broad_index = first_op(&trace, stale_boundary, CountedForgeOp::ListPullRequests);
        assert!(pr_index < merge_index && merge_index < issue_index && issue_index < broad_index);
        let before_broad = &trace[stale_boundary..broad_index];
        assert_eq!(
            before_broad
                .iter()
                .filter(|op| **op == CountedForgeOp::GetPullRequestByNumber)
                .count(),
            3,
            "the exact CI target has one bounded processing budget despite duplicate traffic: {before_broad:?}"
        );
        assert_eq!(
            before_broad
                .iter()
                .filter(|op| **op == CountedForgeOp::GetIssueByNumber)
                .count(),
            1,
            "duplicate issue and heartbeat traffic does not grow target work"
        );
        assert_eq!(
            before_broad
                .iter()
                .filter(|op| **op == CountedForgeOp::MergePullRequest)
                .count(),
            1,
            "mechanical mutation remains serialized"
        );
        assert!(
            !before_broad.contains(&CountedForgeOp::ListPullRequests),
            "targeted landing precedes broad candidate discovery"
        );
        assert_eq!(pull_request_state(&fixture).await, PullRequestState::Merged);
        broad_read.release();
    });
}

#[test]
fn mechanical_poll_racing_targeted_work_runs_one_immediate_broad_follow_up() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let fixture = fixture(&handle).await;
        let server = temper_engine::serve(
            &handle,
            &fixture.daemon,
            "127.0.0.1:0".parse().expect("loopback address"),
        )
        .await
        .expect("webhook server binds");
        let webhook_url = format!("http://{}/forgejo/webhook", server.local_addr());
        let mut targeted_read = fixture
            .forge
            .pause_after(CountedForgeOp::GetPullRequestByNumber, 1);

        fixture.daemon.submit_change_hint(ChangeHint::pull_request(
            fixture.path.clone(),
            fixture.pull_request.number,
            ChangeKind::Ci,
        ));
        targeted_read.wait_until_paused().await;
        let targeted_start = fixture
            .forge
            .operation_trace()
            .iter()
            .rposition(|op| *op == CountedForgeOp::GetPullRequestByNumber)
            .expect("targeted pass starts");

        // This is exactly the callback performed by the periodic cadence. The
        // heartbeat response is a FIFO barrier proving the poll was admitted as
        // dirty while the targeted generation still owns the repository.
        fixture
            .daemon
            .schedule_mechanical_poll(fixture.path.clone());
        assert_eq!(
            post_webhook(&webhook_url, "issues", heartbeat_body()).await,
            202
        );

        let mut broad_read = fixture.forge.pause_after(
            CountedForgeOp::ListPullRequests,
            fixture.forge.count(CountedForgeOp::ListPullRequests) + 1,
        );
        targeted_read.release();
        broad_read.wait_until_paused().await;

        let trace = fixture.forge.operation_trace();
        let broad_index = first_op(&trace, targeted_start, CountedForgeOp::ListPullRequests);
        let completed_targeted = &trace[targeted_start..broad_index];
        assert_eq!(
            completed_targeted
                .iter()
                .filter(|op| **op == CountedForgeOp::ListCiJobs)
                .count(),
            1,
            "the admitted targeted pass completes before broad work"
        );
        assert!(
            !completed_targeted.contains(&CountedForgeOp::ListPullRequests),
            "no broad candidate read runs concurrently with targeted work"
        );
        assert_eq!(
            fixture.forge.count(CountedForgeOp::ListPullRequests),
            1,
            "one retained poll starts one immediate broad dirty follow-up"
        );
        assert_eq!(fixture.forge.count(CountedForgeOp::MergePullRequest), 0);
        assert_eq!(pull_request_state(&fixture).await, PullRequestState::Open);
        broad_read.release();
    });
}
