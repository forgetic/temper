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

/// A reference forwards to its referent, so a `&dyn EnvLookup` (the injected
/// env in [`crate::LoadInputs`]) can be passed wherever an `impl EnvLookup` is
/// expected.
impl<T: EnvLookup + ?Sized> EnvLookup for &T {
    fn get(&self, key: &str) -> Option<String> {
        (**self).get(key)
    }
}

/// The real process environment — the explicit `std::env` boundary.
///
/// Binary-only: it is constructed solely by the `#[doc(hidden)]` shims (`load`,
/// `default_config_path`, `resolve_targets`). Hermetic code takes an injected
/// [`EnvLookup`] (e.g. [`EnvMap`] or [`NoEnv`]) and never names this type.
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

/// An owned, in-memory environment snapshot.
///
/// Built either explicitly (tests) or once at the binary boundary from the real
/// process environment via [`EnvMap::from_system`]. Downstream code takes an
/// `&EnvMap` (or any [`EnvLookup`]) and never touches `std::env`, so a load is
/// hermetic with respect to whatever snapshot it was handed.
#[derive(Debug, Clone, Default)]
pub struct EnvMap(BTreeMap<String, String>);

impl EnvMap {
    /// An empty snapshot — every lookup misses.
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshots the real process environment (`std::env::vars`).
    ///
    /// Binary-only: this is the explicit `std::env` boundary. The composition
    /// root (`src/bin/*`, `service_main`) builds one of these once; everything
    /// downstream takes the resulting plain data. Tests must build an `EnvMap`
    /// in-memory instead so they never read the real environment.
    #[doc(hidden)]
    pub fn from_system() -> Self {
        Self(std::env::vars().collect())
    }

    /// Inserts a key/value pair, for building a snapshot in tests.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<String>) -> &mut Self {
        self.0.insert(key.into(), value.into());
        self
    }
}

impl<K: Into<String>, V: Into<String>> FromIterator<(K, V)> for EnvMap {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        Self(
            iter.into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        )
    }
}

impl EnvLookup for EnvMap {
    fn get(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
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
