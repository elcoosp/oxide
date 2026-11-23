//! TDD pins for the M1 architectural seam `RustFnRegistry for
//! SwappableHost` (the documented "future `NativeFnSource` adapter
//! shape" at `crates/rustfn/src/swap.rs:90`).
//!
//! [`SwappableHost`] already exposes the synchronous `call_now` /
//! `has` / `ids` surface the dev loop drives, and its doc states
//! "`Arc<SwappableHost>` is the shape the dev task and the future
//! `NativeFnSource` adapter share." The object-safe
//! [`RustFnRegistry`] trait is the seam the server crate's
//! `RustFirstDispatcher` consults behind `Arc<dyn RustFnRegistry>`,
//! and only [`RustFnHost`] implements it today. This milestone closes
//! that gap so the dev-loop host and the production host share ONE
//! consult surface — the dispatcher no longer forks between a
//! boot-time registry and a dev-loop adapter.
//!
//! Written BEFORE the implementation; every pin imports public API
//! only. The contracts pinned here:
//!
//! - `has` consults the live swap state (an id the loop dropped is
//!   `false`, the JS-pool fallback owns it again).
//! - `call` (async) resolves the library `Arc` UNDER the swap lock
//!   and runs the call OUTSIDE it — the same two-phase shape
//!   `call_now` runs, lifted onto the runtime (`spawn_blocking` +
//!   the per-function timeout the host already carries).
//! - `call` honors the registry's wire mapping: unknown id →
//!   `NotFound`; the fn's own `Err` → `FnError`; a panic → `Panic`;
//!   an over-budget call → `Timeout`.
//! - `types_json` returns the first mounted library's payload, or
//!   `None` when no library carries one (the host's re-export,
//!   consulted through the swap lock).
//! - `Arc<SwappableHost>` upcasts to `Arc<dyn RustFnRegistry>` —
//!   the consult the dispatcher runs.

use std::sync::Arc;
use std::time::Duration;

use oxide_rustfn::abi::{ABI_VERSION, FnEntry, FnTable, RawBuf, STATUS_OK, leak_name};
use oxide_rustfn::bridge::{decode_odf, encode_odf};
use oxide_rustfn::host::{RegistryErrorKind, RustFnHost, RustFnLibrary, RustFnRegistry};
use oxide_rustfn::shim::run_shim;
use oxide_rustfn::swap::{FnBinding, SwappableHost};

/// Build a `&'static FnTable` over `entries` (the test-fixture pattern
/// the host pins use: the table is leaked, the host keeps the pointer
/// for the library's lifetime).
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

/// The two echo generations the registry pins assert through: distinct
/// module-level `extern "C"` fns (a shim cannot capture), each replying
/// with its generation prefix.
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
        let reply = format!("v1:{text}");
        encode_odf(&reply).map_err(|e| e.to_string())
    });
    // SAFETY: the host always passes a writable slot.
    unsafe {
        if !out.is_null() {
            *out = buf;
        }
    }
    status
}

