//! Criterion bench for the §V2-F08 dev-loop substrate (`feat/server-lib-dev-loop`,
//! the B09 pattern: plain `cargo test` never pays the criterion build — run
//! behind `--features bench-util`).
//!
//! The dev-loop-relevant questions:
//!
//! - `server_lib_loop/poll_idle_{8,32}`: the per-tick cost `oxide dev` pays
//!   when the native crate did NOT change — the watcher walk over the crate
//!   tree, the content hashes and the watch-set filter. This runs every
//!   poll interval for every app with a native-fn crate, so it must be a
//!   walk-shaped cost, not a build-shaped one.
//! - `server_lib_loop/step`: one FULL loop step against fakes (build →
//!   load → bindings → swap) — the orchestration overhead on top of cargo,
//!   i.e. what a rebuild costs minus the compile itself.
//! - `server_lib_swap/{4,16}_bindings`: the host-state swap — mount +
//!   re-point + drop under the swap lock, the operation a rebuild lands
//!   once per edit.
//! - `bindings_from_types/16_fns`: the types-payload parse + the §9.4 id
//!   derivation per rebuilt export.
//!
//! Run with:
//!
//! ```text
//! cargo bench -p oxide-fw-dev --features bench-util -- server_lib
//! ```

use std::path::{Path, PathBuf};

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_compiler::server_fn_id;
use oxide_fw_dev::FileEvent;
use oxide_fw_dev::server_lib::{
    BuildOutput, FnBinding, LibBuild, LibLoad, ServerLibLoop, ServerLibSpec, ServerLibSwap,
    bindings_from_types,
};
use oxide_fw_diagnostics::OxideError;
use oxide_rustfn::abi::{ABI_VERSION, FnEntry, FnTable, RawBuf, leak_name};
use oxide_rustfn::host::RustFnLibrary;
use oxide_rustfn::swap::SwappableHost;
use oxide_rustfn::types::{FnTypeEntry, types_json};

/// A no-op builder (the bench's build pass is free — the question is the
/// loop's orchestration, not cargo).
struct NullBuild(PathBuf);

impl LibBuild for NullBuild {
    fn build(&self, _spec: &ServerLibSpec) -> Result<BuildOutput, OxideError> {
        Ok(BuildOutput {
            cdylib_path: self.0.clone(),
        })
    }
}

extern "C" fn free_impl(buf: RawBuf) {
    let _ = buf;
}

extern "C" fn noop_shim(_inp: *const u8, _len: usize, _out: *mut RawBuf) -> i32 {
    0
}

fn table(entries: &'static [FnEntry]) -> &'static FnTable {
    Box::leak(Box::new(FnTable {
        abi_version: ABI_VERSION,
        len: entries.len(),
        entries: entries.as_ptr(),
        free: free_impl,
    }))
}

/// The generation's table: `fns` exports named `fn_<i>`, recorded against
/// `src/mod_<i>.server.rs` so the bindings are real §9.4 ids.
fn fixture_lib(fns: usize) -> RustFnLibrary {
    let entries: &'static [FnEntry] = Box::leak(
        (0..fns)
            .map(|i| {
                let name: &'static str = Box::leak(format!("fn_{i}").into_boxed_str());
                FnEntry {
                    name: leak_name(name),
                    call: noop_shim,
                }
            })
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    );
    RustFnLibrary::from_static(table(entries), None).expect("a current-ABI table loads")
}

/// A loader that serves one pre-built table + payload per call (the
/// generation counter keeps the swap story honest; the table itself is
/// the same static fixture, so the bench leaks nothing per iteration).
struct GenLoader {
    table: &'static FnTable,
    payload: &'static str,
}

impl LibLoad for GenLoader {
    fn load(&self, _cdylib: &Path, _dest_dir: &Path) -> Result<RustFnLibrary, OxideError> {
        Ok(RustFnLibrary::from_static(self.table, Some(self.payload))
            .expect("a current-ABI table loads"))
    }
}

/// The bench fixture's static table + types payload for `fns` exports
/// (built once per bench setup — the loop's `load` clones the wrapper).
fn fixture_table(fns: usize) -> (&'static FnTable, &'static str) {
    let entries: &'static [FnEntry] = Box::leak(
        (0..fns)
            .map(|i| {
                let name: &'static str = Box::leak(format!("fn_{i}").into_boxed_str());
                FnEntry {
                    name: leak_name(name),
                    call: noop_shim,
                }
            })
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    );
    let payload: &'static str = Box::leak(
        types_json(
            &(0..fns)
                .map(|i| FnTypeEntry {
                    name: format!("fn_{i}"),
                    file: format!("src/mod_{i}.server.rs"),
                    args: Vec::new(),
                    ret: String::new(),
                    decls: Vec::new(),
                })
                .collect::<Vec<_>>(),
        )
        .into_boxed_str(),
    );
    (table(entries), payload)
}

/// A swap that counts (the loop's hand-off against a REAL host is benched
/// separately in `server_lib_swap`).
struct CountingSwap;

impl ServerLibSwap for CountingSwap {
    fn swap(
        &self,
        _library: RustFnLibrary,
        bindings: &[FnBinding],
    ) -> Result<oxide_fw_dev::server_lib::SwapReport, OxideError> {
        Ok(oxide_fw_dev::server_lib::SwapReport {
            generation: 1,
            added: Vec::new(),
            rebound: bindings.iter().map(|b| b.id.clone()).collect(),
            dropped: Vec::new(),
            total_ids: bindings.len(),
        })
    }
}

