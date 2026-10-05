//! The walking root with real children, an ordered paged fake store, scripted
//! person and worker, independent durable-delivery referee and restart cuts.
pub mod commits;
pub mod walking;
pub mod walking_referee;

/// Real held-chat escalation driver (domain/engine.md, section 7.7).
pub mod escalation;
/// Independent escalation obligations (domain/engine.md, section 7.7).
pub mod escalation_referee;

/// Authenticated role-change driver over real held-chat records (domain/engine.md, section 7.8).
pub mod roles;
/// Independent role-cohort and recipient evidence (domain/engine.md, section 7.8).
pub mod roles_referee;
