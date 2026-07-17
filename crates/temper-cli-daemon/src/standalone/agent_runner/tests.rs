use super::*;
use temper_protocol_activity::{AgentActivityEventV1, CaptureModeV1, RunFailedV1};
use temper_protocol_worker::FailureClass;

#[test]
fn run_failure_is_transient() {
    let err = classify_coding_agent_error(CodingAgentError::Run("network".into()));
    assert_eq!(err.class, FailureClass::Transient);
}

#[test]
fn no_product_is_permanent() {
    let err = classify_coding_agent_error(CodingAgentError::NoProduct);
    assert_eq!(err.class, FailureClass::Permanent);
}

#[test]
fn parse_failure_is_transient() {
    let err = classify_coding_agent_error(CodingAgentError::Parse {
        snippet: "some text".into(),
        error: "no JSON object".into(),
    });
    assert_eq!(err.class, FailureClass::Transient);
}

#[test]
fn typed_terminal_report_distinguishes_success_failure_and_cancellation() {
    assert_eq!(
        agent_terminal_report(&Result::<(), CodingAgentError>::Ok(())),
        (
            AgentTerminalStatus::Succeeded,
            Some(AgentTerminalReasonV1::Completed)
        )
    );
    assert_eq!(
        agent_terminal_report(&Result::<(), CodingAgentError>::Err(
            CodingAgentError::AgentStopped("provider stop".into())
        )),
        (
            AgentTerminalStatus::Failed,
            Some(AgentTerminalReasonV1::ModelError)
        )
    );
    assert_eq!(
        agent_terminal_report(&Result::<(), CodingAgentError>::Err(
            CodingAgentError::BudgetExhausted { max_iterations: 7 }
        )),
        (
            AgentTerminalStatus::Failed,
            Some(AgentTerminalReasonV1::BudgetExhausted)
        )
    );
    assert_eq!(
        agent_terminal_report(&Result::<(), CodingAgentError>::Err(
            CodingAgentError::Aborted {
                authority: temper_agent::AgentAbortAuthority::WorkerRequested,
            }
        )),
        (
            AgentTerminalStatus::Cancelled,
            Some(AgentTerminalReasonV1::Aborted)
        )
    );
}

#[test]
fn in_process_runner_stores_tool_config_and_filters_by_role() {
    let tool_config = test_tool_config();
    let expected = tool_config.clone();
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let provider = ProviderConfig::new(
            "test-provider",
            "test-model",
            "https://llm.example",
            "test-key",
        );
        let runner = InProcessAgentRunner::new(handle, provider, 42, None, false)
            .with_tool_config(Some(tool_config));

        assert_eq!(runner.tool_config_for_role("engineer"), Some(&expected));
        assert!(runner.tool_config_for_role("architect").is_none());
    });
}

#[test]
fn in_process_runner_passes_tool_config_to_native_loop() {
    let tool_config = required_bad_tool_config_for_role("architect");
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let provider = ProviderConfig::new(
            "test-provider",
            "test-model",
            "https://llm.example",
            "test-key",
        );
        let runner = InProcessAgentRunner::new(handle, provider, 1, None, false)
            .with_tool_config(Some(tool_config));
        let context = ctx("ai", "temper", "issue", "Issue { number: ItemNumber(42) }");
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(temp.path().join("temper")).expect("prepared repo dir");

        let error = runner
            .run("job-test", &context, temp.path())
            .await
            .expect_err("required codebase-memory startup failure aborts run");
        assert_eq!(error.class, FailureClass::Transient);
        assert!(error.message.contains("codebase-memory tool setup failed"));
        assert!(
            error
                .message
                .contains("required codebase-memory MCP startup failed")
        );
    });
}

