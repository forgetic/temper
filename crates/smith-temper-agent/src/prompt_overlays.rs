//! Operator prompt overlays and repository `AGENTS.md` injection for the
//! coding-workspace agent.
//!
//! The coding agent ([`crate::coding_agent`]) builds a hard-coded per-role
//! system prompt. This module layers two *optional*, operator/repository-supplied
//! context sources on top of that built-in text, without checking any
//! role-specific production prompt into the repo (see
//! `docs/reference/development-conventions.md`):
//!
//! 1. **Operator prompt overlays** — Markdown files an operator drops into a
//!    config dir (default `$XDG_CONFIG_HOME/smith` else `~/.config/smith`). A
//!    per-role file (`prompts/architect.md`, `prompts/engineer.md`,
//!    `prompts/reviewer.md`) and an optional shared `prompts/coding-agent.md`
//!    are appended as an "Operator guidance" section. These live *outside* the
//!    repo, so the no-checked-in-prompts rule is satisfied.
//! 2. **Repository `AGENTS.md`** — the checkout's root `./AGENTS.md` (relative to
//!    the agent's cwd, which is the prepared checkout) is injected as a
//!    "Repository AGENTS.md" context block so the agent honors the repo's own
//!    conventions by default. Root only for the MVP; nested-`AGENTS.md` support
//!    is a possible follow-up.
//!
//! # Precedence
//!
//! The built-in role prompt always comes first; operator overlays layer on top
//! of it; the repository `AGENTS.md` comes last. The pieces are *additive
//! context*, not replacements: nothing here removes or rewrites the built-in
//! role contract.
//!
//! # Anthropic-OAuth folding
//!
//! Anthropic's subscription OAuth path rejects any request whose first `system`
//! block is not exactly the Claude Code identity (HTTP 429). The coding agent
//! therefore folds the role prompt into the *user* turn for that mode. The
//! overlay and `AGENTS.md` blocks must follow the same rule, so this module
//! exposes the two sections separately ([`PromptOverlays::operator_section`] and
//! [`PromptOverlays::agents_md_section`]) and a combined
//! [`PromptOverlays::render_for`] helper that places them in the system block or
//! the user turn depending on whether an identity is required.

use std::path::{Path, PathBuf};

use crate::coding_agent::Capability;

/// Env var that overrides the config dir outright.
pub const CONFIG_DIR_ENV: &str = "SMITH_CONFIG_DIR";
/// Standard XDG config-home env var consulted before `~/.config`.
const XDG_CONFIG_HOME_ENV: &str = "XDG_CONFIG_HOME";
/// Subdirectory under the config dir holding operator prompt overlays.
const PROMPTS_SUBDIR: &str = "prompts";
/// Shared overlay applied to every coding-agent role, if present.
const SHARED_OVERLAY_FILE: &str = "coding-agent.md";
/// Root `AGENTS.md` filename, read relative to the checkout cwd.
const AGENTS_MD_FILE: &str = "AGENTS.md";

/// Resolves the operator config directory from the process environment.
///
/// Precedence: an explicit `--config-dir` override (`explicit`), then the
/// `SMITH_CONFIG_DIR` env var, then `$XDG_CONFIG_HOME/smith`, then
/// `~/.config/smith`. Returns `None` only when none of those can be determined
/// (no override, no env, and no home directory) — a missing *directory* is not
/// an error here; it is handled as a clean no-op when overlays are loaded.
///
/// This is the thin wrapper that reads the live environment; the pure
/// precedence logic lives in [`resolve_config_dir_from`] so it can be tested
/// without mutating process-global env vars.
pub fn resolve_config_dir(explicit: Option<&Path>) -> Option<PathBuf> {
    resolve_config_dir_from(
        explicit,
        env_path(CONFIG_DIR_ENV),
        env_path(XDG_CONFIG_HOME_ENV),
        env_path("HOME"),
    )
}

