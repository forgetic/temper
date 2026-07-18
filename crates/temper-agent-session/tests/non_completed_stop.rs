#[cfg(target_os = "linux")]
use std::collections::{HashMap, HashSet};
use std::io::{self, BufRead as _, BufReader, Read, Write as _};
use std::net::TcpListener;
#[cfg(target_os = "linux")]
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
#[cfg(target_os = "linux")]
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use jig_core::{Reply, Script, StopReason, Turn};
use jig_server::FakeLlm;
use temper_agent_core::{
    AgentContainmentContext, CleanupTrigger, ContainmentCommand, ContainmentScope,
};
use temper_process_containment::{BoundedCapture, CaptureMode, CapturedBytes};
use temper_protocol_agent::{
    AgentLifecycleCancellationAckV1, AgentLifecycleCommandV1, AgentLifecycleFrameV1,
    AgentLifecycleHelloV1, PROVIDER_CREDENTIALS_ENV, WorkspaceContext, WorkspaceGuidance,
    WorkspaceRepository, WorkspaceWorkItem,
};

mod support;

use support::bounded_fixed_provider::{
    BoundedFixedProvider, MAX_PROVIDER_REQUEST_BYTES, PROVIDER_HISTORY_BYTES,
    PROVIDER_REQUEST_TAIL_BYTES, RETAINED_PROVIDER_REQUESTS,
};

const ABORT_CANCELLATION_DEADLINE: Duration = Duration::from_secs(3);
const ABORT_PROCESS_DEADLINE: Duration = Duration::from_secs(5);
const ABORT_MAX_ELAPSED: Duration = Duration::from_secs(8);
const CAPTURED_PROCESS_OUTPUT_BYTES: usize = 64 * 1024;
const MAX_OBSERVED_FIXTURE_PROCESS_COUNT: usize = 8;
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(5);

#[test]
fn budget_exhaustion_exits_nonzero_without_result_and_names_stable_reason() {
    let fake = FakeLlm::start(Script::Fixed(Reply {
        turns: vec![
            Turn::Text(
                r#"{"verdict":"needs_architect","summary":"must not become a result"}"#.to_string(),
            ),
            Turn::ToolCall {
                id: "undispatchable-list".to_string(),
                name: "ls".to_string(),
                args: serde_json::json!({ "path": "." }),
            },
        ],
        usage: Default::default(),
        stop: StopReason::ToolCalls,
    }))
    .expect("start fake LLM");
    let temp = tempfile::tempdir().expect("agent-session tempdir");
    std::fs::create_dir_all(temp.path().join("demo")).expect("workspace repository directory");
    let context_path = temp.path().join("context.json");
    let result_path = temp.path().join("result.json");
    std::fs::write(
        &context_path,
        serde_json::to_vec(&workspace_context()).expect("serialize workspace context"),
    )
    .expect("write workspace context");

    let fake_url = fake.base_url();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_temper-agent"))
        .args([
            "--context",
            context_path.to_str().expect("context path is utf-8"),
            "--result",
            result_path.to_str().expect("result path is utf-8"),
            "--workspace",
            temp.path().to_str().expect("workspace path is utf-8"),
            "--provider",
            "deepseek",
            "--model",
            "jig-agent-session-budget",
            "--provider-url",
            fake_url.as_str(),
            "--max-iterations",
            "1",
            "--subagents",
            "off",
        ])
        .env(
            PROVIDER_CREDENTIALS_ENV,
            r#"{"type":"api-key","api_key":"sk-jig-test"}"#,
        )
        .output()
        .expect("run temper-agent process");

    assert_eq!(output.status.code(), Some(2));
    assert!(
        !result_path.exists(),
        "failed stops must not write a result"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("budget_exhausted"), "stderr was: {stderr}");
    assert!(
        stderr.contains("1-iteration tool budget"),
        "stderr should preserve the typed budget detail: {stderr}"
    );
    assert_eq!(
        fake.requests().len(),
        2,
        "one allowed tool round is followed by the budget-exhausting response"
    );
}