#[test]
fn in_process_terminal_failures_never_capture_tool_diagnostics() {
    const RAW_ERROR_SENTINELS: [&str; 4] = [
        "CREDENTIAL-IN-PROCESS-SENTINEL-353",
        "HEADER-IN-PROCESS-SENTINEL-353",
        "ENVIRONMENT-IN-PROCESS-SENTINEL-353",
        "TOOL-IN-PROCESS-SENTINEL-353",
    ];
    let command = RAW_ERROR_SENTINELS.join("-");
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        for capture in [
            CaptureModeV1::Off,
            CaptureModeV1::Metadata,
            CaptureModeV1::Transcript,
            CaptureModeV1::Diagnostic,
        ] {
            let temp = tempfile::tempdir().expect("in-process failure tempdir");
            std::fs::create_dir_all(temp.path().join("temper")).expect("prepared repo dir");
            let spool_root = temp.path().join("spool");
            let policy = AgentActivityCapturePolicyV1 {
                capture,
                capture_thinking: capture == CaptureModeV1::Diagnostic,
                ..Default::default()
            };
            let provider = ProviderConfig::new(
                "test-provider",
                "test-model",
                "https://llm.example",
                "provider-credential-must-stay-outside-activity",
            );
            let runner = InProcessAgentRunner::new(handle.clone(), provider, 1, None, false)
                .with_tool_config(Some(required_bad_tool_config_with_command(
                    "architect",
                    &command,
                )))
                .with_trace_policy(policy.clone())
                .with_trace_collector(WorkerAgentTraceConfig {
                    policy,
                    spool_root: Some(spool_root.clone()),
                });
            let context = ctx("ai", "temper", "issue", "Issue { number: ItemNumber(353) }");
            let error = runner
                .run("job-in-process-failure-353", &context, temp.path())
                .await
                .expect_err("tool startup fixture remains an agent failure");
            assert_eq!(error.class, FailureClass::Transient);
            for sentinel in RAW_ERROR_SENTINELS {
                assert!(
                    error.message.contains(sentinel),
                    "job diagnostics retain {sentinel}"
                );
            }

            if capture == CaptureModeV1::Off {
                assert!(!spool_root.exists());
                continue;
            }
            let recovered = TraceCollector::new(WorkerAgentTraceConfig {
                policy: AgentActivityCapturePolicyV1 {
                    capture,
                    capture_thinking: capture == CaptureModeV1::Diagnostic,
                    ..Default::default()
                },
                spool_root: Some(spool_root.clone()),
            })
            .recover()
            .expect("recover in-process failure trace");
            assert_eq!(recovered.len(), 1);
            let stored = directory_bytes(&spool_root);
            let canonical = String::from_utf8_lossy(&stored);
            for sentinel in RAW_ERROR_SENTINELS {
                assert!(!canonical.contains(sentinel), "trace leaked {sentinel}");
            }
            let terminal = recovered[0].events.last().expect("terminal event");
            let AgentActivityEventV1::RunFailed(RunFailedV1 { failure }) = &terminal.event else {
                panic!("in-process failure must end with run.failed");
            };
            assert_eq!(failure.code, FailureCodeV1::Internal);
            assert_eq!(failure.message, "agent run failed with a transient error");
            assert!(failure.retryable);
        }
    });
}

fn directory_bytes(root: &std::path::Path) -> Vec<u8> {
    if root.is_file() {
        return std::fs::read(root).expect("read trace file");
    }
    let mut bytes = Vec::new();
    let mut entries = std::fs::read_dir(root)
        .expect("read trace directory")
        .map(|entry| entry.expect("trace directory entry").path())
        .collect::<Vec<_>>();
    entries.sort();
    for entry in entries {
        bytes.extend(directory_bytes(&entry));
    }
    bytes
}

fn test_tool_config() -> AgentToolConfig {
    use temper_protocol_agent::{
        CodebaseMemoryIndex, CodebaseMemoryMode, CodebaseMemoryToolConfig,
    };

    AgentToolConfig {
        codebase_memory: Some(CodebaseMemoryToolConfig {
            mode: CodebaseMemoryMode::Auto,
            command: "codebase-memory-mcp".to_string(),
            args: Vec::new(),
            roles: vec!["engineer".to_string()],
            index: CodebaseMemoryIndex::Background,
            startup_timeout_secs: 5,
            index_timeout_secs: 30,
        }),
    }
}

fn required_bad_tool_config_for_role(role: &str) -> AgentToolConfig {
    required_bad_tool_config_with_command(role, "definitely-not-a-temper-codebase-memory-mcp")
}

fn required_bad_tool_config_with_command(role: &str, command: &str) -> AgentToolConfig {
    use temper_protocol_agent::{
        CodebaseMemoryIndex, CodebaseMemoryMode, CodebaseMemoryToolConfig,
    };

    AgentToolConfig {
        codebase_memory: Some(CodebaseMemoryToolConfig {
            mode: CodebaseMemoryMode::Required,
            command: command.to_string(),
            args: Vec::new(),
            roles: vec![role.to_string()],
            index: CodebaseMemoryIndex::Off,
            startup_timeout_secs: 1,
            index_timeout_secs: 1,
        }),
    }
}

