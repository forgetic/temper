//! Per-tool salient-argument previews for human-facing agent tool-call logs.
//!
//! Given a tool name and its JSON arguments, [`tool_arg_preview`] selects the
//! one argument worth showing on a single log line and renders it bounded to a
//! caller-supplied character budget. Paths are rendered repo-relative against
//! the workspace `cwd`; free-form bash command lines are redacted first. The
//! result is intended to slot into the `agent: [repo#n] tool <name> <preview>`
//! human projection (see `docs/explanation/logging-and-observability.md` §3).
//!
//! The production call site lives in `usage.rs`: the coding agent builds an
//! `ArgPreviewFn` (capturing the workspace `cwd`) that the pure agent core calls
//! to fill `ToolStart.arg_preview` (piece D of the agent-log-cleanup plan,
//! `docs/plans/agent-log-cleanup.md`).

use std::path::Path;

use serde_json::Value;

use crate::observability::{preview, redacted_preview};

/// Renders the salient argument of a tool call as a bounded single-line
/// preview, or `None` when there is nothing useful to show.
///
/// The final returned string (including any quoting) is at most `budget`
/// Unicode scalar values; if the salient value is longer it is truncated on a
/// char boundary with a trailing `…`.
///
/// Salient argument per tool:
/// - `read | write | edit | ls` → the `path` arg, repo-relative, unquoted.
/// - `find | grep` → the `pattern` arg, single-quoted (`'pattern'`).
/// - `bash` → the `command` arg, backtick-quoted and redacted.
/// - anything else → the first string-valued field in `args`, unquoted.
pub(crate) fn tool_arg_preview(
    name: &str,
    args: &Value,
    cwd: &Path,
    budget: usize,
) -> Option<String> {
    match name {
        "read" | "write" | "edit" | "ls" => {
            let raw = string_field(args, "path")?;
            let rel = repo_relative(&raw, cwd);
            bounded_plain(&rel, budget)
        }
        "find" | "grep" => {
            let raw = string_field(args, "pattern")?;
            bounded_quoted(&raw, '\'', '\'', budget, false)
        }
        "bash" => {
            let raw = string_field(args, "command")?;
            bounded_quoted(&raw, '`', '`', budget, true)
        }
        _ => {
            let raw = first_string_field(args)?;
            bounded_plain(&raw, budget)
        }
    }
}

/// Returns the trimmed string value at `key`, or `None` when absent/empty.
fn string_field(args: &Value, key: &str) -> Option<String> {
    let value = args.get(key)?.as_str()?;
    non_empty(value)
}

/// Returns the first string-valued field of an object, or `None`.
fn first_string_field(args: &Value) -> Option<String> {
    let object = args.as_object()?;
    object
        .values()
        .find_map(|value| value.as_str().and_then(non_empty))
}

/// Normalizes whitespace and returns the value only if non-empty after trim.
fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Renders an absolute path under `cwd` (or a `./`-prefixed path) as
/// repo-relative; paths outside the workspace are kept as-is.
fn repo_relative(path: &str, cwd: &Path) -> String {
    if let Some(cwd) = cwd.to_str()
        && let Some(rest) = path.strip_prefix(cwd)
    {
        return rest.trim_start_matches('/').to_string();
    }
    if let Some(rest) = path.strip_prefix("./") {
        return rest.to_string();
    }
    path.to_string()
}

/// Bounds an unquoted value to `budget` chars via the shared char-safe preview.
///
/// `preview` appends `…` *beyond* its char limit when it truncates, so cap one
/// scalar short to keep the result at or under `budget` in the worst case.
fn bounded_plain(value: &str, budget: usize) -> Option<String> {
    if budget == 0 {
        return None;
    }
    let rendered = preview(value, budget - 1);
    non_empty(&rendered)
}

