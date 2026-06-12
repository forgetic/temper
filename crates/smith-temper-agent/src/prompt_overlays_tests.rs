//! Hermetic tests for operator prompt overlays + repository `AGENTS.md`
//! injection. Each test that touches the filesystem uses a unique temp dir; the
//! config-dir precedence is tested over the pure `resolve_config_dir_from` so no
//! process-global env var is mutated (env mutation is `unsafe` on edition 2024
//! and rejected by the repo's lints).

use std::path::{Path, PathBuf};

use super::*;
use crate::coding_agent::Capability;

/// Creates a unique temp dir for one test and returns its path. The name folds
/// in the test-supplied tag and the process id so parallel tests never collide.
fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "smith-prompt-overlays-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// Writes `body` to `dir/relative`, creating parent dirs as needed.
fn write_file(dir: &Path, relative: &str, body: &str) {
    let path = dir.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent dir");
    }
    std::fs::write(&path, body).expect("write file");
}

// ---------------------------------------------------------------------------
// resolve_config_dir_from (pure precedence over already-read env values)
// ---------------------------------------------------------------------------

#[test]
fn resolve_config_dir_prefers_explicit_override() {
    // The explicit `--config-dir` override wins over every env source.
    let explicit = PathBuf::from("/explicit/dir");
    let resolved = resolve_config_dir_from(
        Some(&explicit),
        Some(PathBuf::from("/from/env")),
        Some(PathBuf::from("/xdg")),
        Some(PathBuf::from("/home/tester")),
    )
    .expect("override resolves");
    assert_eq!(resolved, explicit);
}

#[test]
fn resolve_config_dir_reads_smith_config_dir_env() {
    // With no explicit override, SMITH_CONFIG_DIR is used verbatim (no /smith
    // suffix — the env names the dir directly).
    let resolved = resolve_config_dir_from(
        None,
        Some(PathBuf::from("/env/smith")),
        Some(PathBuf::from("/xdg")),
        Some(PathBuf::from("/home/tester")),
    )
    .expect("env resolves");
    assert_eq!(resolved, PathBuf::from("/env/smith"));
}

#[test]
fn resolve_config_dir_falls_back_to_xdg_then_home() {
    // No explicit override, no SMITH_CONFIG_DIR: $XDG_CONFIG_HOME/smith wins.
    let resolved = resolve_config_dir_from(
        None,
        None,
        Some(PathBuf::from("/xdg/config")),
        Some(PathBuf::from("/home/tester")),
    )
    .expect("xdg resolves");
    assert_eq!(resolved, PathBuf::from("/xdg/config/smith"));

    // With XDG also unset, fall back to ~/.config/smith.
    let resolved = resolve_config_dir_from(None, None, None, Some(PathBuf::from("/home/tester")))
        .expect("home resolves");
    assert_eq!(resolved, PathBuf::from("/home/tester/.config/smith"));

    // Nothing resolvable at all ⇒ None (handled as a clean no-op by callers).
    assert!(resolve_config_dir_from(None, None, None, None).is_none());
}

// ---------------------------------------------------------------------------
// PromptOverlays::load — absent files are a clean no-op
// ---------------------------------------------------------------------------

#[test]
fn load_with_no_config_dir_and_no_agents_md_is_empty() {
    let cwd = temp_dir("empty");
    let overlays = PromptOverlays::load(None, &cwd, Capability::CodingWorkspace);
    assert!(overlays.is_empty());
    assert!(overlays.operator_section().is_none());
    assert!(overlays.agents_md_section().is_none());
    assert!(overlays.combined_section().is_none());
    let _ = std::fs::remove_dir_all(&cwd);
}

#[test]
fn load_missing_config_dir_is_a_clean_no_op() {
    let cwd = temp_dir("missing-config");
    // A config dir that does not exist contributes nothing (no panic, no error).
    let config_dir = cwd.join("does-not-exist");
    let overlays = PromptOverlays::load(Some(&config_dir), &cwd, Capability::TriageWorkspace);
    assert!(overlays.is_empty());
    let _ = std::fs::remove_dir_all(&cwd);
}

