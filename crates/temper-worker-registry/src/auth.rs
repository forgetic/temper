// SPDX-License-Identifier: MPL-2.0

//! Daemon-side worker-pool authentication policy.

use std::collections::BTreeMap;
use std::fmt;

use temper_protocol_worker::WorkerAuth;

/// Authentication policy keyed by configured worker-pool name.
///
/// An empty config preserves legacy unauthenticated worker protocol behavior.
/// Once at least one pool is configured, register/poll/result/heartbeat must be
/// associated with a known pool. Pools with a token require a matching bearer;
/// pools without a token require no bearer.
#[derive(Clone, Default, Eq, PartialEq)]
pub struct WorkerPoolAuthConfig {
    pools: BTreeMap<String, Option<WorkerAuth>>,
}

impl WorkerPoolAuthConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert_pool(&mut self, name: impl Into<String>, token: Option<WorkerAuth>) {
        self.pools.insert(name.into(), token);
    }

    pub fn is_enabled(&self) -> bool {
        !self.pools.is_empty()
    }

    pub(crate) fn pool_token(&self, pool: &str) -> Option<&Option<WorkerAuth>> {
        self.pools.get(pool)
    }
}

impl fmt::Debug for WorkerPoolAuthConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkerPoolAuthConfig")
            .field("pools", &self.pools)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_pool_token_values() {
        let mut config = WorkerPoolAuthConfig::new();
        config.insert_pool(
            "builders",
            Some(WorkerAuth::bearer("super-secret-builder-token")),
        );

        let rendered = format!("{config:?}");
        assert!(rendered.contains("builders"), "{rendered}");
        assert!(rendered.contains("[REDACTED]"), "{rendered}");
        assert!(
            !rendered.contains("super-secret-builder-token"),
            "{rendered}"
        );
    }
}
