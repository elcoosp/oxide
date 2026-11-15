//! TDD pins for the §V2-F08 dev-loop hot-swap half (`feat/server-lib-dev-loop`)
//! — the [`SwappableHost`] state the dev crate's `ServerLibLoop` drives when
//! a `*.server.rs` rebuild lands: mount the new library, re-point the §9.4
//! ids under it, drop the ids whose export left, and retire the old mount
//! ONLY once no in-flight call still holds it (the spec: "hot-swaps by
//! loading the new copy and retiring the old `Arc<Library>` after in-flight
//! calls finish").
//!
//! Written BEFORE the implementation; every pin imports public API only.
//!
//! The contracts pinned here:
//!
//! - A swap re-registers every binding under the NEW library; the next
//!   call through the same id serves the new behavior (the host's
//!   re-registration contract, now driven without `&mut` access).
//! - An id absent from the new bindings is DROPPED (`has` → false): the
//!   consult falls back to the JS pool for a removed export instead of
//!   answering through a stale library.
//! - A binding for an id the host never saw is registered (`added`) — the
//!   added-while-dev-runs export is callable on the next call.
//! - Each swap bumps a monotonically increasing generation; the report
//!   lists `added`/`rebound`/`dropped` sorted, and `rebound` covers every
//!   previously-registered id the bindings still name.
//! - `call_now` resolves the library UNDER the swap lock and runs the call
//!   OUTSIDE it: a call in flight never blocks (or is blocked by) a swap,
//!   and the old mount survives until that call drops its `Arc` —
//!   `retire_idle` then reclaims it. A mount an id still references is
//!   never retired, whatever its refcount.
//! - The state is `Send + Sync` (`Arc<SwappableHost>` is the shape the
//!   future `NativeFnSource` adapter and the dev task share).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use oxide_rustfn::abi::{ABI_VERSION, FnEntry, FnTable, RawBuf, leak_name};
use oxide_rustfn::bridge::{decode_odf, encode_odf};
use oxide_rustfn::host::{RegistryErrorKind, RustFnLibrary};
use oxide_rustfn::shim::run_shim;
use oxide_rustfn::swap::{FnBinding, SwappableHost};

/// Build a `&'static FnTable` over `entries` (the test-fixture pattern the
/// host pins use: the table is leaked, the host keeps the pointer for the
/// library's lifetime).
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

/// The two echo generations the swap pins assert through: distinct
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

fn echo_binding(id: &str) -> FnBinding {
    FnBinding::new(id, "echo")
}

/// The §9.4 route's args wire: `call_now` takes RAW JSON (the host
/// converts to the ODF bytes the boundary carries), so the pins speak
/// JSON strings.
fn json_string(s: &str) -> Vec<u8> {
    format!("\"{s}\"").into_bytes()
}

#[test]
fn swap_repoints_existing_ids_and_serves_the_new_behavior() {
    let host = SwappableHost::new();
    let first = host
        .swap(v1_lib(), &[echo_binding("id-echo")])
        .expect("the first swap mounts");
    assert_eq!(first.generation, 1);
    assert!(host.has("id-echo"));
    let out = host
        .call_now("id-echo", &json_string("hello"))
        .expect("v1 answers");
    let reply: String = decode_odf(&out).expect("odf reply");
    assert_eq!(reply, "v1:hello");

    let second = host
        .swap(v2_lib(), &[echo_binding("id-echo")])
        .expect("the second swap re-points");
    assert_eq!(second.generation, 2);
    assert_eq!(second.rebound, vec!["id-echo".to_string()]);
    assert_eq!(second.added, Vec::<String>::new());
    assert_eq!(second.dropped, Vec::<String>::new());
    assert_eq!(second.total_ids, 1);
    let out = host
        .call_now("id-echo", &json_string("hello"))
        .expect("v2 answers");
    let reply: String = decode_odf(&out).expect("odf reply");
    assert_eq!(reply, "v2:hello");
}