#[test]
fn load_blank_overlay_and_blank_agents_md_are_ignored() {
    let config_dir = temp_dir("blank-config");
    let cwd = temp_dir("blank-cwd");
    write_file(&config_dir, "prompts/engineer.md", "   \n\t\n");
    write_file(&cwd, "AGENTS.md", "\n\n  \n");
    let overlays = PromptOverlays::load(Some(&config_dir), &cwd, Capability::CodingWorkspace);
    assert!(
        overlays.is_empty(),
        "whitespace-only files contribute nothing"
    );
    let _ = std::fs::remove_dir_all(&config_dir);
    let _ = std::fs::remove_dir_all(&cwd);
}

// ---------------------------------------------------------------------------
// Per-role overlay selection
// ---------------------------------------------------------------------------

#[test]
fn engineer_overlay_applies_for_coding_capability() {
    let config_dir = temp_dir("engineer-overlay");
    let cwd = temp_dir("engineer-cwd");
    write_file(&config_dir, "prompts/engineer.md", "ENGINEER OVERLAY BODY");
    write_file(
        &config_dir,
        "prompts/architect.md",
        "ARCHITECT OVERLAY BODY",
    );

    let overlays = PromptOverlays::load(Some(&config_dir), &cwd, Capability::CodingWorkspace);
    let section = overlays
        .operator_section()
        .expect("engineer overlay present");
    assert!(section.contains("Operator guidance"));
    assert!(section.contains("ENGINEER OVERLAY BODY"));
    // The architect overlay must NOT leak into an engineer run.
    assert!(!section.contains("ARCHITECT OVERLAY BODY"));
    let _ = std::fs::remove_dir_all(&config_dir);
    let _ = std::fs::remove_dir_all(&cwd);
}

#[test]
fn architect_overlay_applies_for_triage_capability() {
    let config_dir = temp_dir("architect-overlay");
    let cwd = temp_dir("architect-cwd");
    write_file(
        &config_dir,
        "prompts/architect.md",
        "ARCHITECT OVERLAY BODY",
    );
    write_file(&config_dir, "prompts/engineer.md", "ENGINEER OVERLAY BODY");

    let overlays = PromptOverlays::load(Some(&config_dir), &cwd, Capability::TriageWorkspace);
    let section = overlays
        .operator_section()
        .expect("architect overlay present");
    assert!(section.contains("ARCHITECT OVERLAY BODY"));
    assert!(!section.contains("ENGINEER OVERLAY BODY"));
    let _ = std::fs::remove_dir_all(&config_dir);
    let _ = std::fs::remove_dir_all(&cwd);
}

#[test]
fn reviewer_overlay_applies_for_review_capability() {
    let config_dir = temp_dir("reviewer-overlay");
    let cwd = temp_dir("reviewer-cwd");
    write_file(&config_dir, "prompts/reviewer.md", "REVIEWER OVERLAY BODY");

    let overlays = PromptOverlays::load(Some(&config_dir), &cwd, Capability::ReviewWorkspace);
    let section = overlays
        .operator_section()
        .expect("reviewer overlay present");
    assert!(section.contains("REVIEWER OVERLAY BODY"));
    let _ = std::fs::remove_dir_all(&config_dir);
    let _ = std::fs::remove_dir_all(&cwd);
}

#[test]
fn shared_and_role_overlays_compose_in_order() {
    let config_dir = temp_dir("shared-overlay");
    let cwd = temp_dir("shared-cwd");
    write_file(&config_dir, "prompts/coding-agent.md", "SHARED BODY");
    write_file(&config_dir, "prompts/engineer.md", "ROLE BODY");

    let overlays = PromptOverlays::load(Some(&config_dir), &cwd, Capability::CodingWorkspace);
    let section = overlays.operator_section().expect("overlays present");
    let shared_at = section.find("SHARED BODY").expect("shared present");
    let role_at = section.find("ROLE BODY").expect("role present");
    // Shared overlay comes first (lower precedence), then the per-role overlay.
    assert!(
        shared_at < role_at,
        "shared overlay precedes the role overlay"
    );
    let _ = std::fs::remove_dir_all(&config_dir);
    let _ = std::fs::remove_dir_all(&cwd);
}

// ---------------------------------------------------------------------------
// AGENTS.md injection
// ---------------------------------------------------------------------------