#[test]
fn worker_abort_exits_nonzero_without_result_and_names_stable_reason() {
    let started = Instant::now();
    let temp = tempfile::tempdir().expect("aborted agent-session tempdir");
    std::fs::create_dir_all(temp.path().join("demo")).expect("workspace repository directory");
    let context_path = temp.path().join("context.json");
    let result_path = temp.path().join("result.json");
    std::fs::write(
        &context_path,
        serde_json::to_vec(&workspace_context()).expect("serialize workspace context"),
    )
    .expect("write workspace context");

    #[cfg(target_os = "linux")]
    let (descendant_fixture, descendant_identity, descendant_ready) = {
        let fixture = write_nested_setsid_fixture(temp.path()).expect("write setsid fixture");
        (
            fixture,
            temp.path().join("nested-setsid.identity"),
            temp.path().join("nested-setsid.ready"),
        )
    };
    #[cfg(target_os = "linux")]
    let provider_command = "exec \"$TEMPER_ABORT_DESCENDANT_FIXTURE\"";
    #[cfg(not(target_os = "linux"))]
    let provider_command = "true";
    let provider = BoundedFixedProvider::start_looping_tool_response(provider_command)
        .expect("start bounded abort provider");
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind lifecycle endpoint");
    listener
        .set_nonblocking(true)
        .expect("set lifecycle endpoint nonblocking");
    let lifecycle_address = listener
        .local_addr()
        .expect("lifecycle endpoint address")
        .to_string();
    #[cfg(target_os = "linux")]
    let lifecycle_descendant_ready = descendant_ready.clone();
    let lifecycle = thread::spawn(move || {
        let accept_deadline = Instant::now() + ABORT_CANCELLATION_DEADLINE;
        while Instant::now() < accept_deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream
                        .set_read_timeout(Some(ABORT_CANCELLATION_DEADLINE))
                        .expect("bound lifecycle reads");
                    // Consume the client's hello before sending the command. Closing a
                    // socket with that frame unread can reset the connection and discard
                    // the cancellation under load.
                    let mut reader = BufReader::new(
                        stream
                            .try_clone()
                            .expect("clone lifecycle stream for reading"),
                    );
                    let mut line = String::new();
                    reader.read_line(&mut line).expect("read lifecycle hello");
                    serde_json::from_str::<AgentLifecycleHelloV1>(line.trim())
                        .expect("decode lifecycle hello")
                        .validate()
                        .expect("validate lifecycle hello");

                    // Do not request cancellation until the first real bash tool
                    // has created a child in a nested session. This makes the
                    // abort regression prove descendant-complete cleanup rather
                    // than merely exercising a direct agent process.
                    #[cfg(target_os = "linux")]
                    wait_for_path_until(&lifecycle_descendant_ready, accept_deadline)
                        .expect("nested setsid child did not become ready");

                    serde_json::to_writer(
                        &mut stream,
                        &AgentLifecycleCommandV1::Cancel {
                            reason: "test worker cancellation".to_string(),
                        },
                    )
                    .expect("write lifecycle cancellation");
                    stream
                        .write_all(b"\n")
                        .expect("terminate lifecycle cancellation");

                    let mut acknowledged = false;
                    loop {
                        line.clear();
                        if reader
                            .read_line(&mut line)
                            .expect("read lifecycle response")
                            == 0
                        {
                            break;
                        }
                        if let Ok(acknowledgement) =
                            serde_json::from_str::<AgentLifecycleCancellationAckV1>(line.trim())
                        {
                            acknowledgement
                                .validate()
                                .expect("validate lifecycle cancellation acknowledgement");
                            acknowledged = true;
                        } else {
                            serde_json::from_str::<AgentLifecycleFrameV1>(line.trim())
                                .expect("decode lifecycle frame")
                                .validate()
                                .expect("validate lifecycle frame");
                        }
                    }
                    assert!(acknowledged, "agent did not acknowledge cancellation");
                    return;
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("accept lifecycle stream: {error}"),
            }
        }
        panic!("agent did not connect before the named cancellation deadline");
    });

    let provider_url = provider.base_url();

    let mut command = ContainmentCommand::new(env!("CARGO_BIN_EXE_temper-agent"));
    command
        .args([
            "--context",
            context_path.to_str().expect("context path is utf-8"),
            "--result",
            result_path.to_str().expect("result path is utf-8"),
            "--workspace",
            temp.path().to_str().expect("workspace path is utf-8"),
            "--provider",
            "deepseek",
            "--model",
            "jig-agent-session-abort",
            "--provider-url",
            provider_url.as_str(),
            "--max-iterations",
            "100",
            "--subagents",
            "off",
            "--agent-lifecycle-address",
            lifecycle_address.as_str(),
        ])
        .env(
            PROVIDER_CREDENTIALS_ENV,
            r#"{"type":"api-key","api_key":"sk-jig-test"}"#,
        )
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(target_os = "linux")]
    command
        .env(
            "TEMPER_ABORT_DESCENDANT_FIXTURE",
            descendant_fixture.as_os_str(),
        )
        .env(
            "TEMPER_ABORT_DESCENDANT_IDENTITY",
            descendant_identity.as_os_str(),
        )
        .env(
            "TEMPER_ABORT_DESCENDANT_READY",
            descendant_ready.as_os_str(),
        );
    let output = run_contained_with_bounded_output(command).expect("run aborted temper-agent");
    lifecycle.join().expect("join lifecycle endpoint");

    #[cfg(target_os = "linux")]
    {
        let identity = read_start_identity(&descendant_identity)
            .expect("nested setsid child published PID/start identity");
        assert!(identity.start_time > 0, "fixture start identity was zero");
        assert!(
            !exact_process_exists(identity).expect("inspect nested setsid child identity"),
            "nested setsid child {identity:?} survived agent completion"
        );
    }

    assert_eq!(output.status.code(), Some(2));
    assert!(
        !result_path.exists(),
        "aborted stops must not write a result"
    );
    let stderr = String::from_utf8_lossy(output.stderr.as_bytes());
    assert!(stderr.contains("aborted"), "stderr was: {stderr}");
    assert!(
        stderr.contains("worker_requested"),
        "stderr should preserve trusted abort authority: {stderr}"
    );

    assert!(
        output.stdout.as_bytes().len() <= CAPTURED_PROCESS_OUTPUT_BYTES,
        "stdout retention exceeded its named bound"
    );
    assert!(
        output.stderr.as_bytes().len() <= CAPTURED_PROCESS_OUTPUT_BYTES,
        "stderr retention exceeded its named bound"
    );
    assert!(
        output.stdout.dropped_bytes() <= output.stdout.observed_bytes()
            && output.stderr.dropped_bytes() <= output.stderr.observed_bytes(),
        "capture drop accounting exceeded observed bytes"
    );
    assert!(
        output.max_process_count <= MAX_OBSERVED_FIXTURE_PROCESS_COUNT,
        "fixture process count {} exceeded {}",
        output.max_process_count,
        MAX_OBSERVED_FIXTURE_PROCESS_COUNT
    );

    let stats = provider.stats();
    assert!(
        stats.largest_request_bytes <= MAX_PROVIDER_REQUEST_BYTES as u64,
        "provider request exceeded its named bound: {stats:?}"
    );
    assert!(
        stats.retained_request_count <= RETAINED_PROVIDER_REQUESTS,
        "provider retained too many requests: {stats:?}"
    );
    assert!(
        stats.retained_history_bytes <= PROVIDER_HISTORY_BYTES,
        "provider retained too many history bytes: {stats:?}"
    );
    assert!(
        stats.retained_history_bytes
            <= stats
                .retained_request_count
                .saturating_mul(PROVIDER_REQUEST_TAIL_BYTES),
        "provider retained more than one tail per request: {stats:?}"
    );
    assert_eq!(stats.oversized_request_count, 0, "{stats:?}");
    assert!(
        started.elapsed() <= ABORT_MAX_ELAPSED,
        "abort regression exceeded named elapsed bound: {:?}",
        started.elapsed()
    );
}

