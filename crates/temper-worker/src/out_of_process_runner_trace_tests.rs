use temper_protocol_activity::{
    ACTIVITY_ADDRESS_FLAG, AgentActivityCapturePolicyV1, AgentActivityEventV1,
};

use super::{
    OutOfProcessRunner, tests::fake_agent_script, tests::test_context, tests::test_context_for_role,
};
use crate::agent_runner::AgentRunner;
use crate::config::WorkerAgentTraceConfig;
use crate::trace::TraceCollector;

#[test]
#[cfg(unix)]
fn worker_metadata_spool_brackets_children_without_activity_support() {
    let temp = tempfile::tempdir().expect("tempdir");
    let script = fake_agent_script(temp.path());
    let spool_root = temp.path().join("durable-spool");
    let trace_config = WorkerAgentTraceConfig {
        policy: AgentActivityCapturePolicyV1::default(),
        spool_root: Some(spool_root.clone()),
    };
    let runner = OutOfProcessRunner::new(vec![script.display().to_string()])
        .with_env(vec![
            (
                "TEMPER_ARGS_OUT".to_string(),
                temp.path().join("args.txt").display().to_string(),
            ),
            (
                "TEMPER_TOOL_OUT".to_string(),
                temp.path().join("tools.json").display().to_string(),
            ),
        ])
        .with_trace_collector(trace_config.clone());
    let context = test_context();
    let cwd = temp.path().to_path_buf();
    temper_worker_io::block_on(async move { runner.run("metadata-job", &context, &cwd).await })
        .expect("third-party child succeeds");

    let recovered = TraceCollector::new(trace_config)
        .recover()
        .expect("recover spool");
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].events.len(), 2);
    assert!(matches!(
        recovered[0].events[0].event,
        AgentActivityEventV1::RunStarted(_)
    ));
    assert!(matches!(
        recovered[0].events[1].event,
        AgentActivityEventV1::RunFinished(_)
    ));
    let args = std::fs::read_to_string(temp.path().join("args.txt")).unwrap();
    assert!(!args.lines().any(|arg| arg == ACTIVITY_ADDRESS_FLAG));
}

#[test]
#[cfg(unix)]
fn child_crash_leaves_host_failed_metadata_and_trace_storage_errors_are_non_fatal() {
    use std::os::unix::fs::PermissionsExt as _;

    let temp = tempfile::tempdir().expect("tempdir");
    let crash = temp.path().join("crash.sh");
    std::fs::write(&crash, "#!/bin/sh\nexit 17\n").unwrap();
    let mut permissions = std::fs::metadata(&crash).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&crash, permissions).unwrap();
    let trace_config = WorkerAgentTraceConfig {
        policy: AgentActivityCapturePolicyV1::default(),
        spool_root: Some(temp.path().join("crash-spool")),
    };
    let runner = OutOfProcessRunner::new(vec![crash.display().to_string()])
        .with_trace_collector(trace_config.clone());
    let context = test_context_for_role("tester");
    let cwd = temp.path().to_path_buf();
    let error =
        temper_worker_io::block_on(
            async move { runner.run("crashed-child", &context, &cwd).await },
        )
        .expect_err("crash is still a job failure");
    assert!(error.message.contains("status 17"));
    let recovered = TraceCollector::new(trace_config)
        .recover()
        .expect("recover crash run");
    assert_eq!(recovered[0].events.len(), 2);
    assert!(matches!(
        recovered[0].events[1].event,
        AgentActivityEventV1::RunFailed(_)
    ));

    let script = fake_agent_script(temp.path());
    let invalid_root = temp.path().join("not-a-directory");
    std::fs::write(&invalid_root, b"file blocks spool creation").unwrap();
    let runner = OutOfProcessRunner::new(vec![script.display().to_string()])
        .with_env(vec![
            (
                "TEMPER_ARGS_OUT".to_string(),
                temp.path().join("nonfatal-args.txt").display().to_string(),
            ),
            (
                "TEMPER_TOOL_OUT".to_string(),
                temp.path()
                    .join("nonfatal-tools.json")
                    .display()
                    .to_string(),
            ),
        ])
        .with_trace_collector(WorkerAgentTraceConfig {
            policy: AgentActivityCapturePolicyV1::default(),
            spool_root: Some(invalid_root),
        });
    let context = test_context_for_role("tester");
    let cwd = temp.path().to_path_buf();
    temper_worker_io::block_on(async move { runner.run("storage-error", &context, &cwd).await })
        .expect("trace storage failure does not alter child success");
}

#[test]
#[cfg(unix)]
fn first_party_child_receives_per_run_activity_address() {
    let temp = tempfile::tempdir().expect("tempdir");
    let script = fake_agent_script(temp.path());
    let trace_config = WorkerAgentTraceConfig {
        policy: AgentActivityCapturePolicyV1::default(),
        spool_root: Some(temp.path().join("spool")),
    };
    let runner = OutOfProcessRunner::new(vec![script.display().to_string()])
        .with_env(vec![
            (
                "TEMPER_ARGS_OUT".to_string(),
                temp.path().join("args.txt").display().to_string(),
            ),
            (
                "TEMPER_TOOL_OUT".to_string(),
                temp.path().join("tools.json").display().to_string(),
            ),
        ])
        .with_trace_policy(Some(AgentActivityCapturePolicyV1::default()))
        .with_trace_collector(trace_config);
    let context = test_context_for_role("tester");
    let cwd = temp.path().to_path_buf();
    temper_worker_io::block_on(async move { runner.run("first-party", &context, &cwd).await })
        .expect("first-party run succeeds");

    let args = std::fs::read_to_string(temp.path().join("args.txt")).unwrap();
    let args = args.lines().collect::<Vec<_>>();
    let flag = args
        .iter()
        .position(|arg| *arg == ACTIVITY_ADDRESS_FLAG)
        .expect("activity flag");
    assert!(args[flag + 1].starts_with("127.0.0.1:"));
}
