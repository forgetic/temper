//! The walking root with real children, an ordered paged fake store, scripted
//! person and worker, independent durable-delivery referee and restart cuts.
pub mod commits;
pub mod walking;
pub mod walking_referee;

/// Real held-chat escalation driver (domain/engine.md, section 7.7).
pub mod escalation;
/// Independent escalation obligations (domain/engine.md, section 7.7).
pub mod escalation_referee;