/// Bounds a value to `budget` chars then wraps it in `open`/`close` quotes so
/// the closing quote is never lost to truncation. When `redact` is set the
/// value is run through the secret redactor before bounding.
fn bounded_quoted(
    value: &str,
    open: char,
    close: char,
    budget: usize,
    redact: bool,
) -> Option<String> {
    // `open` and `close` are each a single Unicode scalar; reserve two slots.
    let quote_scalars = 2;
    if budget <= quote_scalars {
        return None;
    }
    // `preview` appends `…` *in addition* to its char limit when it truncates,
    // so cap the inner preview one scalar short to keep the wrapped result at
    // or under `budget` in the worst case.
    let inner_budget = budget - quote_scalars - 1;
    let inner = if redact {
        redacted_preview(value, inner_budget)
    } else {
        preview(value, inner_budget)
    };
    let inner = non_empty(&inner)?;
    Some(format!("{open}{inner}{close}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn args(json: serde_json::Value) -> serde_json::Value {
        json
    }

    #[test]
    fn read_absolute_path_under_cwd_is_repo_relative() {
        let out = tool_arg_preview(
            "read",
            &args(serde_json::json!({"path": "/ws/temper/crates/x/src/y.rs"})),
            Path::new("/ws/temper"),
            200,
        );
        assert_eq!(out.as_deref(), Some("crates/x/src/y.rs"));
    }

    #[test]
    fn read_dot_slash_prefix_is_stripped() {
        let out = tool_arg_preview(
            "read",
            &args(serde_json::json!({"path": "./crates/x.rs"})),
            Path::new("/ws/temper"),
            200,
        );
        assert_eq!(out.as_deref(), Some("crates/x.rs"));
    }

    #[test]
    fn read_path_outside_cwd_is_kept_raw() {
        let out = tool_arg_preview(
            "read",
            &args(serde_json::json!({"path": "/etc/passwd"})),
            Path::new("/ws/temper"),
            200,
        );
        assert_eq!(out.as_deref(), Some("/etc/passwd"));
    }

    #[test]
    fn write_and_edit_and_ls_use_path() {
        let cwd = Path::new("/ws/temper");
        for tool in ["write", "edit", "ls"] {
            let out = tool_arg_preview(
                tool,
                &args(serde_json::json!({"path": "/ws/temper/a/b.rs"})),
                cwd,
                200,
            );
            assert_eq!(out.as_deref(), Some("a/b.rs"), "tool {tool}");
        }
    }

    #[test]
    fn bash_command_is_backtick_wrapped_and_redacted() {
        let out = tool_arg_preview(
            "bash",
            &args(serde_json::json!({
                "command": "curl --token=ghp_secretvalue123456 https://example.com"
            })),
            Path::new("/ws/temper"),
            200,
        )
        .expect("bash preview");
        assert!(out.starts_with('`') && out.ends_with('`'), "wrapped: {out}");
        assert!(
            !out.contains("ghp_secretvalue123456"),
            "secret leaked: {out}"
        );
        assert!(out.contains("curl"), "command body present: {out}");
    }

    #[test]
    fn grep_pattern_is_single_quoted() {
        let out = tool_arg_preview(
            "grep",
            &args(serde_json::json!({"pattern": "DEFAULT_POLL_CADENCE"})),
            Path::new("/ws/temper"),
            200,
        );
        assert_eq!(out.as_deref(), Some("'DEFAULT_POLL_CADENCE'"));
    }

    #[test]
    fn find_pattern_is_single_quoted() {
        let out = tool_arg_preview(
            "find",
            &args(serde_json::json!({"pattern": "*.rs"})),
            Path::new("/ws/temper"),
            200,
        );
        assert_eq!(out.as_deref(), Some("'*.rs'"));
    }

    #[test]
    fn unknown_tool_uses_first_string_field() {
        let out = tool_arg_preview(
            "mystery",
            &args(serde_json::json!({"count": 3, "label": "hello world"})),
            Path::new("/ws/temper"),
            200,
        );
        assert_eq!(out.as_deref(), Some("hello world"));
    }

    #[test]
    fn unknown_tool_without_string_field_is_none() {
        let out = tool_arg_preview(
            "mystery",
            &args(serde_json::json!({"count": 3, "flag": true})),
            Path::new("/ws/temper"),
            200,
        );
        assert_eq!(out, None);
    }

    #[test]
    fn non_object_args_for_unknown_tool_is_none() {
        let out = tool_arg_preview(
            "mystery",
            &args(serde_json::json!(["a", "b"])),
            Path::new("/ws/temper"),
            200,
        );
        assert_eq!(out, None);
    }

    #[test]
    fn missing_or_empty_value_is_none() {
        let cwd = Path::new("/ws/temper");
        assert_eq!(
            tool_arg_preview("read", &args(serde_json::json!({})), cwd, 200),
            None
        );
        assert_eq!(
            tool_arg_preview("read", &args(serde_json::json!({"path": "   "})), cwd, 200),
            None
        );
    }

    #[test]
    fn long_path_is_truncated_on_char_boundary_within_budget() {
        // Include a multibyte char so a naive byte-truncate would panic.
        let long = format!("/ws/temper/{}", "ⓧ".repeat(60));
        let out = tool_arg_preview(
            "read",
            &args(serde_json::json!({ "path": long })),
            Path::new("/ws/temper"),
            20,
        )
        .expect("preview");
        assert!(
            out.chars().count() <= 20,
            "len {}: {out}",
            out.chars().count()
        );
        assert!(out.ends_with('…'), "truncated marker: {out}");
    }

    #[test]
    fn long_bash_command_truncates_within_budget_keeping_quotes() {
        let long = format!("echo {}", "λ".repeat(80));
        let out = tool_arg_preview(
            "bash",
            &args(serde_json::json!({ "command": long })),
            Path::new("/ws/temper"),
            16,
        )
        .expect("preview");
        assert!(
            out.chars().count() <= 16,
            "len {}: {out}",
            out.chars().count()
        );
        assert!(out.starts_with('`'), "open backtick: {out}");
        assert!(out.ends_with('`'), "close backtick kept: {out}");
    }
}
