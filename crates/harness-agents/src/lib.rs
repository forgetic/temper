//! Real, in-process LLM role agents for Harness.
//!
//! This crate is the **only** place the LLM SDK (`pi_agent_rust`, imported as
//! `pi`) lives. `harness-forge`, `harness-runner`, and `harness-workflow` stay
//! LLM-agnostic; a real agent is just a different [`harness_runner::Agent`]
//! implementation, selected like any other.
//!
//! ## Layout
//!
//! - [`provider`] — the one place LLM provider/model wiring and API-key loading
//!   live (DeepSeek behind the SDK's OpenAI-compatible route). Swap the model or
//!   backend here.
//! - [`decision`] — runs a one-shot LLM turn through the SDK and parses the
//!   reply into a structured decision.
//! - [`prompts`] — role system prompts embedded as data.
//! - [`engineer`] — the engineer role agent ([`engineer::LlmEngineer`]): the
//!   model decides, [`harness_runner::RoleTools`] mutates.
//!
//! Adding a role (Phase B2) means a new prompt file, a decision enum, and an
//! `impl Agent` adapter alongside [`engineer`]; the provider/decision plumbing
//! is shared.

#![allow(clippy::result_large_err)]

pub mod decision;
pub mod engineer;
pub mod prompts;
pub mod provider;

pub use engineer::{EngineerDecision, EngineerPrep, LlmEngineer, NoPrep};
pub use provider::{ProviderConfig, ProviderError};
