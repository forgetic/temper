//! Forgejo-backed [`harness_forge::Forge`] implementation.
//!
//! This crate adapts the portable Forge interface to Forgejo's HTTP API. It is
//! a best-effort, offline-tested backend: the provider is reached through a
//! mockable [`HttpClient`] seam so contract tests run without a network, and
//! the full `Forge` trait is implemented incrementally across phases (see
//! `docs/reference/forgejo-backend.md`).
//!
//! This module wires together the provider infrastructure every phase uses:
//! configuration, the HTTP seam, error mapping, backend-owned id encoding, and
//! provider DTO scaffolding.

mod ci;
mod ci_match;
mod ci_time;
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
mod types;

pub use client::{HttpClient, HttpError, HttpMethod, HttpRequest, HttpResponse, ReqwestHttpClient};
pub use config::{CasMode, ConfigError, ForgejoConfig, DEFAULT_PAGE_LIMIT};

use harness_forge::{ForgeError, ForgeResult, Version};
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Maximum number of paginated pages a list request will fetch before stopping.
///
/// A safety bound so a misbehaving provider that never returns a short page
/// cannot loop forever. The default page size (50) makes this a generous cap.
const MAX_LIST_PAGES: u32 = 1000;

/// Forgejo Forge backend.
///
/// `C` is the HTTP client; production uses [`ReqwestHttpClient`] and tests use a
/// recording mock. Construct with [`ForgejoForge::new`] for the real client or
/// [`ForgejoForge::with_client`] to inject a custom one.
#[derive(Clone, Debug)]
pub struct ForgejoForge<C = ReqwestHttpClient> {
    config: ForgejoConfig,
    client: C,
    versions: Arc<VersionCache>,
}

impl ForgejoForge<ReqwestHttpClient> {
    /// Builds a backend that talks to the configured base URL over `reqwest`.
    pub fn new(config: ForgejoConfig) -> Self {
        let client = ReqwestHttpClient::new(config.base_url.clone());
        Self::with_client(config, client)
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
        }
    }

    /// Sends a prepared Forgejo API request and returns the raw response.
    ///
    /// The path is relative to the host and excludes the `/api/v1` prefix, which
    /// [`client::build_request`] adds. Transport failures are mapped to
    /// [`ForgeError::Backend`](harness_forge::ForgeError); status interpretation
    /// is the caller's responsibility (see [`crate::error`]).
    pub(crate) async fn send(
        &self,
        method: HttpMethod,
        path: impl AsRef<str>,
        query: Vec<(String, String)>,
        body: Option<String>,
    ) -> ForgeResult<HttpResponse> {
        let request = client::build_request(&self.config.token, method, path, query, body);
        self.client
            .execute(request)
            .await
            .map_err(error::map_transport_error)
    }

    /// Sends a request, mapping any non-success status to a [`ForgeError`].
    pub(crate) async fn request_checked(
        &self,
        context: &str,
        method: HttpMethod,
        path: impl AsRef<str>,
        query: Vec<(String, String)>,
        body: Option<String>,
    ) -> ForgeResult<HttpResponse> {
        let response = self.send(method, path, query, body).await?;
        if response.is_success() {
            Ok(response)
        } else {
            Err(error::map_status_error(context, &response))
        }
    }

    /// Sends a request treating `404` as absence (`Ok(None)`).
    pub(crate) async fn request_optional(
        &self,
        context: &str,
        method: HttpMethod,
        path: impl AsRef<str>,
        query: Vec<(String, String)>,
        body: Option<String>,
    ) -> ForgeResult<Option<HttpResponse>> {
        let response = self.send(method, path, query, body).await?;
        if response.status == 404 {
            return Ok(None);
        }
        if response.is_success() {
            Ok(Some(response))
        } else {
            Err(error::map_status_error(context, &response))
        }
    }

    /// Decodes a JSON response body into `T`, mapping parse errors to
    /// [`ForgeError::Backend`].
    pub(crate) fn decode<T: DeserializeOwned>(
        context: &str,
        response: &HttpResponse,
    ) -> ForgeResult<T> {
        serde_json::from_str(&response.body).map_err(|error| {
            ForgeError::Backend(format!(
                "{context}: failed to decode forgejo response: {error}"
            ))
        })
    }

    /// Fetches every page of a paginated list endpoint and decodes the items.
    ///
    /// Appends `limit`/`page` query parameters to `base_query` and stops when a
    /// page returns fewer than the configured page size (or [`MAX_LIST_PAGES`]
    /// is reached). Ordering is the caller's responsibility.
    pub(crate) async fn list_all<T: DeserializeOwned>(
        &self,
        context: &str,
        path: &str,
        base_query: Vec<(String, String)>,
    ) -> ForgeResult<Vec<T>> {
        let limit = self.config.page_limit.max(1);
        let mut items = Vec::new();
        let mut page = 1u32;
        loop {
            let mut query = base_query.clone();
            query.push(("limit".to_string(), limit.to_string()));
            query.push(("page".to_string(), page.to_string()));
            let response = self
                .request_checked(context, HttpMethod::Get, path, query, None)
                .await?;
            let batch: Vec<T> = Self::decode(context, &response)?;
            let batch_len = batch.len();
            items.extend(batch);
            if batch_len < limit as usize || page >= MAX_LIST_PAGES {
                break;
            }
            page += 1;
        }
        Ok(items)
    }
}

