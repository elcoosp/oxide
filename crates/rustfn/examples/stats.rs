//! The M4 call-outcome observability walkthrough — `StatsRegistry`, the
//! decorator that closes the gap the substrate's own rustdocs name
//! (`crates/rustfn/src/host.rs` on `RegistryReport`: "the registry has
//! no traffic counters to expose yet"; `crates/rustfn/src/chain.rs` on
//! `ChainedReport`: "the chain owns no traffic counters"):
//!
//! ```text
//! cargo run -p oxide-rustfn --example stats
//! ```
//!
//! The loop this example walks: build a boot-time host with one echo fn
//! → wrap it in `StatsRegistry` → upcast to `Arc<dyn RustFnRegistry>`
//! (the consult surface the dispatcher already wires) → drive a mix of
//! calls (served, a not-found miss, a fn-level refusal) → snapshot the
//! `CallStats` (every outcome landed in its bucket) → `reset()` the
//! window and observe the clean slate → wrap a `ChainedRegistry` and
//! confirm the decorator composes (one counter set over every native
//! crate behind the chain).
//!
//! The contract this example lands: `Arc<StatsRegistry>` IS-A
//! `Arc<dyn RustFnRegistry>` — the dispatcher consults ONE instrumented
//! registry through the same object-safe seam it already consults for
//! the boot-time host, the dev-loop host and the chain, and every
//! served call / wire-arm refusal lands in ONE `CallStats` snapshot the
//! dev overlay reads through the same "one consult surface" contract
//! the posture snapshots hold.

use std::sync::Arc;

use oxide_rustfn::abi::{ABI_VERSION, FnEntry, FnTable, RawBuf, leak_name};
use oxide_rustfn::chain::ChainedRegistry;
use oxide_rustfn::host::{RustFnHost, RustFnLibrary, RustFnRegistry};
use oxide_rustfn::shim::run_shim;
use oxide_rustfn::stats::StatsRegistry;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct GreetIn {
    name: String,
}