/// Pure config-dir precedence resolution over already-read env values.
///
/// `config_env` is `SMITH_CONFIG_DIR`, `xdg_env` is `XDG_CONFIG_HOME`, and
/// `home` is `HOME` — each `None` when unset or empty (empty is treated as unset
/// per the XDG convention). Kept separate from the env read so it is testable
/// without `std::env::set_var` (which is `unsafe` on edition 2024 and rejected
/// by the repo's lints).
fn resolve_config_dir_from(
    explicit: Option<&Path>,
    config_env: Option<PathBuf>,
    xdg_env: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(dir) = explicit {
        return Some(dir.to_path_buf());
    }
    if let Some(dir) = config_env {
        return Some(dir);
    }
    if let Some(xdg) = xdg_env {
        return Some(xdg.join("smith"));
    }
    home.map(|home| home.join(".config").join("smith"))
}

/// Returns the value of `var` as a `PathBuf`, treating an empty value as unset
/// (matching the XDG convention that an empty `XDG_CONFIG_HOME` means "use the
/// default").
fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// The per-role operator overlay filename for a capability.
///
/// Architect (triage) and engineer (coding) are named in the issue;
/// reviewer (review) follows the same convention so the read-only review role
/// can also be tuned by an operator.
fn role_overlay_file(capability: Capability) -> &'static str {
    match capability {
        Capability::CodingWorkspace => "engineer.md",
        Capability::TriageWorkspace => "architect.md",
        Capability::ReviewWorkspace => "reviewer.md",
    }
}

/// The loaded overlay + repository context for one run.
///
/// Both fields are `None`/empty when the corresponding source is absent, so the
/// whole struct is a clean no-op for an operator who has configured nothing and
/// a checkout without an `AGENTS.md`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PromptOverlays {
    /// Operator overlay bodies in precedence order (shared first, then per-role),
    /// already trimmed and non-empty. Empty when no overlay files exist.
    operator_overlays: Vec<String>,
    /// The trimmed contents of the checkout's root `AGENTS.md`, if present and
    /// non-empty.
    agents_md: Option<String>,
}

impl PromptOverlays {
    /// Loads overlays + `AGENTS.md` for a capability.
    ///
    /// `config_dir` is the resolved operator config dir (see
    /// [`resolve_config_dir`]); `cwd` is the prepared checkout. Any missing dir
    /// or file is a clean no-op — only files that exist and have non-whitespace
    /// content contribute. I/O errors (unreadable file, permission denied) are
    /// treated as "absent" rather than surfaced: the overlays are best-effort
    /// extra context and must never fail a run.
    pub fn load(config_dir: Option<&Path>, cwd: &Path, capability: Capability) -> Self {
        let mut operator_overlays = Vec::new();
        if let Some(config_dir) = config_dir {
            let prompts_dir = config_dir.join(PROMPTS_SUBDIR);
            // Shared overlay first (lower precedence), then the per-role overlay.
            for file in [SHARED_OVERLAY_FILE, role_overlay_file(capability)] {
                if let Some(body) = read_trimmed(&prompts_dir.join(file)) {
                    operator_overlays.push(body);
                }
            }
        }
        let agents_md = read_trimmed(&cwd.join(AGENTS_MD_FILE));
        Self {
            operator_overlays,
            agents_md,
        }
    }

    /// True when nothing was loaded — neither operator overlays nor `AGENTS.md`.
    pub fn is_empty(&self) -> bool {
        self.operator_overlays.is_empty() && self.agents_md.is_none()
    }

    /// The "Operator guidance" section, or `None` when no overlay files exist.
    ///
    /// Rendered as a clearly-delimited block so it cannot be confused with the
    /// built-in role contract. The shared and per-role overlays are joined with a
    /// blank line in precedence order.
    pub fn operator_section(&self) -> Option<String> {
        if self.operator_overlays.is_empty() {
            return None;
        }
        Some(delimited(
            "Operator guidance",
            &self.operator_overlays.join("\n\n"),
        ))
    }

    /// The "Repository AGENTS.md" context block, or `None` when the checkout has
    /// no (non-empty) root `AGENTS.md`.
    pub fn agents_md_section(&self) -> Option<String> {
        self.agents_md
            .as_deref()
            .map(|body| delimited("Repository AGENTS.md", body))
    }

    /// The combined overlay text (operator guidance then repository `AGENTS.md`),
    /// or `None` when both are absent. Sections are separated by a blank line.
    pub fn combined_section(&self) -> Option<String> {
        let sections: Vec<String> = [self.operator_section(), self.agents_md_section()]
            .into_iter()
            .flatten()
            .collect();
        if sections.is_empty() {
            None
        } else {
            Some(sections.join("\n\n"))
        }
    }