#[cfg(target_os = "linux")]
fn abort_regression_containment_context() -> AgentContainmentContext {
    use temper_process_containment::{
        ContainmentBackendFactory, ContainmentBackendPolicy, ContainmentFactory,
        LinuxSupervisorBackendFactory,
    };

    let backend: Arc<dyn ContainmentBackendFactory> = Arc::new(
        LinuxSupervisorBackendFactory::with_helper_executable(env!("CARGO_BIN_EXE_temper-agent")),
    );
    AgentContainmentContext::new(
        ContainmentFactory::new(ContainmentBackendPolicy::ForceLinuxSupervisor, backend),
        None,
    )
}

#[cfg(not(target_os = "linux"))]
fn abort_regression_containment_context() -> AgentContainmentContext {
    AgentContainmentContext::production(None)
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ProcessStartIdentity {
    pid: u32,
    start_time: u64,
}

#[cfg(target_os = "linux")]
fn write_nested_setsid_fixture(directory: &Path) -> io::Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt as _;

    let fixture = directory.join("nested-setsid-fixture.sh");
    std::fs::write(
        &fixture,
        r#"#!/bin/sh
set -eu
if [ "${1:-}" = "child" ]; then
    stat=$(cat "/proc/$$/stat")
    fields=${stat##*) }
    set -- $fields
    shift 19
    printf '%s %s\n' "$$" "$1" > "$TEMPER_ABORT_DESCENDANT_IDENTITY"
    trap '' TERM
    : > "$TEMPER_ABORT_DESCENDANT_READY"
    while :; do sleep 60; done
fi
setsid /bin/sh "$0" child </dev/null >/dev/null 2>&1 &
limit=0
while [ ! -s "$TEMPER_ABORT_DESCENDANT_IDENTITY" ] && [ "$limit" -lt 300 ]; do
    sleep 0.01
    limit=$((limit + 1))
done
test -s "$TEMPER_ABORT_DESCENDANT_IDENTITY"
"#,
    )?;
    let mut permissions = std::fs::metadata(&fixture)?.permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&fixture, permissions)?;
    Ok(fixture)
}

