//! Backend-agnostic Forge domain model and interface for Temper.
//!
//! This crate intentionally contains no concrete backend logic. Backends
//! implement [`Forge`] using the portable types in [`model`] and [`ids`].
//!
//! Provisioning is split into two additive capability traits that sit alongside
//! [`Forge`] rather than widening it: [`ForgeContent`] (the portable half:
//! repositories, commits, branches) and [`ForgeAdmin`] (the privileged half:
//! owners, users, tokens, access grants, webhooks, CI). A backend that
//! implements all three is a [`ProvisioningForge`].

pub mod admin;
pub mod content;
pub mod forge;
pub mod hint;
pub mod ids;
pub mod model;

pub use admin::{AccessGrant, ForgeAdmin, NewUser, TokenScope, WebhookSpec};
pub use content::{CommitFile, CreateBranch, EnsureRepository, ForgeContent};
pub use forge::{
    CiJobQuery, CiJobSort, CiJobSortField, Forge, ForgeError, ForgeResult, IssueQuery,
    ItemListDetails, ItemSort, ItemSortField, PullRequestQuery, RepositoryQuery, RepositorySort,
    RepositorySortField, SortDirection,
};
pub use hint::*;
pub use ids::*;
pub use model::*;

/// Convenience marker for backends that provide the full provisioning surface:
/// [`Forge`] plus both capability traits ([`ForgeContent`] and [`ForgeAdmin`]).
///
/// A blanket impl covers every type that implements all three, so this trait
/// never needs to be implemented by hand; depend on it where the full
/// provisioning surface is required.
pub trait ProvisioningForge: Forge + ForgeContent + ForgeAdmin {}

impl<T: Forge + ForgeContent + ForgeAdmin> ProvisioningForge for T {}
