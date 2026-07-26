//! Top-level Forge facade for Temper.
//!
//! This crate is the single entry point every other (non-test) crate uses to
//! talk to a forge. It does two things:
//!
//! 1. **Re-exports the backend-agnostic model** ([`temper_forge_model`]) wholesale,
//!    so callers write `temper_forge::Forge`, `temper_forge::Issue`, etc. and
//!    never depend on the model crate directly.
//!
//! 2. **Offers a backend factory** ([`factory`]) that constructs a concrete
//!    backend (Forgejo, GitHub, in-memory, filesystem) and hands it back as an
//!    `Arc<dyn Forge>`. For local backends it can also return a companion
//!    [`ChangeSource`](crate::ChangeSource) alongside that abstract forge.
//!    Callers pick a backend by *which function they call*, never by naming a
//!    concrete backend type.
//!
//! ## The architectural rule this crate exists to enforce
//!
//! `temper-forge` is the ONLY non-test crate permitted to depend on the
//! concrete backend crates (`temper-forge-forgejo`, `temper-forge-github`,
//! `temper-forge-memory`, `temper-forge-filesystem`). Everything downstream
//! depends on `temper-forge` and receives an abstract `Arc<dyn Forge>`, so the
//! pipeline stays provider-agnostic and a second backend can be swapped in
//! without touching logic. This is enforced in CI by `depgraph-rules.toml`.
//!
//! The sole exception is `temper-testing`, the sim/e2e harness, which keeps a
//! direct dependency on the backends for test-only affordances that are not part
//! of the production `Forge` trait (e.g. `MemoryForge::seed_ci_jobs`,
//! `FilesystemForge::as_user`). That exception is documented in the depgraph
//! rules.

// Re-export the entire backend-agnostic model: the `Forge` trait, domain types,
// ids, and value types. Downstream code uses these through `temper_forge::…`.
pub use temper_forge_model::*;

/// Backend construction parameters and provisioning helpers.
///
/// These are *configuration* (and a couple of provider-specific provisioning
/// helpers), not forge *behavior*: they describe how to build a backend, while
/// the resulting [`Forge`] is always handed back abstractly by [`factory`]. They
/// are re-exported here so wiring crates can express backend options (default
/// repo, conditional-write mode, …) without depending on a backend crate.
pub mod config {
    /// Forgejo backend configuration ([`new`](ForgejoConfig::new) plus
    /// `with_*` builder options) and conditional-write mode.
    pub use temper_forge_forgejo::{CasMode as ForgejoCasMode, ForgejoConfig};
    /// GitHub backend configuration.
    pub use temper_forge_github::{CasMode as GitHubCasMode, GitHubConfig};

    /// Forgejo provisioning helpers used by the operator/first-run flows: the
    /// well-known role password and the admin-token mint over HTTP Basic auth.
    /// These are provider-specific by nature (provisioning is not part of the
    /// portable `Forge` trait), surfaced here so provisioning crates need not
    /// depend on the backend crate.
    pub use temper_forge_forgejo::{
        ROLE_PASSWORD as FORGEJO_ROLE_PASSWORD,
        admin_token_via_basic_auth as forgejo_admin_token_via_basic_auth,
    };
}

/// Backend factory: construct a concrete forge, get back an `Arc<dyn Forge>`.
///
/// The plain constructors return the abstract trait object, so callers select a
/// backend by *which constructor they call* and never name a concrete backend
/// type. Local backends also have `*_with_change_source` constructors for tests
/// and composition code that need a lossy wake accelerator without depending on
/// backend internals; the returned [`Forge`] remains the authoritative state.
pub mod factory {
    use std::path::PathBuf;
    use std::sync::Arc;

    use temper_forge_model::{ChangeSource, Forge, InspectionForge, ProvisioningForge};

    /// Abstract forge plus an optional-backend companion change source.
    ///
    /// The source is a latency accelerator only: consumers must route its hints
    /// into their normal scan path and re-read [`Forge`] state before acting.
    pub struct ForgeWithChangeSource {
        pub forge: Arc<dyn Forge>,
        pub change_source: Box<dyn ChangeSource + Send>,
    }

