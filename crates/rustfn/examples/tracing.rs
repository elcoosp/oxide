//! The M5 per-call tracing walkthrough — `TracingRegistry`, the
//! decorator that closes the observability gap the cache plane's
//! `TracingCacheObserver` (F22) already lands for cache events and the
//! rustfn registry story names beside the `StatsRegistry` counters:
//!
//! ```text
//! cargo run -p oxide-rustfn --example tracing
//! ```
//!
//! The loop this example walks: install a `tracing-subscriber` that
//! prints `oxide::rustfn` rows → build a boot-time host with a greet
//! fn and a refusing fn → wrap it in `StatsRegistry` (the counters) →
//! wrap THAT in `TracingRegistry` (the spans) → upcast to
//! `Arc<dyn RustFnRegistry>` (the consult surface the dispatcher
//! already wires) → drive a mix of calls (served, a not-found miss, a
//! fn-level refusal) → observe ONE `tracing` row per call on stderr
//! (the id and the outcome) AND the `CallStats` snapshot (every
//! outcome counted) → confirm the decorators compose (count + trace
//! through one consult).
//!
//! The contract this example lands: `Arc<TracingRegistry>` IS-A
//! `Arc<dyn RustFnRegistry>` — the dispatcher consults ONE traced
//! registry through the same object-safe seam it already consults for
//! the boot-time host, the dev-loop host, the chain and the stats
//! decorator, and every served call / wire-arm refusal opens ONE
//! `tracing` span the operator's `RUST_LOG=oxide::rustfn=debug` scrape
//! surfaces.

use std::sync::Arc;

use oxide_rustfn::abi::{ABI_VERSION, FnEntry, FnTable, RawBuf, leak_name};
use oxide_rustfn::bridge::{decode_odf, encode_odf};
use oxide_rustfn::host::{RustFnHost, RustFnLibrary, RustFnRegistry};
use oxide_rustfn::shim::run_shim;
use oxide_rustfn::stats::StatsRegistry;
use oxide_rustfn::tracing::TracingRegistry;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct GreetIn {
    name: String,
}

/// The greet shim: decodes the ODF input, prefixes `hello,`, re-encodes.
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
        match encode_odf(&format!("hello, {}", args.name)) {
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
/// `FN-THREW` (the registry's `FnError` arm). The decorator traces it.
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
/// ids registered — the inner registry the decorators wrap.
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
    // Install a subscriber so the decorator's `oxide::rustfn` rows
    // actually render. The crate's production code never depends on a
    // subscriber (the decorator emits unconditionally; the subscriber is
    // the host's concern); the example installs one so the walkthrough
    // is observable. `RUST_LOG` overrides the default filter.
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("oxide::rustfn=debug"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    // Compose the decorators: stats FIRST (the counters), tracing OUTSIDE
    // (the spans). `Arc<TracingRegistry>` upcasts to `Arc<dyn
    // RustFnRegistry>`, so the dispatcher consults ONE registry that
    // counts AND traces — the same "one consult surface" composition the
    // stats rustdoc names for `ChainedRegistry`, lifted to two decorators.
    let host = host_with_greet_and_refuse();
    let stats = Arc::new(StatsRegistry::new(host));
    let traced = Arc::new(TracingRegistry::new(stats.clone()));
    let registry: Arc<dyn RustFnRegistry> = traced.clone();

    println!("--- driving a mix of native calls (each opens one oxide::rustfn span) ---");

    // A served call — the span carries `outcome=served` and the elapsed
    // wall time; the inner stats decorator counts it as served.
    let out = rt.block_on(registry.call("fn-greet", &greet_args("oxide")))?;
    let text: String = decode_odf(&out).map_err(|e| e.to_string())?;
    println!("call fn-greet: {text:?}");

    let out = rt.block_on(registry.call("fn-greet", &greet_args("world")))?;
    let text: String = decode_odf(&out).map_err(|e| e.to_string())?;
    println!("call fn-greet: {text:?}");

    // A not-found miss — the span carries `outcome=not_found`; the stats
    // decorator counts it as not_found.
    let miss = rt
        .block_on(registry.call("fn-missing", b"{}"))
        .expect_err("no fn owns the id");
    println!(
        "call fn-missing: refused {} ({})",
        miss.envelope_code(),
        miss.status()
    );

    // A fn-level refusal — the span carries `outcome=fn_error`; the stats
    // decorator counts it as fn_error.
    let refusal = rt
        .block_on(registry.call("fn-refuse", b"{}"))
        .expect_err("the fn refuses");
    println!(
        "call fn-refuse: refused {} ({})",
        refusal.envelope_code(),
        refusal.status(),
    );

    // The stats snapshot — ONE read over the inner decorator's eight
    // counters: two served, one not-found, one fn-error. The tracing
    // decorator observed the same four calls (four spans on stderr); the
    // stats decorator counted them. Both decorators held the "observe,
    // never alter" rule — the bytes and the errors passed through
    // unchanged.
    let s = stats.stats();
    println!(
        "stats snapshot: attempted {} served {} errors {} (not_found {} fn_error {}) — the spans and the counters cover the same four calls",
        s.calls_attempted,
        s.calls_served,
        s.error_count(),
        s.not_found,
        s.fn_error,
    );
    assert_eq!(s.calls_attempted, 4);
    assert_eq!(s.calls_served, 2);
    assert_eq!(s.error_count(), 2);

    println!("--- the consult reads (has/ids) emit NO span ---");
    println!(
        "has fn-greet: {} (no span above — consult reads delegate untraced)",
        registry.has("fn-greet")
    );
    println!("ids: {:?} (no span above)", registry.ids());

    println!("--- done: every call opened one oxide::rustfn span; consult reads opened none ---");
    Ok(())
}
