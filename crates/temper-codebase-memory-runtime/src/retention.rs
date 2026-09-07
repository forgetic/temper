//! Keep startup demand owned by the spawned process cleanup coordinators.
use crate::ProviderBootstrap;
use std::sync::Arc;
use temper_process_containment::{CleanupObserver, CleanupSnapshot};

/// Attach this observer to the invocation's containment factory before spawning
/// clients. It adds no telemetry or cancellation authority. Its retained handle
/// closes the bootstrap only after the last spawned process owner is released,
/// even when the async startup future is dropped before cleanup finishes.
pub fn retain_admission_until_cleanup(bootstrap: ProviderBootstrap) -> Arc<dyn CleanupObserver> {
    Arc::new(AdmissionRetention {
        _bootstrap: bootstrap,
    })
}

struct AdmissionRetention {
    _bootstrap: ProviderBootstrap,
}

impl CleanupObserver for AdmissionRetention {
    fn observe(&self, _snapshot: &CleanupSnapshot) {}
}
