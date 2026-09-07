//! Native subscription cancellation observed through actual contained jobs.

use super::*;
use std::collections::BTreeSet;
use temper_codebase_memory_runtime::{LaunchConfig, ProviderBootstrap, ProviderOwnerManager};

#[test]
#[ignore = "installed provider physical shared/exclusive index work; isolated account only"]
fn installed_provider_shared_and_exclusive_inflight_cancellation() {
    let mut runtime = ProviderRuntime::new();
    runtime.args.insert(0, "CBM_LOG_LEVEL=info".into());
    let repo = runtime.directory.path().join("repo");
    fs::create_dir(&repo).unwrap();
    fs::write(
        repo.join("sentinel.py"),
        "def sentinel():\n    return 1280\n",
    )
    .unwrap();
    git_init(&repo);
    let work_root = seed_work(&runtime);
    assert!(daemon_pid(&runtime).is_none(), "A admission must be cold");
    let manager = ProviderOwnerManager::default();
    let helper = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("temper-agent-containment-helper");
    let context = crate::containment_tests::containment_context();
    let bootstrap = ProviderBootstrap::start(
        &manager,
        LaunchConfig {
            command: "env".into(),
            args: runtime.args.clone(),
            workspace: repo,
            environment: Vec::new(),
            startup_timeout: Duration::from_secs(30),
            admission_timeout: Duration::from_secs(90),
        },
        context.factory(),
        &helper,
    )
    .expect("cold admission");
    let daemon = daemon_pid(&runtime).unwrap();
    let generation = start_tick(daemon).unwrap();
    let completion = bootstrap.completion();
    let a = actor(&runtime, "a");
    ready(&runtime, "a");
    let b = actor(&runtime, "b");
    let b_owner = ready(&runtime, "b");
    bootstrap.serving_admitted();

    request_index(&runtime, "a", "shared", &work_root, "work-shared");
    let shared = stopped_worker(&runtime, daemon, &work_root, "work-shared");
    request_index(&runtime, "b", "shared", &work_root, "work-shared");
    wait_file(&runtime, "b.shared.index_sent");
    // B submits while the exact physical worker is stopped and cannot finish.
    // The cancellation control below distinguishes subscription from mere TCP
    // survival: zero subscribers causes native TERM/KILL within one second.
    std::thread::sleep(Duration::from_secs(2));
    assert!(
        !runtime
            .directory
            .path()
            .join("b.shared.index_result")
            .exists()
    );
    assert_empty(a.cleanup(CleanupTrigger::Cancellation));
    std::thread::sleep(Duration::from_secs(3));
    assert!(
        shared.alive(),
        "B retained the original in-flight physical worker after A was empty"
    );
    assert_eq!(start_tick(daemon), Some(generation));
    shared.signal("-CONT");
    let mut observed = BTreeSet::from([(shared.pid, shared.start)]);
    wait_until(
        || {
            for worker in workers(daemon, &work_root, "work-shared") {
                observed.insert(worker);
            }
            runtime
                .directory
                .path()
                .join("b.shared.index_result")
                .exists()
        },
        "B completes the same shared index",
    );
    assert_eq!(observed.len(), 1, "no replacement physical worker");
    assert_eq!(
        fs::read_to_string(runtime.directory.path().join("b.shared.index_result")).unwrap(),
        "true"
    );
    wait_until(|| !shared.alive(), "shared native worker clean exit");
    assert!(
        !shared.log.exists(),
        "native clean completion removes this exact worker log; cancellation retains it"
    );
    fs::write(runtime.directory.path().join("b.project"), "work-shared").unwrap();
    query(&runtime, "b", "shared-finished");
    assert!(query_result(&runtime, "b", "shared-finished"));

    let exclusive_a = actor(&runtime, "exclusive-a");
    ready(&runtime, "exclusive-a");
    request_index(
        &runtime,
        "exclusive-a",
        "exclusive",
        &work_root,
        "work-exclusive",
    );
    let exclusive = stopped_worker(&runtime, daemon, &work_root, "work-exclusive");
    assert_empty(exclusive_a.cleanup(CleanupTrigger::Cancellation));
    let cancellation_started = Instant::now();
    // Keep STOPPED until the native supervisor cancels it. Normal completion
    // is impossible, and this interval is below the native 15-minute timeout.
    wait_until(
        || !exclusive.alive(),
        "exclusive native worker cancelled while stopped",
    );
    assert!(cancellation_started.elapsed() < Duration::from_secs(30));
    let daemon_log = runtime.directory.path().join("cache/logs/cbm-daemon.log");
    wait_until(
        || cancelled(&daemon_log, &exclusive.log),
        "correlated native worker_cancelled event",
    );
    assert!(workers(daemon, &work_root, "work-exclusive").is_empty());
    assert_eq!(start_tick(daemon), Some(generation));
    query(&runtime, "b", "exclusive-cancelled");
    assert!(query_result(&runtime, "b", "exclusive-cancelled"));

    fs::write(runtime.directory.path().join("b.stop"), "stop").unwrap();
    wait_until(|| start_tick(b_owner).is_none(), "final B client drain");
    let report = completion
        .wait(Duration::from_secs(40))
        .expect("native last-session shutdown");
    assert_eq!(
        report.disposition(),
        temper_process_containment::CleanupDisposition::AlreadyEmpty
    );
    assert_empty(report);
    assert!(
        start_tick(daemon).is_none(),
        "natural shutdown before fallback stop"
    );
    b.cleanup(CleanupTrigger::NormalRootExit);
    manager.reap_completed();
    println!(
        "cbm_native_work cold_a_admission=true a_cleanup=proven b_joined_inflight=true shared_physical_starts={} shared_worker={} shared_start={} shared_clean_completion=true exclusive_worker={} exclusive_start={} exclusive_stopped_until_cancel=true native_cancel_diagnostic=true b_exact_query=true same_daemon=true final_native_exit=true",
        observed.len(),
        shared.pid,
        shared.start,
        exclusive.pid,
        exclusive.start
    );
}