    /// Places the overlay text into the right turn given a role prompt and an
    /// optional required system identity.
    ///
    /// Returns `(system, user_suffix)`:
    /// - When `required_identity` is `None`, the overlays are appended to the
    ///   `role_prompt` (which is the system block); `user_suffix` is empty.
    /// - When `required_identity` is `Some` (Anthropic OAuth), the role prompt is
    ///   already folded into the user turn by the caller, so the overlays must go
    ///   there too: `system` is returned unchanged (the role prompt) and the
    ///   overlay text is returned as the `user_suffix` to fold into the user turn.
    ///
    /// This mirrors how [`crate::coding_agent`] folds `role_prompt` itself.
    pub fn render_for(&self, role_prompt: &str, required_identity: bool) -> RenderedOverlays {
        match self.combined_section() {
            None => RenderedOverlays {
                system_prompt: role_prompt.to_string(),
                user_suffix: None,
            },
            Some(section) if required_identity => RenderedOverlays {
                system_prompt: role_prompt.to_string(),
                user_suffix: Some(section),
            },
            Some(section) => RenderedOverlays {
                system_prompt: format!("{role_prompt}\n\n{section}"),
                user_suffix: None,
            },
        }
    }
}

impl PromptOverlays {
    /// Composes the final `(system, user)` turns for one run.
    ///
    /// Given the built-in `role_prompt`, the `user_context`, and the optional
    /// `required_identity` (the mandatory first system block under Anthropic
    /// OAuth), this places the overlays in the correct turn:
    ///
    /// - No required identity: overlays are appended to the system prompt; the
    ///   user turn is the work-item context unchanged.
    /// - Required identity: the identity is the system prompt, and the role
    ///   prompt, work-item context, and overlays are all folded into the user
    ///   turn (mirroring how `crate::decision` folds the role prompt).
    ///
    /// This is the single entry point [`crate::coding_agent::run_coding_agent`]
    /// uses, so the folding rule lives in one tested place.
    pub fn compose_turns(
        &self,
        role_prompt: &str,
        user_context: &str,
        required_identity: Option<&str>,
    ) -> ComposedTurns {
        let rendered = self.render_for(role_prompt, required_identity.is_some());
        match required_identity {
            Some(identity) => {
                let mut user = format!("{}\n\n{user_context}", rendered.system_prompt);
                if let Some(suffix) = rendered.user_suffix {
                    user.push_str("\n\n");
                    user.push_str(&suffix);
                }
                ComposedTurns {
                    system: identity.to_string(),
                    user,
                }
            }
            None => ComposedTurns {
                system: rendered.system_prompt,
                user: user_context.to_string(),
            },
        }
    }
}

/// The final `(system, user)` turns for a run, as composed by
/// [`PromptOverlays::compose_turns`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComposedTurns {
    /// The system prompt to send (role prompt + overlays, or the required
    /// identity when folding).
    pub system: String,
    /// The user turn to send (work-item context, plus the folded role prompt +
    /// overlays under a required identity).
    pub user: String,
}

/// The result of [`PromptOverlays::render_for`]: the system prompt to send and
/// an optional suffix to fold into the user turn (Anthropic-OAuth mode).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderedOverlays {
    /// The system prompt (role prompt, with overlays appended when not folding).
    pub system_prompt: String,
    /// Overlay text to append to the user turn, or `None` when it went into the
    /// system prompt (or there was nothing to add).
    pub user_suffix: Option<String>,
}

/// Reads a file and returns its trimmed contents, or `None` when the file is
/// absent, unreadable, or blank. Errors are intentionally swallowed: overlay
/// context is best-effort and must never fail a run.
fn read_trimmed(path: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Wraps `body` in a clearly-delimited, titled block so injected context cannot
/// be confused with the built-in role contract.
fn delimited(title: &str, body: &str) -> String {
    format!("=== BEGIN {title} ===\n{body}\n=== END {title} ===")
}

#[cfg(test)]
#[path = "prompt_overlays_tests.rs"]
mod tests;
