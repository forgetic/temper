//! Ignored parallel stress regressions for the local Forgejo fixture.
//!
//! These tests deliberately start real Forgejo processes concurrently. A normal
//! `cargo test` remains offline because every test here is `#[ignore]`d.
//!
//! ```sh
//! cargo test -p temper-forgejo-fixture --test parallel -- --ignored --nocapture
//! ```

use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Barrier};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use temper_forgejo_fixture::{download, ForgejoRunner, ForgejoServer, ForgejoState};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(300);
const RELEASE_TIMEOUT: Duration = Duration::from_secs(60);
const PARALLEL_SERVER_STARTS: usize = 4;
const PARALLEL_RUNNER_REGISTRATIONS: usize = 2;

#[derive(Debug, Deserialize, Serialize)]
struct ParallelMetadata {
    marker: String,
}

#[derive(Clone, Debug)]
struct ServerInfo {
    worker: usize,
    base_url: String,
    data_dir: PathBuf,
    cache_key: String,
    cache_hit: bool,
}

#[derive(Clone, Debug)]
struct RunnerInfo {
    worker: usize,
    base_url: String,
    server_data_dir: PathBuf,
    runner_name: String,
    runner_work_dir: PathBuf,
}

#[test]
#[ignore = "boots several real Forgejo servers concurrently; run with --ignored"]
fn same_state_startups_are_parallel_cache_safe() {
    let run_id = unique_run_id("same-state-startups");
    let state = ForgejoState::new(json!({
        "kind": "parallel-fixture-state-startup-stress",
        "version": 1,
        "run_id": run_id.clone(),
    }))
    .expect("parallel stress state serializes");
    let expected_key = state.cache_key().expect("cache key computes");
    let cache_paths = StateCachePaths::for_key(&expected_key);
    cache_paths.remove();
    let _cleanup = StateCacheCleanup(cache_paths.clone());

    let initialize_calls = Arc::new(AtomicUsize::new(0));
    let start_barrier = Arc::new(Barrier::new(PARALLEL_SERVER_STARTS));
    let (report_tx, report_rx) = mpsc::channel();
    let mut release = ReleaseOnDrop::new();
    let mut handles = Vec::new();

    for worker in 0..PARALLEL_SERVER_STARTS {
        let state = state.clone();
        let marker = run_id.clone();
        let initialize_calls = Arc::clone(&initialize_calls);
        let start_barrier = Arc::clone(&start_barrier);
        let report_tx = report_tx.clone();
        let (release_tx, release_rx) = mpsc::channel();
        release.push(release_tx);
        handles.push(std::thread::spawn(move || -> Result<(), String> {
            start_barrier.wait();
            let cached = match ForgejoServer::start_with_state(&state, |_server| {
                initialize_calls.fetch_add(1, Ordering::SeqCst);
                Ok::<ParallelMetadata, String>(ParallelMetadata {
                    marker: marker.clone(),
                })
            }) {
                Ok(cached) => cached,
                Err(error) => {
                    let message = format!("worker {worker} start_with_state failed: {error}");
                    let _ = report_tx.send(Err(message.clone()));
                    return Err(message);
                }
            };
            if cached.metadata.marker != marker {
                let message = format!(
                    "worker {worker} read wrong metadata marker {:?}, expected {marker:?}",
                    cached.metadata.marker
                );
                let _ = report_tx.send(Err(message.clone()));
                return Err(message);
            }

            let info = ServerInfo {
                worker,
                base_url: cached.server.base_url().to_string(),
                data_dir: cached.server.data_dir().to_path_buf(),
                cache_key: cached.cache_key.clone(),
                cache_hit: cached.cache_hit,
            };
            report_tx
                .send(Ok(info.clone()))
                .map_err(|error| error.to_string())?;

            release_rx
                .recv_timeout(RELEASE_TIMEOUT)
                .map_err(|error| format!("worker {worker} was not released: {error}"))?;
            drop(cached);
            assert_server_teardown(&info.base_url, &info.data_dir)
                .map_err(|error| format!("worker {worker}: {error}"))
        }));
    }
    drop(report_tx);

    let infos = collect_reports(
        report_rx,
        PARALLEL_SERVER_STARTS,
        &mut release,
        &mut handles,
    );
    assert_eq!(infos.len(), PARALLEL_SERVER_STARTS);
    assert_distinct(
        infos.iter().map(|info| info.base_url.as_str()),
        "server base URLs",
    );
    assert_distinct(
        infos.iter().map(|info| info.data_dir.as_path()),
        "server data dirs",
    );
    for info in &infos {
        assert_eq!(
            info.cache_key, expected_key,
            "worker {} reported an unexpected cache key",
            info.worker
        );
        assert_ne!(
            info.data_dir, cache_paths.tree,
            "worker {} used the cached tree itself as its runtime data dir",
            info.worker
        );
        assert!(
            !info.data_dir.starts_with(&cache_paths.root),
            "worker {} runtime data dir {} should be a /tmp copy, not inside the cache root {}",
            info.worker,
            info.data_dir.display(),
            cache_paths.root.display()
        );
    }
    assert!(
        cache_paths.tree.is_dir(),
        "state cache tree was not published at {}",
        cache_paths.tree.display()
    );

    let cache_misses = infos.iter().filter(|info| !info.cache_hit).count();
    assert_eq!(
        cache_misses, 1,
        "a unique empty state cache should be published by exactly one caller"
    );
    assert_eq!(
        initialize_calls.load(Ordering::SeqCst),
        1,
        "the cached-state initializer should run once"
    );

    release.release_all();
    join_all(handles);
    for info in &infos {
        assert!(
            !info.data_dir.exists(),
            "worker {} data dir still exists after teardown: {}",
            info.worker,
            info.data_dir.display()
        );
    }
}

