//! Best-effort optimistic concurrency for GitHub artifacts.
//!
//! GitHub exposes `ETag`s on reads but no portable conditional-write contract
//! for issues and pull requests, so the backend derives a portable [`Version`]
//! from a per-artifact validator and tracks it in a process-local cache.

use crate::CasMode;
use std::collections::HashMap;
use std::sync::Mutex;
use temper_forge_model::{ForgeError, ForgeResult, Version};

/// Captures provider validators for best-effort optimistic concurrency.
///
/// GitHub exposes `ETag`s on reads but no portable conditional-write contract
/// for issues and pull requests, so the backend derives a portable [`Version`]
/// from a per-artifact validator (an `ETag` when present, otherwise the weak
/// `updated_at` timestamp). [`Self::observe`] returns a stable version that
/// advances only when the validator changes; [`Self::check`] re-resolves the
/// fresh validator on a conditional write and reports a stale token as
/// [`ForgeError::Conflict`](temper_forge_model::ForgeError::Conflict).
///
/// The cache is shared behind an [`Arc`](std::sync::Arc) so cloning the backend
/// shares one cache. It is per-process and per-backend-instance: a version is
/// only meaningful when the read that issued it and the conditional write that
/// consumes it go through the same backend. The residual races
/// (read-modify-write is not atomic; `updated_at` has one-second granularity)
/// match the Forgejo backend's documented behavior.
#[derive(Debug, Default)]
pub(crate) struct VersionCache {
    captured: Mutex<HashMap<String, CapturedValidator>>,
}

/// A provider validator captured at read time for a single artifact.
#[derive(Clone, Debug)]
struct CapturedValidator {
    validator: Option<String>,
    version: Version,
    /// A successful mutation endpoint did not return a validator. The next
    /// provider read observes that mutation's validator and must stabilize the
    /// committed portable version rather than spuriously advancing it.
    awaiting_validator: bool,
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
                if existing.awaiting_validator {
                    existing.validator = validator.map(str::to_string);
                    existing.awaiting_validator = false;
                    existing.version
                } else if validator.is_some() && existing.validator.as_deref() == validator {
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
                        awaiting_validator: false,
                    },
                );
                Version::INITIAL
            }
        }
    }

    /// Advances the portable version after a successful provider mutation and
    /// records the validator returned by that mutation. When an endpoint has no
    /// validator, the last captured validator is retained.
    pub(crate) fn commit(&self, key: &str, validator: Option<&str>, previous: Version) -> Version {
        let mut captured = self.captured.lock().expect("version cache mutex poisoned");
        let retained = captured.get(key).and_then(|entry| entry.validator.clone());
        let version = previous.next();
        captured.insert(
            key.to_string(),
            CapturedValidator {
                validator: validator.map(str::to_string).or(retained),
                version,
                awaiting_validator: validator.is_none(),
            },
        );
        version
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
    fn commit_advances_and_stabilizes_the_returned_validator() {
        let cache = VersionCache::default();
        let observed = cache.observe("issue-1", Some("etag-a"));
        let committed = cache.commit("issue-1", Some("etag-b"), observed);
        assert_eq!(committed, observed.next());
        assert_eq!(cache.observe("issue-1", Some("etag-b")), committed);
    }

    #[test]
    fn commit_without_validator_stabilizes_the_next_provider_read() {
        let cache = VersionCache::default();
        let observed = cache.observe("issue-1", Some("etag-a"));
        let committed = cache.commit("issue-1", None, observed);

        assert_eq!(cache.observe("issue-1", Some("etag-b")), committed);
        assert_eq!(cache.observe("issue-1", Some("etag-b")), committed);
        assert_eq!(cache.observe("issue-1", Some("etag-c")), committed.next());
    }

    #[test]
    fn check_detects_stale_token() {
        let cache = VersionCache::default();
        let version = cache.observe("pr-1", Some("etag-a"));
        assert!(
            cache
                .check("pr-1", Some("etag-a"), version, CasMode::BestEffort)
                .is_ok()
        );
        // A changed validator resolves to a new version, so the old token is stale.
        let result = cache.check("pr-1", Some("etag-b"), version, CasMode::BestEffort);
        assert!(matches!(result, Err(ForgeError::Conflict(_))));
    }

    #[test]
    fn check_without_validator_honors_cas_mode() {
        let cache = VersionCache::default();
        assert!(
            cache
                .check("pr-1", None, Version::INITIAL, CasMode::BestEffort)
                .is_ok()
        );
        assert!(matches!(
            cache.check("pr-1", None, Version::INITIAL, CasMode::Strict),
            Err(ForgeError::InvalidRequest(_))
        ));
    }
}
