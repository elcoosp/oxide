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
//!
//! The boot flags (V2-F22/F28/F42): `--trace-log`, `--log-feed-level`
//! and `--cache-hot-key-top` write their override slots through
//! `apply_startup_env` pre-runtime — command-level facts decided before
//! the tokio runtime exists — and win over the env spellings
//! (`OXIDE_TRACE_LOG`, `OXIDE_LOG_FEED_LEVEL`,
//! `OXIDE_CACHE_PLANE_HOT_KEY_TOP`) that remain the shell user's form.

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

/// The `--log-feed-level` boot flag's override (V2-F28), written once
/// pre-runtime by `apply_startup_env` — the `set_dev_mode` shape: a
/// command-level fact decided before the tokio runtime exists, so the
/// read side never races a mid-async write. The env var remains the
/// shell user's spelling; the flag wins over it.
static FEED_LEVEL_OVERRIDE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Install the boot flag's feed-level override (see
/// [`FEED_LEVEL_OVERRIDE`]). `None` clears it (the pins' hygiene).
pub(crate) fn set_feed_level_override(value: Option<String>) {
    if let Ok(mut slot) = FEED_LEVEL_OVERRIDE.lock() {
        *slot = value;
    }
}

fn feed_level_override() -> Option<String> {
    FEED_LEVEL_OVERRIDE
        .lock()
        .ok()
        .and_then(|slot| slot.clone())
}

/// The log feed's own level floor (V2-F26/F28): when set — by the
/// `--log-feed-level` boot flag (which wins) or the
/// `OXIDE_LOG_FEED_LEVEL` env var — to one of the five `tracing` level
/// names (anything but the falsy spellings `""`/`0`/`false`/`off`), the
/// dev boot widens the feed past the host filter — the host filter rides
/// the formatter and export layers per-layer and the capture layer admits
/// its own floor, so the console stays byte-identical while the window
/// captures the events the console never renders (a dev loop with the
/// console at `info` and the feed at `debug`). `None` — unset, a falsy
/// spelling, or an unknown name (one stderr notice naming the source,
/// then the F24 shape) — keeps the subscriber byte-identical; production
/// never attaches the layer either way.
pub(crate) fn feed_filter_level() -> Option<oxide_fw_server::LevelFloor> {
    if let Some(value) = feed_level_override() {
        return feed_floor_from(value, "--log-feed-level");
    }
    match std::env::var("OXIDE_LOG_FEED_LEVEL") {
        Ok(value) => feed_floor_from(value, "OXIDE_LOG_FEED_LEVEL"),
        _ => None,
    }
}

/// The falsy-spellings gate + the parse, shared by the flag and the env
/// var spellings; the notice names the source so a typo is findable.
fn feed_floor_from(value: String, source: &str) -> Option<oxide_fw_server::LevelFloor> {
    if matches!(value.as_str(), "" | "0" | "false" | "off") {
        return None;
    }
    let floor = oxide_fw_server::LevelFloor::parse(&value);
    if floor.is_none() {
        eprintln!(
            "oxide: {source}={value} is not a tracing level; the feed mirrors the host filter"
        );
    }
    floor
}

/// The trace rows' export gate (`OXIDE_TRACE_LOG`, V2-F22): when set to
/// anything but the falsy spellings (`""`, `0`, `false`, `off`), the
/// dev boot attaches the `TracingTraceSink` beside the ring — every
/// completed request's row leaves as one `tracing::info!` event through
/// the subscriber this module installs at boot (console, `OXIDE_LOG=json`,
/// and with `OTEL_EXPORTER_OTLP_ENDPOINT` the OTLP stream). Default off:
/// the console output stays byte-identical, and production never
/// attaches (the F21 posture holds).
///
/// The `--trace-log` boot flag (the F22 polish tail) rides the
/// [`TRACE_LOG_OVERRIDE`] slot — the flag wins when present; absent, the
/// env var keeps its shell spelling.
pub(crate) fn trace_log_enabled() -> bool {
    if let Some(flag) = trace_log_override() {
        return flag;
    }
    match std::env::var("OXIDE_TRACE_LOG") {
        Ok(value) => !matches!(value.as_str(), "" | "0" | "false" | "off"),
        Err(_) => false,
    }
}

/// The `--trace-log` boot flag's override (the F22 polish tail), written
/// once pre-runtime by `apply_startup_env` — the `FEED_LEVEL_OVERRIDE`
/// shape: a command-level fact decided before the tokio runtime exists,
/// so the read side never races a mid-async write. `None` (the flag was
/// absent) leaves the `OXIDE_TRACE_LOG` env var in charge.
static TRACE_LOG_OVERRIDE: std::sync::Mutex<Option<bool>> = std::sync::Mutex::new(None);

/// Install the boot flag's trace-export override (see
/// [`TRACE_LOG_OVERRIDE`]). `None` clears it (the pins' hygiene).
pub(crate) fn set_trace_log_override(value: Option<bool>) {
    if let Ok(mut slot) = TRACE_LOG_OVERRIDE.lock() {
        *slot = value;
    }
}

