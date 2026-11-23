//! TDD pins for the call-outcome observability decorator — the
//! `StatsRegistry` architectural seam the substrate's own rustdocs name
//! as the missing piece (`crates/rustfn/src/host.rs` on `RegistryReport`:
//! "the registry has no traffic counters to expose yet; the report is a
//! posture read"; `crates/rustfn/src/chain.rs` on `ChainedReport`: "the
//! chain owns no traffic counters"). The posture snapshots answer "is
//! the host attached?"; they do NOT answer "how many native calls
//! succeeded vs fell back, and which wire arm refused?" — the question
//! the server crate's `RustFirstDispatcher` and the dev overlay both
//! need to decide whether the native registry is healthy under load.
//!
//! Written BEFORE the implementation; every pin imports public API only.
//!
//! The contracts pinned here:
//!
//! - `StatsRegistry` IS-A `RustFnRegistry`: `Arc<StatsRegistry>` upcasts
//!   to `Arc<dyn RustFnRegistry>`, so the dispatcher consults ONE
//!   instrumented registry through the same object-safe seam it already
//!   wires for the boot-time host, the dev-loop host and the chain — no
//!   fork, no per-consult counter plumbing in the server crate.
//! - `call` counts every outcome: `calls_attempted` bumps on entry,
//!   `calls_served` bumps on `Ok`, and the matching per-kind counter
//!   bumps on `Err` (`not_found` / `wire` / `fn_error` / `panic` /
//!   `timeout` / `internal` — the six [`RegistryErrorKind`] arms). The
//!   inner call's result is passed through unchanged — the decorator
//!   observes, it never alters the wire.
//! - `has` / `ids` / `types_json` delegate to the inner registry
//!   uncounted (they are consult reads, not calls — the posture
//!   snapshots already own that surface).
//! - `stats()` is ONE snapshot over the counters, not N — the same
//!   "one consult surface" contract `RegistryReport` and `ChainedReport`
//!   hold. `CallStats::is_empty()` is true when no call was attempted;
//!   `CallStats::error_count()` is the derived sum of the six error
//!   arms.
//! - `reset()` zeroes the counters (the dev overlay's "refresh the
//!   window" verb; the test suite's "start a clean slate" helper).
//! - The decorator composes with `ChainedRegistry`: wrap
//!   `Arc<ChainedRegistry>` and the consult still walks the members,
//!   while every outcome lands in ONE counter set covering every native
//!   crate behind the chain.
//! - The state is `Send + Sync` (`Arc<StatsRegistry>` is the shape the
//!   dispatcher holds behind `Arc<dyn RustFnRegistry>`).

use std::sync::Arc;
use std::time::Duration;

use oxide_rustfn::abi::{ABI_VERSION, FnEntry, FnTable, RawBuf, leak_name};
use oxide_rustfn::bridge::{decode_odf, encode_odf};
use oxide_rustfn::chain::ChainedRegistry;
use oxide_rustfn::host::{RegistryErrorKind, RustFnHost, RustFnLibrary, RustFnRegistry};
use oxide_rustfn::shim::run_shim;
use oxide_rustfn::stats::{CallStats, StatsRegistry};
use oxide_rustfn::swap::{FnBinding, SwappableHost};

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
/// prefixes `v1:`, re-encodes. The shape every `#[server_fn]` body
/// generates (decode → call → encode over the C ABI).
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

/// A fn that panics — the caught-unwind path the shim maps to status 2
/// (the registry's `Panic` arm). `run_shim` catches the unwind, so the
/// panic never crosses the C boundary.
extern "C" fn panic_shim(_inp: *const u8, _len: usize, out: *mut RawBuf) -> i32 {
    let mut buf = RawBuf::empty();
    let status = run_shim(b"", &mut buf, |_| panic!("the fixture panics"));
    // SAFETY: the host always passes a writable slot.
    unsafe {
        if !out.is_null() {
            *out = buf;
        }
    }
    status
}

