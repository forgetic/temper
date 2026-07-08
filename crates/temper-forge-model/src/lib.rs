//! Backend-agnostic Forge domain model and interface for Temper.
//!
//! This crate intentionally contains no concrete backend logic. Backends
//! implement [`Forge`] using the portable types in [`model`] and [`ids`].
//!
//! Provisioning is split into additive capability traits that sit alongside
//! [`Forge`] rather than widening it: [`ForgeContent`] (the portable half:
//! repositories, commits, branches), [`ForgeAdmin`] (the privileged half:
//! owners, users, tokens, access grants, webhooks, CI), and [`ForgeReadiness`]
//! (read-only provisioning inspection). A backend that implements all of them is
//! a [`ProvisioningForge`].

pub mod admin;
pub mod content;
pub mod forge;
pub mod hint;
pub mod ids;
pub mod inspect;
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
pub use inspect::{ForgeReadiness, ProvisionedUserStatus, WebhookStatus};
pub use model::*;

/// Convenience marker for backends that provide the full provisioning surface:
/// [`Forge`] plus both mutating capability traits ([`ForgeContent`] and
/// [`ForgeAdmin`]) and the read-only [`ForgeReadiness`] inspection surface.
///
/// A blanket impl covers every type that implements all four, so this trait
/// never needs to be implemented by hand; depend on it where the full
/// provisioning surface is required.
pub trait ProvisioningForge: Forge + ForgeContent + ForgeAdmin + ForgeReadiness {
    fn as_forge(&self) -> &dyn Forge;
    fn as_content(&self) -> &dyn ForgeContent;
    fn as_admin(&self) -> &dyn ForgeAdmin;
    fn as_readiness(&self) -> &dyn ForgeReadiness;
}

impl<T: Forge + ForgeContent + ForgeAdmin + ForgeReadiness> ProvisioningForge for T {
    fn as_forge(&self) -> &dyn Forge {
        self
    }

    fn as_content(&self) -> &dyn ForgeContent {
        self
    }

    fn as_admin(&self) -> &dyn ForgeAdmin {
        self
    }

    fn as_readiness(&self) -> &dyn ForgeReadiness {
        self
    }
}
