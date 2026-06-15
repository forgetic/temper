//! GitHub-backed [`temper_forge::Forge`] implementation.
//!
//! This crate adapts the portable Forge interface to GitHub's REST API
//! (`api.github.com`, or a GitHub Enterprise `/api/v3` root). It is a
//! best-effort, offline-tested backend: the provider is reached through a
//! mockable [`HttpClient`] seam so contract tests run without a network,
//! mirroring the structure of the `temper-forge-forgejo` crate.
//!
//! Known first-pass limitations (documented per module):
//!
//! - Native dependency links are not part of GitHub's stable REST surface, so
//!   reads report no dependencies and mutations are rejected (see
//!   [`crate::dependencies`]).
//! - Optimistic concurrency is best-effort, derived from response `ETag`s (or
//!   the weak `updated_at` fallback) exactly like the Forgejo backend.
//!
//! This module is a thin facade: it declares the per-domain modules, defines
//! the [`GitHubForge`] backend handle and its constructors, and re-exports the
//! public API. The request plumbing lives in [`crate::request`] and the
//! optimistic-concurrency cache in [`crate::version`].

mod ci;
mod client;
mod config;
mod dependencies;
mod error;
mod forge_impl;
mod ids;
mod issues;
mod items;
mod map;
mod pulls;
mod repos;
mod request;
mod types;
mod version;

pub use client::{EngineHttpClient, HttpClient, HttpError, HttpMethod, HttpRequest, HttpResponse};
pub use config::{CasMode, ConfigError, DEFAULT_PAGE_LIMIT, GitHubConfig};

use std::sync::Arc;
use version::VersionCache;

/// GitHub Forge backend.
///
/// `C` is the HTTP client; production uses [`EngineHttpClient`] and tests use
/// a recording mock. Construct with [`GitHubForge::new`] for the real client or
/// [`GitHubForge::with_client`] to inject a custom one.
#[derive(Clone, Debug)]
pub struct GitHubForge<C = EngineHttpClient> {
    config: GitHubConfig,
    client: C,
    versions: Arc<VersionCache>,
}

impl GitHubForge<EngineHttpClient> {
    /// Builds a backend that talks to the configured API root over the engine HTTP client.
    pub fn new(config: GitHubConfig) -> Self {
        let client = EngineHttpClient::new(config.base_url.clone());
        Self::with_client(config, client)
    }
}

impl<C> GitHubForge<C> {
    /// Returns the backend configuration.
    pub fn config(&self) -> &GitHubConfig {
        &self.config
    }
}

impl<C: HttpClient> GitHubForge<C> {
    /// Builds a backend from an explicit HTTP client.
    pub fn with_client(config: GitHubConfig, client: C) -> Self {
        Self {
            config,
            client,
            versions: Arc::new(VersionCache::default()),
        }
    }
}
