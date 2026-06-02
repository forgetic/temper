//! Real, in-process LLM role agents for Temper.
//!
//! This crate is the **only** place the LLM SDK (`pi_agent_rust`, imported as
//! `pi`) lives. `temper-forge`, `temper-runner`, `temper-workflow`, and
//! `temper-interaction` stay LLM-agnostic; a real workflow role agent is just a
//! different [`temper_runner::Agent`] implementation, selected like any other.
//! The crate also exposes a transitional in-process product-manager interactive
//! profile responder that runs one LLM turn and returns draft intake issues without
//! mutating Forge state.
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
//!   [`temper_workflow::RoleManifest`] prompt/tool surface, declared-and-bound
//!   external-tool metadata, and an injectable decision seam.
//! - [`prompts`] — the non-workflow product-manager conversational prompt
//!   embedded as data. Production workflow-role prompts are generated from role
//!   manifests; checked-in workflow-role prompt files do not live here.
//! - [`product_manager`] — a non-workflow interactive profile responder: no
//!   [`temper_runner::Agent`] implementation, no workflow tools, and no Forge
//!   mutation; it returns a reply plus draft intake issues for the interaction
//!   layer to file only after explicit human command.
//! - [`registry`] — production builders that validate external-tool bindings and
//!   register one generic agent per compiled workflow role.
//!
//! New workflow roles use [`role::LlmRoleAgent`] with a compiled manifest.
//! Generated prompts carry mechanics, user workflow config carries role
//! behavior, and external tools are visible only after explicit workflow
//! declarations plus runner bindings.

#![allow(clippy::result_large_err)]

mod common;
pub mod decision;
pub mod product_manager;
pub mod prompts;
pub mod provider;
pub mod registry;
pub mod role;

pub use product_manager::{
    PRODUCT_MANAGER_PROFILE_ID, ProductManagerAgent, ProductManagerAuthor,
    ProductManagerConversationTurn, ProductManagerDraftIssue, ProductManagerError,
    ProductManagerRequest, ProductManagerResponse, is_valid_draft_slug,
};
pub use provider::{
    ANTHROPIC_MODEL_ENV, AuthChoice, DEFAULT_ANTHROPIC_MODEL, ProviderConfig, ProviderError,
    default_auth_path,
};
pub use registry::{
    real_registry_from_compiled, real_registry_from_compiled_with_external_tool_executors,
    real_registry_from_compiled_with_external_tools, real_registry_from_workflow,
};
pub use role::{LlmRoleAgent, ProviderRoleDecisionEngine, RoleDecision, RoleDecisionEngine};
