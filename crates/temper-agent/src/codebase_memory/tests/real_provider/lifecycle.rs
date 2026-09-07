//! Explicit installed-provider containment experiments; never run in the quick lane.

use super::*;
use std::path::PathBuf;
use std::process::Stdio;
use temper_process_containment::{
    CleanupTrigger, ContainedProcess, ContainmentCommand, ContainmentScope,
};

const ACTOR_TEST: &str = "codebase_memory::tests::real_provider::lifecycle::provider_job_actor";
const ACTOR_CONFIG: &str = "TEMPER_TEST_CBM_ACTOR_CONFIG";

mod work;

/// The baseline deliberately uses the ordinary nested MCP ownership, before a
/// shared provider owner is introduced. It demonstrates why overlap alone is
/// insufficient; it is not evidence of corrected lifecycle behavior.
#[test]
#[ignore = "installed provider cold containment baseline; isolated account only"]
fn installed_provider_cold_nested_owner_baseline() {
    let runtime = ProviderRuntime::new();
    let repo = runtime.directory.path().join("repo");
    fs::create_dir(&repo).unwrap();
    fs::write(
        repo.join("sentinel.py"),
        "def sentinel():\n    return 1280\n",
    )
    .unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .arg(&repo)
            .status()
            .unwrap()
            .success()
    );
    assert!(daemon_pid(&runtime).is_none(), "baseline must start cold");
    let a = actor(&runtime, "a");
    let a_mcp_owner = ready(&runtime, "a");
    let daemon = daemon_pid(&runtime).expect("A started a daemon");
    let daemon_start = start_tick(daemon).expect("daemon start identity");
    let b = actor(&runtime, "b");
    let b_mcp_owner = ready(&runtime, "b");
    assert_eq!(
        daemon_pid(&runtime),
        Some(daemon),
        "B joined A's generation"
    );
    query(&runtime, "b", "before");
    assert!(query_result(&runtime, "b", "before"));
    let report = a.cleanup(CleanupTrigger::Cancellation);
    assert!(matches!(
        report.recursive_empty(),
        temper_process_containment::RecursiveEmptyProof::Proven { .. }
    ));
    wait_until(|| start_tick(a_mcp_owner).is_none(), "A frontend cleanup");
    query(&runtime, "b", "after");
    let survived = query_result(&runtime, "b", "after");
    let same_daemon = start_tick(daemon) == Some(daemon_start);
    println!(
        "cbm_lifecycle_baseline cold=true shared_generation=true outer_job_cleanup=proven a_mcp_owner={} b_mcp_owner={} daemon={} daemon_start={} daemon_survived={} b_query_survived={}",
        a_mcp_owner, b_mcp_owner, daemon, daemon_start, same_daemon, survived
    );
    b.cleanup(CleanupTrigger::Shutdown);
    wait_until(|| start_tick(b_mcp_owner).is_none(), "B frontend cleanup");
    // Observe before ProviderRuntime's emergency daemon-stop fallback.
    wait_until(
        || daemon_pid(&runtime).is_none(),
        "daemon absence after original-owner cleanup",
    );
    assert!(
        !same_daemon && !survived,
        "baseline must expose original-owner containment"
    );
}

