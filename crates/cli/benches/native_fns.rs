//! The §V2-F08 finish's cost benches — the legs the CLI adoption adds to
//! the consult path: the live-host consult (`RegistryFnSource` over a
//! swapped `SwappableHost`: the `has` gate + the mutex-lookup-and-clone
//! call shape an in-flight request pays), the bundle-time `.server.rs`
//! stub generation (the resolve arm's path walk + the load arm's payload
//! read and `rust_fn_stub_source` render) and the discovery walk (the
//! boot's once-per-session crate scan).
//!
//! Run with `cargo bench -p oxide-fw-cli --features bench-util --bench
//! native_fns` (the bench-util opt-in keeps plain `cargo test` off
//! criterion).

use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use std::sync::Arc;

use oxide_fw_cli::native_fns::{
    discover_native_crate, server_rs_load_hook, server_rs_resolve_hook,
};
use oxide_fw_compiler::{Env as CompilerEnv, server_fn_id};
use oxide_rustfn::abi::{FnEntry, FnTable, RawBuf, leak_name};
use oxide_rustfn::host::RustFnLibrary;
use oxide_rustfn::swap::{FnBinding, SwappableHost};
use oxide_rustfn::types::{FnTypeEntry, types_json};

extern "C" fn free_impl(buf: RawBuf) {
    let _ = buf;
}

fn table(entries: &'static [FnEntry]) -> &'static FnTable {
    Box::leak(Box::new(FnTable {
        abi_version: oxide_rustfn::abi::ABI_VERSION,
        len: entries.len(),
        entries: entries.as_ptr(),
        free: free_impl,
    }))
}

extern "C" fn echo_call(inp: *const u8, len: usize, out: *mut RawBuf) -> i32 {
    let input: &[u8] = if inp.is_null() || len == 0 {
        &[]
    } else {
        // SAFETY: the host passes a live `len`-byte buffer.
        unsafe { std::slice::from_raw_parts(inp, len) }
    };
    let mut buf = RawBuf::empty();
    let status = oxide_rustfn::shim::run_shim(input, &mut buf, |bytes| {
        let text: String = oxide_rustfn::bridge::decode_odf(bytes).map_err(|e| e.to_string())?;
        oxide_rustfn::bridge::encode_odf(&text).map_err(|e| e.to_string())
    });
    // SAFETY: the host always passes a writable slot.
    unsafe {
        if !out.is_null() {
            *out = buf;
        }
    }
    status
}

fn fixture_types(exports: usize) -> &'static str {
    let fns: Vec<FnTypeEntry> = (0..exports)
        .map(|i| FnTypeEntry {
            name: format!("fn_{i}"),
            file: "src/echo.server.rs".to_string(),
            args: vec!["string".to_string()],
            ret: "string".to_string(),
            decls: Vec::new(),
        })
        .collect();
    let payload = types_json(&fns);
    Box::leak(payload.into_boxed_str())
}

/// The consult path an in-flight `/_oxide/fn/<id>` request pays through
/// the live host: the `has` gate, then the call (lookup-and-clone under
/// the swap lock, the run outside it).
fn bench_swappable_consult(c: &mut Criterion) {
    let host = Arc::new(SwappableHost::new());
    let lib = RustFnLibrary::from_static(
        table(Box::leak(
            vec![FnEntry {
                name: leak_name("echo"),
                call: echo_call,
            }]
            .into_boxed_slice(),
        )),
        Some(fixture_types(1)),
    )
    .expect("a current-ABI table loads");
    let id = server_fn_id("src/echo.server.rs", "echo", "bench-secret");
    host.swap(lib, &[FnBinding::new(id.clone(), "echo")])
        .expect("the mount");
    let source =
        oxide_fw_cli::native_fns::swappable_native_fn_source(Arc::clone(&host)).expect("serves");
    let args = br#""bench""#;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let mut group = c.benchmark_group("native_fn_consult");
    group.bench_function("has_gate", |b| {
        b.iter(|| black_box(source.has(black_box(&id))));
    });
    group.bench_function("call_through", |b| {
        b.iter(|| {
            let out = rt.block_on(source.call(black_box(&id), black_box(args)));
            black_box(out.expect("the consult answers"));
        });
    });
    group.finish();
}

/// The bundle-time arms: the resolve walk (`./echo.server` → the module
/// file) and the load render (payload read once, then the stub source).
fn bench_server_rs_hooks(c: &mut Criterion) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let dir = root.join("native");
    std::fs::create_dir_all(dir.join("src")).expect("crate dirs");
    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"native-fns\"\nversion = \"0.1.0\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n",
    )
    .expect("manifest");
    std::fs::write(dir.join("src/echo.server.rs"), "// the module\n").expect("module");
    std::fs::create_dir_all(root.join(".oxide/rust")).expect("mount dir");
    std::fs::write(root.join(".oxide/rust/types.json"), fixture_types(8)).expect("payload");
    let nc = discover_native_crate(root).expect("the crate");
    let importer = root.join("src/routes/page.tsx");
    let module_id = dir.join("src/echo.server.rs").to_string_lossy().to_string();
    let resolve = server_rs_resolve_hook(&nc).expect("the hook");
    let load =
        server_rs_load_hook(root, &nc, "bench-secret", CompilerEnv::Client).expect("the hook");
    let mut group = c.benchmark_group("server_rs_hooks");
    group.bench_function("resolve_arm", |b| {
        b.iter(|| {
            black_box(
                resolve
                    .call(
                        black_box("./echo.server"),
                        black_box(&importer.to_string_lossy()),
                    )
                    .expect("the hook runs"),
            );
        });
    });
    group.bench_function("load_arm", |b| {
        b.iter(|| {
            black_box(load.call(black_box(&module_id)).expect("the hook runs"));
        });
    });
    group.finish();
}

/// The discovery walk (once per boot / per build): the manifest scan over
/// the root + one level of siblings.
fn bench_discovery(c: &mut Criterion) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    for name in ["native", "packages-a", "packages-b", "tooling"] {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).expect("dirs");
        if name == "native" {
            std::fs::write(
                dir.join("Cargo.toml"),
                "[package]\nname = \"native-fns\"\nversion = \"0.1.0\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n",
            )
            .expect("manifest");
        } else {
            std::fs::write(dir.join("package.json"), "{}").expect("manifest");
        }
    }
    let mut group = c.benchmark_group("native_discovery");
    group.bench_function("root_scan", |b| {
        b.iter(|| black_box(discover_native_crate(black_box(root))));
    });
    group.finish();
}

criterion_group!(
    native_fns_benches,
    bench_swappable_consult,
    bench_server_rs_hooks,
    bench_discovery
);
criterion_main!(native_fns_benches);
