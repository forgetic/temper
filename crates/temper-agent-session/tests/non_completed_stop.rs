use std::io::{BufRead as _, BufReader, Write as _};
use std::net::TcpListener;
use std::process::Command;
use std::thread;
use std::time::Duration;

use jig_core::{Reply, Script, StopReason, Turn};
use jig_server::FakeLlm;
use temper_protocol_agent::{
    AgentLifecycleCancellationAckV1, AgentLifecycleCommandV1, AgentLifecycleFrameV1,
    AgentLifecycleHelloV1, PROVIDER_CREDENTIALS_ENV, WorkspaceContext, WorkspaceGuidance,
    WorkspaceRepository, WorkspaceWorkItem,
};

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
    let output = Command::new(env!("CARGO_BIN_EXE_temper-agent"))
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
    let fake = FakeLlm::start(Script::Fixed(Reply {
        turns: vec![
            Turn::Text(
                r#"{"verdict":"needs_architect","summary":"must not become a result"}"#.to_string(),
            ),
            Turn::ToolCall {
                id: "abort-loop".to_string(),
                name: "ls".to_string(),
                args: serde_json::json!({ "path": "." }),
            },
        ],
        usage: Default::default(),
        stop: StopReason::ToolCalls,
    }))
    .expect("start abort fake LLM");
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind lifecycle endpoint");
    listener
        .set_nonblocking(true)
        .expect("set lifecycle endpoint nonblocking");
    let lifecycle_address = listener
        .local_addr()
        .expect("lifecycle endpoint address")
        .to_string();
    let lifecycle = thread::spawn(move || {
        for _ in 0..500 {
            match listener.accept() {
                Ok((mut stream, _)) => {
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
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("accept lifecycle stream: {error}"),
            }
        }
        panic!("agent did not connect to lifecycle endpoint");
    });

    let temp = tempfile::tempdir().expect("aborted agent-session tempdir");
    std::fs::create_dir_all(temp.path().join("demo")).expect("workspace repository directory");
    let context_path = temp.path().join("context.json");
    let result_path = temp.path().join("result.json");
    std::fs::write(
        &context_path,
        serde_json::to_vec(&workspace_context()).expect("serialize workspace context"),
    )
    .expect("write workspace context");
    let fake_url = fake.base_url();

    let output = Command::new(env!("CARGO_BIN_EXE_temper-agent"))
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
            fake_url.as_str(),
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
        .output()
        .expect("run aborted temper-agent process");
    lifecycle.join().expect("join lifecycle endpoint");

    assert_eq!(output.status.code(), Some(2));
    assert!(
        !result_path.exists(),
        "aborted stops must not write a result"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("aborted"), "stderr was: {stderr}");
    assert!(
        stderr.contains("worker_requested"),
        "stderr should preserve trusted abort authority: {stderr}"
    );
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