#[test]
fn swap_drops_ids_whose_export_left() {
    let host = SwappableHost::new();
    host.swap(
        v1_lib(),
        &[echo_binding("id-echo"), echo_binding("id-gone")],
    )
    .expect("two ids mount");
    assert!(host.has("id-gone"));

    // The rebuild lost the `id-gone` export: the bindings name `id-echo`
    // only, and the host stops answering `id-gone` natively — the consult
    // falls back to the JS pool instead of a stale library.
    let report = host
        .swap(v2_lib(), &[echo_binding("id-echo")])
        .expect("the reduced bindings swap");
    assert_eq!(report.dropped, vec!["id-gone".to_string()]);
    assert!(!host.has("id-gone"));
    assert!(host.has("id-echo"));
}

#[test]
fn swap_registers_added_bindings_and_reports_them() {
    let host = SwappableHost::new();
    host.swap(v1_lib(), &[echo_binding("id-echo")])
        .expect("the first swap");
    let report = host
        .swap(v2_lib(), &[echo_binding("id-echo"), echo_binding("id-new")])
        .expect("the extended bindings swap");
    assert_eq!(report.added, vec!["id-new".to_string()]);
    assert!(host.has("id-new"));
    let out = host
        .call_now("id-new", &json_string("x"))
        .expect("the added id answers");
    let reply: String = decode_odf(&out).expect("odf reply");
    assert_eq!(reply, "v2:x");
}

#[test]
fn swap_report_lists_added_rebound_and_dropped_sorted() {
    let host = SwappableHost::new();
    host.swap(
        v1_lib(),
        &[echo_binding("b"), echo_binding("a"), echo_binding("z")],
    )
    .expect("three ids mount");
    let report = host
        .swap(
            v2_lib(),
            &[echo_binding("c"), echo_binding("a"), echo_binding("z")],
        )
        .expect("the reshuffled bindings swap");
    assert_eq!(report.rebound, vec!["a".to_string(), "z".to_string()]);
    assert_eq!(report.added, vec!["c".to_string()]);
    assert_eq!(report.dropped, vec!["b".to_string()]);
    assert_eq!(report.total_ids, 3);
}

#[test]
fn call_now_on_an_unknown_id_is_the_registry_not_found() {
    let host = SwappableHost::new();
    let err = host
        .call_now("id-missing", b"{}")
        .expect_err("nothing is mounted");
    assert!(matches!(err.kind, RegistryErrorKind::NotFound));
}

/// The spec's retire rule, pinned with a REAL in-flight call: the shim
/// blocks until the pin releases it, the main thread swaps and retires
/// underneath the call, and the old mount survives until the call's
/// `Arc` drops.
static IN_FLIGHT_ENTERED: AtomicBool = AtomicBool::new(false);
static IN_FLIGHT_RELEASE: AtomicBool = AtomicBool::new(false);

extern "C" fn blocking_shim(inp: *const u8, len: usize, out: *mut RawBuf) -> i32 {
    let input: &[u8] = if inp.is_null() || len == 0 {
        &[]
    } else {
        // SAFETY: the host passes a live `len`-byte buffer.
        unsafe { std::slice::from_raw_parts(inp, len) }
    };
    let mut buf = RawBuf::empty();
    let status = run_shim(input, &mut buf, |_bytes| {
        IN_FLIGHT_ENTERED.store(true, Ordering::SeqCst);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !IN_FLIGHT_RELEASE.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(Vec::new())
    });
    // SAFETY: the host always passes a writable slot.
    unsafe {
        if !out.is_null() {
            *out = buf;
        }
    }
    status
}

