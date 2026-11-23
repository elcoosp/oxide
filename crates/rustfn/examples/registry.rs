//! The M1 architectural seam `RustFnRegistry for SwappableHost` — a
//! walkthrough of the consult the dispatcher wires behind `Arc<dyn
//! RustFnRegistry>`, driven through the dev-loop hot-swap host:
//!
//! ```text
//! cargo run -p oxide-rustfn --example registry
//! ```
//!
//! The loop the dev task drives (the `SwappableHost` half the
//! `examples/rustfn.rs` walkthrough leaves to the dispatcher): mount a
//! library through the swap → register the §9.4 ids → consult through
//! the `RustFnRegistry` trait (args JSON in, the fn's output out) →
//! hot-swap by mounting the new copy and re-pointing the ids (the old
//! `Arc` retires when the last in-flight call finishes) → consult again
//! and observe the new behavior.
//!
//! The contract this example lands: `Arc<SwappableHost>` IS-A
//! `Arc<dyn RustFnRegistry>` — the boot-time host and the dev-loop
//! host share ONE consult surface, so the dispatcher no longer forks
//! between the two.

use std::sync::Arc;
use std::time::Duration;

use oxide_rustfn::abi::{ABI_VERSION, FnEntry, FnTable, RawBuf, leak_name};
use oxide_rustfn::host::{RustFnLibrary, RustFnRegistry};
use oxide_rustfn::shim::run_shim;
use oxide_rustfn::swap::{FnBinding, RegistryReport, SwappableHost};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct GreetIn {
    name: String,
}

/// The "v1 library": a greet fn over the ODF wire (decode the args,
/// call the body, encode the result — what a `#[server_fn]` shim
/// generates).
extern "C" fn greet_shim_v1(inp: *const u8, len: usize, out: *mut RawBuf) -> i32 {
    let input: &[u8] = if inp.is_null() || len == 0 {
        &[]
    } else {
        // SAFETY: the host passes a live `len`-byte buffer.
        unsafe { std::slice::from_raw_parts(inp, len) }
    };
    let mut buf = RawBuf::empty();
    let status = run_shim(input, &mut buf, |bytes| {
        let args: GreetIn = match oxide_rustfn::bridge::decode_odf(bytes) {
            Ok(args) => args,
            Err(e) => return Err(e.to_string()),
        };
        match oxide_rustfn::bridge::encode_odf(&format!("hello, {} (v1)", args.name)) {
            Ok(bytes) => Ok(bytes),
            Err(e) => Err(e.to_string()),
        }
    });
    // SAFETY: the host always passes a writable slot.
    unsafe {
        if !out.is_null() {
            *out = buf;
        }
    }
    status
}

/// The "v2 library" — the hot-swap's new behavior.
extern "C" fn greet_shim_v2(inp: *const u8, len: usize, out: *mut RawBuf) -> i32 {
    let input: &[u8] = if inp.is_null() || len == 0 {
        &[]
    } else {
        // SAFETY: the host passes a live `len`-byte buffer.
        unsafe { std::slice::from_raw_parts(inp, len) }
    };
    let mut buf = RawBuf::empty();
    let status = run_shim(input, &mut buf, |bytes| {
        let args: GreetIn = match oxide_rustfn::bridge::decode_odf(bytes) {
            Ok(args) => args,
            Err(e) => return Err(e.to_string()),
        };
        let greeting = format!("hello, {} (v2 — hot-swapped)", args.name);
        match oxide_rustfn::bridge::encode_odf(&greeting) {
            Ok(bytes) => Ok(bytes),
            Err(e) => Err(e.to_string()),
        }
    });
    // SAFETY: the host always passes a writable slot.
    unsafe {
        if !out.is_null() {
            *out = buf;
        }
    }
    status
}

extern "C" fn free_impl(buf: RawBuf) {
    if buf.ptr.is_null() || buf.cap == 0 {
        return;
    }
    // SAFETY: every buffer the shims hand out came from `RawBuf::alloc`
    // on this process's allocator (the from_static seam's contract).
    drop(unsafe { Vec::from_raw_parts(buf.ptr, buf.len, buf.cap) });
}

fn leak_table(entries: Vec<FnEntry>) -> &'static FnTable {
    Box::leak(Box::new(FnTable {
        abi_version: ABI_VERSION,
        len: entries.len(),
        entries: Box::leak(entries.into_boxed_slice()).as_ptr(),
        free: free_impl,
    }))
}

fn entry(name: &'static str, call: extern "C" fn(*const u8, usize, *mut RawBuf) -> i32) -> FnEntry {
    FnEntry {
        name: leak_name(name),
        call,
    }
}

