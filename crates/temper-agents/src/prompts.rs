//! Non-workflow LLM prompts as data.
//!
//! Workflow-role prompts are generated from compiled role manifests. This module
//! intentionally exposes only the product-manager interactive profile prompt,
//! which is not a workflow-role prompt and does not mutate Forge state.

/// The product-manager interactive profile prompt (see [`crate::product_manager`]).
pub const PRODUCT_MANAGER_SYSTEM_PROMPT: &str = include_str!("prompts/product_manager.md");
