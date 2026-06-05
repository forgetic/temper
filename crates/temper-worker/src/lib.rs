//! Forgejo-backed production worker wiring for reference-delivery workers.

pub mod worker;
pub mod worker_args;
mod worker_external_tools;
mod worker_role_agent;
mod worker_stop;
mod worker_tick;