/// The §9.4 route's args JSON, the wire shape every `call` takes.
fn greet_args(name: &str) -> Vec<u8> {
    format!(r#"{{"name":"{name}"}}"#).into_bytes()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    let v1 = leak_table(vec![entry("greet", greet_shim_v1)]);
    let v2 = leak_table(vec![entry("greet", greet_shim_v2)]);

    // The dev-loop host: the swap state the dev task drives. The
    // boot-time `RustFnHost` (the `examples/rustfn.rs` walkthrough)
    // freezes at startup; the `SwappableHost` changes UNDER a running
    // server, so a `*.server.rs` rebuild mounts the new library and
    // re-points the ids at it while the calls already in flight finish
    // against the library they resolved.
    let host = Arc::new(SwappableHost::new());

    // The first swap: mount the v1 library and register the §9.4 id.
    // `FnBinding::new(id, fn_name)` is the loop's swap input — the loop
    // owns the id secret (the ONE `server_fn_id` definition) and the
    // types side export, so the host never re-derives ids from raw
    // types payloads.
    let first = host.swap(
        RustFnLibrary::from_static(v1, None)?,
        &[FnBinding::new("fn-greet", "greet")],
    )?;
    println!("swap #1: generation {}", first.generation);
    println!("registered ids: {:?}", host.ids());

    // The consult the dispatcher wires: `Arc<SwappableHost>` upcasts
    // to `Arc<dyn RustFnRegistry>` — the same object-safe seam the
    // server crate's `RustFirstDispatcher` consults before the JS
    // pool. The boot-time host and the dev-loop host now share ONE
    // consult surface; the dispatcher no longer forks between them.
    let registry: Arc<dyn RustFnRegistry> = host.clone();
    let out = rt.block_on(registry.call("fn-greet", &greet_args("oxide")))?;
    println!(
        "call fn-greet: {:?}",
        String::from_utf8(out).unwrap_or_else(|_| "<binary>".into())
    );

    // The hot-swap: mount the v2 copy, re-point the id, the next call
    // serves the new behavior. The old mount stays until
    // `retire_idle` reclaims it — never mid-call.
    let second = host.swap(
        RustFnLibrary::from_static(v2, None)?,
        &[FnBinding::new("fn-greet", "greet")],
    )?;
    println!(
        "swap #2: generation {}, rebound {:?}",
        second.generation, second.rebound
    );
    let out = rt.block_on(registry.call("fn-greet", &greet_args("oxide")))?;
    println!(
        "call fn-greet after hot-swap: {:?}",
        String::from_utf8(out).unwrap_or_else(|_| "<binary>".into())
    );

    // The retire pass reclaims the orphaned v1 mount now that no
    // in-flight call holds it.
    let retire = host.retire_idle();
    println!(
        "retire: {} reclaimed, {} kept (generations: {})",
        retire.retired,
        retire.kept,
        host.generation()
    );

    // The `has` consult the dispatcher reads before a JS dispatch: a
    // hot-swapped-away id is `false`, the JS-pool fallback owns it
    // again. Register a second id, then drop it.
    host.swap(
        RustFnLibrary::from_static(v2, None)?,
        &[
            FnBinding::new("fn-greet", "greet"),
            FnBinding::new("fn-gone", "greet"),
        ],
    )?;
    println!(
        "after adding fn-gone: has fn-gone = {}",
        registry.has("fn-gone")
    );
    host.swap(
        RustFnLibrary::from_static(v2, None)?,
        &[FnBinding::new("fn-greet", "greet")],
    )?;
    println!(
        "after dropping fn-gone: has fn-gone = {} (the JS pool owns it again)",
        registry.has("fn-gone")
    );

    // The timeout budget the substrate carries: the host's default is
    // 30 s; the consult honors it through the registry seam (a call
    // over budget refuses with the `Timeout` wire mapping).
    let _ = Duration::from_secs(30);
    println!("consult owns fn-greet: {}", registry.has("fn-greet"));

    // The M2 introspection surface: `report()` snapshots the host's
    // posture in ONE read — the generation, the mount count, the id
    // count, the sorted ids, whether a types payload is mounted, the
    // per-function timeout, and the blocking-lane count. The dev
    // overlay renders the report (the "is the host healthy?" check),
    // it does not take the lock per field. The boot-time `RustFnHost`
    // answers the same shape (generation 0), so the overlay
    // introspects EITHER host through `RegistryReport`.
    let report: RegistryReport = host.report();
    println!(
        "report: generation {}, mounts {}, ids {:?} (count {}), types {}, timeout {}s, lanes {}",
        report.generation,
        report.mount_count,
        report.ids,
        report.id_count,
        report.has_types,
        report.timeout_secs,
        report.lanes,
    );

    // The boot-time host reports the same shape (generation 0 — it
    // does not swap). The dev overlay's "is the host attached?"
    // sentinel is `RegistryReport::default()` (all-zero), distinct
    // from an empty host's report (which carries the substrate's
    // 30 s / 16-lane defaults).
    let boot_report = oxide_rustfn::host::RustFnHost::new().report();
    println!(
        "boot-time host report: generation {}, mounts {}, ids {:?}, timeout {}s, lanes {}",
        boot_report.generation,
        boot_report.mount_count,
        boot_report.ids,
        boot_report.timeout_secs,
        boot_report.lanes,
    );
    assert_ne!(RegistryReport::default(), report, "the host is attached");

    Ok(())
}
