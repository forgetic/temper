use std::fs;

use super::*;

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("smith-workflow-capture-test-{}", Uuid::new_v4()));
        fs::create_dir(&path).expect("temp capture dir is created");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn fixture_request() -> WorkflowRoleDecisionRequest {
    serde_json::from_str(include_str!(
        "../../../../temper/crates/temper-process-protocol/fixtures/workflow-role-decision-request.json"
    ))
    .expect("Temper workflow-role decision fixture parses")
}

fn provider() -> ProviderConfig {
    ProviderConfig::new(
        "deepseek",
        "deepseek-chat",
        "https://api.example.invalid/v1",
        "sk-secret-do-not-log",
    )
}

fn capture_input<'a>(
    request: &'a WorkflowRoleDecisionRequest,
    trace: &'a WorkflowRoleTrace,
    provider: &'a ProviderConfig,
    system_prompt: &'a str,
    user_context: &'a str,
    model_decision: Option<&'a WorkflowRoleModelDecision>,
    final_reply: Option<&'a WorkflowRoleDecisionReply>,
) -> WorkflowRoleDecisionCaptureInput<'a> {
    WorkflowRoleDecisionCaptureInput {
        request,
        trace,
        provider,
        system_prompt: Some(system_prompt),
        user_context: Some(user_context),
        model_decision,
        final_reply,
        latency_ms: Some(42),
        outcome: "authorized_action",
        failure_class: None,
    }
}

#[test]
fn capture_is_disabled_by_default() {
    let request = fixture_request();
    let trace = WorkflowRoleTrace::from_work_item_context(&request.work_item_context);
    let provider = provider();
    let input = capture_input(&request, &trace, &provider, "prompt", "context", None, None);

    assert!(!WorkflowRoleDecisionCapture::disabled().is_enabled());
    let result = WorkflowRoleDecisionCapture::from_optional_dir(None::<PathBuf>)
        .write_with_local_id(input, 1, "local-1");

    assert_eq!(result, CaptureWriteResult::Disabled);
}

#[test]
fn capture_file_names_use_path_safe_trace_ids_or_local_ids() {
    let mut trace = WorkflowRoleTrace {
        decision_id: Some("decision/work item:42".to_string()),
        ..WorkflowRoleTrace::default()
    };
    let path = capture_file_path(Path::new("/tmp"), &trace, "local-123");
    let file_name = path.file_name().unwrap().to_string_lossy();
    assert_eq!(file_name, "decision-decision-work-item-42.json");

    trace.decision_id = Some("/home/free/.pi/agent/auth.json".to_string());
    trace.work_item_id = Some("work/item:7".to_string());
    let path = capture_file_path(Path::new("/tmp"), &trace, "local-123");
    let file_name = path.file_name().unwrap().to_string_lossy();
    assert_eq!(file_name, "work-item-work-item-7.json");
    assert!(!file_name.contains("auth"));
    assert!(!file_name.contains('/'));

    trace.work_item_id = Some("sk-secret-do-not-log".to_string());
    let path = capture_file_path(Path::new("/tmp"), &trace, "local-123");
    let file_name = path.file_name().unwrap().to_string_lossy();
    assert_eq!(file_name, "decision-local-123.json");
    assert!(!file_name.contains("sk-secret"));
}

#[test]
fn capture_previews_are_redacted_and_bounded() {
    let request = fixture_request();
    let trace = WorkflowRoleTrace::from_work_item_context(&request.work_item_context);
    let provider = provider();
    let prompt = format!(
        "Authorization: Bearer sk-secret-do-not-log {}TAIL",
        "x".repeat(1200)
    );
    let context = r#"{"auth_file":"/home/free/.pi/agent/auth.json","body":"password=hunter2"}"#;
    let input = capture_input(&request, &trace, &provider, &prompt, context, None, None);

    let record = DecisionCaptureFile::from_input(input, 123);
    let rendered = serde_json::to_string(&record).expect("capture serializes");

    assert!(record.prompt.unwrap().preview.ends_with('…'));
    assert!(!rendered.contains("sk-secret-do-not-log"));
    assert!(!rendered.contains("auth.json"));
    assert!(!rendered.contains("hunter2"));
    assert!(!rendered.contains("TAIL"));
}

#[test]
fn capture_write_creates_one_redacted_json_file() {
    let dir = TempDir::new();
    let mut request = fixture_request();
    request.work_item_context["artifact"]["body"] =
        serde_json::json!("password=hunter2 and Bearer sk-secret-do-not-log must not persist");
    let trace = WorkflowRoleTrace::from_work_item_context(&request.work_item_context);
    let provider = provider();
    let prompt = "Prompt with access_token: tok-123";
    let context = serde_json::to_string_pretty(&request.work_item_context)
        .expect("context serializes for test");
    let decision = WorkflowRoleModelDecision::action(
        "advance",
        "because refresh_token=rotating must not persist",
    );
    let reply = WorkflowRoleDecisionReply {
        protocol_version: request.protocol_version,
        action: "advance".to_string(),
        reason: "final reason with Authorization: Bearer sk-final-secret".to_string(),
    };
    let input = capture_input(
        &request,
        &trace,
        &provider,
        prompt,
        &context,
        Some(&decision),
        Some(&reply),
    );

    let result = WorkflowRoleDecisionCapture::directory(dir.path()).write_with_local_id(
        input,
        1234,
        "local-123",
    );
    let path = match result {
        CaptureWriteResult::Written(path) => path,
        other => panic!("expected capture write, got {other:?}"),
    };
    let rendered = fs::read_to_string(&path).expect("capture file is readable");
    let parsed: serde_json::Value = serde_json::from_str(&rendered).expect("capture JSON parses");

    assert_eq!(parsed["schema_version"], 1);
    assert_eq!(parsed["captured_at_unix_ms"], 1234);
    assert_eq!(parsed["workflow"]["workflow_id"], "generic-agent-test");
    assert_eq!(parsed["workflow"]["role_id"], "banana");
    assert_eq!(parsed["provider"]["auth_mode"], "api_key");
    assert_eq!(
        parsed["allowed_actions"],
        serde_json::json!(["no_action", "advance"])
    );
    assert_eq!(
        parsed["available_external_tool_ids"],
        serde_json::json!(["coding_workspace"])
    );
    assert_eq!(parsed["latency_ms"], 42);
    assert_eq!(parsed["outcome"], "authorized_action");
    assert!(!rendered.contains("sk-secret"));
    assert!(!rendered.contains("hunter2"));
    assert!(!rendered.contains("rotating"));
    assert!(!rendered.contains("tok-123"));
}

#[test]
fn capture_write_failure_is_reported_without_creating_directory() {
    let missing_dir =
        std::env::temp_dir().join(format!("smith-missing-capture-dir-{}", Uuid::new_v4()));
    let request = fixture_request();
    let trace = WorkflowRoleTrace::from_work_item_context(&request.work_item_context);
    let provider = provider();
    let input = capture_input(&request, &trace, &provider, "prompt", "context", None, None);

    let result = WorkflowRoleDecisionCapture::directory(&missing_dir)
        .write_with_local_id(input, 1, "local-1");

    match result {
        CaptureWriteResult::Failed(error) => {
            assert_eq!(error.class(), "not_found");
            assert!(!missing_dir.exists());
        }
        other => panic!("expected write failure, got {other:?}"),
    }
}
