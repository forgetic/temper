//! Cohesive multi-file workspace patches for writable coding agents.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use async_trait::async_trait;
use serde::Deserialize;
use tongs::error::{Error, Result};
use tongs::tools::{Tool, ToolEffects, ToolOutput, ToolUpdate};

const MAX_PATCH_BYTES: usize = 2 * 1024 * 1024;

pub(super) struct ApplyPatchTool {
    cwd: PathBuf,
}

impl ApplyPatchTool {
    pub(super) fn new(cwd: &Path) -> Self {
        Self {
            cwd: cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf()),
        }
    }
}

#[derive(Deserialize)]
struct ApplyPatchInput {
    patch: String,
}

#[async_trait]
impl Tool for ApplyPatchTool {
    fn name(&self) -> &str {
        "apply_patch"
    }

    fn description(&self) -> &str {
        "Apply one cohesive unified Git patch to the workspace. Prefer this for planned \
         cross-file source, test, and documentation changes instead of one edit/write \
         turn per file. The patch is checked in full before application; absolute paths, \
         parent traversal, unsafe paths, malformed hunks, and partial application fail. \
         Input: { patch: string } containing `diff --git`, `---`/`+++`, and `@@` hunks."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "patch": {
                    "type": "string",
                    "description": "A unified Git patch relative to the workspace root"
                }
            },
            "required": ["patch"]
        })
    }

    fn effects(&self) -> ToolEffects {
        ToolEffects::write()
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        input: serde_json::Value,
        _on_update: Option<Box<dyn Fn(ToolUpdate) + Send + Sync>>,
    ) -> Result<ToolOutput> {
        let input: ApplyPatchInput = match serde_json::from_value(input) {
            Ok(input) => input,
            Err(error) => {
                return Ok(ToolOutput::error(format!(
                    "apply_patch: invalid input: {error}"
                )));
            }
        };
        if let Err(error) = validate_patch(&input.patch) {
            return Ok(ToolOutput::error(error.to_string()));
        }
        let cwd = self.cwd.clone();
        let patch = input.patch;
        let files = changed_file_count(&patch);
        // The registry wraps this filesystem tool in a dedicated joined owner
        // thread, so the blocking subprocess is contained outside the agent's
        // sans-I/O executor.
        if let Err(error) = git_apply(&cwd, &patch) {
            return Ok(ToolOutput::error(error));
        }
        Ok(ToolOutput::text(format!(
            "Applied cohesive patch across {files} file(s); all hunks applied"
        )))
    }
}

fn validate_patch(patch: &str) -> Result<()> {
    if patch.trim().is_empty() {
        return Err(Error::Tool("apply_patch: patch must not be empty".into()));
    }
    if patch.len() > MAX_PATCH_BYTES {
        return Err(Error::Tool(format!(
            "apply_patch: patch exceeds {MAX_PATCH_BYTES} bytes"
        )));
    }
    if !patch.lines().any(|line| line.starts_with("diff --git ")) {
        return Err(Error::Tool(
            "apply_patch: expected a unified Git patch with `diff --git` headers".into(),
        ));
    }
    Ok(())
}

fn git_apply(cwd: &Path, patch: &str) -> std::result::Result<(), String> {
    run_git_apply(cwd, patch, true)?;
    run_git_apply(cwd, patch, false)
}

fn run_git_apply(cwd: &Path, patch: &str, check: bool) -> std::result::Result<(), String> {
    let discovery_ceiling = cwd.parent().unwrap_or(cwd);
    let mut command = Command::new("git");
    command
        .current_dir(cwd)
        .env("GIT_CEILING_DIRECTORIES", discovery_ceiling)
        // Workspaces may contain one or more nested repositories and may also
        // live below an unrelated checkout. Apply relative to the authorized
        // workspace root instead of letting Git discover an ancestor repo.
        .args(["apply", "--whitespace=nowarn"]);
    if check {
        command.arg("--check");
    }
    let mut child = command
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("apply_patch: cannot start git apply: {error}"))?;
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(patch.as_bytes())
        .map_err(|error| format!("apply_patch: cannot send patch to git apply: {error}"))?;
    let output = child
        .wait_with_output()
        .map_err(|error| format!("apply_patch: cannot wait for git apply: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(format!(
        "apply_patch: git apply{} rejected the patch: {}",
        if check { " --check" } else { "" },
        stderr.trim()
    ))
}

