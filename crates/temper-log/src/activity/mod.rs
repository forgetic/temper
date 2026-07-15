//! OpenTelemetry/tracing projection over durable canonical agent activity.
//!
//! The projector consumes only [`temper_protocol_activity::AgentRunEventV1`]
//! after journal durability. It never observes model/tool call sites directly,
//! so tracing, web, and JSONL export remain projections of one authority.

mod helpers;
mod model;
mod projector;
mod tracing_exporter;

pub use model::{
    ActivitySpanAttributes, ActivitySpanExporter, ActivitySpanKind, ActivitySpanStart,
    ActivitySpanStatus, InMemoryActivitySpanExporter, ProjectedActivitySpan,
};
pub use projector::CanonicalActivityProjector;
pub use tracing_exporter::TracingActivitySpanExporter;

#[cfg(feature = "otel")]
use std::sync::{Arc, Mutex, OnceLock};

/// Projects durable canonical events into the optional process OTel layer.
///
/// This is intentionally a no-op unless `temper-log/otel` is enabled. Lock
/// poisoning and exporter failures are contained; callers receive no error and
/// must never retry product work because telemetry is unavailable.
pub fn project_agent_activity(events: &[temper_protocol_activity::AgentRunEventV1]) {
    #[cfg(feature = "otel")]
    {
        static PROJECTOR: OnceLock<Mutex<CanonicalActivityProjector>> = OnceLock::new();
        let projector = PROJECTOR.get_or_init(|| {
            Mutex::new(CanonicalActivityProjector::new(Arc::new(
                TracingActivitySpanExporter::default(),
            )))
        });
        let mut projector = projector
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        projector.project_all(events);
    }
    #[cfg(not(feature = "otel"))]
    {
        let _ = events;
    }
}

#[cfg(test)]
mod tests;
