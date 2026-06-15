//! Operator CLI for the `temper provision-forgejo` reference-delivery demo
//! subcommand.
//!
//! This is **reference-delivery demo / operator tooling**, not a product
//! feature: `temper init` provisions a real deployment by inlining
//! `temper-provision` directly. This crate keeps the demo launcher / ignored
//! e2e operator path working: it builds a [`ForgejoForge`], distills a
//! [`ProvisionPlan`](temper_provision::ProvisionPlan) (with the demo CI seed
//! commits and, when requested, a webhook), runs the backend-agnostic
//! [`temper_provision::provision`] orchestration, seeds the demo intake issue,
//! and writes the per-role credentials to a `secrets.env` file.

pub mod provision;
pub mod provision_args;