    use crate::config::{ForgejoConfig, GitHubConfig};

    /// Build a Forgejo-backed [`Forge`] from a [`ForgejoConfig`].
    ///
    /// Construct the config with [`crate::config::ForgejoConfig::new`] and the
    /// `with_*` builders (default repo, page limit, conditional-write mode).
    pub fn new_forgejo(config: ForgejoConfig) -> Arc<dyn Forge> {
        Arc::new(temper_forge_forgejo::ForgejoForge::new(config))
    }

    /// Build a token-authenticated Forgejo backend for read-only planning.
    ///
    /// Unlike [`new_forgejo_provisioning`], the returned facade exposes only
    /// ordinary Forge operations and readiness inspection; it has no content or
    /// admin adapter.
    pub fn new_forgejo_inspection(config: ForgejoConfig) -> Arc<dyn InspectionForge> {
        Arc::new(temper_forge_forgejo::ForgejoForge::new(config))
    }

    /// Build a mutation-proof Forgejo inspection backend using HTTP Basic auth.
    ///
    /// This is for generated pre-apply bundles that have the configured admin
    /// login/password but no token yet. The concrete backend remains hidden,
    /// and its HTTP boundary rejects every non-GET request locally.
    pub fn new_forgejo_read_only_basic(
        base_url: impl Into<String>,
        login: impl AsRef<str>,
        password: impl AsRef<str>,
    ) -> Arc<dyn InspectionForge> {
        Arc::new(temper_forge_forgejo::ForgejoForge::new_read_only_basic(
            base_url, login, password,
        ))
    }

    /// Build a Forgejo-backed forge exposing the full provisioning surface
    /// ([`ProvisioningForge`] = [`Forge`] + content + admin).
    ///
    /// Provisioning needs all three capability traits at once (e.g.
    /// `temper_provision::provision` takes `&dyn Forge`, `&dyn ForgeContent`,
    /// and `&dyn ForgeAdmin`). The returned trait object exposes stable adapter
    /// methods for each capability, so provisioning crates never name a concrete
    /// backend.
    pub fn new_forgejo_provisioning(config: ForgejoConfig) -> Arc<dyn ProvisioningForge> {
        Arc::new(temper_forge_forgejo::ForgejoForge::new(config))
    }

    /// Build a GitHub-backed [`Forge`] from a [`GitHubConfig`].
    pub fn new_github(config: GitHubConfig) -> Arc<dyn Forge> {
        Arc::new(temper_forge_github::GitHubForge::new(config))
    }

    /// Build an in-memory [`Forge`] with the default bootstrapped current user.
    ///
    /// Production code uses this only where an ephemeral forge is intentional;
    /// the test harness uses the concrete `MemoryForge` directly for its
    /// test-only seeding affordances.
    pub fn new_memory() -> Arc<dyn Forge> {
        Arc::new(temper_forge_memory::MemoryForge::new())
    }

    /// Build an in-memory [`Forge`] plus a companion [`ChangeSource`].
    pub fn new_memory_with_change_source() -> ForgeWithChangeSource {
        let backend = temper_forge_memory::MemoryForge::new();
        let change_source = Box::new(backend.subscribe_hints());
        ForgeWithChangeSource {
            forge: Arc::new(backend),
            change_source,
        }
    }

    /// Build a filesystem-backed [`Forge`] rooted at `root`.
    pub fn new_filesystem(root: impl Into<PathBuf>) -> Arc<dyn Forge> {
        Arc::new(temper_forge_filesystem::FilesystemForge::new(root))
    }

    /// Build a filesystem-backed [`Forge`] plus a companion [`ChangeSource`].
    pub fn new_filesystem_with_change_source(root: impl Into<PathBuf>) -> ForgeWithChangeSource {
        let backend = temper_forge_filesystem::FilesystemForge::new(root);
        let change_source = Box::new(backend.subscribe_hints());
        ForgeWithChangeSource {
            forge: Arc::new(backend),
            change_source,
        }
    }
}