#[test]
#[ignore = "boots real Forgejo servers and host-mode forgejo-runners; run with --ignored"]
fn concurrent_runner_registrations_use_distinct_identities() {
    match download::ensure_runner_binary() {
        Ok(path) => eprintln!("using forgejo-runner binary {}", path.display()),
        Err(error) => {
            eprintln!(
                "skipping concurrent runner-registration stress: forgejo-runner binary could not be resolved: {error}"
            );
            return;
        }
    }

    let run_id = unique_run_id("runner-registrations");
    let state = ForgejoState::new(json!({
        "kind": "parallel-fixture-runner-registration-stress",
        "version": 1,
        "run_id": run_id.clone(),
    }))
    .expect("runner stress state serializes");
    let cache_key = state.cache_key().expect("cache key computes");
    let cache_paths = StateCachePaths::for_key(&cache_key);
    cache_paths.remove();
    let _cleanup = StateCacheCleanup(cache_paths);

    let cached_servers = (0..PARALLEL_RUNNER_REGISTRATIONS)
        .map(|worker| {
            ForgejoServer::start_with_state(&state, |_| Ok::<(), String>(())).unwrap_or_else(
                |error| panic!("worker {worker} server for runner registration starts: {error}"),
            )
        })
        .collect::<Vec<_>>();

    let register_barrier = Arc::new(Barrier::new(PARALLEL_RUNNER_REGISTRATIONS));
    let (report_tx, report_rx) = mpsc::channel();
    let mut release = ReleaseOnDrop::new();
    let mut handles = Vec::new();

    for (worker, cached) in cached_servers.into_iter().enumerate() {
        let register_barrier = Arc::clone(&register_barrier);
        let report_tx = report_tx.clone();
        let (release_tx, release_rx) = mpsc::channel();
        release.push(release_tx);
        handles.push(std::thread::spawn(move || -> Result<(), String> {
            register_barrier.wait();
            let mut runner = match ForgejoRunner::register(&cached.server) {
                Ok(runner) => runner,
                Err(error) => {
                    let message = format!("worker {worker} runner register failed: {error}");
                    let _ = report_tx.send(Err(message.clone()));
                    return Err(message);
                }
            };
            if !runner.is_running() {
                let message = format!(
                    "worker {worker} runner {} exited immediately; log: {}",
                    runner.name(),
                    runner.log_tail()
                );
                let _ = report_tx.send(Err(message.clone()));
                return Err(message);
            }

            let info = RunnerInfo {
                worker,
                base_url: cached.server.base_url().to_string(),
                server_data_dir: cached.server.data_dir().to_path_buf(),
                runner_name: runner.name().to_string(),
                runner_work_dir: runner.work_dir().to_path_buf(),
            };
            report_tx
                .send(Ok(info.clone()))
                .map_err(|error| error.to_string())?;

            release_rx
                .recv_timeout(RELEASE_TIMEOUT)
                .map_err(|error| format!("worker {worker} was not released: {error}"))?;
            drop(runner);
            if info.runner_work_dir.exists() {
                return Err(format!(
                    "runner work dir still exists after drop: {}",
                    info.runner_work_dir.display()
                ));
            }
            drop(cached);
            assert_server_teardown(&info.base_url, &info.server_data_dir)
                .map_err(|error| format!("worker {worker}: {error}"))
        }));
    }
    drop(report_tx);

    let infos = collect_reports(
        report_rx,
        PARALLEL_RUNNER_REGISTRATIONS,
        &mut release,
        &mut handles,
    );
    assert_distinct(
        infos.iter().map(|info| info.base_url.as_str()),
        "runner-test server base URLs",
    );
    assert_distinct(
        infos.iter().map(|info| info.server_data_dir.as_path()),
        "runner-test server data dirs",
    );
    assert_distinct(
        infos.iter().map(|info| info.runner_name.as_str()),
        "runner names",
    );
    assert_distinct(
        infos.iter().map(|info| info.runner_work_dir.as_path()),
        "runner work dirs",
    );

    release.release_all();
    join_all(handles);
    for info in &infos {
        assert!(
            !info.runner_work_dir.exists(),
            "worker {} runner work dir still exists after teardown: {}",
            info.worker,
            info.runner_work_dir.display()
        );
        assert!(
            !info.server_data_dir.exists(),
            "worker {} server data dir still exists after teardown: {}",
            info.worker,
            info.server_data_dir.display()
        );
    }
}