/// A fixture native crate with `n` distractor files (the sources the
/// watcher hashes every tick) and one `.server.rs` module.
fn fixture_crate(n: usize) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    let native = root.join("native");
    std::fs::create_dir_all(native.join("src")).expect("crate src");
    std::fs::write(
        native.join("Cargo.toml"),
        "[package]\nname = \"native-fns\"\nversion = \"0.1.0\"\n[lib]\ncrate-type = [\"cdylib\"]\n",
    )
    .expect("manifest");
    for i in 0..n {
        std::fs::write(
            native.join("src").join(format!("mod_{i}.rs")),
            "// source\n",
        )
        .expect("source file");
    }
    (dir, root)
}

fn bench_loop(c: &mut Criterion) {
    let mut group = c.benchmark_group("server_lib_loop");
    for files in [8usize, 32] {
        group.bench_with_input(BenchmarkId::new("poll_idle", files), &files, |b, &files| {
            let (dir, root) = fixture_crate(files);
            let mut lp = ServerLibLoop::new(
                ServerLibSpec::new("native-fns", root.join("native")),
                Box::new(NullBuild(root.join("native/target/debug/libnative_fns.so"))),
                Box::new(CountingSwap),
            );
            lp.seed();
            // One distractor edit per iteration: the tick sees traffic
            // (the watcher's hash-diff runs) but none of it is a
            // trigger — the per-tick cost `oxide dev` pays while the
            // native crate is quiet.
            let notes = root.join("native/src/notes.rs");
            let mut n = 0usize;
            b.iter(|| {
                n += 1;
                std::fs::write(&notes, format!("// tick {n}\n")).expect("write");
                let _ = lp.poll_once().expect("the idle poll");
            });
            drop(dir);
        });
    }
    group.bench_with_input(BenchmarkId::new("step", 16), &16usize, |b, &fns| {
        let (dir, root) = fixture_crate(fns);
        let mut lp = ServerLibLoop::new(
            ServerLibSpec::new("native-fns", root.join("native")),
            Box::new(NullBuild(root.join("native/target/debug/libnative_fns.so"))),
            Box::new(CountingSwap),
        )
        .with_loader(Box::new(GenLoader {
            table: fixture_table(fns).0,
            payload: fixture_table(fns).1,
        }));
        lp.seed();
        let target = root.join("native/src/counter.server.rs");
        std::fs::write(&target, "// the server fn module\n").expect("seed edit");
        b.iter(|| {
            std::fs::write(&target, "// edited\n").expect("write");
            let _ = lp.poll_once().expect("the step");
        });
        drop(dir);
    });
    group.finish();
}

fn bench_swap(c: &mut Criterion) {
    let mut group = c.benchmark_group("server_lib_swap");
    for fns in [4usize, 16] {
        group.bench_with_input(BenchmarkId::new("bindings", fns), &fns, |b, &fns| {
            let host = SwappableHost::new();
            let payload = types_json(
                &(0..fns)
                    .map(|i| FnTypeEntry {
                        name: format!("fn_{i}"),
                        file: format!("src/mod_{i}.server.rs"),
                        args: Vec::new(),
                        ret: String::new(),
                        decls: Vec::new(),
                    })
                    .collect::<Vec<_>>(),
            );
            let bindings: Vec<FnBinding> = (0..fns)
                .map(|i| {
                    FnBinding::new(
                        server_fn_id(
                            &format!("src/mod_{i}.server.rs"),
                            &format!("fn_{i}"),
                            "bench-secret",
                        ),
                        format!("fn_{i}"),
                    )
                })
                .collect();
            b.iter(|| {
                let lib = fixture_lib(fns);
                let _ = host.swap(lib, &bindings).expect("the swap");
            });
            let _ = payload;
        });
    }
    group.finish();
}

fn bench_bindings(c: &mut Criterion) {
    let mut group = c.benchmark_group("bindings_from_types");
    let payload = types_json(
        &(0..16usize)
            .map(|i| FnTypeEntry {
                name: format!("fn_{i}"),
                file: format!("src/mod_{i}.server.rs"),
                args: vec!["In".to_string()],
                ret: "Out".to_string(),
                decls: Vec::new(),
            })
            .collect::<Vec<_>>(),
    );
    group.bench_function("16_fns", |b| {
        b.iter(|| {
            let _ = bindings_from_types(&payload, "bench-secret").expect("the bindings");
        })
    });
    group.finish();
}

/// The watcher-event filter the host can run per notify event (the
/// hot path of a busy tree is the FILTER, not the walk).
fn bench_filter(c: &mut Criterion) {
    let mut group = c.benchmark_group("server_lib_filter");
    let (dir, root) = fixture_crate(8);
    let spec = ServerLibSpec::new("native-fns", root.join("native"));
    let server_event = FileEvent::Modified(root.join("native/src/counter.server.rs"));
    let plain_event = FileEvent::Modified(root.join("native/src/mod_3.rs"));
    group.bench_function("server_module", |b| {
        b.iter(|| {
            let _ = oxide_fw_dev::server_lib::is_server_lib_event(&server_event, &spec);
        })
    });
    group.bench_function("plain_source", |b| {
        b.iter(|| {
            let _ = oxide_fw_dev::server_lib::is_server_lib_event(&plain_event, &spec);
        })
    });
    drop(dir);
    group.finish();
}

criterion_group!(
    benches,
    bench_loop,
    bench_swap,
    bench_bindings,
    bench_filter
);
criterion_main!(benches);
