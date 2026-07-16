use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use temper_protocol_activity::{AgentActivityCapturePolicyV1, CaptureModeV1};
use temper_protocol_agent::{AgentLifecycleEventV1, AgentRuntimeLimitsV1};

use super::{OutOfProcessRunner, tests::test_context};
use crate::agent_runner::{AgentRunRequest, AgentRunner, JobProgressReporter};
use crate::config::WorkerAgentTraceConfig;

#[test]
#[cfg(unix)]
fn first_party_lifecycle_is_attempt_bound_and_independent_of_trace_storage() {
    let temp = tempfile::tempdir().expect("tempdir");
    let script = lifecycle_agent_script(temp.path());
    let invalid_spool = temp.path().join("not-a-directory");
    std::fs::write(&invalid_spool, b"trace storage unavailable").unwrap();
    let observed = Arc::new(Mutex::new(Vec::new()));
    let observed_for_reporter = Arc::clone(&observed);
    let progress = JobProgressReporter::new("attempt-394", move |progress| {
        observed_for_reporter.lock().unwrap().push(progress);
    });
    let runner = OutOfProcessRunner::new(vec![script.display().to_string()])
        .with_runtime_limits(Some(AgentRuntimeLimitsV1::default()))
        .with_trace_policy(Some(AgentActivityCapturePolicyV1 {
            capture: CaptureModeV1::Off,
            ..Default::default()
        }))
        .with_trace_collector(WorkerAgentTraceConfig {
            policy: AgentActivityCapturePolicyV1::default(),
            spool_root: Some(invalid_spool),
        });
    let context = test_context();
    let cwd = temp.path().to_path_buf();
    temper_worker_io::block_on(async move {
        runner
            .run_request(AgentRunRequest::new(
                "job-394",
                "attempt-394",
                &context,
                &cwd,
                progress,
            ))
            .await
    })
    .expect("lifecycle remains available when trace capture/storage are unavailable");

    let observed = observed.lock().unwrap();
    assert_eq!(observed.len(), 2);
    assert!(
        observed
            .iter()
            .all(|progress| progress.attempt_id == "attempt-394")
    );
    assert_eq!(observed[0].frame.seq, 1);
    assert!(matches!(
        observed[0].frame.event,
        AgentLifecycleEventV1::ModelStarted { .. }
    ));
    assert!(matches!(
        observed[1].frame.event,
        AgentLifecycleEventV1::AgentFinished { .. }
    ));
}

#[cfg(unix)]
fn lifecycle_agent_script(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = dir.join("lifecycle-agent.sh");
    std::fs::write(
        &path,
        r#"#!/bin/sh
set -eu
lifecycle=""
result=""
while [ "$#" -gt 0 ]; do
  arg="$1"; shift
  case "$arg" in
    --agent-lifecycle-address) lifecycle="$1"; shift ;;
    --result) result="$1"; shift ;;
    --context|--workspace|--runtime-limits|--trace-policy|--tool-config|--submit-for-pr-address|--forge-context-address|--activity-address|--provider|--model|--investigate-model|--provider-url|--max-iterations|--subagents|--capture-dir) shift ;;
  esac
done
python3 - "$lifecycle" <<'PY'
import json, socket, sys
host, port = sys.argv[1].rsplit(':', 1)
stream = socket.create_connection((host, int(port)), timeout=5)
records = [
  {"version":1},
  {"version":1,"seq":1,"scope":{"id":"main"},"event":{"type":"model_started","call_id":"call-1","attempt":0}},
  {"version":1,"seq":2,"scope":{"id":"main"},"event":{"type":"agent_finished","status":"succeeded"}},
]
for record in records:
    stream.sendall(json.dumps(record, separators=(',', ':')).encode() + b'\n')
stream.shutdown(socket.SHUT_WR)
stream.close()
PY
printf '{"summary":"ok"}' > "$result"
"#,
    )
    .expect("write lifecycle fake agent");
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&path, permissions).unwrap();
    path
}