#[test]
fn parses_issue_target_number() {
    assert_eq!(
        parse_target_number("Issue { number: ItemNumber(7) }"),
        Some(7)
    );
}

#[test]
fn parses_pull_request_target_number() {
    assert_eq!(
        parse_target_number("PullRequest { number: ItemNumber(44) }"),
        Some(44)
    );
}

#[test]
fn parses_bare_number_form() {
    // Some fixtures format the target without the `ItemNumber(..)` wrapper.
    assert_eq!(parse_target_number("Issue { number: 7 }"), Some(7));
}

#[test]
fn malformed_target_yields_none() {
    assert_eq!(parse_target_number(""), None);
    assert_eq!(parse_target_number("Issue { }"), None);
    assert_eq!(parse_target_number("number: ItemNumber()"), None);
    assert_eq!(parse_target_number("garbage with no marker"), None);
}

#[test]
fn human_count_below_1000_is_raw() {
    assert_eq!(human_count(0), "0");
    assert_eq!(human_count(1), "1");
    assert_eq!(human_count(999), "999");
}

#[test]
fn human_count_thousands_keep_one_decimal() {
    assert_eq!(human_count(1000), "1k");
    assert_eq!(human_count(1500), "1.5k");
    assert_eq!(human_count(6379), "6.4k");
    assert_eq!(human_count(9999), "10k"); // rounds up out of the decimal band
}

#[test]
fn human_count_large_rounds_to_whole_k() {
    assert_eq!(human_count(10_000), "10k");
    assert_eq!(human_count(10_500), "11k");
    assert_eq!(human_count(470_306), "470k");
    assert_eq!(human_count(1_000_000), "1000k");
}

#[test]
fn totals_suffix_renders_humanized_counts() {
    let totals = RunTotals {
        input: 470_306,
        output: 6379,
        tool_calls: 52,
    };
    assert_eq!(totals_suffix(totals), "470k in / 6.4k out, 52 tool calls");
}

#[test]
fn totals_suffix_singular_tool_call() {
    let totals = RunTotals {
        input: 0,
        output: 0,
        tool_calls: 1,
    };
    assert_eq!(totals_suffix(totals), "0 in / 0 out, 1 tool call");
}

#[test]
fn run_kind_maps_roles_to_activities() {
    assert_eq!(run_kind("architect"), "triage");
    assert_eq!(run_kind("engineer"), "coding");
    assert_eq!(run_kind("reviewer"), "review");
    assert_eq!(run_kind("mechanical"), "run");
}

fn ctx(owner: &str, name: &str, kind: &str, target: &str) -> WorkspaceContext {
    use temper_protocol_agent::{WorkspaceRepository, WorkspaceWorkItem};
    WorkspaceContext {
        trace_context: None,
        artifact_context: None,
        repos: vec![WorkspaceRepository {
            id: format!("forgejo:{owner}/{name}"),
            owner: owner.to_string(),
            name: name.to_string(),
            default_branch: "main".to_string(),
            dir: name.to_string(),
            access: "writable".to_string(),
            base_branch: "main".to_string(),
            branch_hint: None,
        }],
        work_item: WorkspaceWorkItem {
            role: "architect".to_string(),
            queue: "intake".to_string(),
            kind: kind.to_string(),
            target: target.to_string(),
            context: "{}".to_string(),
        },
        action: "triage_intake".to_string(),
        correlation_key: "k".to_string(),
        checkout: None,
        allowed_verdicts: Vec::new(),
        verdict_contracts: Default::default(),
        source_metadata: Default::default(),
        guidance: Default::default(),
        pull_request_freshness: None,
        agent_session: None,
    }
}

#[test]
fn builds_issue_ref_from_context() {
    let context = ctx("ai", "temper", "issue", "Issue { number: ItemNumber(42) }");
    let item = work_item_ref(&context).expect("issue ref");
    assert_eq!(item.repo(), "ai/temper");
    assert_eq!(item.to_string(), "ai/temper#42");
}

#[test]
fn builds_pull_request_ref_from_context() {
    let context = ctx(
        "ai",
        "temper",
        "pull_request",
        "PullRequest { number: ItemNumber(44) }",
    );
    let item = work_item_ref(&context).expect("pr ref");
    assert_eq!(item.to_string(), "ai/temper PR#44");
}

#[test]
fn work_item_ref_is_none_on_unparseable_target() {
    let context = ctx("ai", "temper", "issue", "Issue { }");
    assert!(work_item_ref(&context).is_none());
}