fn changed_file_count(patch: &str) -> usize {
    patch
        .lines()
        .filter(|line| line.starts_with("diff --git "))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_shape_size_and_counts_files() {
        assert!(validate_patch("").is_err());
        assert!(validate_patch("--- a/a\n+++ b/a\n").is_err());
        let patch = "diff --git a/a b/a\ndiff --git a/b b/b\n";
        assert!(validate_patch(patch).is_ok());
        assert_eq!(changed_file_count(patch), 2);
    }

    #[test]
    fn applies_all_files_and_rejects_traversal_without_partial_changes() {
        let root = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(root.path())
            .status()
            .unwrap();
        let workspace = root.path().join("nested-workspace");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::write(workspace.join("one.txt"), "old one\n").unwrap();
        std::fs::write(workspace.join("two.txt"), "old two\n").unwrap();
        let valid = "diff --git a/one.txt b/one.txt\n--- a/one.txt\n+++ b/one.txt\n@@ -1 +1 @@\n-old one\n+new one\ndiff --git a/two.txt b/two.txt\n--- a/two.txt\n+++ b/two.txt\n@@ -1 +1 @@\n-old two\n+new two\n";
        git_apply(&workspace, valid).unwrap();
        assert_eq!(
            std::fs::read_to_string(workspace.join("one.txt")).unwrap(),
            "new one\n"
        );
        assert_eq!(
            std::fs::read_to_string(workspace.join("two.txt")).unwrap(),
            "new two\n"
        );

        let unsafe_patch = "diff --git a/one.txt b/../escape.txt\n--- a/one.txt\n+++ b/../escape.txt\n@@ -1 +1 @@\n-new one\n+escape\n";
        assert!(git_apply(&workspace, unsafe_patch).is_err());
        assert_eq!(
            std::fs::read_to_string(workspace.join("one.txt")).unwrap(),
            "new one\n"
        );
        assert!(!root.path().join("escape.txt").exists());
    }

    #[test]
    fn tool_applies_paths_from_a_multi_repository_workspace_root() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        let repository = workspace.join("repo");
        std::fs::create_dir_all(&repository).unwrap();
        std::fs::write(repository.join("one.txt"), "old\n").unwrap();
        let tool = ApplyPatchTool::new(&workspace);
        let patch = "diff --git a/repo/one.txt b/repo/one.txt\n--- a/repo/one.txt\n+++ b/repo/one.txt\n@@ -1 +1 @@\n-old\n+new\n";
        temper_agent_io::block_on_with(move |_cx, _handle| async move {
            tool.execute("patch", serde_json::json!({ "patch": patch }), None)
                .await
        })
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(repository.join("one.txt")).unwrap(),
            "new\n"
        );
    }

    #[test]
    fn joined_tool_applies_inside_the_agent_executor() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("one.txt"), "old\n").unwrap();
        let tool =
            temper_agent_core::joined_filesystem_tool(Box::new(ApplyPatchTool::new(root.path())));
        let patch = "diff --git a/one.txt b/one.txt\n--- a/one.txt\n+++ b/one.txt\n@@ -1 +1 @@\n-old\n+new\n";
        let output = temper_agent_io::block_on_with(move |_cx, _handle| async move {
            tool.execute("patch", serde_json::json!({ "patch": patch }), None)
                .await
        })
        .unwrap();
        assert!(!output.is_error, "joined output must succeed");
        assert_eq!(
            std::fs::read_to_string(root.path().join("one.txt")).unwrap(),
            "new\n"
        );
    }
}