#[test]
fn retire_idle_waits_for_the_in_flight_call_then_reclaims() {
    let blocking: &'static [FnEntry] =
        Box::leak(vec![entry_named("echo", blocking_shim)].into_boxed_slice());
    let host = Arc::new(SwappableHost::new());
    host.swap(
        RustFnLibrary::from_static(table(blocking), None).expect("loads"),
        &[echo_binding("id-echo")],
    )
    .expect("the blocking build mounts");

    let worker = {
        let host = Arc::clone(&host);
        std::thread::spawn(move || host.call_now("id-echo", &json_string("slow")))
    };
    while !IN_FLIGHT_ENTERED.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(2));
    }

    // The swap lands while the v1 call is in flight (the call runs OUTSIDE
    // the swap lock) and re-points the id; the NEXT call serves the new
    // build.
    let report = host
        .swap(v2_lib(), &[echo_binding("id-echo")])
        .expect("swap");
    assert_eq!(report.generation, 2);

    // The old mount is still on loan to the in-flight call: retire_idle
    // keeps it (it is not idle), and the swap never had to wait on it.
    let retire = host.retire_idle();
    assert_eq!(retire.retired, 0, "the in-flight mount is not idle");

    IN_FLIGHT_RELEASE.store(true, Ordering::SeqCst);
    worker
        .join()
        .expect("the in-flight call finishes")
        .expect("v1 answered");

    // The loan returned: the orphaned v1 mount is reclaimable, the live
    // v2 mount stays.
    let retire = host.retire_idle();
    assert_eq!(retire.retired, 1, "the orphaned mount retires");
    assert_eq!(retire.kept, 1, "the live mount stays");

    let out = host
        .call_now("id-echo", &json_string("after"))
        .expect("v2 answers");
    let reply: String = decode_odf(&out).expect("odf reply");
    assert_eq!(reply, "v2:after");

    // Idempotent: nothing left to retire.
    assert_eq!(host.retire_idle().retired, 0);
}

/// The retire pass reclaims exactly the orphan: the mount the bindings
/// re-pointed AWAY from is reclaimable, the LIVE mount (the one the ids
/// reference now) is never retired — whatever the pass count.
#[test]
fn retire_idle_reclaims_the_orphan_and_keeps_the_live_mount() {
    let host = SwappableHost::new();
    host.swap(v1_lib(), &[echo_binding("id-a")])
        .expect("a mounts");
    host.swap(v2_lib(), &[echo_binding("id-a"), echo_binding("id-b")])
        .expect("b mounts and adds id-b");
    let retire = host.retire_idle();
    assert_eq!(retire.retired, 1, "the orphaned first mount retires");
    assert_eq!(retire.kept, 1, "the live mount stays");
    assert!(host.call_now("id-a", &json_string("q")).is_ok());
    assert!(host.call_now("id-b", &json_string("q")).is_ok());

    // A swap that removes id-b re-points id-a and orphans nothing new
    // (the live mount is the same one) — the next pass reclaims nothing.
    host.swap(v1_lib(), &[echo_binding("id-a")])
        .expect("the reduced swap");
    let retire = host.retire_idle();
    assert_eq!(retire.retired, 1, "the second-generation mount retires");
    assert_eq!(retire.kept, 1, "the live mount still stays");
    assert!(!host.has("id-b"), "the removed export dropped");
    assert!(host.call_now("id-a", &json_string("q")).is_ok());
}

#[test]
fn the_host_state_is_send_sync_and_shareable() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Arc<SwappableHost>>();
    let host = SwappableHost::new();
    host.swap(v1_lib(), &[echo_binding("id-echo")])
        .expect("mounts");
    std::thread::spawn(move || {
        assert!(host.has("id-echo"));
    })
    .join()
    .expect("the shared handle crosses threads");
}

#[test]
fn the_generation_starts_at_zero_and_counts_swaps() {
    let host = SwappableHost::new();
    assert_eq!(host.generation(), 0);
    host.swap(v1_lib(), &[echo_binding("id-echo")])
        .expect("first");
    assert_eq!(host.generation(), 1);
    host.swap(v2_lib(), &[echo_binding("id-echo")])
        .expect("second");
    assert_eq!(host.generation(), 2);
}
