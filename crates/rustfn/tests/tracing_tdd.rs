//! TDD pins for the per-call tracing decorator — the
//! `TracingRegistry` architectural seam the cache plane's
//! `TracingCacheObserver` (F22) already lands for cache events and the
//! rustfn registry story names as the missing observability arm: the
//! `StatsRegistry` (M4) answers "how many calls succeeded vs fell back"
//! (a counter snapshot), but NO decorator answers "which id ran, how
//! long it took, and what outcome it returned" — the per-call
//! structured-trace surface the dev overlay and the operator's
//! `oxide::rustfn` log scrape need to attribute one slow native call
//! inside a mixed consult. The cache plane's answer is
//! `TracingCacheObserver` (one `tracing` event per cache op under
//! `oxide::cache`); this module is that arm for the registry plane.
//!
//! Written BEFORE the implementation; every pin imports public API only.
//!
//! The contracts pinned here:
//!
//! - `TracingRegistry` IS-A `RustFnRegistry`: `Arc<TracingRegistry>`
//!   upcasts to `Arc<dyn RustFnRegistry>`, so the dispatcher consults
//!   ONE traced registry through the same object-safe seam it already
//!   wires for the boot-time host, the dev-loop host, the chain and the
//!   stats decorator — no fork, no per-consult tracing plumbing in the
//!   server crate.
//! - `call` opens ONE `tracing` span per native call (target
//!   `oxide::rustfn`, the id and the outcome kind as fields) and the
//!   inner call's result — bytes or `RegistryError` — passes through
//!   UNCHANGED. The decorator observes, it never alters the wire (the
//!   same "observe, never alter" rule `StatsRegistry` holds).
//! - `has` / `ids` / `types_json` delegate to the inner registry
//!   UNTRACED (they are consult reads, not calls — a span per `has`
//!   would drown the call spans in consult noise, the same carve-out
//!   `StatsRegistry` holds).
//! - The decorator composes with `StatsRegistry`: wrap an
//!   `Arc<StatsRegistry>` and the consult still counts AND traces —
//!   every outcome lands in ONE `CallStats` snapshot AND ONE tracing
//!   span, the same "one consult surface" composition the stats
//!   rustdoc names for `ChainedRegistry`.
//! - The state is `Send + Sync` (`Arc<TracingRegistry>` is the shape
//!   the dispatcher holds behind `Arc<dyn RustFnRegistry>`).

use std::fmt::Write as _;
use std::sync::Arc;

use oxide_rustfn::abi::{ABI_VERSION, FnEntry, FnTable, RawBuf, leak_name};
use oxide_rustfn::bridge::{decode_odf, encode_odf};
use oxide_rustfn::host::{RegistryErrorKind, RustFnHost, RustFnLibrary, RustFnRegistry};
use oxide_rustfn::shim::run_shim;
use oxide_rustfn::stats::StatsRegistry;
use oxide_rustfn::tracing::TracingRegistry;