#[cfg(target_os = "linux")]
fn wait_for_path_until(path: &Path, deadline: Instant) -> io::Result<()> {
    while Instant::now() < deadline {
        if path.exists() {
            return Ok(());
        }
        thread::sleep(PROCESS_POLL_INTERVAL);
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        format!("timed out waiting for {}", path.display()),
    ))
}

#[cfg(target_os = "linux")]
fn read_start_identity(path: &Path) -> io::Result<ProcessStartIdentity> {
    let contents = std::fs::read_to_string(path)?;
    let mut fields = contents.split_whitespace();
    let parse = |value: Option<&str>, field: &str| -> io::Result<u64> {
        value
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, format!("missing {field}")))?
            .parse()
            .map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid {field}: {error}"),
                )
            })
    };
    Ok(ProcessStartIdentity {
        pid: u32::try_from(parse(fields.next(), "pid")?)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "pid exceeds u32"))?,
        start_time: parse(fields.next(), "start time")?,
    })
}

#[cfg(target_os = "linux")]
fn exact_process_exists(identity: ProcessStartIdentity) -> io::Result<bool> {
    let stat = match std::fs::read_to_string(format!("/proc/{}/stat", identity.pid)) {
        Ok(stat) => stat,
        Err(error)
            if error.kind() == io::ErrorKind::NotFound || error.raw_os_error() == Some(3) =>
        {
            return Ok(false);
        }
        Err(error) => return Err(error),
    };
    let (_, fields) = stat
        .rsplit_once(") ")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "malformed /proc stat"))?;
    let start_time = fields
        .split_whitespace()
        .nth(19)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "short /proc stat"))?
        .parse::<u64>()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(start_time == identity.start_time)
}

struct BoundedProcessOutput {
    status: ExitStatus,
    stdout: CapturedBytes,
    stderr: CapturedBytes,
    max_process_count: usize,
}