/// Captures provider validators for best-effort optimistic concurrency.
///
/// Forgejo exposes no confirmed conditional-write contract, so the backend
/// derives a portable [`Version`] from a per-artifact validator (an `ETag` when
/// present, otherwise the weak `updated_at` timestamp). [`Self::observe`] returns
/// a stable version that advances only when the validator changes, so repeated
/// reads of an unchanged artifact report the same version while any mutation
/// bumps it. [`Self::check`] re-resolves the fresh validator on a conditional
/// write and reports a stale token as
/// [`ForgeError::Conflict`](harness_forge::ForgeError::Conflict).
///
/// The cache is shared behind an [`Arc`] so cloning the backend shares one
/// cache. It is per-process and per-backend-instance: a version is only
/// meaningful when the read that issued it and the conditional write that
/// consumes it go through the same backend, which is how the workflow layer's
/// `LeaseManager` uses it. The residual races (read-modify-write is not atomic;
/// `updated_at` has one-second granularity) are documented in
/// `docs/reference/forgejo-backend.md`.
#[derive(Debug, Default)]
pub(crate) struct VersionCache {
    captured: Mutex<HashMap<String, CapturedValidator>>,
}

/// A provider validator captured at read time for a single artifact.
#[derive(Clone, Debug)]
struct CapturedValidator {
    validator: Option<String>,
    version: Version,
}

impl VersionCache {
    /// Records the current `validator` for `key` and returns its stable version.
    ///
    /// A new key starts at [`Version::INITIAL`]. A validator that matches the
    /// stored one reuses the stored version; any change (including a missing
    /// validator) advances it.
    pub(crate) fn observe(&self, key: &str, validator: Option<&str>) -> Version {
        let mut captured = self.captured.lock().expect("version cache mutex poisoned");
        match captured.get_mut(key) {
            Some(existing) => {
                if validator.is_some() && existing.validator.as_deref() == validator {
                    existing.version
                } else {
                    existing.version = existing.version.next();
                    existing.validator = validator.map(str::to_string);
                    existing.version
                }
            }
            None => {
                captured.insert(
                    key.to_string(),
                    CapturedValidator {
                        validator: validator.map(str::to_string),
                        version: Version::INITIAL,
                    },
                );
                Version::INITIAL
            }
        }
    }

    /// Verifies a conditional-write precondition for `key`.
    ///
    /// With a fresh `validator`, resolves it to a version and returns
    /// [`ForgeError::Conflict`] when it differs from `expected`. With no
    /// validator, [`CasMode::Strict`] refuses the write
    /// ([`ForgeError::InvalidRequest`]) while [`CasMode::BestEffort`] proceeds
    /// (a documented weak read-before-write).
    pub(crate) fn check(
        &self,
        key: &str,
        validator: Option<&str>,
        expected: Version,
        mode: CasMode,
    ) -> ForgeResult<()> {
        match validator {
            None => match mode {
                CasMode::Strict => Err(ForgeError::InvalidRequest(format!(
                    "no provider validator captured for conditional update of {key}"
                ))),
                CasMode::BestEffort => Ok(()),
            },
            Some(validator) => {
                let current = self.observe(key, Some(validator));
                if current == expected {
                    Ok(())
                } else {
                    Err(ForgeError::Conflict(format!(
                        "stale conditional update of {key}: expected version {expected}, \
                         found {current}"
                    )))
                }
            }
        }
    }
}

#[cfg(test)]
mod version_cache_tests {
    use super::*;

    #[test]
    fn observe_is_stable_until_validator_changes() {
        let cache = VersionCache::default();
        let first = cache.observe("pr-1", Some("etag-a"));
        let second = cache.observe("pr-1", Some("etag-a"));
        assert_eq!(first, second);
        let bumped = cache.observe("pr-1", Some("etag-b"));
        assert_eq!(bumped, first.next());
    }

    #[test]
    fn check_detects_stale_token() {
        let cache = VersionCache::default();
        let version = cache.observe("pr-1", Some("etag-a"));
        assert!(cache
            .check("pr-1", Some("etag-a"), version, CasMode::BestEffort)
            .is_ok());
        // A changed validator resolves to a new version, so the old token is stale.
        let result = cache.check("pr-1", Some("etag-b"), version, CasMode::BestEffort);
        assert!(matches!(result, Err(ForgeError::Conflict(_))));
    }

    #[test]
    fn check_without_validator_honors_cas_mode() {
        let cache = VersionCache::default();
        assert!(cache
            .check("pr-1", None, Version::INITIAL, CasMode::BestEffort)
            .is_ok());
        assert!(matches!(
            cache.check("pr-1", None, Version::INITIAL, CasMode::Strict),
            Err(ForgeError::InvalidRequest(_))
        ));
    }
}
