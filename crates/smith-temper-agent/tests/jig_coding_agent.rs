use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use jig_core::{Reply, Script, StopReason, Turn};
use jig_server::FakeLlm;
use smith_temper_agent::{
    ProviderConfig, WorkspaceContext, WorkspaceGuidance, WorkspaceRepository, WorkspaceWorkItem,
    run_coding_agent, run_coding_agent_native,
};

#[test]
fn jig_coding_agent_tool_loop_creates_product_diff() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let checkout = TempCheckout::new("jig-coding-agent-tool-loop");
    checkout.init_git();

    let observed_continuation = Arc::new(AtomicUsize::new(0));
    let fake = coding_agent_fake(Arc::clone(&observed_continuation));
    let provider = ProviderConfig::new(
        "jig-openai-compatible",
        "jig-coding-agent-tool-loop",
        "https://example.invalid/unused-production-url",
        "sk-jig-test",
    )
    .with_base_url_override(fake.base_url());

    let result = runtime
        .block_on(run_coding_agent(
            &provider,
            &workspace_context(),
            checkout.path(),
            6,
            None,
        ))
        .expect("jig-backed coding agent succeeds");

    assert_eq!(result.verdict, None);
    assert!(result.summary.as_deref().unwrap_or("").contains("NOTES.md"));
    assert_eq!(
        fs::read_to_string(checkout.path().join("NOTES.md")).expect("NOTES.md was written"),
        "project notes\n"
    );

    let status = checkout.git(&["status", "--porcelain=v1", "--untracked-files=all"]);
    assert!(
        status.lines().any(|line| line == "?? NOTES.md"),
        "status was {status:?}"
    );
    assert!(
        !status.lines().any(|line| line.contains(".temper-")),
        "bookkeeping-only diff leaked into status: {status:?}"
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

/// The same scenario as above, but driven by the **native sans-IO agent loop**
/// (`run_coding_agent_native` → `smith_agent::run_sub_agent`) on the asupersync
/// runtime instead of pi's `Agent::run` on tokio. This is the path the worker
/// takes in production; it must produce the same product diff + result.
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
    let result = smith_io_engine::block_on(async move {
        run_coding_agent_native(&provider, &context, &cwd, 6, None).await
    })
    .expect("native jig-backed coding agent succeeds");

    assert_eq!(result.verdict, None);
    assert!(result.summary.as_deref().unwrap_or("").contains("NOTES.md"));
    assert_eq!(
        fs::read_to_string(checkout.path().join("NOTES.md")).expect("NOTES.md was written"),
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

fn coding_agent_fake(observed_continuation: Arc<AtomicUsize>) -> FakeLlm {
    FakeLlm::start(Script::rule(move |view| {
        if view.prior_tool_results == 0 {
            Reply {
                turns: vec![Turn::ToolCall {
                    id: "call_write_notes".to_string(),
                    name: "write".to_string(),
                    args: serde_json::json!({
                        "path": "NOTES.md",
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
        repository: WorkspaceRepository {
            id: "repo-1".to_string(),
            owner: "acme".to_string(),
            name: "demo".to_string(),
            default_branch: "main".to_string(),
        },
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
        base_branch: "main".to_string(),
        branch_hint: "agent/pr-for-code-25".to_string(),
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

struct TempCheckout {
    path: PathBuf,
}

impl TempCheckout {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "smith-{name}-{}-{}",
            std::process::id(),
            unique_nanos()
        ));
        fs::create_dir_all(&path).expect("create temp checkout");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn init_git(&self) {
        fs::write(self.path.join("README.md"), "# demo\n").expect("seed README");
        self.git(&["init", "-b", "main"]);
        self.git(&["config", "user.email", "jig@example.invalid"]);
        self.git(&["config", "user.name", "Jig Test"]);
        self.git(&["add", "README.md"]);
        self.git(&["commit", "-m", "seed"]);
    }

    fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(&self.path)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {:?} failed\nstdout:\n{}\nstderr:\n{}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("git stdout is utf8")
    }
}

impl Drop for TempCheckout {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn unique_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time after epoch")
        .as_nanos()
}