/// The echo shim: decodes the ODF input, prefixes `hello,`, re-encodes.
/// The shape every `#[server_fn]` body generates (decode → call →
/// encode over the C ABI).
extern "C" fn greet_shim(inp: *const u8, len: usize, out: *mut RawBuf) -> i32 {
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
        match oxide_rustfn::bridge::encode_odf(&format!("hello, {}", args.name)) {
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

/// A fn that returns `Err` — the status-1 path the envelope maps to 400
/// `FN-THREW` (the registry's `FnError` arm). The decorator counts it.
extern "C" fn refusing_shim(_inp: *const u8, _len: usize, out: *mut RawBuf) -> i32 {
    let mut buf = RawBuf::empty();
    let status = run_shim(b"", &mut buf, |_| {
        Err("the fixture refuses (width must be positive)".to_string())
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

/// A boot-time host with the greet fn and the refusing fn mounted, two
/// ids registered — the inner registry the decorator wraps.
fn host_with_greet_and_refuse() -> Arc<RustFnHost> {
    let table = leak_table(vec![
        entry("greet", greet_shim),
        entry("refuse", refusing_shim),
    ]);
    let lib = RustFnLibrary::from_static(table, None).expect("the library loads");
    let mut host = RustFnHost::new();
    let index = host.mount(lib);
    host.register("fn-greet", index, "greet");
    host.register("fn-refuse", index, "refuse");
    Arc::new(host)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    // Wrap the host in the observability decorator. `Arc<StatsRegistry>`
    // upcasts to `Arc<dyn RustFnRegistry>`, so the dispatcher consults
    // ONE instrumented registry through the same object-safe seam it
    // already wires for the boot-time host.
    let host = host_with_greet_and_refuse();
    let decorated = Arc::new(StatsRegistry::new(host));
    let registry: Arc<dyn RustFnRegistry> = decorated.clone();

    println!(
        "fresh decorator: stats {:?} (is_empty {})",
        decorated.stats(),
        decorated.stats().is_empty(),
    );

    // Drive a mix of outcomes: two served calls, one not-found miss,
    // one fn-level refusal. Every outcome lands in its bucket.
    let out = rt.block_on(registry.call("fn-greet", &greet_args("oxide")))?;
    println!(
        "call fn-greet: {:?}",
        String::from_utf8(out).unwrap_or_else(|_| "<binary>".into())
    );
    let out = rt.block_on(registry.call("fn-greet", &greet_args("world")))?;
    println!(
        "call fn-greet: {:?}",
        String::from_utf8(out).unwrap_or_else(|_| "<binary>".into())
    );
    let miss = rt
        .block_on(registry.call("fn-missing", b"{}"))
        .expect_err("no fn owns the id");
    println!(
        "call fn-missing: refused {} ({})",
        miss.envelope_code(),
        miss.status()
    );
    let refusal = rt
        .block_on(registry.call("fn-refuse", b"{}"))
        .expect_err("the fn refuses");
    println!(
        "call fn-refuse: refused {} ({})",
        refusal.envelope_code(),
        refusal.status(),
    );

    // The snapshot — ONE read over the decorator's eight counters: two
    // served, one not-found, one fn-error. `calls_attempted` equals
    // `calls_served` plus `error_count()` — every call lands in exactly
    // one outcome bucket. The served percentage is integer arithmetic
    // (a float ratio would cast `u64 → f64` and lose precision); the
    // checked chain keeps the zero-attempted case panic-free.
    let stats = decorated.stats();
    let served_pct = stats
        .calls_served
        .checked_mul(100)
        .and_then(|p| p.checked_div(stats.calls_attempted))
        .unwrap_or(0);
    println!(
        "snapshot: attempted {} served {} errors {} (not_found {} fn_error {}) — served {}%",
        stats.calls_attempted,
        stats.calls_served,
        stats.error_count(),
        stats.not_found,
        stats.fn_error,
        served_pct,
    );
    assert_eq!(stats.calls_attempted, 4);
    assert_eq!(stats.calls_served, 2);
    assert_eq!(stats.error_count(), 2);

    // `reset()` zeroes the window — the dev overlay's "refresh the
    // measurement interval" verb. The next snapshot is the clean slate.
    decorated.reset();
    let after = decorated.stats();
    println!(
        "after reset: stats {:?} (is_empty {})",
        after,
        after.is_empty()
    );
    assert!(after.is_empty());

    // The decorator composes with `ChainedRegistry`: wrap the chain and
    // every outcome across every member lands in ONE counter set. The
    // consult still walks the members in push order; the decorator only
    // observes the final outcome.
    let mut chain_host = RustFnHost::new();
    let table = leak_table(vec![entry("greet", greet_shim)]);
    let lib = RustFnLibrary::from_static(table, None).expect("the library loads");
    let idx = chain_host.mount(lib);
    chain_host.register("fn-greet", idx, "greet");
    let chain = Arc::new(ChainedRegistry::from_members(vec![Arc::new(chain_host)]));
    let chain_decorated = Arc::new(StatsRegistry::new(chain));
    let chain_registry: Arc<dyn RustFnRegistry> = chain_decorated.clone();
    let _ = rt.block_on(chain_registry.call("fn-greet", &greet_args("chained")))?;
    let _ = rt
        .block_on(chain_registry.call("fn-missing", b"{}"))
        .expect_err("no member owns the id");
    let chain_stats = chain_decorated.stats();
    println!(
        "chained snapshot: attempted {} served {} not_found {}",
        chain_stats.calls_attempted, chain_stats.calls_served, chain_stats.not_found,
    );
    assert_eq!(chain_stats.calls_attempted, 2);
    assert_eq!(chain_stats.calls_served, 1);
    assert_eq!(chain_stats.not_found, 1);

    Ok(())
}
