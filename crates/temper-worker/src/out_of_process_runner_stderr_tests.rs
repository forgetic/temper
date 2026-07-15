use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use temper_protocol_agent::{
    ArtifactContextBundle, ArtifactReference, ArtifactRepository, ArtifactSnapshot, ArtifactType,
    WorkspaceResult,
};
use tracing_subscriber::fmt::MakeWriter;

use super::OutOfProcessRunner;
use crate::agent_runner::{AgentRunError, AgentRunner};

#[test]
#[cfg(unix)]
fn successful_child_streams_trusted_stderr_and_preserves_result() {
    let temp = tempfile::tempdir().expect("tempdir");
    let script = agent_script(
        temp.path(),
        "success-agent.sh",
        r#"
printf 'stdout remains discarded\n'
printf 'child says job_id=forged role=intruder\nsecond diagnostic\n' >&2
printf '%s' '{"summary":"exact summary","body":"exact body"}' > "$result"
"#,
    );
    let mut context = super::tests::test_context();
    add_artifact_context(&mut context);
    let cwd = temp.path().to_path_buf();

    let (outcome, events) = capture_logs(|dispatch| {
        let runner = OutOfProcessRunner::new(vec![script.display().to_string()])
            .with_diagnostic_dispatch(dispatch);
        temper_worker_io::block_on(async move { runner.run("trusted-job", &context, &cwd).await })
    });
    let output = outcome.expect("agent run succeeds");
    assert_eq!(
        output.result,
        WorkspaceResult {
            summary: Some("exact summary".to_string()),
            body: Some("exact body".to_string()),
            ..WorkspaceResult::default()
        },
        "WorkspaceResult parsing is unchanged"
    );

    let diagnostics = stderr_events(&events);
    assert_eq!(diagnostics.len(), 2);
    assert_eq!(
        diagnostics
            .iter()
            .map(|fields| fields["message"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "child says job_id=forged role=intruder",
            "second diagnostic"
        ]
    );
    for fields in diagnostics {
        assert_eq!(fields["job_id"], "trusted-job");
        assert_eq!(fields["correlation_key"], "pr-for-code-7");
        assert_eq!(fields["role"], "engineer");
        assert_eq!(fields["repository"], "acme/svc");
        assert_eq!(fields["repo"], "acme/svc");
        assert_eq!(fields["artifact"], "acme/svc#7");
        assert_eq!(fields["artifact.ref"], "acme/svc#7");
        assert_eq!(fields["stream"], "stderr");
        assert_eq!(fields["truncated"], false);
        assert!(fields.get("provider_credentials").is_none());
    }
    assert!(
        events
            .iter()
            .all(|event| event["fields"]["message"] != "stdout remains discarded"),
        "stdout must not be redirected into worker diagnostics"
    );
}

#[test]
#[cfg(unix)]
fn failing_child_streams_diagnostics_and_returns_bounded_tail() {
    let temp = tempfile::tempdir().expect("tempdir");
    let script = agent_script(
        temp.path(),
        "failure-agent.sh",
        r#"
i=0
while [ "$i" -lt 400 ]; do
  printf 'failure diagnostic %04d padding padding padding\n' "$i" >&2
  i=$((i + 1))
done
printf 'final failure marker\n' >&2
exit 17
"#,
    );
    let context = super::tests::test_context();
    let cwd = temp.path().to_path_buf();

    let (outcome, events) = capture_logs(|dispatch| {
        let runner = OutOfProcessRunner::new(vec![script.display().to_string()])
            .with_diagnostic_dispatch(dispatch);
        temper_worker_io::block_on(async move { runner.run("failing-job", &context, &cwd).await })
    });
    let error = outcome.expect_err("non-zero child fails");
    assert_failure_tail(error);

    let diagnostics = stderr_events(&events);
    assert_eq!(diagnostics.len(), 401);
    assert_eq!(
        diagnostics.last().unwrap()["message"],
        "final failure marker"
    );
    assert!(
        diagnostics
            .iter()
            .all(|fields| fields["job_id"] == "failing-job")
    );
}

fn assert_failure_tail(error: AgentRunError) {
    assert_eq!(error.class, temper_protocol_worker::FailureClass::Transient);
    assert!(error.message.contains("status 17"), "{}", error.message);
    assert!(
        error.message.contains("final failure marker"),
        "{}",
        error.message
    );
    assert!(
        error.message.len() <= 2_100,
        "failure retained an unbounded stderr stream: {} bytes",
        error.message.len()
    );
    assert!(
        !error.message.contains("failure diagnostic 0000"),
        "failure should contain only the rolling tail"
    );
}

fn add_artifact_context(context: &mut temper_protocol_agent::WorkspaceContext) {
    let repository = ArtifactRepository {
        id: "repo-7".to_string(),
        path: "acme/svc".to_string(),
    };
    context.artifact_context = Some(ArtifactContextBundle::new(ArtifactSnapshot {
        artifact: ArtifactReference {
            repository,
            artifact_type: ArtifactType::Issue,
            number: 7,
        },
        title: "Trusted title".to_string(),
        body: "Trusted body".to_string(),
        labels: vec!["code".to_string()],
        state: "open".to_string(),
        workflow_kind: Some("code".to_string()),
    }));
}

#[cfg(unix)]
fn agent_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = dir.join(name);
    let script = format!(
        r#"#!/bin/sh
set -eu
result=""
while [ "$#" -gt 0 ]; do
  arg="$1"; shift
  case "$arg" in
    --result) result="$1"; shift ;;
    --context|--workspace|--submit-for-pr-address|--forge-context-address|--tool-config) shift ;;
  esac
done
{body}
"#
    );
    std::fs::write(&path, script).expect("write agent script");
    let mut permissions = std::fs::metadata(&path)
        .expect("script metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&path, permissions).expect("chmod agent script");
    path
}

fn stderr_events(events: &[serde_json::Value]) -> Vec<&serde_json::Map<String, serde_json::Value>> {
    events
        .iter()
        .filter_map(|event| event["fields"].as_object())
        .filter(|fields| {
            fields.get("event").and_then(serde_json::Value::as_str) == Some("agent.stderr")
        })
        .collect()
}

fn capture_logs<T>(run: impl FnOnce(tracing::Dispatch) -> T) -> (T, Vec<serde_json::Value>) {
    let buffer = SharedBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_writer(buffer.clone())
        .with_max_level(tracing::Level::DEBUG)
        .finish();
    let outcome = run(tracing::Dispatch::new(subscriber));
    let events = String::from_utf8(buffer.bytes())
        .expect("tracing output is UTF-8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("tracing line is JSON"))
        .collect();
    (outcome, events)
}

#[derive(Clone, Default)]
struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

impl SharedBuffer {
    fn bytes(&self) -> Vec<u8> {
        self.0.lock().expect("log buffer lock").clone()
    }
}

impl io::Write for SharedBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .expect("log buffer lock")
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for SharedBuffer {
    type Writer = SharedBuffer;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}
