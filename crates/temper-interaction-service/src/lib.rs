//! Deployable interaction service transports and CLI bindings.

mod interaction_api;
pub mod interaction_args;
pub mod interaction_bindings;
pub mod interaction_commands;
mod interaction_http;
pub mod interaction_repl;
pub mod interaction_serve;
pub mod interaction_service;

#[cfg(test)]
mod dogfood_interaction_profile_tests;
#[cfg(test)]
mod interaction_args_tests;
#[cfg(test)]
mod interaction_repl_tests;
#[cfg(test)]
mod interaction_service_tests;
#[cfg(test)]
mod interaction_source_guard_tests;
