//! Reply parsing and role-contract validation for the coding-workspace run.
//!
//! [`super::run`] runs the agent loop; this module turns the model's final
//! message into a validated [`WorkspaceResult`] and enforces the per-role
//! product/verdict contract temper relies on.

use std::path::Path;

use temper_protocol_agent::{WorkspaceContext, WorkspaceResult};
use tongs::model::ContentBlock;

use super::{Capability, CodingAgentError};

/// Concatenates the assistant message's text blocks (ignoring thinking/tool
/// blocks).
pub(super) fn collect_text(content: &[ContentBlock]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

/// Parses the model's reply into a [`WorkspaceResult`], tolerating a code-fenced
/// or prose-wrapped JSON object. An empty / no-object reply is treated as an
/// empty head-path result (no verdict, no diff claim) so the contract check is
/// the single authority on whether that is acceptable.
///
/// A reply may contain several balanced `{...}` objects — prose the model wrote
/// while narrating the change (a `tracing::info!{…}` snippet, an example body)
/// can sit ahead of the real result envelope, which is emitted last. We try the
/// candidates last-first and accept the last one that deserializes into a
/// [`WorkspaceResult`], so a stray brace in the narration no longer hijacks the
/// parse. If none deserialize we surface the error from the last candidate (the
/// one most likely to have been the intended result).
pub(crate) fn parse_result(text: &str) -> Result<WorkspaceResult, CodingAgentError> {
    let candidates = extract_json_objects(text);
    if candidates.is_empty() {
        if text.trim().is_empty() {
            return Ok(WorkspaceResult::default());
        }
        return Err(CodingAgentError::Parse {
            snippet: snippet(text),
            error: "no JSON object found in reply".to_string(),
        });
    }
    let mut last_error = None;
    for candidate in candidates.iter().rev() {
        match serde_json::from_str::<WorkspaceResult>(candidate) {
            Ok(result) => return Ok(result),
            Err(error) => last_error.get_or_insert_with(|| error.to_string()),
        };
    }
    Err(CodingAgentError::Parse {
        snippet: snippet(text),
        error: last_error.unwrap_or_else(|| "no JSON object matched the result shape".to_string()),
    })
}

/// Rejects a verdict that is not in the action's declared vocabulary (W3).
///
/// When `allowed_verdicts` is non-empty, any verdict the model emits must be one
/// of them; the engine would otherwise fail the tick with an "undeclared
/// verdict" error, so we surface a clearer one here. A result with no verdict
/// (the engineer head path) and an empty `allowed_verdicts` (no declared
/// vocabulary, or an older temper) both pass through unchecked.
pub(crate) fn validate_verdict_vocabulary(
    result: &WorkspaceResult,
    allowed_verdicts: &[String],
) -> Result<(), CodingAgentError> {
    if allowed_verdicts.is_empty() {
        return Ok(());
    }
    let Some(verdict) = result
        .verdict
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    else {
        return Ok(());
    };
    if allowed_verdicts.iter().any(|allowed| allowed == verdict) {
        Ok(())
    } else {
        Err(CodingAgentError::UndeclaredVerdict {
            emitted: verdict.to_string(),
            allowed: allowed_verdicts.to_vec(),
        })
    }
}

/// Enforces the role contract that temper relies on: an engineer (writable)
/// run that emits no verdict must have left a real product diff in the working
/// tree, otherwise there is nothing to land. Read-only roles need a verdict.
pub(crate) fn validate_contract(
    capability: Capability,
    result: &WorkspaceResult,
    cwd: &Path,
    context: &WorkspaceContext,
) -> Result<(), CodingAgentError> {
    let has_verdict = result
        .verdict
        .as_deref()
        .map(|verdict| !verdict.trim().is_empty())
        .unwrap_or(false);

    match capability {
        Capability::CodingWorkspace => {
            if has_verdict {
                return Ok(());
            }
            // The cwd is the workspace root; a writable repo's product lives in
            // its own sibling dir. The run produced a product if ANY writable
            // repo has working-tree changes or a committed tree diff from its
            // base branch. An empty checkpoint commit can put HEAD ahead with
            // an identical tree, so commits-ahead alone is not product.
            let produced = context
                .repos
                .iter()
                .filter(|repo| repo.is_writable())
                .any(|repo| {
                    let dir = cwd.join(&repo.dir);
                    working_tree_has_changes(&dir)
                        || tree_differs_from_base(&dir, &repo.base_branch)
                });
            if produced {
                Ok(())
            } else {
                Err(CodingAgentError::NoProduct)
            }
        }
        Capability::TriageWorkspace | Capability::ReviewWorkspace => {
            if has_verdict {
                Ok(())
            } else {
                Err(CodingAgentError::AgentStopped(
                    "read-only role finished without emitting a verdict".to_string(),
                ))
            }
        }
    }
}

/// Returns true when `HEAD`'s tree differs from `origin/<base_branch>` in
/// `cwd` (checkpoint commits a phase-6b run pushed mid-run). Falls back to
/// `false` when git cannot answer, leaving the working-tree check decisive.
fn tree_differs_from_base(cwd: &Path, base_branch: &str) -> bool {
    let base_branch = base_branch.trim();
    if base_branch.is_empty() {
        return false;
    }
    std::process::Command::new("git")
        .arg("diff")
        .arg("--name-only")
        .arg(format!("origin/{base_branch}"))
        .arg("HEAD")
        .current_dir(cwd)
        .output()
        .map(|output| output.status.success() && !output.stdout.is_empty())
        .unwrap_or(false)
}

/// Returns true when `git status --porcelain` reports any change in `cwd`.
/// Falls back to `false` when git cannot be invoked, which the contract check
/// then surfaces as [`CodingAgentError::NoProduct`].
fn working_tree_has_changes(cwd: &Path) -> bool {
    std::process::Command::new("git")
        .arg("status")
        .arg("--porcelain=v1")
        .arg("--untracked-files=all")
        .current_dir(cwd)
        .output()
        .map(|output| output.status.success() && !output.stdout.is_empty())
        .unwrap_or(false)
}

/// Returns every balanced top-level `{...}` substring, in source order. Nested
/// objects are folded into their enclosing top-level object (we only emit a
/// candidate when depth returns to zero), so a `WorkspaceResult` with `children`
/// stays a single candidate. Shares the brace-matching logic with
/// [`crate::decision`] but is kept local to avoid a cross-module dependency on a
/// private helper.
fn extract_json_objects(text: &str) -> Vec<String> {
    let mut objects = Vec::new();
    let mut depth = 0usize;
    let mut start = None;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, ch) in text.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => {
                if depth == 0 {
                    start = Some(offset);
                }
                depth += 1;
            }
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0
                    && let Some(start) = start.take()
                {
                    objects.push(text[start..=offset].to_string());
                }
            }
            _ => {}
        }
    }
    objects
}

/// A short, single-line snippet of the model reply for error messages.
fn snippet(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.len() > 200 {
        format!("{}…", &collapsed[..200])
    } else {
        collapsed
    }
}