fn collect_reports<T: std::fmt::Debug>(
    report_rx: mpsc::Receiver<Result<T, String>>,
    expected: usize,
    release: &mut ReleaseOnDrop,
    handles: &mut Vec<std::thread::JoinHandle<Result<(), String>>>,
) -> Vec<T> {
    let mut infos = Vec::new();
    let mut errors = Vec::new();
    for _ in 0..expected {
        match report_rx.recv_timeout(STARTUP_TIMEOUT) {
            Ok(Ok(info)) => infos.push(info),
            Ok(Err(error)) => errors.push(error),
            Err(error) => errors.push(format!("timed out waiting for worker report: {error}")),
        }
    }
    if !errors.is_empty() {
        release.release_all();
        let joined = std::mem::take(handles);
        for handle in joined {
            match handle.join() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => errors.push(error),
                Err(panic) => {
                    errors.push(format!("worker thread panicked: {}", panic_payload(panic)))
                }
            }
        }
        panic!("parallel stress workers failed:\n{}", errors.join("\n"));
    }
    infos
}

fn join_all(handles: Vec<std::thread::JoinHandle<Result<(), String>>>) {
    let mut errors = Vec::new();
    for handle in handles {
        match handle.join() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => errors.push(error),
            Err(panic) => errors.push(format!("worker thread panicked: {}", panic_payload(panic))),
        }
    }
    assert!(
        errors.is_empty(),
        "parallel stress workers failed during teardown:\n{}",
        errors.join("\n")
    );
}

fn panic_payload(panic: Box<dyn std::any::Any + Send + 'static>) -> String {
    if let Some(message) = panic.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = panic.downcast_ref::<String>() {
        message.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

fn assert_distinct<T, I>(values: I, label: &str)
where
    T: Eq + std::hash::Hash + std::fmt::Debug,
    I: IntoIterator<Item = T>,
{
    let values = values.into_iter().collect::<Vec<_>>();
    let unique = values.iter().collect::<HashSet<_>>();
    assert_eq!(
        unique.len(),
        values.len(),
        "{label} should be distinct: {values:?}"
    );
}

fn assert_server_teardown(base_url: &str, data_dir: &Path) -> Result<(), String> {
    let version_url = format!("{base_url}/api/v1/version");
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(500))
        .build()
        .map_err(|error| error.to_string())?;
    for _ in 0..25 {
        let port_is_down = client.get(&version_url).send().is_err();
        let data_dir_is_gone = !data_dir.exists();
        if port_is_down && data_dir_is_gone {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err(format!(
        "server did not tear down cleanly: url_down={} data_dir_exists={} ({})",
        client.get(&version_url).send().is_err(),
        data_dir.exists(),
        data_dir.display()
    ))
}

#[derive(Clone)]
struct StateCachePaths {
    root: PathBuf,
    tree: PathBuf,
    lock: PathBuf,
}

impl StateCachePaths {
    fn for_key(key: &str) -> Self {
        let root = workspace_root()
            .join(".cache")
            .join("forgejo")
            .join("states")
            .join(key);
        Self {
            tree: root.join("tree"),
            lock: root.with_extension("lock"),
            root,
        }
    }

    fn remove(&self) {
        remove_path_if_exists(&self.root);
        remove_path_if_exists(&self.lock);
    }
}

struct StateCacheCleanup(StateCachePaths);

impl Drop for StateCacheCleanup {
    fn drop(&mut self) {
        self.0.remove();
    }
}

struct ReleaseOnDrop {
    senders: Vec<mpsc::Sender<()>>,
}

impl ReleaseOnDrop {
    fn new() -> Self {
        Self {
            senders: Vec::new(),
        }
    }

    fn push(&mut self, sender: mpsc::Sender<()>) {
        self.senders.push(sender);
    }

    fn release_all(&mut self) {
        for sender in self.senders.drain(..) {
            let _ = sender.send(());
        }
    }
}

impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.release_all();
    }
}

fn remove_path_if_exists(path: &Path) {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => {
            std::fs::remove_dir_all(path)
                .unwrap_or_else(|error| panic!("removing {} failed: {error}", path.display()));
        }
        Ok(_) => {
            std::fs::remove_file(path)
                .unwrap_or_else(|error| panic!("removing {} failed: {error}", path.display()));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => panic!("checking {} failed: {error}", path.display()),
    }
}

fn workspace_root() -> PathBuf {
    let mut candidates = Vec::new();
    if let Ok(path) = std::env::current_dir() {
        candidates.push(path);
    }
    if let Some(value) = std::env::var_os("CARGO_MANIFEST_DIR") {
        if !value.is_empty() {
            candidates.push(PathBuf::from(value));
        }
    }
    candidates
        .into_iter()
        .find_map(|start| {
            start
                .ancestors()
                .find(|dir| {
                    dir.join("Cargo.toml").is_file()
                        && dir.join("crates/temper-forgejo-fixture").is_dir()
                })
                .map(Path::to_path_buf)
        })
        .expect("workspace root resolves")
}

fn unique_run_id(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after Unix epoch")
        .as_nanos();
    format!("{prefix}-{}-{nanos}", std::process::id())
}
