//! Ignored parallel regressions for Temper's Forgejo provisioning helpers.
//!
//! The test starts the same cached reference-delivery state from multiple
//! threads and proves each caller receives an independent server copy.
//!
//! ```sh
//! cargo test -p temper-testing --test forgejo_parallel -- --ignored --nocapture
//! ```

use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Barrier};
use std::time::Duration;
use temper_testing::forgejo_server::start_cached_provisioned_server;

const PARALLELISM: usize = 2;
const STARTUP_TIMEOUT: Duration = Duration::from_secs(300);
const RELEASE_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Debug)]
struct ProvisionedInfo {
    worker: usize,
    base_url: String,
    data_dir: PathBuf,
    cache_key: String,
    cache_hit: bool,
    owner: String,
    name: String,
    repository_id: String,
}

#[test]
#[ignore = "boots real Forgejo servers from the shared provisioned-state cache; run with --ignored"]
fn cached_reference_delivery_state_is_safe_for_parallel_callers() {
    let start_barrier = Arc::new(Barrier::new(PARALLELISM));
    let (report_tx, report_rx) = mpsc::channel();
    let mut release = ReleaseOnDrop::new();
    let mut handles = Vec::new();

    for worker in 0..PARALLELISM {
        let start_barrier = Arc::clone(&start_barrier);
        let report_tx = report_tx.clone();
        let (release_tx, release_rx) = mpsc::channel();
        release.push(release_tx);
        handles.push(std::thread::spawn(move || -> Result<(), String> {
            start_barrier.wait();
            let cached = match start_cached_provisioned_server() {
                Ok(cached) => cached,
                Err(error) => {
                    let message =
                        format!("worker {worker} provisioned server start failed: {error}");
                    let _ = report_tx.send(Err(message.clone()));
                    return Err(message);
                }
            };
            let info = ProvisionedInfo {
                worker,
                base_url: cached.server.base_url().to_string(),
                data_dir: cached.server.data_dir().to_path_buf(),
                cache_key: cached.cache_key.clone(),
                cache_hit: cached.cache_hit,
                owner: cached.provisioned.owner.clone(),
                name: cached.provisioned.name.clone(),
                repository_id: cached.provisioned.repository.as_str().to_string(),
            };
            if let Err(error) = read_exact_repo(&info, &cached.provisioned.admin_token) {
                let message = format!("worker {worker} exact repo read failed: {error}");
                let _ = report_tx.send(Err(message.clone()));
                return Err(message);
            }
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

    let infos = collect_reports(report_rx, PARALLELISM, &mut release, &mut handles);
    assert_eq!(infos.len(), PARALLELISM);
    assert_distinct(
        infos.iter().map(|info| info.base_url.as_str()),
        "server base URLs",
    );
    assert_distinct(
        infos.iter().map(|info| info.data_dir.as_path()),
        "server data dirs",
    );
    assert_eq!(
        infos
            .iter()
            .map(|info| info.cache_key.as_str())
            .collect::<HashSet<_>>()
            .len(),
        1,
        "all parallel callers should report the same provisioned-state cache key"
    );
    let temp_dir = std::env::temp_dir();
    assert!(
        infos
            .iter()
            .all(|info| info.data_dir.starts_with(&temp_dir)),
        "runtime data dirs should be per-test temp copies under {}: {infos:?}",
        temp_dir.display()
    );
    eprintln!(
        "parallel cached provisioned starts: cache_hits={}/{} cache_key={}",
        infos.iter().filter(|info| info.cache_hit).count(),
        infos.len(),
        infos[0].cache_key
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

fn read_exact_repo(info: &ProvisionedInfo, token: &str) -> Result<(), String> {
    let client = temper_io_engine::http::BlockingJsonClient::new();
    let url = format!(
        "{}/api/v1/repos/{}/{}",
        info.base_url, info.owner, info.name
    );
    let response = client
        .send("GET", url.as_str(), Some(token), None)
        .map_err(|error| format!("repo read {url} failed to send: {error}"))?;
    let status = response.status;
    let body = String::from_utf8_lossy(&response.body).into_owned();
    if !(200..300).contains(&status) {
        return Err(format!("repo read {url} failed: {status} {body}"));
    }
    let json: Value =
        serde_json::from_str(&body).map_err(|error| format!("repo read JSON failed: {error}"))?;
    let full_name = format!("{}/{}", info.owner, info.name);
    if json["full_name"].as_str() != Some(full_name.as_str()) {
        return Err(format!(
            "repo read returned wrong full_name: expected {full_name}, body {json}"
        ));
    }
    if info.repository_id != format!("forgejo:{full_name}") {
        return Err(format!(
            "unexpected portable repository id {} for {full_name}",
            info.repository_id
        ));
    }
    Ok(())
}

fn collect_reports<T>(
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
        panic!(
            "parallel provisioned workers failed:\n{}",
            errors.join("\n")
        );
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
        "parallel provisioned workers failed during teardown:\n{}",
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
    let client = temper_io_engine::http::BlockingJsonClient::new();
    for _ in 0..25 {
        let port_is_down = client
            .send("GET", version_url.as_str(), None, None)
            .is_err();
        let data_dir_is_gone = !data_dir.exists();
        if port_is_down && data_dir_is_gone {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err(format!(
        "server did not tear down cleanly: url_down={} data_dir_exists={} ({})",
        client
            .send("GET", version_url.as_str(), None, None)
            .is_err(),
        data_dir.exists(),
        data_dir.display()
    ))
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
