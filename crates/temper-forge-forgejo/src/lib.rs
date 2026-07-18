//! Forgejo-backed [`temper_forge_model::Forge`] implementation.
//!
//! This crate adapts the portable Forge interface to Forgejo's HTTP API. It is
//! a best-effort, offline-tested backend: the provider is reached through a
//! mockable [`HttpClient`] seam so contract tests run without a network, and
//! the full `Forge` trait is implemented incrementally across phases (see
//! `docs/reference/forgejo-backend.md`).
//!
//! This module owns the backend type and constructors and wires together the
//! provider infrastructure every phase uses: the request plumbing (see
//! [`request`]), the version cache (see [`version`]), configuration, the HTTP
//! seam, error mapping, backend-owned id encoding, and provider DTO scaffolding.

mod candidate_index;
mod ci;
mod ci_cache;
mod ci_match;
mod ci_time;
mod ci_ui;
mod ci_ui_parse;
mod client;
mod config;
mod dependencies;
mod error;
mod forge_impl;
mod ids;
mod issues;
mod items;
mod map;
mod provision;
mod pulls;
mod read_only_basic;
mod repos;
mod request;
mod types;
mod version;

pub use client::{EngineHttpClient, HttpClient, HttpError, HttpMethod, HttpRequest, HttpResponse};
pub use config::{CasMode, DEFAULT_PAGE_LIMIT, ForgejoConfig, WebUiCredentials};
pub use provision::{ROLE_PASSWORD, admin_token_via_basic_auth};
pub use read_only_basic::ReadOnlyBasicAuthClient;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use version::VersionCache;

/// Forgejo Forge backend.
///
/// `C` is the HTTP client; production uses [`EngineHttpClient`] and tests use a
/// recording mock. Construct with [`ForgejoForge::new`] for the real client or
/// [`ForgejoForge::with_client`] to inject a custom one.
#[derive(Clone, Debug)]
pub struct ForgejoForge<C = EngineHttpClient> {
    config: ForgejoConfig,
    client: C,
    versions: Arc<VersionCache>,
    /// Cumulative provider requests, shared by clones for per-apply deltas.
    provider_requests: Arc<AtomicU64>,
    /// Repository-scoped label name/id maps used by issue fan-out. Forgejo's
    /// issue endpoints require numeric ids, while the portable surface uses
    /// names. Shared across clones and invalidated after label upserts.
    label_ids: Arc<Mutex<HashMap<String, HashMap<String, u64>>>>,
    /// Memo of terminal web-UI CI reads, so an idle mechanical tick skips the
    /// expensive login+scrape for a pull request whose head SHA has not changed
    /// since its CI settled (ADR 0019 cost mitigation). Shared across clones.
    ci_reads: Arc<ci_cache::CiReadCache>,
}

impl ForgejoForge<EngineHttpClient> {
    /// Builds a backend that talks to the configured base URL over `reqwest`.
    pub fn new(config: ForgejoConfig) -> Self {
        let client = EngineHttpClient::new(config.base_url.clone());
        Self::with_client(config, client)
    }
}

impl ForgejoForge<ReadOnlyBasicAuthClient<EngineHttpClient>> {
    /// Builds a mutation-proof backend for pre-apply inspection over HTTP Basic
    /// authentication.
    pub fn new_read_only_basic(
        base_url: impl Into<String>,
        login: impl AsRef<str>,
        password: impl AsRef<str>,
    ) -> Self {
        let base_url = base_url.into();
        let client = EngineHttpClient::new(base_url.clone());
        Self::with_read_only_basic_client(base_url, login, password, client)
    }
}

impl<C: HttpClient> ForgejoForge<ReadOnlyBasicAuthClient<C>> {
    /// Builds a mutation-proof Basic-auth backend over an explicit HTTP seam.
    ///
    /// Production uses [`Self::new_read_only_basic`]; recording tests inject a
    /// client here to verify authentication and local mutation rejection.
    pub fn with_read_only_basic_client(
        base_url: impl Into<String>,
        login: impl AsRef<str>,
        password: impl AsRef<str>,
        client: C,
    ) -> Self {
        // The regular request builder requires a token-shaped configuration.
        // This non-secret sentinel is always removed by the client boundary
        // before a GET reaches the transport.
        let config = ForgejoConfig::new(base_url, "read-only-basic-auth");
        Self::with_client(
            config,
            ReadOnlyBasicAuthClient::new(client, login, password),
        )
    }
}

impl<C> ForgejoForge<C> {
    /// Returns the backend configuration.
    pub fn config(&self) -> &ForgejoConfig {
        &self.config
    }
}

impl<C: HttpClient> ForgejoForge<C> {
    /// Builds a backend from an explicit HTTP client.
    pub fn with_client(config: ForgejoConfig, client: C) -> Self {
        Self {
            config,
            client,
            versions: Arc::new(VersionCache::default()),
            provider_requests: Arc::new(AtomicU64::new(0)),
            label_ids: Arc::new(Mutex::new(HashMap::new())),
            ci_reads: Arc::new(ci_cache::CiReadCache::default()),
        }
    }

    /// Returns the cumulative number of provider requests sent by this backend
    /// backend instance and its clones.
    pub fn provider_request_count(&self) -> u64 {
        self.provider_requests.load(Ordering::Relaxed)
    }

    /// Records one provider request that is about to cross the HTTP seam.
    pub(crate) fn record_provider_request(&self) {
        self.provider_requests.fetch_add(1, Ordering::Relaxed);
    }

    /// Returns the underlying HTTP client.
    ///
    /// The CI web-UI read path ([`crate::ci_ui`]) builds raw [`HttpRequest`]s
    /// (no `/api/v1` prefix, cookie auth, form bodies) that bypass
    /// [`client::build_request`], so it issues them through this seam directly.
    pub(crate) fn http_client(&self) -> &C {
        &self.client
    }

    /// The web-UI CI read memo (ADR 0019 cost mitigation); see [`ci_cache`].
    pub(crate) fn ci_read_cache(&self) -> &ci_cache::CiReadCache {
        &self.ci_reads
    }
}