/// A fn that sleeps past the host's budget — the timeout arm. The
/// `rustfn_tdd::registry_call_timeout_maps_to_504_fn_timeout` pin uses
/// the same 250 ms / 50 ms shape; the sleep rides inside the
/// `spawn_blocking` task the host's call path owns, never on the test
/// thread.
extern "C" fn slow_shim(_inp: *const u8, _len: usize, _out: *mut RawBuf) -> i32 {
    std::thread::sleep(Duration::from_millis(250));
    oxide_rustfn::abi::STATUS_OK
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

fn panic_lib() -> RustFnLibrary {
    let entries: &'static [FnEntry] =
        Box::leak(vec![entry_named("panic", panic_shim)].into_boxed_slice());
    RustFnLibrary::from_static(table(entries), None).expect("a current-ABI table loads")
}

fn slow_lib() -> RustFnLibrary {
    let entries: &'static [FnEntry] =
        Box::leak(vec![entry_named("slow", slow_shim)].into_boxed_slice());
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

/// The §9.4 route's args wire: `call` takes RAW JSON (the host converts
/// to the ODF bytes the boundary carries).
fn json_string(s: &str) -> Vec<u8> {
    format!("\"{s}\"").into_bytes()
}

/// The decorator upcasts: `Arc<StatsRegistry>` IS-A `Arc<dyn
/// RustFnRegistry>`, the shape the dispatcher wires behind the same
/// object-safe seam it already consults for the boot-time host, the
/// dev-loop host and the chain. Without the impl this fails to compile
/// (the trait bound is unsatisfied).
#[test]
fn the_stats_registry_is_a_rust_fn_registry() {
    fn assert_registry<T: RustFnRegistry>() {}
    assert_registry::<StatsRegistry>();
    let decorated = Arc::new(StatsRegistry::new(echo_host()));
    let registry: Arc<dyn RustFnRegistry> = decorated;
    assert!(registry.has("id-echo"), "the decorator delegates has");
}

/// A fresh decorator has seen no traffic: `stats()` is empty, every
/// counter is zero, and the derived `error_count()` is zero.
#[test]
fn stats_starts_empty_on_a_fresh_decorator() {
    let decorated = Arc::new(StatsRegistry::new(echo_host()));
    let stats = decorated.stats();
    assert!(stats.is_empty(), "no call was attempted");
    assert_eq!(stats.calls_attempted, 0);
    assert_eq!(stats.calls_served, 0);
    assert_eq!(stats.error_count(), 0);
    assert_eq!(stats.not_found, 0);
    assert_eq!(stats.wire, 0);
    assert_eq!(stats.fn_error, 0);
    assert_eq!(stats.panic, 0);
    assert_eq!(stats.timeout, 0);
    assert_eq!(stats.internal, 0);
}

/// A served call bumps `calls_attempted` and `calls_served`; the error
/// arms stay zero. The inner call's bytes pass through unchanged.
#[tokio::test]
async fn a_served_call_increments_attempted_and_served() {
    let decorated = Arc::new(StatsRegistry::new(echo_host()));
    let registry: Arc<dyn RustFnRegistry> = decorated.clone();
    let out = registry
        .call("id-echo", &json_string("hi"))
        .await
        .expect("the inner call answers");
    assert_eq!(
        decode_odf::<String>(&out).expect("odf reply"),
        "v1:hi",
        "the decorator passes the bytes through"
    );
    let stats = decorated.stats();
    assert!(!stats.is_empty());
    assert_eq!(stats.calls_attempted, 1);
    assert_eq!(stats.calls_served, 1);
    assert_eq!(stats.error_count(), 0);
}

/// A call on an id no member owns refuses `NotFound` — the
/// `not_found` counter bumps, `calls_served` stays zero.
#[tokio::test]
async fn a_not_found_call_increments_the_not_found_counter() {
    let decorated = Arc::new(StatsRegistry::new(echo_host()));
    let registry: Arc<dyn RustFnRegistry> = decorated.clone();
    let err = registry
        .call("id-missing", b"{}")
        .await
        .expect_err("no fn owns the id");
    assert!(matches!(err.kind, RegistryErrorKind::NotFound));
    let stats = decorated.stats();
    assert_eq!(stats.calls_attempted, 1);
    assert_eq!(stats.calls_served, 0);
    assert_eq!(stats.not_found, 1);
    assert_eq!(stats.error_count(), 1);
}

/// Garbage args JSON refuses `Wire` (the host's ODF conversion fails
/// before the boundary) — the `wire` counter bumps.
#[tokio::test]
async fn a_wire_error_call_increments_the_wire_counter() {
    let decorated = Arc::new(StatsRegistry::new(echo_host()));
    let registry: Arc<dyn RustFnRegistry> = decorated.clone();
    let err = registry
        .call("id-echo", b"not json")
        .await
        .expect_err("wire");
    assert!(matches!(err.kind, RegistryErrorKind::Wire));
    let stats = decorated.stats();
    assert_eq!(stats.calls_attempted, 1);
    assert_eq!(stats.wire, 1);
    assert_eq!(stats.error_count(), 1);
}

/// A fn that returns `Err` refuses `FnError` — the `fn_error` counter
/// bumps.
#[tokio::test]
async fn a_fn_error_call_increments_the_fn_error_counter() {
    let mut host = RustFnHost::new();
    let index = host.mount(refusing_lib());
    host.register("id-refuse", index, "refuse");
    let decorated = Arc::new(StatsRegistry::new(Arc::new(host)));
    let registry: Arc<dyn RustFnRegistry> = decorated.clone();
    let err = registry
        .call("id-refuse", b"{}")
        .await
        .expect_err("the fn refuses");
    assert!(matches!(err.kind, RegistryErrorKind::FnError));
    let stats = decorated.stats();
    assert_eq!(stats.calls_attempted, 1);
    assert_eq!(stats.fn_error, 1);
    assert_eq!(stats.error_count(), 1);
}

/// A fn that panics refuses `Panic` (the shim catches the unwind) — the
/// `panic` counter bumps.
#[tokio::test]
async fn a_panic_call_increments_the_panic_counter() {
    let mut host = RustFnHost::new();
    let index = host.mount(panic_lib());
    host.register("id-panic", index, "panic");
    let decorated = Arc::new(StatsRegistry::new(Arc::new(host)));
    let registry: Arc<dyn RustFnRegistry> = decorated.clone();
    let err = registry
        .call("id-panic", b"{}")
        .await
        .expect_err("the fn panics");
    assert!(matches!(err.kind, RegistryErrorKind::Panic));
    let stats = decorated.stats();
    assert_eq!(stats.calls_attempted, 1);
    assert_eq!(stats.panic, 1);
    assert_eq!(stats.error_count(), 1);
}

/// A fn that exceeds the per-function budget refuses `Timeout` — the
/// `timeout` counter bumps. The host's `with_timeout` sizes the budget;
/// the decorator wraps the host, so the inner call's `Timeout` outcome
/// lands in the counter the same way every other arm does.
#[tokio::test]
async fn a_timeout_call_increments_the_timeout_counter() {
    let mut host = RustFnHost::new().with_timeout(Duration::from_millis(50));
    let index = host.mount(slow_lib());
    host.register("id-slow", index, "slow");
    let decorated = Arc::new(StatsRegistry::new(Arc::new(host)));
    let registry: Arc<dyn RustFnRegistry> = decorated.clone();
    let err = registry
        .call("id-slow", b"{}")
        .await
        .expect_err("the fn is over budget");
    assert!(matches!(err.kind, RegistryErrorKind::Timeout));
    let stats = decorated.stats();
    assert_eq!(stats.calls_attempted, 1);
    assert_eq!(stats.timeout, 1);
    assert_eq!(stats.error_count(), 1);
}

/// `has`, `ids` and `types_json` delegate to the inner registry
/// uncounted — they are consult reads, not calls, so they never bump
/// `calls_attempted`. The posture snapshots already own that surface.
#[test]
fn the_decorator_delegates_has_ids_and_types_json_without_counting() {
    let decorated = Arc::new(StatsRegistry::new(echo_host()));
    assert!(decorated.has("id-echo"));
    assert!(!decorated.has("id-missing"));
    assert_eq!(decorated.ids(), vec!["id-echo".to_string()]);
    assert!(decorated.types_json().is_none(), "the echo lib is untyped");
    // The consult reads did not bump the call counter.
    assert!(decorated.stats().is_empty());
}

/// `reset()` zeroes the counters — the dev overlay's "refresh the
/// window" verb. After a served call, `reset()` clears the slate and the
/// next snapshot is empty again.
#[tokio::test]
async fn reset_zeroes_the_counters() {
    let decorated = Arc::new(StatsRegistry::new(echo_host()));
    let registry: Arc<dyn RustFnRegistry> = decorated.clone();
    let _ = registry
        .call("id-echo", &json_string("one"))
        .await
        .expect("the first call answers");
    assert_eq!(decorated.stats().calls_attempted, 1);
    decorated.reset();
    let stats = decorated.stats();
    assert!(stats.is_empty(), "reset cleared the slate");
    assert_eq!(stats.calls_served, 0);
}

/// The decorator composes with `ChainedRegistry`: wrap the chain and
/// every outcome across every member lands in ONE counter set. The
/// consult still walks the members in push order; the decorator only
/// observes the final outcome.
#[tokio::test]
async fn the_decorator_composes_with_the_chained_registry() {
    let mut a = RustFnHost::new();
    let idx_a = a.mount(echo_lib());
    a.register("id-a", idx_a, "echo");
    let chain = Arc::new(ChainedRegistry::from_members(vec![Arc::new(a)]));
    let decorated = Arc::new(StatsRegistry::new(chain));
    let registry: Arc<dyn RustFnRegistry> = decorated.clone();
    let out = registry
        .call("id-a", &json_string("chained"))
        .await
        .expect("the chain answers through the decorator");
    assert_eq!(decode_odf::<String>(&out).expect("odf"), "v1:chained");
    // A miss on the chain refuses NotFound — the decorator counts it.
    let _ = registry
        .call("id-missing", b"{}")
        .await
        .expect_err("no member owns the id");
    let stats = decorated.stats();
    assert_eq!(stats.calls_attempted, 2);
    assert_eq!(stats.calls_served, 1);
    assert_eq!(stats.not_found, 1);
    assert_eq!(stats.error_count(), 1);
}

/// The decorator also wraps the dev-loop swappable host — the same
/// consult surface the dispatcher wires for the dev task. A served call
/// through the swap path bumps the counters the same way the boot-time
/// path does.
#[tokio::test]
async fn the_decorator_wraps_the_swappable_host() {
    let host = Arc::new(SwappableHost::new());
    host.swap(echo_lib(), &[FnBinding::new("id-echo", "echo")])
        .expect("the swap mounts");
    let decorated = Arc::new(StatsRegistry::new(host));
    let registry: Arc<dyn RustFnRegistry> = decorated.clone();
    let _ = registry
        .call("id-echo", &json_string("swap"))
        .await
        .expect("the swappable host answers");
    let stats = decorated.stats();
    assert_eq!(stats.calls_attempted, 1);
    assert_eq!(stats.calls_served, 1);
}

/// `inner()` exposes the wrapped registry handle — the dev overlay's
/// "introspect the underlying consult" seam (so it can render the
/// posture snapshot AND the call stats from one wired handle).
#[test]
fn inner_exposes_the_wrapped_registry() {
    let host = echo_host();
    let host_dyn: Arc<dyn RustFnRegistry> = host;
    let decorated = Arc::new(StatsRegistry::new(Arc::clone(&host_dyn)));
    let inner: &Arc<dyn RustFnRegistry> = decorated.inner();
    assert!(Arc::ptr_eq(inner, &host_dyn));
}

/// `CallStats` is `Clone + Default` (the snapshot shape the posture
/// reports hold — the dev overlay diffs two snapshots, and `Default` is
/// the "no traffic yet" baseline). A populated snapshot differs from the
/// default; a cloned snapshot equals its original.
#[tokio::test]
async fn call_stats_is_clone_eq_and_default() {
    fn assert_clone_default<T: Clone + Default + PartialEq + Eq>() {}
    assert_clone_default::<CallStats>();
    let default = CallStats::default();
    assert!(default.is_empty());
    let decorated = Arc::new(StatsRegistry::new(echo_host()));
    let registry: Arc<dyn RustFnRegistry> = decorated.clone();
    let _ = registry
        .call("id-echo", &json_string("one"))
        .await
        .expect("the call answers");
    let snapshot = decorated.stats();
    let cloned = snapshot.clone();
    assert_eq!(snapshot, cloned);
    assert_ne!(
        default, snapshot,
        "a populated snapshot differs from default"
    );
}

/// The decorator state is `Send + Sync` and the shared handle crosses
/// threads — the shape the dispatcher holds behind
/// `Arc<dyn RustFnRegistry>`.
#[test]
fn the_decorator_state_is_send_sync_and_shareable() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Arc<StatsRegistry>>();
    let decorated = Arc::new(StatsRegistry::new(echo_host()));
    let moved = Arc::clone(&decorated);
    std::thread::spawn(move || {
        assert!(moved.has("id-echo"));
    })
    .join()
    .expect("the shared handle crosses threads");
    assert!(decorated.stats().is_empty(), "has does not count");
}
