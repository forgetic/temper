// SPDX-License-Identifier: MPL-2.0

//! Durable target-era operator contract regression family.
//!
//! The facade stays intentionally thin. Responsibilities live in
//! `tests/target_ux/`; focused worker, agent, registry, transport, and webhook
//! suites remain the authority for their internal behavior.

#[allow(dead_code)]
#[path = "check_cli/support.rs"]
mod check_support;

#[path = "target_ux/compatibility.rs"]
mod compatibility;
#[path = "target_ux/deployment.rs"]
mod deployment;
#[path = "target_ux/onboarding.rs"]
mod onboarding;
#[path = "target_ux/runtime.rs"]
mod runtime;
#[path = "target_ux/support.rs"]
mod support;
#[path = "target_ux/webhook.rs"]
mod webhook;
