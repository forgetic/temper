// SPDX-License-Identifier: MPL-2.0

//! A small environment-lookup abstraction so resolution (and its precedence
//! rules) can be unit-tested without touching the process environment.

use std::collections::BTreeMap;

/// A source of environment variables.
pub trait EnvLookup {
    /// The value for `key`, or `None` if unset.
    fn get(&self, key: &str) -> Option<String>;

    /// The value for `key` if it is set to a non-blank value (trimmed).
    fn non_empty(&self, key: &str) -> Option<String> {
        self.get(key)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    }
}

/// The real process environment.
pub struct SystemEnv;

impl EnvLookup for SystemEnv {
    fn get(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }
}

/// An in-memory environment for tests.
impl EnvLookup for BTreeMap<String, String> {
    fn get(&self, key: &str) -> Option<String> {
        BTreeMap::get(self, key).cloned()
    }
}

/// The empty environment — every lookup misses. Useful for file-only resolution
/// in tests.
pub struct NoEnv;

impl EnvLookup for NoEnv {
    fn get(&self, _key: &str) -> Option<String> {
        None
    }
}
