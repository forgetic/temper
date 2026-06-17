//! Shared fixtures and filesystem helpers for the coding-agent unit tests.
//!
//! The hermetic tests in the sibling modules build `WorkspaceContext` values and
//! scratch directories from these helpers; nothing here touches the network or a
//! live provider.

use crate::coding_agent::*;

use std::path::{Path, PathBuf};

pub(super) const CONTEXT_FIXTURE: &str = r#"{
  "repos": [
    {
      "id": "repo-1",
      "owner": "acme",
      "name": "service",
      "default_branch": "main",
      "dir": "service",
      "access": "writable",
      "base_branch": "main",
      "branch_hint": "agent/pr-for-code-7"
    }
  ],
  "work_item": {
    "role": "engineer",
    "queue": "code_ready",
    "kind": "code",
    "target": "Issue { number: ItemNumber(7) }",
    "context": "{\"artifact\":{\"title\":\"Implement docs\"}}"
  },
  "action": "open_pr",
  "correlation_key": "pr-for-code-7",
  "checkout": "writable",
  "allowed_verdicts": ["needs_architect", "needs_human"],
  "guidance": {
    "role_guidance": "Make a real product change.",
    "tool_guidance": "Use docs/product-change.md for this fixture.",
    "tool_constraints": ["No .temper-only diffs."]
  }
}"#;

pub(super) fn parsed_fixture() -> WorkspaceContext {
    serde_json::from_str(CONTEXT_FIXTURE).expect("context fixture parses")
}

/// A one-writable-repo context whose repo is checked out at `dir` under the
/// workspace root, for `validate_contract` tests.
pub(super) fn context_with_writable_dir(dir: &str) -> WorkspaceContext {
    WorkspaceContext {
        repos: vec![WorkspaceRepository {
            id: "r".to_string(),
            owner: "o".to_string(),
            name: "n".to_string(),
            default_branch: "main".to_string(),
            dir: dir.to_string(),
            access: "writable".to_string(),
            base_branch: "main".to_string(),
            branch_hint: Some("agent/x".to_string()),
        }],
        work_item: WorkspaceWorkItem {
            role: "engineer".to_string(),
            queue: "code_ready".to_string(),
            kind: "code".to_string(),
            target: "Issue { number: ItemNumber(1) }".to_string(),
            context: "{}".to_string(),
        },
        action: "open_pr".to_string(),
        correlation_key: "x".to_string(),
        checkout: Some("writable".to_string()),
        allowed_verdicts: Vec::new(),
        guidance: WorkspaceGuidance::default(),
    }
}

/// The Claude Code identity string `required_system_identity()` returns for
/// Anthropic OAuth; used here only as an opaque non-empty marker for the
/// folding tests (no provider is built).
pub(super) const TEST_IDENTITY: &str = "test-claude-code-identity";

/// Creates a unique temp dir for one test (folds tag + pid to avoid collisions).
pub(super) fn overlay_temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("anvil-agent-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// Writes `body` to `dir/relative`, creating parents as needed.
pub(super) fn overlay_write(dir: &Path, relative: &str, body: &str) {
    let path = dir.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent dir");
    }
    std::fs::write(&path, body).expect("write file");
}
