//! Prompt overlay integration: precedence (built-in → overlay → repo AGENTS.md)
//! and Anthropic-OAuth folding, exercised against the real `system_prompt`.
//! These mirror exactly how `run_coding_agent_native` assembles the effective
//! prompt.

use super::common::*;
use crate::codebase_memory::CodebaseMemoryToolMetadata;
use crate::coding_agent::*;
use crate::prompt_overlays::PromptOverlays;

#[test]
fn compose_turns_precedence_builtin_then_overlay_then_agents_md() {
    // engineer (coding) capability with both an operator overlay and a repo
    // AGENTS.md present. Without a required identity (e.g. ChatGPT OAuth /
    // DeepSeek) all three land in the system prompt in precedence order, and the
    // user turn is just the work-item context.
    let config_dir = overlay_temp_dir("precedence-config");
    let cwd = overlay_temp_dir("precedence-cwd");
    overlay_write(&config_dir, "prompts/engineer.md", "OPERATOR ENGINEER NOTE");
    overlay_write(&cwd, "AGENTS.md", "REPO AGENTS NOTE");

    let role_prompt = system_prompt(Capability::CodingWorkspace, &[]);
    let user = user_context(&parsed_fixture());
    let overlays = PromptOverlays::load(Some(&config_dir), &cwd, Capability::CodingWorkspace);
    let turns = overlays.compose_turns(&role_prompt, &user, None);

    let builtin_at = turns
        .system
        .find("ROLE: engineer")
        .expect("built-in present");
    let overlay_at = turns
        .system
        .find("OPERATOR ENGINEER NOTE")
        .expect("overlay present");
    let agents_at = turns
        .system
        .find("REPO AGENTS NOTE")
        .expect("AGENTS.md present");
    // Built-in role contract first, then operator overlay, then repo AGENTS.md.
    assert!(
        builtin_at < overlay_at,
        "built-in precedes operator overlay"
    );
    assert!(
        overlay_at < agents_at,
        "operator overlay precedes repo AGENTS.md"
    );
    // Not folding ⇒ the user turn is exactly the work-item context.
    assert_eq!(turns.user, user);

    let _ = std::fs::remove_dir_all(&config_dir);
    let _ = std::fs::remove_dir_all(&cwd);
}

#[test]
fn compose_turns_folds_into_user_turn_under_anthropic_identity() {
    // Under a required system identity (Anthropic OAuth) the overlay + AGENTS.md
    // must NOT appear in the system prompt; the system block is exactly the
    // identity, and the role prompt + overlays + work-item context fold into the
    // user turn, mirroring how the role prompt itself folds.
    let config_dir = overlay_temp_dir("fold-config");
    let cwd = overlay_temp_dir("fold-cwd");
    overlay_write(
        &config_dir,
        "prompts/architect.md",
        "OPERATOR ARCHITECT NOTE",
    );
    overlay_write(&cwd, "AGENTS.md", "REPO AGENTS NOTE");

    let role_prompt = system_prompt(Capability::TriageWorkspace, &[]);
    let user = user_context(&parsed_fixture());
    let overlays = PromptOverlays::load(Some(&config_dir), &cwd, Capability::TriageWorkspace);
    let turns = overlays.compose_turns(&role_prompt, &user, Some(TEST_IDENTITY));

    // The system block is exactly the identity — no role/overlay/AGENTS.md leak.
    assert_eq!(turns.system, TEST_IDENTITY);
    assert!(!turns.system.contains("ROLE: architect"));
    assert!(!turns.system.contains("OPERATOR ARCHITECT NOTE"));
    assert!(!turns.system.contains("REPO AGENTS NOTE"));

    // The role prompt, overlay, AGENTS.md, and work-item context all fold in.
    assert!(turns.user.contains("ROLE: architect"));
    assert!(turns.user.contains("OPERATOR ARCHITECT NOTE"));
    assert!(turns.user.contains("REPO AGENTS NOTE"));
    assert!(turns.user.contains("Work item context"));

    let _ = std::fs::remove_dir_all(&config_dir);
    let _ = std::fs::remove_dir_all(&cwd);
}

#[test]
fn compose_turns_folds_codebase_memory_guidance_under_anthropic_identity() {
    // Dynamic codebase-memory guidance is appended to the built-in role prompt
    // only after tools are actually registered. Under Anthropic OAuth it must
    // follow the same identity-only system block rule as the rest of the role
    // prompt: no extra system text, all guidance folded into the user turn.
    let cwd = overlay_temp_dir("fold-codebase-memory-cwd");
    let section = codebase_memory_prompt_section(&[CodebaseMemoryToolMetadata {
        name: "codebase_memory_search_code".to_string(),
        description: "OVERLAY-MCP-DESCRIPTION-SENTINEL-384".to_string(),
    }])
    .expect("metadata renders a codebase-memory prompt section");
    let role_prompt = format!(
        "{}{}",
        system_prompt(Capability::CodingWorkspace, &[]),
        section
    );
    let user = user_context(&parsed_fixture());
    let overlays = PromptOverlays::load(None, &cwd, Capability::CodingWorkspace);
    let turns = overlays.compose_turns(&role_prompt, &user, Some(TEST_IDENTITY));

    assert_eq!(turns.system, TEST_IDENTITY);
    assert!(!turns.system.contains("CODEBASE MEMORY"));
    assert!(turns.user.contains("ROLE: engineer"));
    assert!(turns.user.contains("CODEBASE MEMORY"));
    assert!(turns.user.contains("Use them early for non-trivial tasks"));
    assert!(
        turns
            .user
            .contains("Treat the graph as an index, not truth")
    );
    assert!(!turns.user.contains("codebase_memory_search_code"));
    assert!(!turns.user.contains("OVERLAY-MCP-DESCRIPTION-SENTINEL-384"));
    assert!(!turns.user.contains("Registered codebase-memory tools:"));
    assert!(turns.user.contains("Work item context"));

    let _ = std::fs::remove_dir_all(&cwd);
}

#[test]
fn compose_turns_absent_files_leave_role_prompt_unchanged() {
    // No config dir and no AGENTS.md ⇒ the system prompt is exactly the built-in
    // role prompt (no folding) and the user turn is just the work-item context.
    let cwd = overlay_temp_dir("absent-cwd");
    let role_prompt = system_prompt(Capability::ReviewWorkspace, &[]);
    let user = user_context(&parsed_fixture());
    let overlays = PromptOverlays::load(None, &cwd, Capability::ReviewWorkspace);

    let plain = overlays.compose_turns(&role_prompt, &user, None);
    assert_eq!(plain.system, role_prompt);
    assert_eq!(plain.user, user);

    // Under a required identity with no overlays, the role prompt still folds
    // into the user turn (the identity-only-first-block rule is unconditional).
    let folded = overlays.compose_turns(&role_prompt, &user, Some(TEST_IDENTITY));
    assert_eq!(folded.system, TEST_IDENTITY);
    assert!(folded.user.contains("ROLE: reviewer"));

    let _ = std::fs::remove_dir_all(&cwd);
}
