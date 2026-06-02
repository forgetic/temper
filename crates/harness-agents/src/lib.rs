//! Real, in-process LLM role agents for Harness.
//!
//! This crate is the **only** place the LLM SDK (`pi_agent_rust`, imported as
//! `pi`) lives. `harness-forge`, `harness-runner`, and `harness-workflow` stay
//! LLM-agnostic; a real workflow role agent is just a different
//! [`harness_runner::Agent`] implementation, selected like any other. The crate
//! also exposes a non-workflow product-manager conversational adapter that runs
//! one LLM turn and returns draft intake issues without mutating Forge state.
//!
//! ## Layout
//!
//! - [`provider`] — the one place LLM provider/model and **auth-mode** wiring
//!   live. Three modes: **`ApiKey`** (the default — DeepSeek behind the SDK's
//!   OpenAI-compatible route, key read at runtime), **`ChatGptOAuth`** (a
//!   ChatGPT/OpenAI-Codex subscription — provider `openai-codex`, bearer resolved
//!   fresh per decision from the shared `~/.pi/agent/auth.json` both pi CLIs
//!   write, tolerant of its dual on-disk schema, refreshed near expiry), and
//!   **`AnthropicOAuth`** (Anthropic OAuth subscription — provider `anthropic`,
//!   bearer from the same shared auth file plus Claude Code-compatible request
//!   headers). Swap the model, backend, or credential here.
//! - [`decision`] — runs a one-shot LLM turn through the SDK and parses the
//!   reply into a structured decision.
//! - [`role`] — a manifest-driven workflow role agent that uses a compiled
//!   [`harness_workflow::RoleManifest`] prompt/tool surface and an injectable
//!   decision seam.
//! - [`prompts`] — legacy workflow-role prompts plus the non-workflow
//!   conversational prompt embedded as data. Production workflow-role workers no
//!   longer use the legacy role prompt constants; the product-manager prompt is a
//!   separate conversational path.
//! - [`engineer`], [`architect`], [`reviewer`], [`owner`], [`human`] — legacy
//!   reference-delivery role agents kept for compatibility tests until the
//!   cleanup phase. Production workflow roles use [`role::LlmRoleAgent`].
//! - [`product_manager`] — a non-workflow conversational adapter: no
//!   [`harness_runner::Agent`] implementation, no workflow tools, and no Forge
//!   mutation; it returns a reply plus draft intake issues for an integration
//!   layer to file only after explicit human command.
//! - [`registry`] — production builders that register one generic agent per
//!   compiled workflow role, plus legacy reference-delivery builders for tests.
//!
//! New workflow roles use [`role::LlmRoleAgent`] with a compiled manifest; no
//! production role worker should import a checked-in workflow-role prompt.

#![allow(clippy::result_large_err)]

pub mod architect;
mod common;
pub mod decision;
pub mod engineer;
pub mod human;
pub mod owner;
pub mod product_manager;
pub mod prompts;
pub mod provider;
pub mod registry;
pub mod reviewer;
pub mod role;

pub use architect::{ArchitectDecision, LlmArchitect};
pub use engineer::{EngineerDecision, EngineerPrep, LlmEngineer, NoPrep};
pub use human::{HumanDecision, LlmHuman};
pub use owner::{LlmOwner, OwnerDecision};
pub use product_manager::{
    ProductManagerAgent, ProductManagerAuthor, ProductManagerConversationTurn,
    ProductManagerDraftIssue, ProductManagerError, ProductManagerRequest, ProductManagerResponse,
    is_valid_draft_slug,
};
pub use provider::{
    ANTHROPIC_MODEL_ENV, AuthChoice, DEFAULT_ANTHROPIC_MODEL, ProviderConfig, ProviderError,
    default_auth_path,
};
pub use registry::{
    RealRegistryConfig, real_registry, real_registry_from_compiled, real_registry_from_workflow,
    real_registry_with,
};
pub use reviewer::{LlmReviewer, ReviewerDecision};
pub use role::{LlmRoleAgent, ProviderRoleDecisionEngine, RoleDecision, RoleDecisionEngine};