fn actor(runtime: &ProviderRuntime, name: &str) -> ContainedProcess {
    let config_path = runtime.directory.path().join(format!("{name}.json"));
    fs::write(
        &config_path,
        json!({"args":runtime.args,"root":runtime.directory.path(),"name":name}).to_string(),
    )
    .expect("actor config");
    let context = crate::containment_tests::containment_context();
    let mut command = ContainmentCommand::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", ACTOR_TEST, "--ignored", "--nocapture"])
        .env(ACTOR_CONFIG, config_path)
        .current_dir(runtime.directory.path().join("repo"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    context
        .factory()
        .prepare(context.containment_spec(name, ContainmentScope::Job))
        .expect("prepare actual job owner")
        .spawn(command)
        .expect("spawn contained job")
}

#[test]
#[ignore = "internal installed-provider actor"]
fn provider_job_actor() {
    let Ok(config_path) = std::env::var(ACTOR_CONFIG) else {
        return;
    };
    let config: Value = serde_json::from_slice(&fs::read(config_path).unwrap()).unwrap();
    let args: Vec<String> = serde_json::from_value(config["args"].clone()).unwrap();
    let root = PathBuf::from(config["root"].as_str().unwrap());
    let name = config["name"].as_str().unwrap().to_string();
    let mcp = StdioMcpServerConfig::new("env", args)
        .with_startup_timeout(Duration::from_secs(30))
        .with_call_timeout(Duration::from_secs(5));
    temper_agent_io::block_on(async move {
        let client = StdioMcpClient::connect(mcp)
            .await
            .expect("actor MCP admission");
        if name == "a" {
            let indexed = client
                .call_tool(
                    "index_repository",
                    json!({"repo_path": root.join("repo"), "name": "lifecycle-fixture"}),
                    TIMEOUT,
                )
                .await
                .expect("index baseline sentinel");
            assert!(!indexed.is_error, "baseline index must complete");
        }
        fs::write(
            root.join(format!("{name}.ready")),
            client.child_id().to_string(),
        )
        .unwrap();
        let started = Instant::now();
        let mut previous = String::new();
        let mut previous_index = String::new();
        while started.elapsed() < Duration::from_secs(120) {
            if root.join(format!("{name}.stop")).exists() {
                break;
            }
            let next_index =
                fs::read_to_string(root.join(format!("{name}.index"))).unwrap_or_default();
            if !next_index.is_empty() && next_index != previous_index {
                let request: Value = serde_json::from_str(&next_index).unwrap();
                let sequence = request["sequence"].as_str().unwrap();
                fs::write(root.join(format!("{name}.{sequence}.index_sent")), "sent").unwrap();
                let response = client
                    .call_tool("index_repository", request["arguments"].clone(), TIMEOUT)
                    .await;
                let succeeded = response.is_ok_and(|response| !response.is_error);
                fs::write(
                    root.join(format!("{name}.{sequence}.index_result")),
                    succeeded.to_string(),
                )
                .unwrap();
                previous_index = next_index;
            }
            let next = fs::read_to_string(root.join(format!("{name}.query"))).unwrap_or_default();
            if !next.is_empty() && next != previous {
                let expected_project = fs::read_to_string(root.join(format!("{name}.project")))
                    .unwrap_or_else(|_| "lifecycle-fixture".into());
                let expected_name = if expected_project == "lifecycle-fixture" {
                    "sentinel"
                } else {
                    "work_sentinel"
                };
                let expected_file = if expected_project == "lifecycle-fixture" {
                    "sentinel.py"
                } else {
                    "work.py"
                };
                let qualified = format!(
                    "{}.{}.{}",
                    expected_project,
                    expected_file.trim_end_matches(".py"),
                    expected_name
                );
                let result = client.call_tool("search_graph", json!({"project":expected_project, "name_pattern":format!("^{expected_name}$"), "format":"json"}), Duration::from_secs(5))
                    .await.is_ok_and(|mut result| {
                        super::super::super::provider_output::normalize(&mut result);
                        let Ok(value) = serde_json::from_str::<Value>(&result.text) else { return false; };
                        let rows = value["results"].as_array();
                        !result.is_error && rows.is_some_and(|rows| rows.len() == 1
                            && rows[0]["name"] == expected_name
                            && rows[0]["file_path"] == expected_file
                            && rows[0]["qualified_name"].as_str().is_some_and(|name| name == qualified))
                    });
                fs::write(
                    root.join(format!("{name}.{next}.result")),
                    result.to_string(),
                )
                .unwrap();
                previous = next;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    });
}

fn ready(runtime: &ProviderRuntime, name: &str) -> u32 {
    let path = runtime.directory.path().join(format!("{name}.ready"));
    wait_until(|| path.is_file(), "actor ready");
    fs::read_to_string(path).unwrap().parse().unwrap()
}

fn query(runtime: &ProviderRuntime, name: &str, sequence: &str) {
    fs::write(
        runtime.directory.path().join(format!("{name}.query")),
        sequence,
    )
    .unwrap();
}

fn query_result(runtime: &ProviderRuntime, name: &str, sequence: &str) -> bool {
    let path = runtime
        .directory
        .path()
        .join(format!("{name}.{sequence}.result"));
    wait_until(|| path.is_file(), "query result");
    fs::read_to_string(path).unwrap() == "true"
}

pub(super) fn daemon_pid(runtime: &ProviderRuntime) -> Option<u32> {
    let output = super::control::native_control(runtime, "status").expect("bounded native status");
    assert!(
        output.contains("daemon: active") || output.contains("daemon: not running"),
        "unrecognized native status"
    );
    output.lines().find_map(|line| {
        line.trim()
            .strip_prefix("pid: ")
            .and_then(|pid| pid.parse().ok())
    })
}

pub(super) fn start_tick(pid: u32) -> Option<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(") ")?
        .1
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

fn wait_until(mut ready: impl FnMut() -> bool, label: &str) {
    let started = Instant::now();
    while !ready() {
        assert!(
            started.elapsed() < Duration::from_secs(40),
            "timed out: {label}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// First corrected topology smoke. The stronger in-flight index acceptance is
/// separate; this observes real daemon-backed B reads only after A is empty.
#[test]
#[ignore = "installed shared provider ownership; isolated account only"]
fn installed_provider_shared_owner_survives_actual_job_cancellation() {
    use temper_codebase_memory_runtime::{LaunchConfig, ProviderBootstrap, ProviderOwnerManager};
    let runtime = ProviderRuntime::new();
    let repo = runtime.directory.path().join("repo");
    fs::create_dir(&repo).unwrap();
    fs::write(
        repo.join("sentinel.py"),
        "def sentinel():\n    return 1280\n",
    )
    .unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .arg(&repo)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        daemon_pid(&runtime).is_none(),
        "A admission must start cold"
    );
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
    .expect("cold A admission creates shared owner");
    let daemon = daemon_pid(&runtime).expect("A admission started native daemon");
    let daemon_start = start_tick(daemon).unwrap();
    let completion = bootstrap.completion();
    let a = actor(&runtime, "a");
    let a_owner = ready(&runtime, "a");
    let b = actor(&runtime, "b");
    let b_owner = ready(&runtime, "b");
    bootstrap.serving_admitted();
    query(&runtime, "b", "before");
    assert!(
        query_result(&runtime, "b", "before"),
        "structured exact sentinel before cancellation"
    );
    let report = a.cleanup(CleanupTrigger::Cancellation);
    assert!(matches!(
        report.recursive_empty(),
        temper_process_containment::RecursiveEmptyProof::Proven { .. }
    ));
    assert!(start_tick(a_owner).is_none());
    query(&runtime, "b", "after");
    assert!(
        query_result(&runtime, "b", "after"),
        "B daemon-backed query after completed A cleanup"
    );
    assert_eq!(
        start_tick(daemon),
        Some(daemon_start),
        "same generation survives A cleanup"
    );
    assert!(
        completion.wait(Duration::from_millis(50)).is_none(),
        "shared owner remains while B serves"
    );
    fs::write(runtime.directory.path().join("b.stop"), "stop").unwrap();
    wait_until(|| start_tick(b_owner).is_none(), "final B client drain");
    let shared = completion
        .wait(Duration::from_secs(40))
        .expect("upstream final session naturally ends shared owner");
    assert!(matches!(
        shared.recursive_empty(),
        temper_process_containment::RecursiveEmptyProof::Proven { .. }
    ));
    assert_eq!(
        shared.disposition(),
        temper_process_containment::CleanupDisposition::AlreadyEmpty
    );
    assert!(
        start_tick(daemon).is_none(),
        "native exit observed before fallback Drop"
    );
    b.cleanup(CleanupTrigger::NormalRootExit);
    manager.reap_completed();
    println!(
        "cbm_shared_lifecycle cold_a_admission=true same_generation=true a_outer_job_cleanup=proven b_exact_graph_query=true daemon={daemon} daemon_start={daemon_start} final_native_exit=true shared_cleanup=already_empty"
    );
}