/// Build a `&'static FnTable` over `entries` (the test-fixture pattern
/// the host and swap pins use: the table is leaked, the host keeps the
/// pointer for the library's lifetime).
fn table(entries: &'static [FnEntry]) -> &'static FnTable {
    Box::leak(Box::new(FnTable {
        abi_version: ABI_VERSION,
        len: entries.len(),
        entries: entries.as_ptr(),
        free: free_impl,
    }))
}

extern "C" fn free_impl(buf: RawBuf) {
    let _ = buf;
}

fn entry_named(
    name: &'static str,
    call: extern "C" fn(*const u8, usize, *mut RawBuf) -> i32,
) -> FnEntry {
    FnEntry {
        name: leak_name(name),
        call,
    }
}

/// The echo shim the served-call pin drives: decodes the ODF string,
/// prefixes `v1:`, re-encodes.
extern "C" fn echo_v1(inp: *const u8, len: usize, out: *mut RawBuf) -> i32 {
    let input: &[u8] = if inp.is_null() || len == 0 {
        &[]
    } else {
        // SAFETY: the host passes a live `len`-byte buffer.
        unsafe { std::slice::from_raw_parts(inp, len) }
    };
    let mut buf = RawBuf::empty();
    let status = run_shim(input, &mut buf, |bytes| {
        let text: String = decode_odf(bytes).map_err(|e| e.to_string())?;
        encode_odf(&format!("v1:{text}")).map_err(|e| e.to_string())
    });
    // SAFETY: the host always passes a writable slot.
    unsafe {
        if !out.is_null() {
            *out = buf;
        }
    }
    status
}

/// A fn that returns `Err` — the status-1 path the envelope maps to
/// 400 `FN-THREW` (the registry's `FnError` arm).
extern "C" fn refusing_shim(_inp: *const u8, _len: usize, out: *mut RawBuf) -> i32 {
    let mut buf = RawBuf::empty();
    let status = run_shim(b"", &mut buf, |_| {
        Err("the width must be positive".to_string())
    });
    // SAFETY: the host always passes a writable slot.
    unsafe {
        if !out.is_null() {
            *out = buf;
        }
    }
    status
}

fn echo_lib() -> RustFnLibrary {
    let entries: &'static [FnEntry] =
        Box::leak(vec![entry_named("echo", echo_v1)].into_boxed_slice());
    RustFnLibrary::from_static(table(entries), None).expect("a current-ABI table loads")
}

fn refusing_lib() -> RustFnLibrary {
    let entries: &'static [FnEntry] =
        Box::leak(vec![entry_named("refuse", refusing_shim)].into_boxed_slice());
    RustFnLibrary::from_static(table(entries), None).expect("a current-ABI table loads")
}

/// A boot-time host with one mounted echo library, `id-echo` registered
/// at the `echo` export — the inner registry the decorator wraps.
fn echo_host() -> Arc<RustFnHost> {
    let mut host = RustFnHost::new();
    let index = host.mount(echo_lib());
    host.register("id-echo", index, "echo");
    Arc::new(host)
}

/// A boot-time host whose only export refuses — the `FnError` arm.
fn refusing_host() -> Arc<RustFnHost> {
    let mut host = RustFnHost::new();
    let index = host.mount(refusing_lib());
    host.register("id-refuse", index, "refuse");
    Arc::new(host)
}

/// The §9.4 route's args wire: `call` takes RAW JSON (the host converts
/// to the ODF bytes the boundary carries).
fn json_string(s: &str) -> Vec<u8> {
    format!("\"{s}\"").into_bytes()
}

/// The decorator upcasts: `Arc<TracingRegistry>` IS-A `Arc<dyn
/// RustFnRegistry>`, the shape the dispatcher wires behind the same
/// object-safe seam it already consults for the boot-time host, the
/// dev-loop host, the chain and the stats decorator. Without the impl
/// this fails to compile (the trait bound is unsatisfied).
#[test]
fn the_tracing_registry_is_a_rust_fn_registry() {
    fn assert_registry<T: RustFnRegistry>() {}
    assert_registry::<TracingRegistry>();
    let decorated = Arc::new(TracingRegistry::new(echo_host()));
    let registry: Arc<dyn RustFnRegistry> = decorated;
    assert!(registry.has("id-echo"), "the decorator delegates has");
}

/// A served call passes its bytes through UNCHANGED — the decorator
/// observes, it never alters the wire. The echo fn prefixes `v1:`; the
/// traced call returns the same bytes the inner host would.
#[tokio::test]
async fn a_served_call_passes_the_bytes_through_unchanged() {
    let decorated = Arc::new(TracingRegistry::new(echo_host()));
    let registry: Arc<dyn RustFnRegistry> = decorated;
    let out = registry
        .call("id-echo", &json_string("hello"))
        .await
        .expect("the traced call serves");
    let text: String = decode_odf(&out).expect("the bytes decode");
    assert_eq!(text, "v1:hello", "the decorator did not alter the wire");
}

/// A call that the inner registry refuses (`FnError`) passes the
/// `RegistryError` through UNCHANGED — the decorator traces the
/// refusal, it does not swallow or rewrite it.
#[tokio::test]
async fn a_fn_error_passes_the_registry_error_through_unchanged() {
    let decorated = Arc::new(TracingRegistry::new(refusing_host()));
    let registry: Arc<dyn RustFnRegistry> = decorated;
    let err = registry
        .call("id-refuse", b"{}")
        .await
        .expect_err("the traced call refuses");
    assert_eq!(err.kind, RegistryErrorKind::FnError);
    assert_eq!(err.message, "the width must be positive");
}

/// A call to an id no member owns refuses with `NotFound` — the
/// decorator delegates the miss through, it does not fabricate a
/// served outcome.
#[tokio::test]
async fn a_not_found_passes_the_not_found_error_through_unchanged() {
    let decorated = Arc::new(TracingRegistry::new(echo_host()));
    let registry: Arc<dyn RustFnRegistry> = decorated;
    let err = registry
        .call("id-missing", &json_string("x"))
        .await
        .expect_err("the traced call misses");
    assert_eq!(err.kind, RegistryErrorKind::NotFound);
}

/// `has` / `ids` / `types_json` delegate UNTRACED — they are consult
/// reads, not calls (the same carve-out `StatsRegistry` holds: a span
/// per `has` would drown the call spans in consult noise).
#[test]
fn consult_reads_delegate_untraced() {
    let decorated = Arc::new(TracingRegistry::new(echo_host()));
    assert!(decorated.has("id-echo"));
    assert!(!decorated.has("id-missing"));
    assert_eq!(decorated.ids(), vec!["id-echo".to_string()]);
    assert_eq!(decorated.types_json(), None);
}

/// The decorator composes with `StatsRegistry`: wrap an
/// `Arc<StatsRegistry>` and the consult STILL counts AND traces —
/// every outcome lands in ONE `CallStats` snapshot AND ONE tracing
/// span. This is the "one consult surface" composition the stats
/// rustdoc names for `ChainedRegistry` (wrap a chain, every outcome
/// across every member lands in one counter set), lifted to the
/// tracing arm.
#[tokio::test]
async fn the_decorator_composes_with_stats_registry() {
    let stats = Arc::new(StatsRegistry::new(echo_host()));
    let traced = Arc::new(TracingRegistry::new(stats.clone()));
    let registry: Arc<dyn RustFnRegistry> = traced;
    let _ = registry
        .call("id-echo", &json_string("hi"))
        .await
        .expect("the composed call serves");
    // The stats decorator (the inner registry) counted the call.
    assert_eq!(stats.stats().calls_served, 1);
    assert_eq!(stats.stats().calls_attempted, 1);
}

/// A `tracing` span is emitted per native call, under the
/// `oxide::rustfn` target, carrying the id and the outcome. This is the
/// acceptance pin: a tracing subscriber that captures the span sees one
/// event per `call`, and NO event for a consult read (`has`).
#[tokio::test]
async fn a_call_emits_one_tracing_span_under_the_oxide_rustfn_target() {
    use tracing_subscriber::layer::SubscriberExt;

    let (span_tx, span_rx) = std::sync::mpsc::channel::<String>();
    let span_collector = SpanCollector { tx: span_tx };
    let _guard =
        tracing::subscriber::set_default(tracing_subscriber::registry().with(span_collector));
    let decorated = Arc::new(TracingRegistry::new(echo_host()));
    let registry: Arc<dyn RustFnRegistry> = decorated;
    // A consult read emits NO span.
    let _ = registry.has("id-echo");
    assert!(
        span_rx.try_recv().is_err(),
        "a consult read emits no tracing span"
    );
    // A served call emits ONE span.
    let _ = registry
        .call("id-echo", &json_string("traced"))
        .await
        .expect("the traced call serves");
    let span = span_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("a served call emitted a tracing span");
    assert!(
        span.contains("id-echo"),
        "the span carries the id: {span:?}"
    );
    assert!(
        span.contains("served"),
        "the span carries the served outcome: {span:?}"
    );
}

/// A `tracing` span for a refused call carries the refusal outcome
/// (the `FnError` arm), so an operator's log scrape attributes the
/// 400 `FN-THREW` back to the native id.
#[tokio::test]
async fn a_refused_call_span_carries_the_refusal_outcome() {
    use tracing_subscriber::layer::SubscriberExt;

    let (span_tx, span_rx) = std::sync::mpsc::channel::<String>();
    let _guard = tracing::subscriber::set_default(
        tracing_subscriber::registry().with(SpanCollector { tx: span_tx }),
    );
    let decorated = Arc::new(TracingRegistry::new(refusing_host()));
    let registry: Arc<dyn RustFnRegistry> = decorated;
    let _ = registry
        .call("id-refuse", b"{}")
        .await
        .expect_err("the traced call refuses");
    let span = span_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("a refused call emitted a tracing span");
    assert!(
        span.contains("id-refuse"),
        "the span carries the id: {span:?}"
    );
    assert!(
        span.contains("fn_error") || span.contains("refused"),
        "the span carries the refusal outcome: {span:?}"
    );
}

/// The state is `Send + Sync`: `Arc<TracingRegistry>` is the shape the
/// dispatcher holds behind `Arc<dyn RustFnRegistry>`, so it must move
/// across threads and share behind a single `Arc`.
#[test]
fn the_tracing_registry_state_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<TracingRegistry>();
    let decorated = Arc::new(TracingRegistry::new(echo_host()));
    let _handle: Arc<dyn RustFnRegistry> = decorated;
}

/// A `tracing` layer that records the span name (the message +
/// `oxide::rustfn` target) for every span/event the decorator emits.
/// The layer is `Send + Sync` (the `tracing` contract) so the
/// `set_default` guard compiles.
struct SpanCollector {
    tx: std::sync::mpsc::Sender<String>,
}

impl<S> tracing_subscriber::Layer<S> for SpanCollector
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);
        let _ = self.tx.send(visitor.0);
    }
}

struct MessageVisitor(String);

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let _ = write!(self.0, "{}={:?} ", field.name(), value);
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        let _ = write!(self.0, "{}={value} ", field.name());
    }
}
