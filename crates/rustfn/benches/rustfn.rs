//! Criterion bench for the §V2-F08 native-function host — the costs
//! the `*.server.rs` model (D33) adds over a plain JS server fn:
//!
//! - `bridge_json_to_odf`: the args JSON → ODF conversion every call
//!   pays before the boundary.
//! - `shim_call`: the in-process shim round trip (decode → call →
//!   encode over the C ABI) — the floor the cdylib path inherits.
//! - `host_registry_hit`: the full registry call (id lookup, the
//!   semaphore permit, the `spawn_blocking` hop, the timeout arm) — the
//!   per-call overhead the dispatcher consult adds.

use std::hint::black_box;
use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use oxide_rustfn::abi::{ABI_VERSION, FnEntry, FnTable, RawBuf};
use oxide_rustfn::bridge::encode_odf;
use oxide_rustfn::chain::ChainedRegistry;
use oxide_rustfn::host::{RustFnHost, RustFnLibrary, RustFnRegistry};
use oxide_rustfn::shim::run_shim;
use oxide_rustfn::swap::{FnBinding, SwappableHost};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct BenchIn {
    url: String,
    width: u32,
}

fn json_args(width: u32) -> Vec<u8> {
    format!(r#"{{"url":"a.png","width":{width}}}"#).into_bytes()
}

extern "C" fn free_impl(buf: RawBuf) {
    if buf.ptr.is_null() || buf.cap == 0 {
        return;
    }
    // SAFETY: every buffer this table hands out came from `RawBuf::alloc`.
    drop(unsafe { Vec::from_raw_parts(buf.ptr, buf.len, buf.cap) });
}

extern "C" fn thumbnail_shim(inp: *const u8, len: usize, out: *mut RawBuf) -> i32 {
    let input: &[u8] = if inp.is_null() || len == 0 {
        &[]
    } else {
        // SAFETY: the host passes a live `len`-byte buffer.
        unsafe { std::slice::from_raw_parts(inp, len) }
    };
    let mut buf = RawBuf::empty();
    let status = run_shim(input, &mut buf, |bytes| {
        let args: BenchIn = match oxide_rustfn::bridge::decode_odf(bytes) {
            Ok(args) => args,
            Err(e) => return Err(e.to_string()),
        };
        match encode_odf(&format!("{}x{}", args.url, args.width)) {
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

fn loaded_library() -> RustFnLibrary {
    let entries: &'static [FnEntry] = Box::leak(
        vec![FnEntry {
            name: oxide_rustfn::abi::leak_name("thumbnail"),
            call: thumbnail_shim,
        }]
        .into_boxed_slice(),
    );
    let t: &'static FnTable = Box::leak(Box::new(FnTable {
        abi_version: ABI_VERSION,
        len: entries.len(),
        entries: entries.as_ptr(),
        free: free_impl,
    }));
    RustFnLibrary::from_static(t, None).expect("the bench table loads")
}

fn bench_bridge(c: &mut Criterion) {
    let args = json_args(8);
    c.bench_function("bridge_json_to_odf", |b| {
        b.iter(|| {
            let raw: serde_json::Value =
                serde_json::from_slice(black_box(&args)).expect("bench args parse");
            let value = oxide_fw_odf::Value::from_json(&raw);
            black_box(oxide_fw_odf::encode(&value).expect("bench value encodes"))
        });
    });
}

fn bench_shim(c: &mut Criterion) {
    let args = encode_odf(&BenchIn {
        url: "a.png".into(),
        width: 8,
    })
    .expect("encodes");
    c.bench_function("shim_call", |b| {
        b.iter(|| {
            let mut out = RawBuf::empty();
            let status = thumbnail_shim(args.as_ptr(), args.len(), &raw mut out);
            debug_assert_eq!(status, 0);
            // SAFETY: the shim allocated exactly this buffer.
            black_box(unsafe { out.as_slice() }.len())
        });
    });
}

fn bench_host_registry(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("the bench runtime");
    let mut host = RustFnHost::new();
    let index = host.mount(loaded_library());
    host.register("bench-id", index, "thumbnail");
    let host = Arc::new(host);
    let args = json_args(8);
    c.bench_function("host_registry_hit", |b| {
        b.iter(|| {
            let host = host.clone();
            let args = args.clone();
            rt.block_on(host.call("bench-id", black_box(&args)))
                .expect("the bench call succeeds")
        });
    });
}

/// The M1 consult: `Arc<SwappableHost>` upcast to `Arc<dyn
/// RustFnRegistry>` and called through the swap lock — the per-call
/// overhead the dev-loop host adds over the boot-time host (the extra
/// `Mutex` lock + the `Arc` clone), the cost the dispatcher consult
/// pays when the dev loop owns the registry.
fn bench_swappable_registry(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("the bench runtime");
    let host = Arc::new(SwappableHost::new());
    host.swap(loaded_library(), &[FnBinding::new("bench-id", "thumbnail")])
        .expect("the bench swap mounts");
    let registry: Arc<dyn RustFnRegistry> = host;
    let args = json_args(8);
    c.bench_function("swappable_registry_hit", |b| {
        b.iter(|| {
            let registry = registry.clone();
            let args = args.clone();
            rt.block_on(registry.call("bench-id", black_box(&args)))
                .expect("the bench call succeeds")
        });
    });
}

/// The M1 multi-host consult: a two-member `ChainedRegistry` upcast to
/// `Arc<dyn RustFnRegistry>` and called through the chain's `has`-walk —
/// the per-call overhead the dispatcher consult pays when the workspace
/// ships more than one native crate (the extra `has` per member + the
/// delegate), on top of the boot-time host's `host_registry_hit` floor.
fn bench_chained_registry(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("the bench runtime");
    // Two members, disjoint ids — the consult walks ONE `has` miss then a
    // hit (the worst case is the last member; the average over a real
    // workspace sits between). `bench-id-b` lives on the second member.
    let mut a = RustFnHost::new();
    let idx_a = a.mount(loaded_library());
    a.register("bench-id-a", idx_a, "thumbnail");
    let mut b = RustFnHost::new();
    let idx_b = b.mount(loaded_library());
    b.register("bench-id-b", idx_b, "thumbnail");
    let chain = Arc::new(ChainedRegistry::from_members(vec![
        Arc::new(a),
        Arc::new(b),
    ]));
    let registry: Arc<dyn RustFnRegistry> = chain;
    let args = json_args(8);
    c.bench_function("chained_registry_hit", |b| {
        b.iter(|| {
            let registry = registry.clone();
            let args = args.clone();
            rt.block_on(registry.call("bench-id-b", black_box(&args)))
                .expect("the bench call succeeds")
        });
    });
}

criterion_group!(
    benches,
    bench_bridge,
    bench_shim,
    bench_host_registry,
    bench_swappable_registry,
    bench_chained_registry
);
criterion_main!(benches);
