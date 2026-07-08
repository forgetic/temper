// SPDX-License-Identifier: MPL-2.0

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use temper_engine::{
    Daemon, PollBackstopConfig, RoleFeedMode, RoleFeedTarget, spawn_poll_backstop,
};
use temper_forge::{CreateIssue, CreateRepository, Forge, ItemNumber, RepositoryId, UserId};
use temper_protocol_worker::{
    Assign, Capability, Capacity, ErrorCode, Poll, Register, WORKER_PROTOCOL_VERSION,
    WorkerProtocolMessage,
};
use temper_workflow::{RawWorkflowSpec, RoleId, ValidatedWorkflow};

const FIXTURE: &str = include_str!("../../temper-workflow/fixtures/reference-delivery.json");

fn workflow() -> ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(FIXTURE).expect("workflow parses");
    spec.validate().expect("workflow validates")
}

fn repo_input() -> CreateRepository {
    CreateRepository {
        owner: "acme".into(),
        name: "service".into(),
        default_branch: "main".into(),
        description: None,
    }
}

async fn create_repo(forge: &dyn Forge) -> RepositoryId {
    forge
        .create_repository(repo_input())
        .await
        .expect("repository is created")
        .id
}

async fn create_ready_issue(forge: &dyn Forge, repo: &RepositoryId) -> ItemNumber {
    forge
        .create_issue(
            repo,
            CreateIssue {
                title: "ready code issue".into(),
                body: "Implement change-source wake wiring.".into(),
                labels: vec!["code".into(), "ready".into()],
                assignees: Vec::<UserId>::new(),
            },
        )
        .await
        .expect("issue is created")
        .number
}

fn register(worker_id: &str) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Register(Register {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        capabilities: vec![Capability {
            role: "engineer".to_string(),
            repo: "acme/service".to_string(),
        }],
        capacity: Capacity {
            max_concurrent_jobs: 1,
        },
        worker_pool: None,
        labels: None,
    })
}

fn poll_with_wait(worker_id: &str, max_wait_ms: u64) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Poll(Poll {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        free_capacity: 1,
        max_wait_ms: Some(max_wait_ms),
    })
}

async fn deliver(daemon: &Daemon, message: WorkerProtocolMessage) -> Option<WorkerProtocolMessage> {
    daemon
        .deliver_protocol_message(message)
        .await
        .expect("daemon accepts in-process message")
}

async fn register_engineer(daemon: &Daemon) {
    assert!(deliver(daemon, register("worker-a")).await.is_none());
}

fn assert_scanned_issue_assignment(msg: WorkerProtocolMessage, issue: ItemNumber) -> Assign {
    match msg {
        WorkerProtocolMessage::Assign(assign) => {
            assert_eq!(assign.repo, "acme/service");
            assert_eq!(assign.role, "engineer");
            assert_eq!(assign.artifact.kind, "issue");
            assert_eq!(assign.artifact.item, json!(issue.get()));
            assert!(
                assign
                    .job_id
                    .contains(&format!("/issue-{}/engineer/", issue.get()))
            );
            assign
        }
        other => panic!("expected assign, got {other:?}"),
    }
}

fn assert_poll_timeout(msg: WorkerProtocolMessage) {
    match msg {
        WorkerProtocolMessage::Error(error) => assert_eq!(error.code, ErrorCode::PollTimeout),
        other => panic!("expected poll timeout, got {other:?}"),
    }
}

enum Backend {
    Memory,
    Filesystem(TempRoot),
}

impl Backend {
    fn memory() -> Self {
        Self::Memory
    }

    fn filesystem() -> Self {
        Self::Filesystem(TempRoot::new("change-source-wake"))
    }

    fn forge_with_change_source(&self) -> temper_forge::factory::ForgeWithChangeSource {
        match self {
            Self::Memory => temper_forge::factory::new_memory_with_change_source(),
            Self::Filesystem(root) => {
                temper_forge::factory::new_filesystem_with_change_source(root.path())
            }
        }
    }
}

struct TempRoot {
    path: PathBuf,
}