fn run_contained_with_bounded_output(
    command: ContainmentCommand,
) -> io::Result<BoundedProcessOutput> {
    let containment = abort_regression_containment_context()
        .with_cleanup_timing(Duration::from_millis(100), Duration::from_millis(10));
    let prepared = containment.factory().prepare(
        containment.containment_spec("bounded-abort-regression", ContainmentScope::Agent),
    )?;
    let process = prepared.spawn(command)?;
    let pid = process.id();
    let stdout = process
        .take_stdout()?
        .ok_or_else(|| io::Error::other("abort fixture stdout was not piped"))?;
    let stderr = process
        .take_stderr()?
        .ok_or_else(|| io::Error::other("abort fixture stderr was not piped"))?;
    let stdout_reader = spawn_capture_reader("stdout", stdout)?;
    let stderr_reader = spawn_capture_reader("stderr", stderr)?;

    let deadline = Instant::now() + ABORT_PROCESS_DEADLINE;
    let mut max_process_count = 1;
    let status = loop {
        max_process_count = max_process_count.max(fixture_process_count(pid));
        if let Some(status) = process.try_wait_root()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = process.cleanup(CleanupTrigger::Timeout);
            let _ = join_capture_reader(stdout_reader, "stdout");
            let _ = join_capture_reader(stderr_reader, "stderr");
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "abort fixture exceeded named process deadline after contained cleanup",
            ));
        }
        thread::sleep(PROCESS_POLL_INTERVAL);
    };
    let cleanup = process.cleanup(CleanupTrigger::NormalRootExit);
    if !cleanup.proves_quiescence() {
        let _ = join_capture_reader(stdout_reader, "stdout");
        let _ = join_capture_reader(stderr_reader, "stderr");
        return Err(io::Error::other(format!(
            "abort regression completed without recursive-empty proof: {cleanup:?}"
        )));
    }
    let stdout = join_capture_reader(stdout_reader, "stdout")?;
    let stderr = join_capture_reader(stderr_reader, "stderr")?;
    Ok(BoundedProcessOutput {
        status,
        stdout,
        stderr,
        max_process_count,
    })
}

fn spawn_capture_reader(
    name: &'static str,
    mut reader: impl Read + Send + 'static,
) -> io::Result<JoinHandle<io::Result<CapturedBytes>>> {
    thread::Builder::new()
        .name(format!("abort-fixture-{name}"))
        .spawn(move || {
            let mut capture = BoundedCapture::new(CaptureMode::Tail, CAPTURED_PROCESS_OUTPUT_BYTES);
            capture.drain(&mut reader)?;
            Ok(capture.finish().expect("tail capture cannot overflow"))
        })
}

fn join_capture_reader(
    reader: JoinHandle<io::Result<CapturedBytes>>,
    name: &str,
) -> io::Result<CapturedBytes> {
    reader
        .join()
        .map_err(|_| io::Error::other(format!("abort fixture {name} reader panicked")))?
}

#[cfg(target_os = "linux")]
fn fixture_process_count(root: u32) -> usize {
    let mut parents = HashMap::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return 1;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        let Some((_, fields)) = stat.rsplit_once(") ") else {
            continue;
        };
        let Some(ppid) = fields
            .split_whitespace()
            .nth(1)
            .and_then(|value| value.parse::<u32>().ok())
        else {
            continue;
        };
        parents.insert(pid, ppid);
    }
    let mut owned = HashSet::from([root]);
    loop {
        let before = owned.len();
        for (&pid, &ppid) in &parents {
            if owned.contains(&ppid) {
                owned.insert(pid);
            }
        }
        if owned.len() == before {
            return owned.len();
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn fixture_process_count(_root: u32) -> usize {
    1
}

fn workspace_context() -> WorkspaceContext {
    WorkspaceContext {
        trace_context: None,
        artifact_context: None,
        repos: vec![WorkspaceRepository {
            id: "forgejo:acme/demo".to_string(),
            owner: "acme".to_string(),
            name: "demo".to_string(),
            default_branch: "main".to_string(),
            dir: "demo".to_string(),
            access: "writable".to_string(),
            base_branch: "main".to_string(),
            branch_hint: Some("agent/pr-for-code-440".to_string()),
        }],
        work_item: WorkspaceWorkItem {
            role: "engineer".to_string(),
            queue: "code_ready".to_string(),
            kind: "code".to_string(),
            target: "Issue { number: ItemNumber(440) }".to_string(),
            context: "{}".to_string(),
        },
        action: "open_pr".to_string(),
        correlation_key: "pr-for-code-440".to_string(),
        checkout: Some("writable".to_string()),
        allowed_verdicts: vec!["needs_architect".to_string()],
        verdict_contracts: Default::default(),
        source_metadata: Default::default(),
        guidance: WorkspaceGuidance::default(),
        pull_request_freshness: None,
        agent_session: None,
    }
}
