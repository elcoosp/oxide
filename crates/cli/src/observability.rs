//! The CLI's tracing setup (§20), split verbatim out of lib.rs by
//! scripts/split_cli.py (V2-A01).
//!
//! The subscriber's filter arrangement (V2-F26): the host filter
//! (`RUST_LOG`, default `info`) is the registry's global filter by
//! default; `OXIDE_LOG_FEED_LEVEL` moves it per-layer onto the formatter
//! and export layers and gives the log-feed capture layer its own floor
//! — the console stays byte-identical while the dev-plane window widens
//! past the host filter (see `oxide_fw_server::log_feed`'s module docs
//! for the seam and the pinned composition).

/// Install the tracing subscriber (§20). Console logging is always on;
/// OpenTelemetry OTLP export is opt-in via `OTEL_EXPORTER_OTLP_ENDPOINT`.
/// When the env var is unset, no OTLP layer is installed and behaviour is
/// unchanged from before.
///
/// The OTLP exporter uses the SDK's default batch processor (spans are
/// shipped asynchronously). The provider is stored in a process-wide
/// `OnceLock` so it lives for the process duration — dropping it would
/// silently close the exporter.
/// The dev boot's log-feed ring (V2-F24): `init_tracing` creates one when
/// the command is the dev boot (the capture layer rides the subscriber it
/// installs), and the serve boot reads the same instance for the
/// `GET /__oxide/logs` endpoint — the one-instance-two-readers posture
/// the trace feed rides. `None` for a non-dev command: no capture layer
/// was installed and the endpoint stays absent (production
/// byte-identical).
static LOG_FEED_RING: std::sync::OnceLock<std::sync::Arc<oxide_fw_server::RingLogSink>> =
    std::sync::OnceLock::new();

/// The ring the dev boot attached (the endpoint's window source).
pub(crate) fn log_feed_ring() -> Option<std::sync::Arc<oxide_fw_server::RingLogSink>> {
    LOG_FEED_RING.get().cloned()
}

