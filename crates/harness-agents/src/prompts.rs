//! LLM prompts as data.
//!
//! Each role or conversational adapter prompt is a checked-in Markdown file
//! embedded at compile time, so prompts can be tuned without touching control
//! flow.

/// The engineer role's system prompt (see [`crate::engineer`]).
pub const ENGINEER_SYSTEM_PROMPT: &str = include_str!("prompts/engineer.md");

/// The architect role's system prompt (see [`crate::architect`]).
pub const ARCHITECT_SYSTEM_PROMPT: &str = include_str!("prompts/architect.md");

/// The reviewer role's system prompt (see [`crate::reviewer`]).
pub const REVIEWER_SYSTEM_PROMPT: &str = include_str!("prompts/reviewer.md");

/// The owner role's system prompt (see [`crate::owner`]).
pub const OWNER_SYSTEM_PROMPT: &str = include_str!("prompts/owner.md");

/// The human-stakeholder role's system prompt (see [`crate::human`]).
pub const HUMAN_SYSTEM_PROMPT: &str = include_str!("prompts/human.md");

/// The product-manager conversational prompt (see [`crate::product_manager`]).
pub const PRODUCT_MANAGER_SYSTEM_PROMPT: &str = include_str!("prompts/product_manager.md");
