//! The M6 per-id hot-key attribution walkthrough — `HotKeyRegistry`,
//! the decorator that closes the attribution gap the cache plane's
//! `HotKeyObserver` (F38) already lands for cache events and the
//! rustfn registry story names beside the `StatsRegistry` aggregate
//! counters:
//!
//! ```text
//! cargo run -p oxide-rustfn --example hot_keys
//! ```
//!
//! The loop this example walks: build a boot-time host with a greet fn
//! → wrap it in `StatsRegistry` (the counters) → wrap THAT in
//! `HotKeyRegistry` (the per-id ledger) → upcast to
//! `Arc<dyn RustFnRegistry>` (the consult surface the dispatcher
//! already wires) → drive a skewed call mix (one id called 5×, another
//! 2×, a third once) → read the `top(n)` window (count desc, id asc on
//! ties — the hottest id leads) → exercise the bounded ledger (a cap of
//! 2 admits two ids by first sight, refuses the third, counts it as
//! `unattributed`) → `reset()` the window and observe the clean slate.
//!
//! The contract this example lands: `Arc<HotKeyRegistry>` IS-A
//! `Arc<dyn RustFnRegistry>` — the dispatcher consults ONE attributed
//! registry through the same object-safe seam it already consults for
//! the boot-time host, the dev-loop host, the chain, the stats
//! decorator and the tracing decorator, and every call attributes to
//! its id (served OR error) through one ledger the dev overlay reads
//! through the same "one consult surface" contract the posture
//! snapshots and the call counters hold.

use std::sync::Arc;

use oxide_rustfn::abi::{ABI_VERSION, FnEntry, FnTable, RawBuf, leak_name};
use oxide_rustfn::bridge::encode_odf;
use oxide_rustfn::host::{RustFnHost, RustFnLibrary, RustFnRegistry};
use oxide_rustfn::hot_keys::HotKeyRegistry;
use oxide_rustfn::shim::run_shim;
use oxide_rustfn::stats::StatsRegistry;
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

/// A boot-time host with the greet fn mounted, three ids registered —
/// the inner registry the decorators wrap. Three ids exercise the sort
/// + tie-break + bound contract.
fn host_three_ids() -> Arc<RustFnHost> {
    let table = leak_table(vec![entry("greet", greet_shim)]);
    let lib = RustFnLibrary::from_static(table, None).expect("the library loads");
    let mut host = RustFnHost::new();
    let index = host.mount(lib);
    host.register("fn-greet", index, "greet");
    host.register("fn-search", index, "greet");
    host.register("fn-thumb", index, "greet");
    Arc::new(host)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    // Compose the decorators: stats FIRST (the counters), hot-keys
    // OUTSIDE (the per-id ledger). `Arc<HotKeyRegistry>` upcasts to
    // `Arc<dyn RustFnRegistry>`, so the dispatcher consults ONE registry
    // that counts AND attributes — the same "one consult surface"
    // composition the stats rustdoc names for `ChainedRegistry`, lifted
    // to two decorators.
    let host = host_three_ids();
    let stats = Arc::new(StatsRegistry::new(host));
    let attributed = Arc::new(HotKeyRegistry::new(stats.clone()));
    let registry: Arc<dyn RustFnRegistry> = attributed.clone();

    println!("--- driving a skewed call mix (5× greet, 2× search, 1× thumb) ---");
    for i in 0..5 {
        let _ = rt.block_on(registry.call("fn-greet", &greet_args(&format!("g{i}"))))?;
    }
    for i in 0..2 {
        let _ = rt.block_on(registry.call("fn-search", &greet_args(&format!("s{i}"))))?;
    }
    let _ = rt.block_on(registry.call("fn-thumb", &greet_args("t")))?;

    // The top window — count DESC, id ASC on ties. greet (5) leads,
    // search (2) trails, thumb (1) last. The inner stats decorator
    // counted all 8 calls (served = 8); the hot-key decorator attributed
    // each to its id.
    let top = attributed.top(10);
    println!("top window (count desc, id asc on ties):");
    for row in &top {
        println!("  {} -> {} calls", row.id, row.calls);
    }
    assert_eq!(top.len(), 3);
    assert_eq!(top[0].id, "fn-greet");
    assert_eq!(top[0].calls, 5);
    assert_eq!(top[1].id, "fn-search");
    assert_eq!(top[1].calls, 2);
    assert_eq!(top[2].id, "fn-thumb");
    assert_eq!(top[2].calls, 1);

    // The inner stats snapshot — the aggregate counters see all 8 calls.
    let s = stats.stats();
    println!(
        "stats snapshot: attempted {} served {} (the counters and the ledger cover the same 8 calls)",
        s.calls_attempted, s.calls_served,
    );
    assert_eq!(s.calls_attempted, 8);

    // top(2) truncates to the hottest two.
    let two = attributed.top(2);
    assert_eq!(two.len(), 2);
    println!(
        "top(2) truncated to the hottest two: {} {}",
        two[0].id, two[1].id
    );

    println!("--- the bounded ledger (cap=2: first-sight admission, honest saturation) ---");
    let bounded = Arc::new(HotKeyRegistry::bounded(host_three_ids(), 2));
    let breg: Arc<dyn RustFnRegistry> = bounded.clone();
    // Two ids admit by first sight.
    let _ = rt.block_on(breg.call("fn-greet", &greet_args("a")))?;
    let _ = rt.block_on(breg.call("fn-search", &greet_args("b")))?;
    // The third id is NEW and the map is full → refused, counted as
    // unattributed (the honest saturation posture).
    let _ = rt.block_on(breg.call("fn-thumb", &greet_args("c")))?;
    let btop = bounded.top(10);
    println!(
        "bounded (cap=2) top: {} tracked, unattributed={}",
        btop.len(),
        bounded.unattributed(),
    );
    for row in &btop {
        println!("  {} -> {} calls", row.id, row.calls);
    }
    assert_eq!(btop.len(), 2, "only the two admitted ids are in the ledger");
    assert_eq!(
        bounded.unattributed(),
        1,
        "the refused id counted as unattributed"
    );
    // A resident id called again past the bound still counts.
    let _ = rt.block_on(breg.call("fn-greet", &greet_args("a2")))?;
    let greet_row = bounded
        .top(10)
        .into_iter()
        .find(|r| r.id == "fn-greet")
        .expect("greet tracked");
    assert_eq!(
        greet_row.calls, 2,
        "the resident id counted the second call"
    );
    assert_eq!(
        bounded.unattributed(),
        1,
        "no new unattributed for a resident id"
    );

    println!("--- reset() clears the window (the dev overlay's refresh verb) ---");
    bounded.reset();
    assert_eq!(bounded.top(10).len(), 0);
    assert_eq!(bounded.unattributed(), 0);
    println!(
        "after reset: tracked={} unattributed={}",
        bounded.tracked(),
        bounded.unattributed()
    );

    println!(
        "--- done: every call attributed to its id; the bounded ledger refused the third honestly ---"
    );
    Ok(())
}
