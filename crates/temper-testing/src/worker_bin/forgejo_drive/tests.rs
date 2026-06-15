use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration as StdDuration;
use temper_forge_model::{ChangeHint, ChangeKind, RepositoryPath};
use temper_runner::{Progress, Worker, WorkerError};
use temper_wake::send_wake_with_hint;

use super::*;
use crate::worker_bin::args::{
    AgentsKind, Backend, ClockKind, ForgejoArgs, WorkerArgs, WorkerKind,
};

struct BurstWorker {
    ticks: AtomicU64,
    reasons: Mutex<Vec<TickReason>>,
    socket: PathBuf,
}

#[async_trait]
impl Worker for BurstWorker {
    async fn tick(&self, _now: DateTime<Utc>) -> Result<Progress, WorkerError> {
        let tick = self.ticks.fetch_add(1, Ordering::SeqCst) + 1;
        if tick == 1 {
            let hint = ChangeHint::repo(RepositoryPath::new("acme", "service"), ChangeKind::Issue);
            for _ in 0..3 {
                send_wake_with_hint(&self.socket, Some("wake-secret"), &hint).expect("wake sends");
            }
        }
        Ok(Progress::unchanged())
    }

    fn name(&self) -> &str {
        "burst-worker"
    }
}

#[async_trait]
impl ForgejoDriveWorker for BurstWorker {
    async fn tick_for_reason(
        &self,
        now: DateTime<Utc>,
        reason: TickReason,
        _hints: &[ChangeHint],
    ) -> Result<ForgejoTickReport, WorkerError> {
        self.reasons.lock().expect("reasons lock").push(reason);
        Worker::tick(self, now).await.map(ForgejoTickReport::single)
    }

    fn name(&self) -> &str {
        Worker::name(self)
    }
}

#[test]
fn forgejo_drive_hint_wake_bypasses_idle_backoff() {
    let root = temp_root("hint-wake-bypasses-idle");
    std::fs::create_dir_all(&root).expect("temp root exists");
    let socket = root.join("worker.sock");
    let secret_file = root.join("wake-secret");
    let stop_file = root.join("stop");
    std::fs::write(&secret_file, "wake-secret\n").expect("secret writes");
    let args = WorkerArgs {
        kind: WorkerKind::Mechanical,
        backend: Backend::Forgejo(ForgejoArgs {
            base_url: "http://127.0.0.1:1".into(),
            token: "token".into(),
            username: None,
            password: None,
        }),
        root: root.clone(),
        owner: "acme".into(),
        name: "service".into(),
        repositories: vec![RepositoryPath::new("acme", "service")],
        poll_interval: Duration::seconds(60),
        idle_poll_max_interval: Duration::seconds(60),
        audit_interval: Some(Duration::milliseconds(600_000)),
        stop_file: Some(stop_file.clone()),
        run_secs: None,
        clock: ClockKind::Wall,
        agents: AgentsKind::Fake,
        wake_socket: Some(socket.clone()),
        wake_secret_file: Some(secret_file),
        workflow_file: None,
    };
    let worker = std::sync::Arc::new(BurstWorker {
        ticks: AtomicU64::new(0),
        reasons: Mutex::new(Vec::new()),
        socket,
    });
    let stop_file_for_thread = stop_file.clone();
    let stopper = thread::spawn(move || {
        thread::sleep(StdDuration::from_millis(800));
        std::fs::write(stop_file_for_thread, b"stop").expect("stop file writes");
    });
    let worker_for_drive = std::sync::Arc::clone(&worker);
    let report = temper_engine_io::block_on_with(move |cx, _handle| async move {
        drive_async(&cx, &args, &*worker_for_drive).await
    })
    .expect("drive succeeds");
    stopper.join().expect("stopper joins");

    assert_eq!(worker.ticks.load(Ordering::SeqCst), 2);
    assert_eq!(report.ticks, 2);
    assert_eq!(
        *worker.reasons.lock().expect("reasons lock"),
        vec![TickReason::Initial, TickReason::Wake]
    );
    let _ = std::fs::remove_dir_all(root);
}

fn temp_root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "temper-testing-forgejo-{name}-{}-{}",
        std::process::id(),
        Utc::now()
            .timestamp_nanos_opt()
            .expect("timestamp has nanoseconds")
    ))
}
