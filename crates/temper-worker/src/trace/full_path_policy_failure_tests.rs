// SPDX-License-Identifier: MPL-2.0

//! First-party terminal-policy coverage from the private process carrier to
//! the worker's durable activity batch.

use temper_protocol_activity::{
    AgentActivityCapturePolicyV1, AgentActivityEventV1, CaptureModeV1, FailureCodeV1, RunFailedV1,
};
use temper_protocol_agent::AgentRuntimeLimitsV1;
use temper_protocol_worker::FailureClass;

use super::full_path_fixture::workspace_context;
use super::*;
use crate::config::WorkerAgentTraceConfig;
use crate::{AgentRunner, OutOfProcessRunner};

const PRIVATE_STDERR: &str = "provider /private/path selector=secret Bearer credential";

#[test]
#[cfg(unix)]
fn deterministic_policy_terminal_becomes_non_retryable_policy_activity() {
    use std::os::unix::fs::PermissionsExt as _;

    let temporary = tempfile::tempdir().expect("policy trace temporary directory");
    let script = temporary.path().join("policy-terminal.sh");
    std::fs::write(
        &script,
        format!(
            r#"#!/bin/sh
set -eu
terminal=""
while [ "$#" -gt 0 ]; do
  arg="$1"; shift
  case "$arg" in
    --terminal-output) terminal="$1"; shift ;;
    --context|--result|--workspace|--runtime-limits|--agent-lifecycle-address) shift ;;
  esac
done
printf '%s\n' '{PRIVATE_STDERR}' >&2
cat > "$terminal" <<'JSON'
{{"protocol_version":1,"policy_failure":{{"reason":"decision_anchor_recovery_exhausted"}}}}
JSON
exit 2
"#
        ),
    )
    .expect("write policy terminal fixture");
    let mut permissions = std::fs::metadata(&script).unwrap().permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&script, permissions).unwrap();

    let policy = AgentActivityCapturePolicyV1 {
        capture: CaptureModeV1::Metadata,
        ..Default::default()
    };
    let collector_config = WorkerAgentTraceConfig {
        policy: policy.clone(),
        spool_root: Some(temporary.path().join("spool")),
    };
    let runner = OutOfProcessRunner::new(vec![script.display().to_string()])
        .with_runtime_limits(Some(AgentRuntimeLimitsV1::default()))
        .with_trace_policy(Some(policy))
        .with_trace_collector(collector_config.clone());
    let context = workspace_context();
    let cwd = temporary.path().to_path_buf();
    let error = temper_worker_io::block_on(async move {
        runner.run("job-policy-terminal", &context, &cwd).await
    })
    .expect_err("policy fixture terminates without a product");

    assert_eq!(error.class, FailureClass::Permanent);
    assert_eq!(error.failure_code, FailureCodeV1::Policy);
    assert!(!error.message.contains(PRIVATE_STDERR));

    let recovered = TraceCollector::new(collector_config)
        .recover()
        .expect("recover policy activity spool");
    assert_eq!(recovered.len(), 1);
    let batch = recovered[0]
        .pending_batch(100)
        .expect("policy run has a forwarding batch");
    let terminal = batch
        .events
        .last()
        .expect("policy run has a terminal event");
    let AgentActivityEventV1::RunFailed(RunFailedV1 { failure }) = &terminal.event else {
        panic!("policy run did not end with run.failed")
    };
    assert_eq!(failure.code, FailureCodeV1::Policy);
    assert_eq!(failure.message, "agent run failed with a permanent error");
    assert!(!failure.retryable);

    let wire = serde_json::to_string(&batch).expect("serialize policy activity batch");
    assert!(!wire.contains(PRIVATE_STDERR));
    for forbidden in ["/private/path", "selector=secret", "Bearer", "credential"] {
        assert!(!wire.contains(forbidden), "activity leaked {forbidden}");
    }
}
