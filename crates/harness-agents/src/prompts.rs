//! Role prompts as data.
//!
//! Each role's system prompt is a checked-in Markdown file embedded at compile
//! time, so prompts can be tuned without touching control flow.

/// The engineer role's system prompt (see `service` in [`crate::engineer`]).
pub const ENGINEER_SYSTEM_PROMPT: &str = include_str!("prompts/engineer.md");
