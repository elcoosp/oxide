//! The CLI's tracing setup (§20), split verbatim out of lib.rs by
//! scripts/split_cli.py (V2-A01).

/// Install the tracing subscriber (§20). Console logging is always on;
/// OpenTelemetry OTLP export is opt-in via `OTEL_EXPORTER_OTLP_ENDPOINT`.
/// When the env var is unset, no OTLP layer is installed and behaviour is
/// unchanged from before.
///
/// The OTLP exporter uses the SDK's default batch processor (spans are
/// shipped asynchronously). The provider is stored in a process-wide
/// `OnceLock` so it lives for the process duration — dropping it would
/// silently close the exporter.
pub(crate) fn init_tracing() {
    use tracing_subscriber::layer::SubscriberExt;

    let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok();
    // SPEC-24 / §20: `OXIDE_LOG=json` selects structured JSON log lines; the
    // default is human-readable. Each formatter is a distinct concrete layer
    // type, so the whole subscriber is built per branch (boxed as a
    // `Subscriber`).
    let json = std::env::var("OXIDE_LOG")
        .map(|v| v.eq_ignore_ascii_case("json"))
        .unwrap_or(false);

    macro_rules! build {
        ($otel:expr) => {{
            let base = tracing_subscriber::registry()
                .with($otel)
                .with(env_filter());
            if json {
                Box::new(
                    base.with(
                        tracing_subscriber::fmt::layer()
                            .with_target(false)
                            .json()
                            .flatten_event(true),
                    ),
                ) as Box<dyn tracing::Subscriber + Send + Sync>
            } else {
                Box::new(base.with(tracing_subscriber::fmt::layer().with_target(false)))
                    as Box<dyn tracing::Subscriber + Send + Sync>
            }
        }};
    }

    let subscriber: Box<dyn tracing::Subscriber + Send + Sync> = match endpoint {
        Some(ep) => match build_otel_layer(&ep) {
            Ok(otel_layer) => build!(otel_layer),
            Err(e) => {
                eprintln!(
                    "oxide: OTLP export disabled ({e}); continuing with console logging only"
                );
                build!(tracing_subscriber::layer::Identity::new())
            }
        },
        None => build!(tracing_subscriber::layer::Identity::new()),
    };

    let _ = tracing::subscriber::set_global_default(subscriber);
}

/// The `RUST_LOG`-style filter every subscriber branch installs: the
/// env var wins when it parses, the default is `info` — the console
/// formatter renders info-level events (the dev-plane request traces
/// among them, once `OXIDE_TRACE_LOG` attaches the export arm).
pub(crate) fn env_filter() -> tracing_subscriber::EnvFilter {
    tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"))
}

/// The trace rows' export gate (`OXIDE_TRACE_LOG`, V2-F22): when set to
/// anything but the falsy spellings (`""`, `0`, `false`, `off`), the
/// dev boot attaches the `TracingTraceSink` beside the ring — every
/// completed request's row leaves as one `tracing::info!` event through
/// the subscriber this module installs at boot (console, `OXIDE_LOG=json`,
/// and with `OTEL_EXPORTER_OTLP_ENDPOINT` the OTLP stream). Default off:
/// the console output stays byte-identical, and production never
/// attaches (the F21 posture holds).
pub(crate) fn trace_log_enabled() -> bool {
    match std::env::var("OXIDE_TRACE_LOG") {
        Ok(value) => !matches!(value.as_str(), "" | "0" | "false" | "off"),
        Err(_) => false,
    }
}

/// Build the `tracing-opentelemetry` layer for the given OTLP endpoint.
/// The returned layer is backed by a batch span processor whose lifetime is
/// process-wide via a leaked `TracerProvider` — acceptable for a CLI that
/// exits with the process.
pub(crate) fn build_otel_layer(
    endpoint: &str,
) -> Result<
    tracing_opentelemetry::OpenTelemetryLayer<
        tracing_subscriber::Registry,
        opentelemetry_sdk::trace::Tracer,
    >,
    String,
> {
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry_otlp::WithExportConfig;

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()
        .map_err(|e| format!("OTLP exporter build failed: {e}"))?;
    let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .build();
    // Leak the provider so its batch exporter lives for the process lifetime.
    let provider: &'static opentelemetry_sdk::trace::SdkTracerProvider =
        Box::leak(Box::new(provider));
    let tracer = provider.tracer("oxide");
    Ok(tracing_opentelemetry::layer().with_tracer(tracer))
}
