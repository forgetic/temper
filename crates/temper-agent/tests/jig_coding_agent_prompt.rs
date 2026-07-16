use std::fs;
use std::io::{BufRead as _, BufReader};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use jig_core::{Reply, Script, StopReason, Turn};
use jig_server::FakeLlm;
use temper_agent::activity::AgentActivityConfig;
use temper_agent::{
    ProviderConfig, WorkspaceContext, run_coding_agent_native_with_totals_tool_config_and_hosts,
};
use temper_protocol_activity::{
    AgentActivityCapturePolicyV1, AgentActivityChildRecordV1, AgentActivityEventV1,
    AgentActivityFrameV1, CaptureModeV1, CapturedContentV1, PromptSnapshotV1,
};

#[allow(dead_code)]
#[path = "support/coding_agent_workspace.rs"]
mod coding_agent_workspace;
use coding_agent_workspace::{REPO_DIR, TempCheckout};

#[test]
fn coding_agent_prompt_snapshot_equals_anthropic_provider_startup_context() {
    let checkout = TempCheckout::new("jig-coding-agent-prompt-snapshot");
    checkout.init_git();
    fs::write(
        checkout.path().join("AGENTS.md"),
        "REPOSITORY-AGENTS-GUIDANCE-SENTINEL-364\n",
    )
    .expect("write AGENTS.md");
    let config_dir = tempfile::tempdir().expect("prompt overlay config");
    fs::create_dir_all(config_dir.path().join("prompts")).expect("prompt overlay directory");
    fs::write(
        config_dir.path().join("prompts/engineer.md"),
        "OPERATOR-PROMPT-OVERLAY-SENTINEL-364\n",
    )
    .expect("write prompt overlay");

    let fake = coding_agent_fake(Arc::new(AtomicUsize::new(0)));
    let provider = ProviderConfig::anthropic_oauth(Some(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/jig_auth.json"),
    ))
    .with_base_url_override(fake.base_url());
    let mut context: WorkspaceContext = serde_json::from_str(include_str!(
        "../../temper-protocol-agent/fixtures/workspace-context-artifact-context.json"
    ))
    .expect("artifact context fixture");
    context.repos[0].dir = REPO_DIR.to_string();
    context.repos[0].access = "writable".to_string();
    context.checkout = Some("writable".to_string());
    context.allowed_verdicts = vec!["needs_architect".to_string()];
    context.guidance.role_guidance = Some("Exercise exact prompt capture.".to_string());
    context.verdict_contracts = serde_json::from_value(serde_json::json!({
        "needs_architect": {
            "max_children": 0,
            "requires_body": true
        }
    }))
    .expect("verdict contracts");

    let activity = ActivityCapture::start();
    let activity_config = AgentActivityConfig {
        policy: AgentActivityCapturePolicyV1 {
            capture: CaptureModeV1::Transcript,
            ..Default::default()
        },
        address: Some(activity.address.clone()),
        ..Default::default()
    };
    let cwd = checkout.path().to_path_buf();
    let config_path = config_dir.path().to_path_buf();
    let result = temper_agent_io::block_on_with(move |_cx, handle| async move {
        run_coding_agent_native_with_totals_tool_config_and_hosts(
            handle,
            &provider,
            &context,
            &cwd,
            6,
            Some(&config_path),
            true,
            None,
            None,
            None,
            activity_config,
            temper_protocol_agent::AgentRuntimeLimitsV1::default(),
        )
        .await
    })
    .expect("Anthropic jig-backed coding agent succeeds")
    .0;
    assert_eq!(result.verdict, None);

    let records = activity.finish();
    let prompt_records = records
        .iter()
        .filter(|record| matches!(record.frame.event, AgentActivityEventV1::PromptPrepared(_)))
        .collect::<Vec<_>>();
    assert_eq!(prompt_records.len(), 1, "one main-scope prompt snapshot");
    let prompt_record = prompt_records[0];
    let AgentActivityEventV1::PromptPrepared(prompt) = &prompt_record.frame.event else {
        unreachable!();
    };
    let prompt_bytes = match prompt.content.as_ref().expect("captured prompt") {
        CapturedContentV1::Inline(inline) => inline.text.as_bytes().to_vec(),
        CapturedContentV1::Blob { blob } => prompt_record
            .blobs
            .iter()
            .find(|attachment| attachment.blob.digest == blob.digest)
            .expect("prompt blob attachment")
            .decode()
            .expect("decode prompt blob"),
    };
    let snapshot: PromptSnapshotV1 =
        serde_json::from_slice(&prompt_bytes).expect("decode exact prompt snapshot");
    assert_eq!(
        snapshot
            .to_canonical_json_bytes()
            .expect("canonical prompt snapshot"),
        prompt_bytes
    );

    let provider_request = fake
        .requests()
        .into_iter()
        .next()
        .expect("first provider request");
    let view = provider_request.view.expect("normalized provider request");
    assert_eq!(view.messages[0].role, "system");
    assert_eq!(
        snapshot.system_prompt.as_deref(),
        Some(view.messages[0].content.as_str())
    );
    assert_eq!(view.messages[1].role, "user");
    assert_eq!(snapshot.initial_user_message, view.messages[1].content);

    let request_json: serde_json::Value =
        serde_json::from_slice(&provider_request.body).expect("provider request JSON");
    let mut provider_tools = request_json["tools"].clone();
    for tool in provider_tools.as_array_mut().expect("provider tools array") {
        let schema = tool
            .get_mut("input_schema")
            .and_then(serde_json::Value::as_object_mut)
            .expect("provider tool schema");
        if schema.get("required").is_some_and(|required| {
            required
                .as_array()
                .is_some_and(|required| required.is_empty())
        }) {
            // Anthropic materializes an empty list that the captured ToolDef omits.
            schema.remove("required");
        }
    }
    assert_eq!(
        serde_json::to_value(&snapshot.tools).expect("snapshot tools JSON"),
        provider_tools,
        "captured prompt tools equal the provider ToolDef slice"
    );
    let tool_names = snapshot
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let effective_prompt = &snapshot.initial_user_message;
    for optional_name in [
        "submit_for_pr",
        "forge_get_item",
        "forge_list_related",
        "investigate",
        "delegate",
    ] {
        assert_eq!(
            effective_prompt.contains(optional_name),
            tool_names.contains(optional_name),
            "guidance availability disagreed with the tool manifest for {optional_name}"
        );
    }
    for required in [
        "ROLE: engineer",
        "SUB-AGENT",
        "OPERATOR-PROMPT-OVERLAY-SENTINEL-364",
        "REPOSITORY-AGENTS-GUIDANCE-SENTINEL-364",
        "needs_architect",
        "Role guidance:",
        "Artifact context bundle",
        "Workflow context:",
        "kind: code",
        "parents: repo-1#277",
        "dependencies: repo-2#88",
        "target branch: main",
        "correlation key: context-for-code-279",
        "repo-1#280 — Render artifact context [open]",
    ] {
        assert!(
            snapshot.initial_user_message.contains(required),
            "Anthropic-folded user prompt omitted {required}"
        );
    }
    assert!(
        snapshot
            .system_prompt
            .as_deref()
            .is_some_and(|system| !system.contains("ROLE: engineer")),
        "Anthropic OAuth keeps the provider identity as the system block"
    );
    for forbidden in ["body_hex", "create_issue_intents", "workflow_kind"] {
        assert!(
            !snapshot.initial_user_message.contains(forbidden),
            "prompt leaked machine bookkeeping field {forbidden}"
        );
    }
}

