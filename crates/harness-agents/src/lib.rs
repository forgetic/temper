//! Real, in-process LLM role agents for Harness.
//!
//! This crate is the **only** place the LLM SDK (`pi_agent_rust`, imported as
//! `pi`) lives. `harness-forge`, `harness-runner`, and `harness-workflow` stay
//! LLM-agnostic; a real agent is just a different [`harness_runner::Agent`]
//! implementation, selected like any other.
//!
//! ## Layout
//!
//! - [`provider`] — the one place LLM provider/model and **auth-mode** wiring
//!   live. Two modes: **`ApiKey`** (the default — DeepSeek behind the SDK's
//!   OpenAI-compatible route, key read at runtime) and **`ChatGptOAuth`** (a
//!   ChatGPT/OpenAI-Codex subscription — provider `openai-codex`, bearer resolved
//!   fresh per decision from the shared `~/.pi/agent/auth.json` both pi CLIs
//!   write, tolerant of its dual on-disk schema, refreshed near expiry). Swap the
//!   model, backend, or credential here.
//! - [`decision`] — runs a one-shot LLM turn through the SDK and parses the
//!   reply into a structured decision.
//! - [`prompts`] — role system prompts embedded as data.
//! - [`engineer`], [`architect`], [`reviewer`], [`owner`], [`human`] — the role
//!   agents: the model decides, [`harness_runner::RoleTools`] mutates. Each is a
//!   thin adapter (prompt + decision enum + mapping); the [`common`],
//!   [`decision`], and [`provider`] plumbing is shared.
//! - [`registry`] — the [`registry::real_registry`] builder mapping every role to
//!   its LLM agent, mirroring the testing crate's `fake_registry`.
//!
//! Adding a role means a new prompt file, a decision enum, and an `impl Agent`
//! adapter; the provider/decision plumbing is shared.

#![allow(clippy::result_large_err)]

pub mod architect;
mod common;
pub mod decision;
pub mod engineer;
pub mod human;
pub mod owner;
pub mod prompts;
pub mod provider;
pub mod registry;
pub mod reviewer;

pub use architect::{ArchitectDecision, LlmArchitect};
pub use engineer::{EngineerDecision, EngineerPrep, LlmEngineer, NoPrep};
pub use human::{HumanDecision, LlmHuman};
pub use owner::{LlmOwner, OwnerDecision};
pub use provider::{AuthChoice, ProviderConfig, ProviderError, default_auth_path};
pub use registry::{RealRegistryConfig, real_registry, real_registry_with};
pub use reviewer::{LlmReviewer, ReviewerDecision};
