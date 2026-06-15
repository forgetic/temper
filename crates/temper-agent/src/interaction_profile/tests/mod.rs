//! Unit tests for the generic interaction-profile responder and its config.
//!
//! Split by domain responsibility: shared fixtures live in [`common`], responder
//! request/reply/render behavior in [`responder`], and config JSON validation in
//! [`config`].

mod common;

mod config;
mod responder;
