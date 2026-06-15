//! Low-level Forgejo REST operations outside the portable `Forge` trait.
//!
//! The org/repo provisioning REST bodies were absorbed into the
//! `temper-forge-forgejo` backend's [`ForgeContent`]/[`ForgeAdmin`] impls
//! (issue #179). This slim crate now only retains the free-function REST surface
//! still consumed directly by `temper-forgejo-provision`; issue #180 rewrites
//! that adapter onto the trait surface, after which this crate can be removed.

pub mod forgejo_rest;

pub use forgejo_rest::*;