pub(crate) fn init_tracing() {
    use tracing_subscriber::layer::{Layer as _, SubscriberExt};

    let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok();
    // SPEC-24 / §20: `OXIDE_LOG=json` selects structured JSON log lines; the
    // default is human-readable. Each formatter is a distinct concrete layer
    // type, so the whole subscriber is built per branch (boxed as a
    // `Subscriber`).
    let json = std::env::var("OXIDE_LOG")
        .map(|v| v.eq_ignore_ascii_case("json"))
        .unwrap_or(false);

    // V2-F24: the dev boot's log-feed capture. When the command is the
    // dev boot (`set_dev_mode` — the same gate the dev runtime rides,
    // applied pre-runtime in `apply_startup_env`), one ring is created
    // here and stashed for the serve boot (the `GET /__oxide/logs`
    // endpoint serves the same instance). The capture layer is a second
    // read on the event stream — it changes no rendering, so the console
    // output stays byte-identical (the F22 posture). Production never
    // installs the layer.
    let log_feed_capture: Option<oxide_fw_server::CaptureLayer> = if oxide_fw_server::dev_mode() {
        let ring = std::sync::Arc::new(oxide_fw_server::RingLogSink::new(
            oxide_fw_server::LOG_FEED_CAPACITY,
        ));
        let _ = LOG_FEED_RING.set(ring.clone());
        Some(oxide_fw_server::CaptureLayer::new(
            ring as std::sync::Arc<dyn oxide_fw_server::LogSink>,
        ))
    } else {
        None
    };

    // V2-F26: the feed's own level floor (`OXIDE_LOG_FEED_LEVEL`). Set,
    // the dev boot widens the feed past the host filter: the host filter
    // rides the formatter and export layers per-layer and the capture
    // layer admits its own floor — the console stays byte-identical while
    // the window captures the events the console never renders. Unset
    // keeps the F24 subscriber shape (the global filter), byte-identical.
    let feed_floor = feed_filter_level();

    macro_rules! build {
        ($otel:expr) => {{
            let host = env_filter();
            if let Some(floor) = feed_floor {
                // The widening shape: no global filter — the host filter
                // is per-layer, the feed's floor is the capture layer's
                // own gate (the gate is load-bearing: with no global
                // filter the subscriber hands every event to every layer,
                // and the gate keeps the window exactly as wide as the
                // floor).
                let gate =
                    std::sync::Arc::new(floor) as std::sync::Arc<dyn oxide_fw_server::LogGate>;
                let capture = log_feed_capture.clone().map(|layer| {
                    layer.with_gate(gate.clone()).with_filter(
                        tracing_subscriber::filter::filter_fn(move |meta| gate.admits(meta)),
                    )
                });
                if json {
                    Box::new(
                        tracing_subscriber::registry()
                            .with($otel.with_filter(host.clone()))
                            .with(capture)
                            .with(
                                tracing_subscriber::fmt::layer()
                                    .with_target(false)
                                    .json()
                                    .flatten_event(true)
                                    .with_filter(host),
                            ),
                    ) as Box<dyn tracing::Subscriber + Send + Sync>
                } else {
                    Box::new(
                        tracing_subscriber::registry()
                            .with($otel.with_filter(host.clone()))
                            .with(capture)
                            .with(
                                tracing_subscriber::fmt::layer()
                                    .with_target(false)
                                    .with_filter(host),
                            ),
                    ) as Box<dyn tracing::Subscriber + Send + Sync>
                }
            } else {
                // The F24 shape: the host filter is the registry's global
                // filter, the capture layer rides the default gate —
                // byte-identical to the pre-F26 boot.
                let base = tracing_subscriber::registry()
                    .with($otel)
                    .with(host)
                    .with(log_feed_capture.clone());
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

/// The log feed's own level floor (`OXIDE_LOG_FEED_LEVEL`, V2-F26): when
/// set to one of the five `tracing` level names (anything but the falsy
/// spellings `""`/`0`/`false`/`off`), the dev boot widens the feed past
/// the host filter — the host filter rides the formatter and export
/// layers per-layer and the capture layer admits its own floor, so the
/// console stays byte-identical while the window captures the events the
/// console never renders (a dev loop with the console at `info` and the
/// feed at `debug`). `None` — unset, a falsy spelling, or an unknown
/// name (one stderr notice, then the F24 shape) — keeps the subscriber
/// byte-identical; production never attaches the layer either way.
fn feed_filter_level() -> Option<oxide_fw_server::LevelFloor> {
    match std::env::var("OXIDE_LOG_FEED_LEVEL") {
        Ok(value) if !matches!(value.as_str(), "" | "0" | "false" | "off") => {
            let floor = oxide_fw_server::LevelFloor::parse(&value);
            if floor.is_none() {
                eprintln!(
                    "oxide: OXIDE_LOG_FEED_LEVEL={value} is not a tracing level; the feed mirrors the host filter"
                );
            }
            floor
        }
        _ => None,
    }
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

/// The trace rows' file-export gate (`OXIDE_TRACE_FILE`, V2-F25): when
/// set to anything but the falsy spellings (`""`, `0`, `false`, `off`),
/// the dev boot opens the path (`FileTraceSink::create` — a bad path
/// aborts the boot) and attaches the sink beside the ring and the
/// tracing arm — every completed request's row appends as one JSONL
/// line (the same camelCase row shape the endpoint serves). Default
/// off: no file is written, and production never attaches (the F21
/// posture holds).
pub(crate) fn trace_file_path() -> Option<std::path::PathBuf> {
    match std::env::var("OXIDE_TRACE_FILE") {
        Ok(value) if !matches!(value.as_str(), "" | "0" | "false" | "off") => {
            Some(std::path::PathBuf::from(value))
        }
        _ => None,
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
