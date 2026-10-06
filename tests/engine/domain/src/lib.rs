//! The walking root with real children, an ordered paged fake store, scripted
//! person and worker, independent durable-delivery referee and restart cuts.
pub mod commits;
pub mod walking;
pub mod walking_referee;

/// Real held-chat escalation driver.
pub mod escalation;
/// Independent escalation obligations.
pub mod escalation_referee;

/// Authenticated role-change driver over real held-chat records.
pub mod roles;
/// Independent role-cohort and recipient evidence.
pub mod roles_referee;