fn trace_log_override() -> Option<bool> {
    TRACE_LOG_OVERRIDE.lock().ok().and_then(|slot| *slot)
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

/// The `--cache-hot-key-top` boot flag's override (V2-F42, the F41 polish
/// tail), written once pre-runtime by `apply_startup_env` — the
/// [`FEED_LEVEL_OVERRIDE`] shape: a command-level fact decided before the
/// tokio runtime exists, so the read side never races a mid-async write.
/// `None` (the flag was absent) leaves the
/// `OXIDE_CACHE_PLANE_HOT_KEY_TOP` env var in charge (the F41 spelling).
static CACHE_HOT_KEY_TOP_OVERRIDE: std::sync::Mutex<Option<usize>> = std::sync::Mutex::new(None);

/// Install the boot flag's hot-key window override (see
/// [`CACHE_HOT_KEY_TOP_OVERRIDE`]). `None` clears it (the pins' hygiene).
///
/// # Examples
///
/// ```
/// use oxide_fw_cli::observability::set_cache_hot_key_top_override;
///
/// set_cache_hot_key_top_override(Some(50));
/// assert_eq!(oxide_fw_cli::observability::cache_hot_key_top(), 50);
/// set_cache_hot_key_top_override(None);
/// ```
pub fn set_cache_hot_key_top_override(value: Option<usize>) {
    if let Ok(mut slot) = CACHE_HOT_KEY_TOP_OVERRIDE.lock() {
        *slot = value;
    }
}

fn cache_hot_key_top_override() -> Option<usize> {
    CACHE_HOT_KEY_TOP_OVERRIDE
        .lock()
        .ok()
        .and_then(|slot| *slot)
}

/// The §20 scrape's hot-key top window (`OXIDE_CACHE_PLANE_HOT_KEY_TOP`,
/// V2-F41): the F40 scrape's top-n stops being a compile-time constant —
/// the boot resolves the operator's spelling ONCE at the
/// `set_cache_plane` wire (the `trace_file_path` posture: a boot-time
/// read, not a per-scrape env poll) and every scrape renders that
/// window. The falsy spellings (`""`/`0`/`false`/`off`) keep the F40
/// default ([`crate::framework_shim::CACHE_PLANE_HOT_KEY_TOP`]); a
/// parsed value rides verbatim, clamped to the ledger's own bound
/// (`CACHE_HOT_KEY_CAPACITY` — rows past the occupancy do not exist, so
/// the honest cap keeps one scrape bounded); anything unparsable
/// degrades loudly-but-honestly — one stderr notice naming the source,
/// then the default (the `feed_floor_from` posture).
///
/// V2-F42: the `--cache-hot-key-top` boot flag wins when present — the
/// [`CACHE_HOT_KEY_TOP_OVERRIDE`] slot is a command-level fact written
/// pre-runtime by `apply_startup_env`; absent, the env var keeps its
/// shell spelling (the `trace_log_enabled` shape).
pub fn cache_hot_key_top() -> usize {
    if let Some(top) = cache_hot_key_top_override() {
        return top;
    }
    let raw = std::env::var("OXIDE_CACHE_PLANE_HOT_KEY_TOP").ok();
    cache_hot_key_top_from(raw.as_deref())
}

/// The parse core behind [`cache_hot_key_top`]: `None` is the unset
/// spelling. Pure — the pins drive it without touching the process env;
/// the one env-read pin covers the real spelling.
pub fn cache_hot_key_top_from(raw: Option<&str>) -> usize {
    let Some(raw) = raw else {
        return crate::framework_shim::CACHE_PLANE_HOT_KEY_TOP;
    };
    if matches!(raw, "" | "0" | "false" | "off") {
        return crate::framework_shim::CACHE_PLANE_HOT_KEY_TOP;
    }
    match raw.parse::<usize>() {
        Ok(top) => top.min(crate::framework_shim::CACHE_HOT_KEY_CAPACITY),
        Err(_) => {
            eprintln!(
                "oxide: OXIDE_CACHE_PLANE_HOT_KEY_TOP={raw} is not a window size; \
                 the scrape keeps the default window"
            );
            crate::framework_shim::CACHE_PLANE_HOT_KEY_TOP
        }
    }
}

/// The V2-F43 layered resolver — the boot wire's entry point now that
/// the window has a project baseline (`cache.hotKeyTop`, the
/// `CacheConfig::hot_key_top` field) beside the F42 flag and the F41 env
/// spelling. The precedence:
///
/// 1. the `--cache-hot-key-top` flag's override slot (the F42 shape —
///    written pre-runtime by `apply_startup_env`),
/// 2. the `OXIDE_CACHE_PLANE_HOT_KEY_TOP` env spelling (the F41
///    resolver: parsed values ride clamped, garbage degrades
///    loudly-but-honestly to the DEFAULT — the notice names the source,
///    and the project baseline is never a silent recovery for a typoed
///    operator spelling),
/// 3. the `cache.hotKeyTop` baseline, clamped here at the wire to the
///    ledger's own bound (the config crate deliberately cannot see
///    [`crate::framework_shim::CACHE_HOT_KEY_CAPACITY`]),
/// 4. the F40 default — the boot shape is byte-identical when no layer
///    speaks.
///
/// The env layer's falsy spellings (`""`/`0`/`false`/`off`) stop THAT
/// layer only: a committed baseline below still answers — an operator's
/// empty spelling is not the project's config being refused.
pub fn cache_hot_key_top_with_config(config_top: Option<u64>) -> usize {
    if let Some(top) = cache_hot_key_top_override() {
        return top;
    }
    match std::env::var("OXIDE_CACHE_PLANE_HOT_KEY_TOP") {
        Ok(raw) if !matches!(raw.as_str(), "" | "0" | "false" | "off") => {
            cache_hot_key_top_from(Some(&raw))
        }
        _ => match config_top {
            Some(top) => usize::try_from(top)
                .unwrap_or(usize::MAX)
                .min(crate::framework_shim::CACHE_HOT_KEY_CAPACITY),
            None => crate::framework_shim::CACHE_PLANE_HOT_KEY_TOP,
        },
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
