// SPDX-License-Identifier: MPL-2.0

//! OpenTelemetry export seam — compiled only under the `otel` feature.
//!
//! This is the deliberately-thin seam described in §6 ("OTel posture") of the
//! [logging & observability design][design]: temper adopts the OTel *semantic
//! model* now (spans, dotted attributes like `artifact.ref`, the closed `event`
//! enum, numeric `duration_ms`) but does **not** wire an exporter or collector.
//! A single-process standalone daemon writing to journald does not need one.
//!
//! What lives here is the one place a real exporter would be added the day
//! temper goes multi-process / multi-host (Tempo / Jaeger / Honeycomb). That
//! change is "add one layer next to journald" — it requires **no emit-site
//! changes**, because the fields are already dotted, OTel-shaped names.
//!
//! Until then [`otel_layer`] builds an [`OpenTelemetryLayer`] over a
//! [`SdkTracerProvider`] with **no span processors configured**, so every span
//! it receives is dropped: a no-op sink that nonetheless exercises the genuine
//! `tracing-opentelemetry` construction, keeping the seam honest and compiling.
//! Wiring a collector later is swapping the empty provider for one carrying a
//! batch span processor + OTLP exporter — nothing in temper's emit sites moves.
//!
//! [design]: https://example.invalid/docs/explanation/logging-and-observability.md
//! [`OpenTelemetryLayer`]: tracing_opentelemetry::OpenTelemetryLayer
//! [`SdkTracerProvider`]: opentelemetry_sdk::trace::SdkTracerProvider

use opentelemetry::trace::TracerProvider as _;
use tracing_subscriber::Layer;
use tracing_subscriber::registry::LookupSpan;

/// Build the OpenTelemetry [`tracing`] layer for `init_logging` to install
/// alongside journald / stderr.
///
/// The backing [`SdkTracerProvider`] has no span processors, so the layer is a
/// no-op sink today (see the module docs): the seam compiles and installs, but
/// nothing is exported until a collector is wired. Returned as an `impl Layer`
/// so the concrete OTel types stay confined to this feature-gated module.
///
/// [`tracing`]: https://docs.rs/tracing
/// [`SdkTracerProvider`]: opentelemetry_sdk::trace::SdkTracerProvider
pub(crate) fn otel_layer<S>() -> impl Layer<S>
where
    S: tracing::Subscriber + for<'span> LookupSpan<'span>,
{
    // An exporter-less provider: no span processor means every span is dropped.
    // This is the placeholder a real OTLP/batch exporter replaces later.
    let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder().build();
    let tracer = provider.tracer("temper");
    tracing_opentelemetry::layer().with_tracer(tracer)
}
