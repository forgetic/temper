use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use jig_core::{Reply, Script, StopReason, Turn};
use jig_server::FakeLlm;
use temper_agent::{
    PlanPublication, ProviderConfig, PublishPlanHook, WorkspaceContext, WorkspaceGuidance,
    WorkspaceRepository, WorkspaceWorkItem, run_coding_agent_native_with_hooks,
};

#[allow(dead_code)]
#[path = "support/coding_agent_workspace.rs"]
mod coding_agent_workspace;
use coding_agent_workspace::{REPO_DIR, TempCheckout};

/// Records `publish_plan` calls so the test can assert the host receives the
/// model-supplied summary/phases plus workspace-derived routing data.
struct RecordingPlanPublisher {
    publications: std::sync::Mutex<Vec<PlanPublication>>,
    events: Arc<std::sync::Mutex<Vec<String>>>,
    product_path: PathBuf,
}

#[async_trait::async_trait]
impl PublishPlanHook for RecordingPlanPublisher {
    async fn publish_plan(&self, publication: PlanPublication) -> Result<(), String> {
        assert!(
            !self.product_path.exists(),
            "publish_plan must run before product edits create NOTES.md"
        );
        self.publications
            .lock()
            .expect("publications lock")
            .push(publication);
        self.events
            .lock()
            .expect("events lock")
            .push("publish_plan".to_string());
        Ok(())
    }
}

/// The agent, given the `publish_plan` tool, publishes its plan before the final
/// success result. The tool routes through the host hook and fills repo/base/work
/// branch data from the [`WorkspaceContext`], not model-authored forge actions.
#[test]
fn agent_invokes_publish_plan_hook_before_final_success() {
    let checkout = TempCheckout::new("jig-coding-agent-publish-plan-tool");
    checkout.init_git();

    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorder = Arc::new(RecordingPlanPublisher {
        publications: std::sync::Mutex::new(Vec::new()),
        events: Arc::clone(&events),
        product_path: checkout.repo_path().join("NOTES.md"),
    });
    let fake = publish_plan_tool_fake(Arc::clone(&events));
    let provider = ProviderConfig::new(
        "jig-openai-compatible",
        "jig-coding-agent-publish-plan-tool",
        "https://example.invalid/unused-production-url",
        "sk-jig-test",
    )
    .with_base_url_override(fake.base_url());

    let context = workspace_context();
    let cwd = checkout.path().to_path_buf();
    let hook = recorder.clone();
    let result = temper_agent_io::block_on_with(move |_cx, handle| async move {
        run_coding_agent_native_with_hooks(
            handle,
            &provider,
            &context,
            &cwd,
            6,
            None,
            false,
            None,
            None,
            None,
            Some(hook as Arc<dyn PublishPlanHook>),
        )
        .await
        .map(|(result, _totals)| result)
    })
    .expect("native coding agent with a publish_plan hook succeeds");

    assert_eq!(result.verdict, None);
    let publications = recorder.publications.lock().expect("publications lock");
    assert_eq!(publications.len(), 1);
    let publication = &publications[0];
    assert_eq!(publication.summary, "Implement deterministic notes");
    assert_eq!(
        publication.phases,
        vec!["create notes file".to_string(), "verify result".to_string()]
    );
    assert_eq!(publication.target_repos.len(), 1);
    let target = &publication.target_repos[0];
    assert_eq!(target.repo_path, "acme/demo");
    assert_eq!(target.dir, REPO_DIR);
    assert_eq!(target.base_branch, "main");
    assert_eq!(target.branch_hint.as_deref(), Some("agent/pr-for-code-25"));
    assert_eq!(
        events.lock().expect("events lock").clone(),
        vec!["publish_plan".to_string(), "final_success".to_string()],
        "plan publication should happen before the model emits final success"
    );
    assert_eq!(
        fs::read_to_string(checkout.repo_path().join("NOTES.md")).expect("NOTES.md written"),
        "project notes\n"
    );
}

fn publish_plan_tool_fake(events: Arc<std::sync::Mutex<Vec<String>>>) -> FakeLlm {
    FakeLlm::start(Script::rule(move |view| match view.prior_tool_results {
        0 => Reply {
            turns: vec![Turn::ToolCall {
                id: "call_publish_plan".to_string(),
                name: "publish_plan".to_string(),
                args: serde_json::json!({
                    "summary": "Implement deterministic notes",
                    "phases": ["create notes file", "verify result"]
                }),
            }],
            usage: Default::default(),
            stop: StopReason::ToolCalls,
        },
        1 => Reply {
            turns: vec![Turn::ToolCall {
                id: "call_write".to_string(),
                name: "write".to_string(),
                args: serde_json::json!({
                    "path": "demo/NOTES.md",
                    "content": "project notes\n"
                }),
            }],
            usage: Default::default(),
            stop: StopReason::ToolCalls,
        },
        _ => {
            events
                .lock()
                .expect("events lock")
                .push("final_success".to_string());
            Reply::text(
                r#"{"summary":"wrote NOTES.md after publishing the plan","plan":{"phases":["create notes file","verify result"]}}"#,
            )
        }
    }))
    .expect("start fake LLM")
}

fn workspace_context() -> WorkspaceContext {
    WorkspaceContext {
        repos: vec![WorkspaceRepository {
            id: "repo-1".to_string(),
            owner: "acme".to_string(),
            name: "demo".to_string(),
            default_branch: "main".to_string(),
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
        guidance: WorkspaceGuidance {
            role_guidance: Some(
                "Make a real product diff by creating NOTES.md. Do not create .temper-only bookkeeping diffs."
                    .to_string(),
            ),
            tool_guidance: Some("Use the available workspace tools to edit files.".to_string()),
            tool_constraints: vec!["Do not run git commit.".to_string()],
        },
    }
}
