use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use jig_core::{Reply, Script, StopReason, Turn};
use jig_server::FakeLlm;
use temper_agent::{
    ProviderConfig, WorkspaceContext, WorkspaceGuidance, WorkspaceRepository, WorkspaceWorkItem,
    run_coding_agent_native,
};

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