extern "C" fn echo_v2(inp: *const u8, len: usize, out: *mut RawBuf) -> i32 {
    let input: &[u8] = if inp.is_null() || len == 0 {
        &[]
    } else {
        // SAFETY: the host passes a live `len`-byte buffer.
        unsafe { std::slice::from_raw_parts(inp, len) }
    };
    let mut buf = RawBuf::empty();
    let status = run_shim(input, &mut buf, |bytes| {
        let text: String = decode_odf(bytes).map_err(|e| e.to_string())?;
        let reply = format!("v2:{text}");
        encode_odf(&reply).map_err(|e| e.to_string())
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

fn v1_lib() -> RustFnLibrary {
    let entries: &'static [FnEntry] =
        Box::leak(vec![entry_named("echo", echo_v1)].into_boxed_slice());
    RustFnLibrary::from_static(table(entries), None).expect("a current-ABI table loads")
}

fn v2_lib() -> RustFnLibrary {
    let entries: &'static [FnEntry] =
        Box::leak(vec![entry_named("echo", echo_v2)].into_boxed_slice());
    RustFnLibrary::from_static(table(entries), None).expect("a current-ABI table loads")
}

/// A library that carries a types payload (the `from_static` second
/// arg), so the `types_json` pin can assert the consult re-exports it.
fn v2_lib_with_types(types: &'static str) -> RustFnLibrary {
    let entries: &'static [FnEntry] =
        Box::leak(vec![entry_named("echo", echo_v2)].into_boxed_slice());
    RustFnLibrary::from_static(table(entries), Some(types)).expect("a current-ABI table loads")
}

/// A library that exports a refusing fn, for the `FnError` wire arm.
fn refusing_lib() -> RustFnLibrary {
    let entries: &'static [FnEntry] =
        Box::leak(vec![entry_named("refuse", refusing_shim)].into_boxed_slice());
    RustFnLibrary::from_static(table(entries), None).expect("a current-ABI table loads")
}

/// A fn that sleeps past the host's budget — the timeout arm. The
/// `rustfn_tdd::registry_call_timeout_maps_to_504_fn_timeout` pin uses
/// the same 250 ms / 50 ms shape; the sleep rides inside the
/// `spawn_blocking` task the host's call path owns, never on the test
/// thread.
extern "C" fn slow_shim(_inp: *const u8, _len: usize, _out: *mut RawBuf) -> i32 {
    std::thread::sleep(Duration::from_millis(250));
    STATUS_OK
}

/// A library that exports the slow fn, for the `Timeout` wire arm.
fn slow_lib() -> RustFnLibrary {
    let entries: &'static [FnEntry] =
        Box::leak(vec![entry_named("slow", slow_shim)].into_boxed_slice());
    RustFnLibrary::from_static(table(entries), None).expect("a current-ABI table loads")
}

fn slow_binding(id: &str) -> FnBinding {
    FnBinding::new(id, "slow")
}

fn echo_binding(id: &str) -> FnBinding {
    FnBinding::new(id, "echo")
}

/// The §9.4 route's args wire: `call` takes RAW JSON (the host converts
/// to the ODF bytes the boundary carries), so the pins speak JSON
/// strings.
fn json_string(s: &str) -> Vec<u8> {
    format!("\"{s}\"").into_bytes()
}

/// A single-thread runtime, the shape the host's own `call` pins and
/// the example drive — the registry's `call` is async and the pin
/// needs a runtime to poll it.
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("the pin runtime")
}

/// The consult upcasts: `Arc<SwappableHost>` IS-A `Arc<dyn
/// RustFnRegistry>`, the shape the dispatcher wires. This is the
/// milestone's headline contract — without the impl this fails to
/// compile (the trait bound is unsatisfied).
#[test]
fn the_swappable_host_is_a_rust_fn_registry() {
    fn assert_registry<T: RustFnRegistry>() {}
    assert_registry::<SwappableHost>();
    let host = Arc::new(SwappableHost::new());
    let registry: Arc<dyn RustFnRegistry> = host;
    assert!(!registry.has("id-echo"), "an empty host owns nothing");
}

/// `has` consults the live swap state: an id the loop dropped is
/// `false`, the JS-pool fallback owns it again. The dispatcher consult
/// reads `has` before a JS dispatch, so a hot-swapped-away id MUST NOT
/// answer natively.
#[test]
fn has_reflects_the_live_swap_state() {
    let host = Arc::new(SwappableHost::new());
    host.swap(
        v1_lib(),
        &[echo_binding("id-echo"), echo_binding("id-gone")],
    )
    .expect("two ids mount");
    let registry: Arc<dyn RustFnRegistry> = host.clone();
    assert!(registry.has("id-echo"));
    assert!(registry.has("id-gone"));
    host.swap(v2_lib(), &[echo_binding("id-echo")])
        .expect("the reduced swap drops id-gone");
    assert!(registry.has("id-echo"), "the re-pointed id stays");
    assert!(
        !registry.has("id-gone"),
        "the dropped id is no longer native-owned"
    );
}

/// `call` (async) runs the fn through the registry's wire mapping:
/// the args JSON → ODF at the boundary → the fn → ODF back, the same
/// shape `RustFnHost::call` runs. The reply decodes to the v2 prefix
/// after a hot-swap re-points the id.
#[test]
fn call_serves_the_hot_swapped_behavior() {
    let rt = runtime();
    let host = Arc::new(SwappableHost::new());
    host.swap(v1_lib(), &[echo_binding("id-echo")])
        .expect("v1 mounts");
    let registry: Arc<dyn RustFnRegistry> = host.clone();
    let out = rt
        .block_on(registry.call("id-echo", &json_string("hello")))
        .expect("v1 answers");
    let reply: String = decode_odf(&out).expect("odf reply");
    assert_eq!(reply, "v1:hello");

    let report = host
        .swap(v2_lib(), &[echo_binding("id-echo")])
        .expect("the swap re-points");
    assert_eq!(report.generation, 2);
    let out = rt
        .block_on(registry.call("id-echo", &json_string("hello")))
        .expect("v2 answers");
    let reply: String = decode_odf(&out).expect("odf reply");
    assert_eq!(reply, "v2:hello");
}

/// `call` honors the registry's `NotFound` arm: an id the host never
/// knew (or dropped) refuses with `RegistryErrorKind::NotFound`, the
/// 404 the dispatcher maps on the wire.
#[test]
fn call_on_an_unknown_id_is_not_found() {
    let rt = runtime();
    let host = Arc::new(SwappableHost::new());
    let registry: Arc<dyn RustFnRegistry> = host;
    let err = rt
        .block_on(registry.call("id-missing", b"{}"))
        .expect_err("nothing is mounted");
    assert!(
        matches!(err.kind, RegistryErrorKind::NotFound),
        "got {:?}",
        err.kind
    );
}

/// `call` honors the registry's `FnError` arm: a fn that returns `Err`
/// surfaces as `RegistryErrorKind::FnError` with the fn's message, the
/// 400 `FN-THREW` the envelope maps.
#[test]
fn call_surfaces_the_fn_error_message() {
    let rt = runtime();
    let host = Arc::new(SwappableHost::new());
    host.swap(refusing_lib(), &[FnBinding::new("id-refuse", "refuse")])
        .expect("the refusing lib mounts");
    let registry: Arc<dyn RustFnRegistry> = host;
    let err = rt
        .block_on(registry.call("id-refuse", b"{}"))
        .expect_err("the fn refused");
    assert!(
        matches!(err.kind, RegistryErrorKind::FnError),
        "got {:?}",
        err.kind
    );
    assert!(
        err.message.contains("the width must be positive"),
        "the fn's message rides: {err}"
    );
}

/// `call` honors the registry's `Wire` arm: args that are not JSON
/// refuse before the boundary is crossed, the 400 `FN-BAD-REQUEST`.
#[test]
fn call_on_bad_json_args_is_a_wire_error() {
    let rt = runtime();
    let host = Arc::new(SwappableHost::new());
    host.swap(v1_lib(), &[echo_binding("id-echo")])
        .expect("the echo lib mounts");
    let registry: Arc<dyn RustFnRegistry> = host;
    let err = rt
        .block_on(registry.call("id-echo", b"not json"))
        .expect_err("the args did not parse");
    assert!(
        matches!(err.kind, RegistryErrorKind::Wire),
        "got {:?}",
        err.kind
    );
}

/// `call` honors the registry's `Timeout` arm: a fn that exceeds the
/// per-function budget refuses with `RegistryErrorKind::Timeout`, the
/// 504 the envelope maps. The host with `with_timeout` sizes the
/// budget; the swap state carries the host's timeout through the
/// consult (built via `SwappableHost::with_host` so the configured
/// budget rides the swap lock the consult reads). The slow shim sleeps
/// 250 ms inside the `spawn_blocking` task the host's call path owns;
/// the 50 ms budget refuses before it returns — the clock pin, not just
/// the type.
#[test]
fn call_over_budget_is_a_timeout() {
    let rt = runtime();
    let host = Arc::new(SwappableHost::with_host(
        RustFnHost::new().with_timeout(Duration::from_millis(50)),
    ));
    host.swap(slow_lib(), &[slow_binding("id-slow")])
        .expect("the slow lib mounts");
    let registry: Arc<dyn RustFnRegistry> = host;
    let err = rt
        .block_on(registry.call("id-slow", b"{}"))
        .expect_err("the fn is over budget");
    assert!(matches!(err.kind, RegistryErrorKind::Timeout));
    assert_eq!(err.status(), 504);
    assert_eq!(err.envelope_code(), "FN-TIMEOUT");
}

/// `types_json` re-exports the first mounted library's payload (the
/// same consult the host exposes), consulted through the swap lock so
/// a hot-swap that replaces the library is consistent with `has`.
#[test]
fn types_json_returns_the_first_mounted_payload() {
    let host = Arc::new(SwappableHost::new());
    host.swap(
        v2_lib_with_types("RFN-TYPES {\"fns\":[]}"),
        &[echo_binding("id-echo")],
    )
    .expect("the typed lib mounts");
    let registry: Arc<dyn RustFnRegistry> = host.clone();
    assert_eq!(
        registry.types_json().as_deref(),
        Some("RFN-TYPES {\"fns\":[]}")
    );
}

/// `types_json` is `None` when no mounted library carries a payload —
/// the consult the `.d.ts` emitter skips, the same shape the host
/// exposes.
#[test]
fn types_json_is_none_when_no_library_carries_one() {
    let host = Arc::new(SwappableHost::new());
    host.swap(v1_lib(), &[echo_binding("id-echo")])
        .expect("the untyped lib mounts");
    let registry: Arc<dyn RustFnRegistry> = host;
    assert!(registry.types_json().is_none());
}

/// The consult stays consistent across a concurrent swap: a call in
/// flight resolves its library `Arc` under the swap lock and runs
/// OUTSIDE it, so a swap that lands mid-call does not block the call
/// (and the call does not block the swap). The pin drives the call
/// and the swap on two threads sharing `Arc<SwappableHost>` — the
/// registry's two-phase resolve keeps the call and the swap
/// non-blocking, and the next call serves the new behavior.
#[test]
fn the_registry_call_and_a_swap_do_not_deadlock() {
    let rt = runtime();
    let host = Arc::new(SwappableHost::new());
    host.swap(v1_lib(), &[echo_binding("id-echo")])
        .expect("v1 mounts");
    // The call resolves its library Arc and runs OUTSIDE the swap lock;
    // the swap lands on the main thread while the call runs on the
    // current-thread runtime. The two-phase shape means neither waits
    // on the other.
    let call_host = Arc::clone(&host);
    let call_handle = {
        let rt_handle = rt.handle().clone();
        std::thread::spawn(move || {
            rt_handle.block_on(async move {
                let registry: Arc<dyn RustFnRegistry> = call_host;
                registry.call("id-echo", &json_string("first")).await
            })
        })
    };
    // The swap lands on the main thread while the call is in flight on
    // its own thread — the registry's resolve-under-lock / run-outside
    // contract keeps them non-blocking.
    let swap_report = host
        .swap(v2_lib(), &[echo_binding("id-echo")])
        .expect("the swap re-points");
    assert_eq!(swap_report.generation, 2);
    let out = call_handle
        .join()
        .expect("the call thread finished")
        .expect("the first call answered");
    let reply: String = decode_odf(&out).expect("odf reply");
    // The call may serve v1 or v2 depending on whether it resolved
    // before or after the swap — either is correct (the contract is
    // non-blocking, not ordering). Assert it answered one of them.
    assert!(
        reply == "v1:first" || reply == "v2:first",
        "the call answered: {reply}"
    );
    // The next call serves the new behavior.
    let registry: Arc<dyn RustFnRegistry> = host;
    let out = rt
        .block_on(registry.call("id-echo", &json_string("second")))
        .expect("v2 answers");
    let reply: String = decode_odf(&out).expect("odf reply");
    assert_eq!(reply, "v2:second");
}