impl TempRoot {
    fn new(suite: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "temper-engine-{suite}-{}-{}",
            std::process::id(),
            chrono::Utc::now()
                .timestamp_nanos_opt()
                .expect("timestamp has nanos")
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("temp root is created");
        Self { path }
    }

    fn path(&self) -> PathBuf {
        self.path.clone()
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn role_target(repo: RepositoryId) -> RoleFeedTarget {
    RoleFeedTarget {
        repo,
        role: RoleId::new("engineer"),
        mode: RoleFeedMode::Wake,
    }
}

fn normal_role_target(repo: RepositoryId) -> RoleFeedTarget {
    RoleFeedTarget {
        repo,
        role: RoleId::new("engineer"),
        mode: RoleFeedMode::Normal,
    }
}

#[test]
fn memory_change_source_hint_wakes_daemon_scan_before_poll_deadline() {
    change_source_hint_wakes_daemon_scan_before_poll_deadline(Backend::memory());
}

#[test]
fn filesystem_change_source_hint_wakes_daemon_scan_before_poll_deadline() {
    change_source_hint_wakes_daemon_scan_before_poll_deadline(Backend::filesystem());
}

fn change_source_hint_wakes_daemon_scan_before_poll_deadline(backend: Backend) {
    temper_engine_io::block_on_with(move |cx, handle| async move {
        let temper_forge::factory::ForgeWithChangeSource {
            forge,
            change_source,
        } = backend.forge_with_change_source();
        let repo = create_repo(forge.as_ref()).await;
        let workflow = Arc::new(workflow());
        let compiled = Arc::new(workflow.compile());
        let daemon = Daemon::new(Arc::new(handle.clone())).with_change_source(
            Arc::clone(&forge),
            workflow,
            compiled,
            change_source,
            vec![role_target(repo.clone())],
            temper_engine::system_clock(),
        );
        register_engineer(&daemon).await;

        let poll_daemon = daemon.clone();
        let poll_task = handle.spawn(async move {
            let started = Instant::now();
            let reply = deliver(&poll_daemon, poll_with_wait("worker-a", 5_000))
                .await
                .expect("poll returns a protocol message");
            (reply, started.elapsed())
        });

        temper_engine_io::runtime::sleep_for(&cx, Duration::from_millis(100)).await;
        let issue = create_ready_issue(forge.as_ref(), &repo).await;

        let (reply, elapsed) = poll_task.await;
        assert_scanned_issue_assignment(reply, issue);
        assert!(elapsed < Duration::from_secs(2), "elapsed: {elapsed:?}");
    })
}

#[test]
fn poll_backstop_assigns_work_when_change_hints_are_missing() {
    temper_engine_io::block_on_with(move |cx, handle| async move {
        let forge = temper_forge::factory::new_memory();
        let repo = create_repo(forge.as_ref()).await;
        let workflow = Arc::new(workflow());
        let compiled = Arc::new(workflow.compile());
        let spawner: Arc<dyn temper_engine_io::Spawner> = Arc::new(handle.clone());
        let daemon = Daemon::new(Arc::clone(&spawner));
        spawn_poll_backstop(
            &spawner,
            daemon.clone(),
            Arc::clone(&forge),
            Arc::clone(&workflow),
            Arc::clone(&compiled),
            PollBackstopConfig {
                targets: vec![normal_role_target(repo.clone())],
                cadence: Duration::from_millis(100),
            },
            temper_engine::system_clock(),
        );
        register_engineer(&daemon).await;

        let poll_daemon = daemon.clone();
        let poll_task = handle.spawn(async move {
            deliver(&poll_daemon, poll_with_wait("worker-a", 1_500))
                .await
                .expect("poll returns a protocol message")
        });

        temper_engine_io::runtime::sleep_for(&cx, Duration::from_millis(50)).await;
        let issue = create_ready_issue(forge.as_ref(), &repo).await;

        let reply = poll_task.await;
        if matches!(reply, WorkerProtocolMessage::Error(_)) {
            assert_poll_timeout(reply);
            panic!("poll backstop did not assign work before the long-poll deadline");
        }
        assert_scanned_issue_assignment(reply, issue);
    })
}
