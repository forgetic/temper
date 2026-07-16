//! Hermetic unit tests for the coding-workspace agent: context parsing,
//! prompt construction, role→capability/tool selection, and result
//! serialization. No network or live provider is touched here; the live agent
//! loop is exercised by the gated e2e in the CLI crate.
//!
//! Split by domain responsibility: shared fixtures live in [`common`], with one
//! module per concern.

mod common;

mod capability;
mod context;
mod effective_prompts;
mod overlays;
mod prompt;
mod result;
mod run_errors;
