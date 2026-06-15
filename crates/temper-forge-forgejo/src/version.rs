//! Best-effort optimistic-concurrency version tracking for the Forgejo backend.

use crate::config::CasMode;
use std::collections::HashMap;
use std::sync::Mutex;
use temper_forge::{ForgeError, ForgeResult, Version};

/// Captures provider validators for best-effort optimistic concurrency.
///
/// Forgejo exposes no confirmed conditional-write contract, so the backend
/// derives a portable [`Version`] from a per-artifact validator (an `ETag` when
/// present, otherwise the weak `updated_at` timestamp). [`Self::observe`] returns
/// a stable version that advances only when the validator changes, so repeated
/// reads of an unchanged artifact report the same version while any mutation
/// bumps it. [`Self::check`] re-resolves the fresh validator on a conditional
/// write and reports a stale token as
/// [`ForgeError::Conflict`](temper_forge::ForgeError::Conflict).
///
/// The cache is shared behind an [`Arc`](std::sync::Arc) so cloning the backend
/// shares one cache. It is per-process and per-backend-instance: a version is
/// only meaningful when the read that issued it and the conditional write that
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
mod tests {
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