#[test]
fn agents_md_is_injected_from_checkout_root() {
    let cwd = temp_dir("agents-md-cwd");
    write_file(&cwd, "AGENTS.md", "# Repo conventions\nUse tabs.");

    let overlays = PromptOverlays::load(None, &cwd, Capability::CodingWorkspace);
    let section = overlays.agents_md_section().expect("AGENTS.md injected");
    assert!(section.contains("Repository AGENTS.md"));
    assert!(section.contains("# Repo conventions"));
    assert!(section.contains("Use tabs."));
    let _ = std::fs::remove_dir_all(&cwd);
}

#[test]
fn combined_section_orders_operator_guidance_before_agents_md() {
    let config_dir = temp_dir("combined-config");
    let cwd = temp_dir("combined-cwd");
    write_file(&config_dir, "prompts/engineer.md", "OPERATOR BODY");
    write_file(&cwd, "AGENTS.md", "AGENTS BODY");

    let overlays = PromptOverlays::load(Some(&config_dir), &cwd, Capability::CodingWorkspace);
    let combined = overlays.combined_section().expect("combined present");
    let operator_at = combined.find("OPERATOR BODY").expect("operator present");
    let agents_at = combined.find("AGENTS BODY").expect("agents present");
    // Precedence: built-in (in coding_agent) → operator overlay → repo AGENTS.md.
    assert!(
        operator_at < agents_at,
        "operator guidance precedes repository AGENTS.md"
    );
    let _ = std::fs::remove_dir_all(&config_dir);
    let _ = std::fs::remove_dir_all(&cwd);
}

// ---------------------------------------------------------------------------
// render_for — Anthropic-OAuth folding
// ---------------------------------------------------------------------------

#[test]
fn render_appends_to_system_prompt_when_identity_not_required() {
    let config_dir = temp_dir("render-system-config");
    let cwd = temp_dir("render-system-cwd");
    write_file(&config_dir, "prompts/engineer.md", "OVERLAY BODY");
    write_file(&cwd, "AGENTS.md", "AGENTS BODY");
    let overlays = PromptOverlays::load(Some(&config_dir), &cwd, Capability::CodingWorkspace);

    let rendered = overlays.render_for("ROLE PROMPT", false);
    // Not folding: overlays land in the system prompt; nothing folds into user.
    assert!(rendered.system_prompt.contains("ROLE PROMPT"));
    assert!(rendered.system_prompt.contains("OVERLAY BODY"));
    assert!(rendered.system_prompt.contains("AGENTS BODY"));
    assert!(rendered.user_suffix.is_none());
    let _ = std::fs::remove_dir_all(&config_dir);
    let _ = std::fs::remove_dir_all(&cwd);
}

#[test]
fn render_folds_into_user_turn_under_required_identity() {
    let config_dir = temp_dir("render-fold-config");
    let cwd = temp_dir("render-fold-cwd");
    write_file(&config_dir, "prompts/engineer.md", "OVERLAY BODY");
    write_file(&cwd, "AGENTS.md", "AGENTS BODY");
    let overlays = PromptOverlays::load(Some(&config_dir), &cwd, Capability::CodingWorkspace);

    let rendered = overlays.render_for("ROLE PROMPT", true);
    // Folding (Anthropic OAuth): the system prompt is just the role prompt; the
    // overlays + AGENTS.md must go into the user turn, never the system block.
    assert_eq!(rendered.system_prompt, "ROLE PROMPT");
    let suffix = rendered.user_suffix.expect("overlays fold into user turn");
    assert!(suffix.contains("OVERLAY BODY"));
    assert!(suffix.contains("AGENTS BODY"));
    let _ = std::fs::remove_dir_all(&config_dir);
    let _ = std::fs::remove_dir_all(&cwd);
}

#[test]
fn render_with_no_overlays_returns_role_prompt_unchanged() {
    let cwd = temp_dir("render-empty-cwd");
    let overlays = PromptOverlays::load(None, &cwd, Capability::CodingWorkspace);
    // Absent overlays: role prompt unchanged in both folding modes, no suffix.
    let plain = overlays.render_for("ROLE PROMPT", false);
    assert_eq!(plain.system_prompt, "ROLE PROMPT");
    assert!(plain.user_suffix.is_none());
    let folded = overlays.render_for("ROLE PROMPT", true);
    assert_eq!(folded.system_prompt, "ROLE PROMPT");
    assert!(folded.user_suffix.is_none());
    let _ = std::fs::remove_dir_all(&cwd);
}
