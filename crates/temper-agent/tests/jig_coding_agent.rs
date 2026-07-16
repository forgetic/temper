use std::fs;
use std::io::{BufRead as _, BufReader};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use jig_core::{Reply, Script, StopReason, Turn};
use jig_server::FakeLlm;
use temper_agent::activity::AgentActivityConfig;
use temper_agent::{
    ProviderConfig, SubmitForPrHost, WorkspaceContext, WorkspaceGuidance, WorkspaceRepository,
    WorkspaceWorkItem, run_coding_agent_native, run_coding_agent_native_with_submit_for_pr,
    run_coding_agent_native_with_totals_tool_config_and_hosts,
};
use temper_protocol_activity::{
    AgentActivityCapturePolicyV1, AgentActivityChildRecordV1, AgentActivityEventV1,
    AgentActivityFrameV1, CaptureModeV1, CapturedContentV1, PromptSnapshotV1,
};
use temper_protocol_agent::{SubmitForPrGate, SubmitForPrRequest, SubmitForPrResponse};

#[path = "support/coding_agent_workspace.rs"]
mod coding_agent_workspace;
use coding_agent_workspace::{REPO_DIR, TempCheckout, run_git, seed_repo};

/// A jig-backed engineer turn driven by the native sans-IO agent loop
/// (`run_coding_agent_native` → `temper_agent_core::run_sub_agent`) on the skein
/// runtime. This is the path the worker takes in production; it must produce
/// a real product diff + result.
#[test]
fn jig_coding_agent_native_tool_loop_creates_product_diff() {
    let checkout = TempCheckout::new("jig-coding-agent-native-tool-loop");
    checkout.init_git();

    let observed_continuation = Arc::new(AtomicUsize::new(0));
    let fake = coding_agent_fake(Arc::clone(&observed_continuation));
    let provider = ProviderConfig::new(
        "jig-openai-compatible",
        "jig-coding-agent-native-tool-loop",
        "https://example.invalid/unused-production-url",
        "sk-jig-test",
    )
    .with_base_url_override(fake.base_url());

    let context = workspace_context();
    let cwd = checkout.path().to_path_buf();
    let result = temper_agent_io::block_on_with(move |_cx, handle| async move {
        run_coding_agent_native(handle, &provider, &context, &cwd, 6, None).await
    })
    .expect("native jig-backed coding agent succeeds");

    assert_eq!(result.verdict, None);
    assert!(result.summary.as_deref().unwrap_or("").contains("NOTES.md"));
    assert_eq!(
        fs::read_to_string(checkout.repo_path().join("NOTES.md")).expect("NOTES.md was written"),
        "project notes\n"
    );

    let status = checkout.git(&["status", "--porcelain=v1", "--untracked-files=all"]);
    assert!(
        status.lines().any(|line| line == "?? NOTES.md"),
        "status was {status:?}"
    );
    assert!(
        fake.requests().len() > 1,
        "expected a tool loop, got one model turn"
    );
    assert!(
        observed_continuation.load(Ordering::SeqCst) >= 1,
        "fake provider did not observe a tool-result continuation"
    );
}

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
    let mut context = workspace_context();
    let artifact_fixture: WorkspaceContext = serde_json::from_str(include_str!(
        "../../temper-protocol-agent/fixtures/workspace-context-artifact-context.json"
    ))
    .expect("artifact context fixture");
    context.artifact_context = artifact_fixture.artifact_context;
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
            // Anthropic's request encoder materializes an empty `required`
            // list for optional-only schemas. The captured ToolDef value omits
            // it, so normalize only that transport-level default.
            schema.remove("required");
        }
    }
    assert_eq!(
        serde_json::to_value(&snapshot.tools).expect("snapshot tools JSON"),
        provider_tools,
        "prompt tool names, descriptions, schemas, and order equal the provider ToolDef slice"
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

#[test]
fn submit_for_pr_failure_returns_to_same_native_run_for_fix_and_retry() {
    let checkout = TempCheckout::new("jig-submit-for-pr-retry");
    checkout.init_git();

    let submit_calls = Arc::new(std::sync::Mutex::new(Vec::<SubmitForPrRequest>::new()));
    let submit_attempts = Arc::new(AtomicUsize::new(0));
    let fake = submit_retry_fake();
    let provider = ProviderConfig::new(
        "jig-openai-compatible",
        "jig-submit-for-pr-retry",
        "https://example.invalid/unused-production-url",
        "sk-jig-test",
    )
    .with_base_url_override(fake.base_url());

    let submit_calls_for_host = Arc::clone(&submit_calls);
    let submit_attempts_for_host = Arc::clone(&submit_attempts);
    let host: SubmitForPrHost = Arc::new(move |request, _context, cwd| {
        submit_calls_for_host
            .lock()
            .expect("submit calls lock")
            .push(request.clone());
        let attempt = submit_attempts_for_host.fetch_add(1, Ordering::SeqCst);
        SubmitForPrResponse {
            accepted: attempt > 0,
            message: if attempt == 0 {
                "fake host gate failed"
            } else {
                "fake host gate passed"
            }
            .to_string(),
            gates: vec![SubmitForPrGate {
                command_id: format!("fake-submit-{attempt}"),
                argv: vec!["fake-gate".to_string(), attempt.to_string()],
                cwd: cwd.display().to_string(),
                exit_status: if attempt == 0 { "failed" } else { "passed" }.to_string(),
                exit_code: Some(if attempt == 0 { 1 } else { 0 }),
                stdout_tail: format!("stdout attempt {attempt}"),
                stderr_tail: if attempt == 0 {
                    "needs NOTES.md".to_string()
                } else {
                    String::new()
                },
                timed_out: false,
                elapsed_ms: 25 + attempt as u64,
            }],
        }
    });

    let context = workspace_context();
    let cwd = checkout.path().to_path_buf();
    let result = temper_agent_io::block_on_with(move |_cx, handle| async move {
        run_coding_agent_native_with_submit_for_pr(
            handle,
            &provider,
            &context,
            &cwd,
            8,
            None,
            Some(host),
        )
        .await
    })
    .expect("native submit retry run succeeds");

    assert_eq!(result.verdict, None);
    assert!(
        result
            .summary
            .as_deref()
            .unwrap_or_default()
            .contains("submit_for_pr passed")
    );
    assert_eq!(
        fs::read_to_string(checkout.repo_path().join("NOTES.md")).expect("NOTES.md was written"),
        "project notes after submit failure\n"
    );
    assert_eq!(submit_attempts.load(Ordering::SeqCst), 2);
    let submit_calls = submit_calls.lock().expect("submit calls lock");
    assert_eq!(submit_calls.len(), 2);
    assert_eq!(submit_calls[0].summary.as_deref(), Some("ready for PR"));
    assert_eq!(
        submit_calls[1].summary.as_deref(),
        Some("fixed and ready for PR")
    );
    assert_eq!(
        fake.requests().len(),
        3,
        "failure, fix+retry, then terminal JSON should stay in one live run"
    );
}

fn submit_retry_fake() -> FakeLlm {
    FakeLlm::start(Script::rule(move |view| match view.prior_tool_results {
        0 => Reply {
            turns: vec![Turn::ToolCall {
                id: "call_submit_initial".to_string(),
                name: "submit_for_pr".to_string(),
                args: serde_json::json!({ "summary": "ready for PR" }),
            }],
            usage: Default::default(),
            stop: StopReason::ToolCalls,
        },
        1 => Reply {
            turns: vec![
                Turn::ToolCall {
                    id: "call_write_notes_after_failure".to_string(),
                    name: "write".to_string(),
                    args: serde_json::json!({
                        "path": "demo/NOTES.md",
                        "content": "project notes after submit failure\n"
                    }),
                },
                Turn::ToolCall {
                    id: "call_submit_retry".to_string(),
                    name: "submit_for_pr".to_string(),
                    args: serde_json::json!({ "summary": "fixed and ready for PR" }),
                },
            ],
            usage: Default::default(),
            stop: StopReason::ToolCalls,
        },
        _ => Reply::text(r#"{"summary":"submit_for_pr passed after fixing NOTES.md."}"#),
    }))
    .expect("start fake LLM")
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

fn workspace_context() -> WorkspaceContext {
    WorkspaceContext {
        trace_context: None,
        artifact_context: None,
        repos: vec![WorkspaceRepository {
            id: "repo-1".to_string(),
            owner: "acme".to_string(),
            name: "demo".to_string(),
            default_branch: "main".to_string(),
            // The repo sits in a sibling subdir under the workspace root (cwd);
            // a single-repo job is just a one-entry manifest.
            dir: REPO_DIR.to_string(),
            access: "writable".to_string(),
            base_branch: "main".to_string(),
            branch_hint: Some("agent/pr-for-code-25".to_string()),
        }],
        work_item: WorkspaceWorkItem {
            role: "engineer".to_string(),
            queue: "code_ready".to_string(),
            kind: "code".to_string(),
            target: "Issue { number: ItemNumber(25) }".to_string(),
            context: serde_json::json!({
                "artifact": {
                    "type": "issue",
                    "number": 25,
                    "title": "Create deterministic notes",
                    "body": "Create NOTES.md whose first line is exactly `project notes`.",
                    "labels": ["code", "ready"],
                    "state": "Open"
                }
            })
            .to_string(),
        },
        action: "open_pr".to_string(),
        correlation_key: "pr-for-code-25".to_string(),
        checkout: Some("writable".to_string()),
        allowed_verdicts: vec!["needs_architect".to_string()],
        verdict_contracts: Default::default(),
        source_metadata: Default::default(),
        guidance: WorkspaceGuidance {
            role_guidance: Some(
                "Make a real product diff by creating NOTES.md. Do not create .temper-only bookkeeping diffs."
                    .to_string(),
            ),
            tool_guidance: Some("Use the available workspace tools to edit files.".to_string()),
            tool_constraints: vec!["Do not run git commit.".to_string()],
        },
        pull_request_freshness: None,
        agent_session: None,
    }
}

// ---------------------------------------------------------------------------
// Multi-repo (coordinated) coding turn — the real agent edits two sibling repos
// in one workspace (temper ADR 0023). Same native loop the worker drives in
// production; a jig fake LLM stands in for the model.
// ---------------------------------------------------------------------------

/// A coordinated engineer turn over a TWO-repo workspace: the agent's cwd is the
/// workspace root, with `alpha/` and `beta/` checked out as siblings. The fake
/// LLM writes a product file into each, proving the real agent (and its
/// contract validation) operate across repos, not a single checkout.
#[test]
fn jig_coding_agent_native_edits_two_sibling_repos_in_one_workspace() {
    let workspace = TempCheckout::new("jig-coding-agent-multi-repo");
    seed_repo(workspace.path(), "alpha");
    seed_repo(workspace.path(), "beta");

    let fake = multi_repo_agent_fake();
    let provider = ProviderConfig::new(
        "jig-openai-compatible",
        "jig-coding-agent-multi-repo",
        "https://example.invalid/unused-production-url",
        "sk-jig-test",
    )
    .with_base_url_override(fake.base_url());

    let context = multi_repo_context();
    let cwd = workspace.path().to_path_buf();
    let result = temper_agent_io::block_on_with(move |_cx, handle| async move {
        run_coding_agent_native(handle, &provider, &context, &cwd, 6, None).await
    })
    .expect("native multi-repo coding agent succeeds");

    // One agent turn, two repos edited; the contract passed because BOTH writable
    // repos produced a diff.
    assert_eq!(result.verdict, None);
    assert_eq!(
        fs::read_to_string(workspace.path().join("alpha/NOTES.md")).expect("alpha NOTES.md"),
        "alpha notes\n"
    );
    assert_eq!(
        fs::read_to_string(workspace.path().join("beta/NOTES.md")).expect("beta NOTES.md"),
        "beta notes\n"
    );
    for dir in ["alpha", "beta"] {
        let status = run_git(
            &workspace.path().join(dir),
            &["status", "--porcelain=v1", "--untracked-files=all"],
        );
        assert!(
            status.lines().any(|line| line == "?? NOTES.md"),
            "{dir} status was {status:?}"
        );
    }
}

/// A fake LLM that, in one turn, writes `NOTES.md` into each repo's sibling dir,
/// then returns the engineer success result.
fn multi_repo_agent_fake() -> FakeLlm {
    FakeLlm::start(Script::rule(move |view| {
        if view.prior_tool_results == 0 {
            Reply {
                turns: vec![
                    Turn::ToolCall {
                        id: "call_write_alpha".to_string(),
                        name: "write".to_string(),
                        args: serde_json::json!({
                            "path": "alpha/NOTES.md",
                            "content": "alpha notes\n"
                        }),
                    },
                    Turn::ToolCall {
                        id: "call_write_beta".to_string(),
                        name: "write".to_string(),
                        args: serde_json::json!({
                            "path": "beta/NOTES.md",
                            "content": "beta notes\n"
                        }),
                    },
                ],
                usage: Default::default(),
                stop: StopReason::ToolCalls,
            }
        } else {
            Reply::text(r#"{"summary":"Added NOTES.md to alpha and beta."}"#)
        }
    }))
    .expect("start fake LLM")
}

fn multi_repo_context() -> WorkspaceContext {
    let repo = |name: &str| WorkspaceRepository {
        id: name.to_string(),
        owner: "acme".to_string(),
        name: name.to_string(),
        default_branch: "main".to_string(),
        dir: name.to_string(),
        access: "writable".to_string(),
        base_branch: "main".to_string(),
        branch_hint: Some("agent/coord-for-code-7".to_string()),
    };
    WorkspaceContext {
        trace_context: None,
        artifact_context: None,
        repos: vec![repo("alpha"), repo("beta")],
        work_item: WorkspaceWorkItem {
            role: "engineer".to_string(),
            queue: "code_ready".to_string(),
            kind: "code".to_string(),
            target: "Issue { number: ItemNumber(7) }".to_string(),
            context: serde_json::json!({
                "artifact": {
                    "type": "issue",
                    "number": 7,
                    "title": "Cross-repo notes",
                    "body": "Add NOTES.md to both repos.",
                    "labels": ["code", "ready", "coordinated"],
                    "state": "Open"
                }
            })
            .to_string(),
        },
        action: "open_pr".to_string(),
        correlation_key: "coord-for-code-7".to_string(),
        checkout: Some("writable".to_string()),
        allowed_verdicts: vec!["needs_architect".to_string()],
        verdict_contracts: Default::default(),
        source_metadata: Default::default(),
        guidance: WorkspaceGuidance {
            role_guidance: Some(
                "Make a real product diff in each writable repo by creating NOTES.md.".to_string(),
            ),
            tool_guidance: Some("Use the available workspace tools to edit files.".to_string()),
            tool_constraints: vec!["Do not run git commit.".to_string()],
        },
        pull_request_freshness: None,
        agent_session: None,
    }
}
