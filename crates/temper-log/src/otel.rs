// SPDX-License-Identifier: MPL-2.0

//! Optional OpenTelemetry layer used by every Temper process.
//!
//! `otel` installs the layer and canonical activity projection without choosing
//! a network exporter. `otel-otlp` additionally installs the standard OTLP/HTTP
//! span exporter, configured by `OTEL_EXPORTER_OTLP_*`. Export happens through a
//! batch processor; collector failures are isolated inside the SDK and never
//! enter job execution or retry paths.

#[cfg(feature = "otel-otlp")]
use opentelemetry::trace::TracerProvider as _;
use tracing_subscriber::Layer;
use tracing_subscriber::registry::LookupSpan;

/// Build the OpenTelemetry layer installed next to journald / JSON / stderr.
pub(crate) fn otel_layer<S>() -> impl Layer<S>
where
    S: tracing::Subscriber + for<'span> LookupSpan<'span>,
{
    #[cfg(feature = "otel-otlp")]
    {
        let provider = tracer_provider();
        let tracer = provider.tracer("temper");
        tracing_opentelemetry::layer().with_tracer(tracer)
    }
    #[cfg(not(feature = "otel-otlp"))]
    {
        // Embedders may install their own global provider before init_logging;
        // without one this remains the standard OpenTelemetry no-op provider.
        tracing_opentelemetry::layer().with_tracer(opentelemetry::global::tracer("temper"))
    }
}

#[cfg(feature = "otel-otlp")]
fn tracer_provider() -> opentelemetry_sdk::trace::SdkTracerProvider {
    use opentelemetry_otlp::WithExportConfig as _;

    // The exporter reads OTEL_EXPORTER_OTLP_ENDPOINT,
    // OTEL_EXPORTER_OTLP_TRACES_ENDPOINT, headers, timeout, and compression
    // through the upstream SDK. None of those values become span fields.
    match opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_protocol(opentelemetry_otlp::Protocol::HttpBinary)
        .build()
    {
        Ok(exporter) => opentelemetry_sdk::trace::SdkTracerProvider::builder()
            .with_batch_exporter(exporter)
            .build(),
        Err(_) => opentelemetry_sdk::trace::SdkTracerProvider::builder().build(),
    }
}
