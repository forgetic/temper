const TWO_ROOT_MODEL_SELECTOR: &str = "temper-v1-production.src.model.affinity_topic";
const TWO_ROOT_ROUTE_SELECTOR: &str = "temper-v1-production.src.route.worker_slot";
const TWO_ROOT_CALLER_SELECTOR: &str = "temper-v1-production.src.route.worker_for";
const TWO_ROOT_TEST_SELECTOR: &str =
    "temper-v1-production.tests.route.keeps_worker_affinity";
const TWO_ROOT_PROVIDER_TEXT: &str = "PRIVATE-TWO-ROOT-PROVIDER-TEXT";
const TWO_ROOT_SOURCE_TEXT: &str = "PRIVATE-TWO-ROOT-SOURCE";

struct TwoRootCorrectionHarness {
    _server: tempfile::TempDir,
    _workspace: tempfile::TempDir,
    log_path: std::path::PathBuf,
    admission: temper_agent_core::LineageAdmissionHandle,
    registry: ToolRegistry,
    machine: AgentMachine,
}

async fn two_root_correction_harness() -> TwoRootCorrectionHarness {
    let server = tempfile::tempdir().unwrap();
    std::fs::write(
        server.path().join("fake_codebase_memory_mcp.py"),
        include_str!("lineage_two_root_correction_provider.py"),
    )
    .unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let log_path = workspace.path().join("mcp.log");
    let context = crate::codebase_memory::tests::test_support::workspace_context(
        workspace.path(),
        &[("acme", "demo", "repo")],
    );
    std::fs::create_dir_all(workspace.path().join("repo/src")).unwrap();
    std::fs::create_dir_all(workspace.path().join("repo/tests")).unwrap();
    std::fs::write(
        workspace.path().join("repo/src/route.rs"),
        "fn worker_for() {}\nfn worker_slot() {}\n",
    )
    .unwrap();
    std::fs::write(
        workspace.path().join("repo/src/model.rs"),
        "fn affinity_topic() {}\n",
    )
    .unwrap();
    std::fs::write(
        workspace.path().join("repo/tests/route.rs"),
        "fn keeps_worker_affinity() {}\n",
    )
    .unwrap();

    let toolset = crate::codebase_memory::build_codebase_memory_toolset(
        Some(&crate::codebase_memory::tests::test_support::config(
            &server,
            CodebaseMemoryMode::Required,
            CodebaseMemoryIndex::Off,
            "two-root-correction",
            &log_path,
            serde_json::json!({}),
        )),
        "engineer",
        &context,
        workspace.path(),
    )
    .await
    .unwrap();
    let admission = toolset.lineage_admission().unwrap();
    let mut tools = toolset.into_tools();
    tools.push(Box::new(OrdinaryReadTool));
    tools.push(Box::new(BlockedMutationTool));
    let registry = ToolRegistry::from_tools(tools);
    let catalog = Arc::new(ToolInvocationCatalog::from_registry(&registry).unwrap());
    let mut machine = AgentMachine::with_invocation_catalog(
        vec![Message::User(UserMessage {
            content: UserContent::Text("repair routing".to_string()),
            timestamp: 0,
        })],
        30,
        catalog,
    )
    .with_lineage_admission(admission.clone());
    let _ = machine.on_start(EngineTime::ZERO);

    TwoRootCorrectionHarness {
        _server: server,
        _workspace: workspace,
        log_path,
        admission,
        registry,
        machine,
    }
}

fn has_active_root_handoff(requests: &[AgentRequest]) -> bool {
    requests.iter().any(|request| match request {
        AgentRequest::CallLlm { messages, .. } => messages.last().is_some_and(|message| {
            matches!(message, Message::User(message)
                if matches!(&message.content, UserContent::Text(text)
                    if text.contains("Active-root selector handoff")))
        }),
        _ => false,
    })
}

fn assert_decision_anchor_mutation_denial(requests: &[AgentRequest], id: &str) {
    assert!(requests.iter().any(|request| matches!(
        request,
        AgentRequest::RunTool {
            call,
            denial: Some(ToolCallDenial::DecisionAnchorMutation),
            rejection: None,
            ..
        } if call.id == id
    )));
}

fn assert_two_root_privacy(
    retained_metadata: &[Option<serde_json::Value>],
    retained_diagnostics: &[ToolFailureDiagnostic],
    correction_text: &str,
    correction_references: &[String],
    implementation_root: &DecisionAnchorLineageV1,
    focused_root: &DecisionAnchorLineageV1,
) {
    let metadata = serde_json::to_string(retained_metadata).unwrap();
    for private in [
        TWO_ROOT_PROVIDER_TEXT,
        TWO_ROOT_SOURCE_TEXT,
        TWO_ROOT_MODEL_SELECTOR,
        TWO_ROOT_ROUTE_SELECTOR,
        TWO_ROOT_CALLER_SELECTOR,
        TWO_ROOT_TEST_SELECTOR,
        "src/model.rs",
        "src/route.rs",
        "tests/route.rs",
    ] {
        assert!(!metadata.contains(private), "retained metadata exposed {private}");
        assert!(
            !correction_text.contains(private),
            "correction handoff exposed {private}"
        );
    }
    let diagnostics = format!("{retained_diagnostics:?}");
    for private in [
        TWO_ROOT_PROVIDER_TEXT,
        TWO_ROOT_SOURCE_TEXT,
        TWO_ROOT_MODEL_SELECTOR,
        TWO_ROOT_ROUTE_SELECTOR,
        TWO_ROOT_CALLER_SELECTOR,
        TWO_ROOT_TEST_SELECTOR,
        "repo/src/model.rs",
        "repo/src/route.rs",
        implementation_root.root_binding.as_str(),
        focused_root.root_binding.as_str(),
    ]
    .into_iter()
    .chain(correction_references.iter().map(String::as_str))
    {
        assert!(
            !diagnostics.contains(private),
            "retained diagnostics exposed {private}"
        );
    }
}

#[test]
fn two_root_model_correction_is_stable_when_first_preview_finishes_first() {
    run_two_root_correction_regression(false, false);
}

#[test]
fn two_root_model_correction_is_stable_when_preview_completion_reverses() {
    run_two_root_correction_regression(true, false);
}

#[test]
fn two_root_route_retention_is_stable_when_first_preview_finishes_first() {
    run_two_root_correction_regression(false, true);
}

#[test]
fn two_root_route_retention_is_stable_when_preview_completion_reverses() {
    run_two_root_correction_regression(true, true);
}
