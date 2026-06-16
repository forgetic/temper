// SPDX-License-Identifier: MPL-2.0

//! The two span layers that make machine + debug output coherent (§4).
//!
//! Today events are flat/orphaned. Two spans fix that at zero cost to the emit
//! sites:
//!
//! - [`work_item_span`] — opened on lease claim, closed on completion. It sets
//!   `artifact.ref` (the §3 join key) **once**, and every child event — including
//!   all `debug`/`trace` lines — inherits it automatically. No per-call ceremony
//!   is needed to thread the join key onto debug output.
//! - [`agent_run_span`] — nested inside the work-item span for one LLM run. It
//!   carries the run `kind` (`triage`, `coding`, …).
//!
//! Both return a [`tracing::Span`]; WI-4 opens them at the right boundaries
//! (`let _guard = work_item_span(&item, role, transition).entered();` or
//! `.instrument(agent_run_span("coding"))` on a future). These are OTel spans by
//! construction; `artifact.ref` is a span attribute and `event` maps to span
//! events.

use tracing::Span;

use crate::WorkItemRef;

/// Opens the per-work-item span (§4a).
///
/// Sets `repo`, `artifact.ref`, `role`, and `transition` as span fields, so
/// every child event inherits the join key and workflow position. `transition`
/// is optional at open time (the first claim may not have one yet); pass `None`
/// to leave the field empty.
///
/// The span is created at `info` level so it is present whenever any of its
/// child events would be (the §5 policy keeps `info` as the floor).
///
/// ```
/// use temper_log::{WorkItemRef, work_item_span};
///
/// let item = WorkItemRef::issue("acme/widgets", 42);
/// let span = work_item_span(&item, "architect", Some("triage_intake"));
/// let _guard = span.enter();
/// // … emit child events here; each inherits artifact.ref=acme/widgets#42 …
/// ```
pub fn work_item_span(item: &WorkItemRef, role: &str, transition: Option<&str>) -> Span {
    tracing::info_span!(
        "work_item",
        repo = item.repo(),
        artifact.ref = %item,
        role = role,
        transition = transition.unwrap_or(""),
    )
}

/// Opens the per-agent-run span nested inside the work-item span (§4b).
///
/// Carries the run `kind` (`triage`, `coding`, …). Because it is opened while
/// the work-item span is current, it inherits `artifact.ref`; its own field adds
/// `agent_run.kind` to every line emitted during the run.
///
/// ```
/// use temper_log::agent_run_span;
///
/// let span = agent_run_span("coding");
/// let _guard = span.enter();
/// ```
pub fn agent_run_span(kind: &str) -> Span {
    tracing::info_span!("agent_run", kind = kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use tracing::subscriber::with_default;
    use tracing_subscriber::fmt::MakeWriter;

    /// A writer that discards everything; just enough to back an *enabled*
    /// subscriber so the spans under test carry live metadata. Without an
    /// installed subscriber, `info_span!` is disabled and has no metadata.
    struct Sink;

    impl io::Write for Sink {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for Sink {
        type Writer = Sink;
        fn make_writer(&'a self) -> Self::Writer {
            Sink
        }
    }

    /// Runs `f` with a real (enabled) subscriber installed for this thread.
    fn with_subscriber<T>(f: impl FnOnce() -> T) -> T {
        let subscriber = tracing_subscriber::fmt()
            .with_writer(Sink)
            .with_max_level(tracing::Level::TRACE)
            .finish();
        with_default(subscriber, f)
    }

    #[test]
    fn work_item_span_has_the_section_four_fields() {
        with_subscriber(|| {
            let item = WorkItemRef::issue("acme/widgets", 42);
            let span = work_item_span(&item, "architect", Some("triage_intake"));
            let meta = span.metadata().expect("work_item span has metadata");
            assert_eq!(meta.name(), "work_item");
            let fields: Vec<&str> = meta.fields().iter().map(|f| f.name()).collect();
            assert!(fields.contains(&"repo"));
            assert!(fields.contains(&"artifact.ref"));
            assert!(fields.contains(&"role"));
            assert!(fields.contains(&"transition"));
        });
    }

    #[test]
    fn work_item_span_accepts_absent_transition() {
        with_subscriber(|| {
            let item = WorkItemRef::pull_request("acme/api", 19);
            // Exercises the None branch — must not panic and must keep the field
            // present (empty) so downstream record/inheritance is uniform.
            let span = work_item_span(&item, "mechanical", None);
            let meta = span.metadata().expect("metadata present");
            let fields: Vec<&str> = meta.fields().iter().map(|f| f.name()).collect();
            assert!(fields.contains(&"transition"));
        });
    }

    #[test]
    fn agent_run_span_carries_kind() {
        with_subscriber(|| {
            let span = agent_run_span("coding");
            let meta = span.metadata().expect("agent_run span has metadata");
            assert_eq!(meta.name(), "agent_run");
            let fields: Vec<&str> = meta.fields().iter().map(|f| f.name()).collect();
            assert!(fields.contains(&"kind"));
        });
    }
}