struct ActivityCapture {
    address: String,
    thread: std::thread::JoinHandle<Vec<AgentActivityChildRecordV1>>,
}

impl ActivityCapture {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind activity capture");
        let address = listener.local_addr().expect("activity address").to_string();
        let thread = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept activity client");
            let mut records = Vec::new();
            for line in BufReader::new(stream).lines() {
                let line = line.expect("activity line");
                let record = serde_json::from_str::<AgentActivityChildRecordV1>(&line)
                    .or_else(|_| {
                        serde_json::from_str::<AgentActivityFrameV1>(&line).map(|frame| {
                            AgentActivityChildRecordV1 {
                                frame,
                                blobs: Vec::new(),
                            }
                        })
                    })
                    .expect("typed child activity record");
                let terminal = matches!(record.frame.event, AgentActivityEventV1::ScopeFinished(_));
                records.push(record);
                if terminal {
                    break;
                }
            }
            records
        });
        Self { address, thread }
    }

    fn finish(self) -> Vec<AgentActivityChildRecordV1> {
        self.thread.join().expect("join activity capture")
    }
}

fn coding_agent_fake(observed_continuation: Arc<AtomicUsize>) -> FakeLlm {
    FakeLlm::start(Script::rule(move |view| {
        if view.prior_tool_results == 0 {
            Reply {
                turns: vec![Turn::ToolCall {
                    id: "call_write_notes".to_string(),
                    name: "write".to_string(),
                    args: serde_json::json!({
                        "path": "demo/NOTES.md",
                        "content": "project notes\n"
                    }),
                }],
                usage: Default::default(),
                stop: StopReason::ToolCalls,
            }
        } else {
            observed_continuation.fetch_add(1, Ordering::SeqCst);
            Reply::text(r#"{"summary":"Created NOTES.md with project notes."}"#)
        }
    }))
    .expect("start fake LLM")
}