fn git_init(root: &std::path::Path) {
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .arg(root)
            .status()
            .unwrap()
            .success()
    );
}

fn seed_work(runtime: &ProviderRuntime) -> PathBuf {
    let root = runtime.directory.path().join("work");
    fs::create_dir(&root).unwrap();
    fs::write(
        root.join("work.py"),
        "def work_sentinel():\n    return 1280\n",
    )
    .unwrap();
    for file in 0..300 {
        let content = (0..100)
            .map(|symbol| {
                format!("def symbol_{file}_{symbol}(value):\n    return value + {symbol}\n\n")
            })
            .collect::<String>();
        fs::write(root.join(format!("unit_{file}.py")), content).unwrap();
    }
    git_init(&root);
    root
}

fn request_index(
    runtime: &ProviderRuntime,
    actor: &str,
    sequence: &str,
    root: &std::path::Path,
    project: &str,
) {
    fs::write(
        runtime.directory.path().join(format!("{actor}.index")),
        json!({"sequence":sequence,"arguments":{"repo_path":root,"name":project}}).to_string(),
    )
    .unwrap();
}

fn wait_file(runtime: &ProviderRuntime, file: &str) {
    wait_until(|| runtime.directory.path().join(file).exists(), file);
}

fn assert_empty(report: temper_process_containment::CleanupReport) {
    assert!(matches!(
        report.recursive_empty(),
        temper_process_containment::RecursiveEmptyProof::Proven { .. }
    ));
}

struct StoppedWorker {
    pid: u32,
    start: u64,
    log: PathBuf,
}

impl StoppedWorker {
    fn alive(&self) -> bool {
        start_tick(self.pid) == Some(self.start)
    }
    fn signal(&self, signal: &str) {
        assert!(self.alive(), "signal only the captured task-owned identity");
        assert!(
            Command::new("kill")
                .args([signal, &self.pid.to_string()])
                .status()
                .unwrap()
                .success()
        );
    }
}

impl Drop for StoppedWorker {
    fn drop(&mut self) {
        if self.alive() {
            // Failure recovery only; fixture containment still owns cleanup.
            let _ = Command::new("kill")
                .args(["-CONT", &self.pid.to_string()])
                .status();
        }
    }
}

fn stopped_worker(
    runtime: &ProviderRuntime,
    daemon: u32,
    root: &std::path::Path,
    project: &str,
) -> StoppedWorker {
    let mut identity = None;
    wait_until(
        || {
            identity = workers(daemon, root, project).into_iter().next();
            identity.is_some()
        },
        "exact requested physical index worker",
    );
    let (pid, start) = identity.unwrap();
    let log = fs::read_link(format!("/proc/{pid}/fd/2")).unwrap();
    assert!(log.starts_with(runtime.directory.path().join("cache/logs")) && log.is_file());
    let worker = StoppedWorker { pid, start, log };
    worker.signal("-STOP");
    wait_until(
        || {
            fs::read_to_string(format!("/proc/{pid}/stat"))
                .ok()
                .is_some_and(|stat| {
                    stat.rsplit_once(") ")
                        .is_some_and(|(_, tail)| tail.starts_with('T'))
                })
        },
        "physical worker stopped",
    );
    worker
}

fn workers(daemon: u32, root: &std::path::Path, project: &str) -> Vec<(u32, u64)> {
    let mut children = BTreeSet::new();
    // The daemon launches from application threads, whose children are listed
    // separately from the process leader's /proc children file.
    for task in fs::read_dir(format!("/proc/{daemon}/task"))
        .into_iter()
        .flatten()
        .flatten()
    {
        if let Ok(pids) = fs::read_to_string(task.path().join("children")) {
            children.extend(
                pids.split_whitespace()
                    .filter_map(|pid| pid.parse::<u32>().ok()),
            );
        }
    }
    children
        .into_iter()
        .filter_map(|pid| {
            let bytes = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
            let args = bytes.split(|byte| *byte == 0).collect::<Vec<_>>();
            if args.get(2).copied() != Some(b"--index-worker".as_slice()) {
                return None;
            }
            // Observe the pinned native worker identity; never launch its private ABI.
            let arguments: Value = serde_json::from_slice(args.get(6)?).ok()?;
            if arguments["name"] != project || arguments["repo_path"].as_str()? != root.to_str()? {
                return None;
            }
            Some((pid, start_tick(pid)?))
        })
        .collect()
}

fn cancelled(daemon_log: &std::path::Path, worker_log: &std::path::Path) -> bool {
    let Ok(metadata) = fs::metadata(daemon_log) else {
        return false;
    };
    assert!(
        metadata.len() < 4 * 1024 * 1024,
        "bounded private diagnostic log"
    );
    let Ok(contents) = fs::read_to_string(daemon_log) else {
        return false;
    };
    contents.lines().any(|line| {
        line.contains("index.supervisor.worker_cancelled")
            && line.contains(worker_log.to_str().unwrap())
    })
}
