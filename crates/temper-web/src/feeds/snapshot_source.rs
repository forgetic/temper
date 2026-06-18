// SPDX-License-Identifier: MPL-2.0

//! Feed 1a — the cold-start snapshot source.
//!
//! The board's cold start is the daemon's raw `GET /v1/state` snapshot projected
//! into the board card shape. The fetch is behind a [`SnapshotSource`] trait so
//! the server runs standalone (and tests pass) without a live daemon: a fixture
//! source or an empty source stands in. The HTTP-fetching production source is
//! constructed at the binary entry from the explicit daemon URL config — never
//! from ambient env in library code.

use crate::project::snapshot::RawDaemonSnapshot;

/// Yields the raw daemon dispatch snapshot for a board cold start. Implementors:
/// the live daemon HTTP client (built at the entry point), a fixture, or empty.
///
/// `Send + Sync` so a shared `Arc<dyn SnapshotSource>` can be threaded into the
/// background re-poll pump (`pump_snapshot_source`) as well as used at cold start.
/// All implementors (empty, fixture, HTTP) are already trivially `Send + Sync`.
pub trait SnapshotSource: Send + Sync {
    /// Fetch the current raw daemon snapshot, or `None` when no daemon is
    /// reachable/configured (the server then serves an empty board).
    fn fetch(&self) -> Option<RawDaemonSnapshot>;
}

/// A source that always yields an empty board — the standalone default when no
/// daemon URL is configured.
#[derive(Debug, Clone, Default)]
pub struct EmptySnapshotSource;

impl SnapshotSource for EmptySnapshotSource {
    fn fetch(&self) -> Option<RawDaemonSnapshot> {
        None
    }
}

/// A source backed by a canned raw-snapshot JSON string — used by tests and the
/// `--snapshot-fixture` dev mode so the board renders without a daemon.
#[derive(Debug, Clone)]
pub struct FixtureSnapshotSource {
    raw: String,
}

impl FixtureSnapshotSource {
    /// Build from a raw daemon-snapshot JSON string.
    #[must_use]
    pub fn new(raw: impl Into<String>) -> Self {
        Self { raw: raw.into() }
    }
}

impl SnapshotSource for FixtureSnapshotSource {
    fn fetch(&self) -> Option<RawDaemonSnapshot> {
        serde_json::from_str(&self.raw).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_source_yields_none() {
        assert!(EmptySnapshotSource.fetch().is_none());
    }

    #[test]
    fn fixture_source_parses_documented_shape() {
        let raw = r#"{ "workers": { "healthy": 2, "total": 2 }, "queued": [], "in_flight": [],
                       "role_saturation": [] }"#;
        let snapshot = FixtureSnapshotSource::new(raw).fetch().expect("parses");
        assert_eq!(snapshot.workers.total, 2);
    }

    #[test]
    fn fixture_source_yields_none_on_garbage() {
        assert!(FixtureSnapshotSource::new("not json").fetch().is_none());
    }
}
