//! The V2-F08 finish pins (`feat/rustfn-cli-native-finish`).
//!
//! The WP row (`docs/spec-v0.2-draft.md` Appendix V2.O, F08) records the
//! substrate + dev-loop halves as landed and names the remaining halves:
//! "the CLI adoption (the `cmd_dev` task + the consult adapter holding the
//! host), the production `dist/native/<lib>` emission and the bundle-time
//! `.server.rs` import resolution". The pins below are that WP's
//! acceptance at the seam level, written RED first (they failed on
//! unresolved-symbol errors before `native_fns` existed):
//!
//! 1. [`the_consult_answers_the_swapped_generation_without_a_restart`] —
//!    the §V2-F08 dev-loop acceptance "the dev loop reflects a function
//!    body change in under 3 s", read at the consult seam: after the
//!    loop's build → load → swap lands a new generation, the consult
//!    answers the NEW behavior through the SAME source object.
//! 2. [`the_emission_writes_dist_native_types_and_the_mount_copy`] —
//!    "Production: `oxide build` builds release, writes
//!    `dist/native/<lib>`" (§V2-F08).
//! 3. [`server_rs_imports_resolve_and_load_the_generated_stub`] — "the
//!    stub generator emits for `.server.rs` exports the same
//!    `serverFn`-style client stub as for `.server.ts`" (§V2-F08).
//!
//! The build-id cover pin lives in `crates/cache/tests/build_id_tdd.rs`
//! (the covered set is the cache crate's contract); the discovery /
//! refuse-loud / dev-task-wiring pins ride this file.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use oxide_fw_cli::native_fns::{
    CargoEmitSource, DevNativeHandle, NativeCrate, NativeEmit, NativeEmitSource,
    build_server_fn_dispatcher_with, compose_load_hooks, compose_resolve_hooks,
    dev_native_consult_with, discover_native_crate, emit_native_lib,
    ensure_native_crate_consistency, server_rs_load_hook, server_rs_resolve_hook,
    swappable_native_fn_source,
};
use oxide_fw_compiler::{Env as CompilerEnv, rust_fn_stub_source, server_fn_id};
use oxide_fw_dev::server_lib::{BuildOutput, LibBuild, LibLoad, ServerLibSpec};
use oxide_fw_diagnostics::OxideError;
use oxide_rustfn::abi::{FnEntry, FnTable, RawBuf, leak_name};
use oxide_rustfn::host::RustFnLibrary;
use oxide_rustfn::swap::SwappableHost;
use oxide_rustfn::types::{FnTypeEntry, types_json};

// ---------------------------------------------------------------------------
// The in-process cdylib fixture (the `server_lib_loop` walkthrough's shape:
// static FnTables standing in for `cargo build --lib` output).
// ---------------------------------------------------------------------------

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

extern "C" fn echo_v1(inp: *const u8, len: usize, out: *mut RawBuf) -> i32 {
    echo_shim(inp, len, out, "v1")
}

extern "C" fn echo_v2(inp: *const u8, len: usize, out: *mut RawBuf) -> i32 {
    echo_shim(inp, len, out, "v2")
}

fn echo_shim(inp: *const u8, len: usize, out: *mut RawBuf, prefix: &str) -> i32 {
    let input: &[u8] = if inp.is_null() || len == 0 {
        &[]
    } else {
        // SAFETY: the host passes a live `len`-byte buffer.
        unsafe { std::slice::from_raw_parts(inp, len) }
    };
    let mut buf = RawBuf::empty();
    let status = oxide_rustfn::shim::run_shim(input, &mut buf, |bytes| {
        let text: String = oxide_rustfn::bridge::decode_odf(bytes).map_err(|e| e.to_string())?;
        let reply = format!("{prefix}:{text}");
        oxide_rustfn::bridge::encode_odf(&reply).map_err(|e| e.to_string())
    });
    // SAFETY: the host always passes a writable slot.
    unsafe {
        if !out.is_null() {
            *out = buf;
        }
    }
    status
}

/// The fixture types payload: one `echo` export recorded against the
/// watched module (the file half of the §9.4 id triple).
fn echo_types() -> &'static str {
    let payload = types_json(&[FnTypeEntry {
        name: "echo".to_string(),
        file: "src/echo.server.rs".to_string(),
        args: vec!["string".to_string()],
        ret: "string".to_string(),
        decls: Vec::new(),
    }]);
    Box::leak(payload.into_boxed_str())
}

fn echo_entries(call: extern "C" fn(*const u8, usize, *mut RawBuf) -> i32) -> &'static [FnEntry] {
    Box::leak(
        vec![FnEntry {
            name: leak_name("echo"),
            call,
        }]
        .into_boxed_slice(),
    )
}

// ---------------------------------------------------------------------------
// Fixtures: the project layout (the §V2-F08 crate shape) and the fake seams.
// ---------------------------------------------------------------------------

/// The crate layout the walkthrough example scaffolds: a `native/` crate
/// beside the app with a `[lib] crate-type = ["cdylib"]` manifest and a
/// watched `src/*.server.rs` module.
fn write_native_crate(root: &Path) -> PathBuf {
    let dir = root.join("native");
    std::fs::create_dir_all(dir.join("src")).expect("native crate dirs");
    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"native-fns\"\nversion = \"0.1.0\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n",
    )
    .expect("manifest");
    std::fs::write(dir.join("src/lib.rs"), "// the native fns crate\n").expect("lib");
    std::fs::write(dir.join("src/echo.server.rs"), "// the watched module\n").expect("module");
    dir
}

/// The loop's fake build pass: generation N's stand-in for
/// `cargo build --lib` (the real pass is `CargoLibBuild`, proven by the
/// `#[ignore]` real-cargo pins in the dev crate).
struct FakeBuild {
    generation: Arc<AtomicU64>,
}

impl LibBuild for FakeBuild {
    fn build(&self, _spec: &ServerLibSpec) -> Result<BuildOutput, OxideError> {
        let n = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(BuildOutput {
            cdylib_path: PathBuf::from(format!("fake://target/debug/echo_g{n}.so")),
        })
    }
}

/// The loop's fake load pass: the generation's library with the fixture
/// types payload (mirroring `CopyLoader`'s copy-then-load contract).
struct FakeLoad {
    generation: Arc<AtomicU64>,
    dest_writes: Arc<AtomicU64>,
}

impl LibLoad for FakeLoad {
    fn load(&self, cdylib: &Path, dest_dir: &Path) -> Result<RustFnLibrary, OxideError> {
        let n = self.generation.load(Ordering::SeqCst);
        // The copy half: the mount dir gains the versioned file, like the
        // production `CopyLoader` (the consult's boot walk reads it).
        std::fs::create_dir_all(dest_dir).expect("mount dir");
        std::fs::write(
            dest_dir.join(format!("lib-{n}.so")),
            cdylib.to_string_lossy().as_bytes(),
        )
        .expect("mount copy");
        self.dest_writes.fetch_add(1, Ordering::SeqCst);
        let call = if n % 2 == 1 { echo_v1 } else { echo_v2 };
        Ok(
            RustFnLibrary::from_static(table(echo_entries(call)), Some(echo_types()))
                .expect("a current-ABI table loads"),
        )
    }
}

/// A fake emission source: "cargo build --release" that writes a real
/// (non-loadable) file so the byte-copy legs run.
struct FakeEmitSource;

impl NativeEmitSource for FakeEmitSource {
    fn emit(&self, _spec: &ServerLibSpec, rootp: &Path) -> Result<NativeEmit, OxideError> {
        // The production pass copies the built cdylib into the mount dir
        // (`RustFnLibrary::load`'s copy half); the fake mirrors the shape.
        let dest_dir = rootp.join(".oxide/rust");
        std::fs::create_dir_all(&dest_dir).expect("mount dir");
        let mounted = dest_dir.join("lib-1.so");
        std::fs::write(&mounted, b"cdylib-bytes").expect("mount copy");
        Ok(NativeEmit {
            cdylib: mounted,
            types_json: echo_types().to_string(),
        })
    }
}

// ---------------------------------------------------------------------------
// Pins
// ---------------------------------------------------------------------------

#[test]
fn discover_native_crate_finds_the_crate_layout_and_skips_dep_trees() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let dir = write_native_crate(root);
    let found = discover_native_crate(root).expect("the cdylib crate is discovered");
    assert_eq!(found.dir, dir, "the crate dir is the discovered one");
    assert_eq!(found.name, "native-fns", "the manifest's package name");

    // A dependency tree crate is not a first-party fact.
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write_native_crate(&root.join("node_modules/pkg"));
    assert!(
        discover_native_crate(root).is_none(),
        "a crate under node_modules is not discovered"
    );

    // A non-cdylib crate (no [lib] crate-type) is not the native crate.
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    std::fs::create_dir_all(root.join("plain/src")).expect("dirs");
    std::fs::write(
        root.join("plain/Cargo.toml"),
        "[package]\nname = \"plain\"\nversion = \"0.1.0\"\n",
    )
    .expect("manifest");
    assert!(
        discover_native_crate(root).is_none(),
        "a crate without a cdylib [lib] is not the native crate"
    );
}

#[test]
fn the_consult_answers_the_swapped_generation_without_a_restart() {
    // The §V2-F08 acceptance at the consult seam: the source object is
    // constructed ONCE (as the dispatcher holds it), the swap lands a new
    // generation under the same host, and the consult answers the NEW
    // behavior. A restart or a rebuilt source would defeat the dev loop.
    // (The boot-mount convention refuses an EMPTY host — the live dev
    // source is the `dev_native_consult_with` handle's, which accepts the
    // pre-first-swap emptiness.)
    let host = Arc::new(SwappableHost::new());
    assert!(
        swappable_native_fn_source(Arc::clone(&host)).is_none(),
        "the boot-mount convention: an empty host yields no source"
    );
    let secret = "consult-secret";
    let id = server_fn_id("src/echo.server.rs", "echo", secret);
    let bindings = vec![oxide_rustfn::swap::FnBinding::new(id.clone(), "echo")];

    // Generation 1: mount the v1 table with the fixture bindings, THEN
    // construct the consult source (the boot walk's order).
    let lib1 = RustFnLibrary::from_static(table(echo_entries(echo_v1)), Some(echo_types()))
        .expect("v1 loads");
    host.swap(lib1, &bindings).expect("the first mount");
    let source = swappable_native_fn_source(Arc::clone(&host)).expect("the host serves");
    assert!(source.has(&id), "the mounted id is native-owned");
    let out1 = futures_block_on(source.call(&id, br#""hi""#)).expect("v1 answers");
    let text1: String = oxide_rustfn::bridge::decode_odf(&out1).expect("the v1 reply decodes");
    assert_eq!(text1, "v1:hi", "the first generation's behavior");

    // Generation 2: the dev loop's swap — same host, SAME source object.
    let lib2 = RustFnLibrary::from_static(table(echo_entries(echo_v2)), Some(echo_types()))
        .expect("v2 loads");
    host.swap(lib2, &bindings).expect("the rebind");
    let out2 = futures_block_on(source.call(&id, br#""hi""#)).expect("v2 answers");
    let text2: String = oxide_rustfn::bridge::decode_odf(&out2).expect("the v2 reply decodes");
    assert_eq!(text2, "v2:hi", "the swapped generation's behavior");
}

#[test]
fn the_dispatcher_prefers_the_caller_held_native_source() {
    // `build_server_fn_dispatcher_with` honors the caller-held source (the
    // dev loop's live host) over the boot mount walk; `None` keeps the
    // pre-finish shape (the boot walk, then the JS pool).
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let host = Arc::new(SwappableHost::new());
    let lib = RustFnLibrary::from_static(table(echo_entries(echo_v2)), Some(echo_types()))
        .expect("the fixture table loads");
    let id = server_fn_id("src/echo.server.rs", "echo", "dispatcher-secret");
    host.swap(
        lib,
        &[oxide_rustfn::swap::FnBinding::new(id.clone(), "echo")],
    )
    .expect("the mount");
    let native = swappable_native_fn_source(Arc::clone(&host)).expect("the host serves");
    // No pool (the JS half stays down) — the native half alone dispatches.
    let d = build_server_fn_dispatcher_with(root, None, Some(Arc::clone(&native)))
        .expect("the native half builds the dispatcher");
    let reply = futures_block_on(d.dispatch(&id, br#""held""#)).expect("the held source answers");
    let text: String = oxide_rustfn::bridge::decode_odf(&reply).expect("the reply decodes");
    assert_eq!(text, "v2:held", "the override answers, not a rebuilt mount");
}

#[test]
fn the_boot_mount_walk_stays_the_none_fallback() {
    // The pre-finish shape is unchanged when no caller-held host exists:
    // an empty `.oxide/rust` mounts nothing and the dispatcher degrades to
    // the JS pool (here: `None`, no pool either).
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".oxide/rust")).expect("mount dir");
    assert!(
        build_server_fn_dispatcher_with(root, None, None).is_none(),
        "no libraries and no pool — no dispatcher"
    );
}

#[test]
fn the_emission_writes_dist_native_types_and_the_mount_copy() {
    // §V2-F08 "Production: `oxide build` … writes `dist/native/<lib>`":
    // the cdylib bytes land under dist/native beside the types payload,
    // and the mount copy under `.oxide/rust` stays for the serve boot walk.
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let dir = write_native_crate(root);
    let spec = ServerLibSpec::new("native-fns", &dir);
    let report = emit_native_lib(root, &root.join("dist"), &spec, &FakeEmitSource)
        .expect("the emission runs");
    assert_eq!(report.fn_count, 1, "the fixture payload carries one fn");
    let lib = root.join("dist/native/lib-1.so");
    assert!(lib.is_file(), "dist/native/lib-1.<ext> exists");
    assert_eq!(
        std::fs::read(&lib).expect("lib bytes"),
        b"cdylib-bytes",
        "the mounted copy's bytes are the deployed bytes"
    );
    let types = std::fs::read_to_string(root.join("dist/native/types.json"))
        .expect("dist/native/types.json");
    assert!(types.contains("\"echo\""), "the payload lists the export");
    let mount_types = std::fs::read_to_string(root.join(".oxide/rust/types.json"))
        .expect(".oxide/rust/types.json");
    assert_eq!(types, mount_types, "one payload, two write sites");
}

#[test]
fn the_emission_reports_the_real_cargo_production_source() {
    // The production source is constructible without running cargo (the
    // `#[ignore]` real-cargo pins in the dev crate prove the cargo half).
    let _: CargoEmitSource = CargoEmitSource;
}

#[tokio::test]
async fn the_dev_task_wiring_swaps_and_the_consult_follows() {
    // The cmd_dev wiring, end to end with the injectable seams: discovery
    // builds the loop + the swappable consult; the loop's poll lands a
    // swap; the SAME handle's source answers the new behavior. The task
    // spawns into the ambient runtime (`cmd_dev`'s), so the pin runs
    // inside `#[tokio::test]`'s.
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    let dir = write_native_crate(&root);
    let generation = Arc::new(AtomicU64::new(0));
    let dest_writes = Arc::new(AtomicU64::new(0));
    let handle: DevNativeHandle = dev_native_consult_with(
        &root,
        "wiring-secret",
        Box::new(FakeBuild {
            generation: Arc::clone(&generation),
        }),
        Box::new(FakeLoad {
            generation: Arc::clone(&generation),
            dest_writes: Arc::clone(&dest_writes),
        }),
        Duration::from_millis(10),
    )
    .expect("the crate is discovered and the task starts");
    // The seed pass runs one poll at boot (no changes — no swap).
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert_eq!(
        generation.load(Ordering::SeqCst),
        0,
        "the boot poll reports no change"
    );
    // An edit to the watched module fires the watch → build → load → swap
    // chain inside the task.
    std::fs::write(dir.join("src/echo.server.rs"), "// an edit\n").expect("edit");
    for _ in 0..40 {
        if generation.load(Ordering::SeqCst) >= 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(
        generation.load(Ordering::SeqCst),
        1,
        "the edit triggered exactly one rebuild"
    );
    let id = server_fn_id("src/echo.server.rs", "echo", "wiring-secret");
    let out = handle
        .source
        .call(&id, br#""again""#)
        .await
        .expect("the swap serves");
    let text: String = oxide_rustfn::bridge::decode_odf(&out).expect("the reply decodes");
    // The first rebuild's generation (n=1 → the fake load's odd arm → v1):
    // the consult serves the SWAPPED generation, not the boot-empty host.
    assert_eq!(text, "v1:again", "the consult follows the swapped host");
    handle.shutdown();
}

#[test]
fn server_rs_imports_resolve_and_load_the_generated_stub() {
    // §V2-F08: "the stub generator emits for `.server.rs` exports the same
    // `serverFn`-style client stub as for `.server.ts`" — the bundle-time
    // resolve + load hooks rewrite the import to the generated module.
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let dir = write_native_crate(root);
    let nc = discover_native_crate(root).expect("the crate");
    std::fs::create_dir_all(root.join(".oxide/rust")).expect("mount dir");
    std::fs::write(root.join(".oxide/rust/types.json"), echo_types()).expect("payload");
    // The importer page sits INSIDE the crate, BESIDE the server module
    // (the spec's `src/thumb.server.rs` layout: the crate's src/ IS the
    // app's src/), so `./echo.server` resolves to the sibling module.
    let importer = dir.join("src/page.tsx");
    std::fs::write(&importer, "export default () => null;").expect("page");

    // The resolve arm: `./echo.server` (and the explicit `.server.rs`
    // spelling) resolves to the module file, so the built-in resolver's
    // missing-`.rs` extension never fires.
    let resolve = server_rs_resolve_hook(&nc).expect("the crate yields the hook");
    let resolved = resolve
        .call("./echo.server", &importer.to_string_lossy())
        .expect("the hook runs")
        .expect("the specifier resolves");
    assert_eq!(
        resolved,
        dir.join("src/echo.server.rs"),
        "the absolute module path"
    );
    assert!(
        resolve
            .call("./unrelated", &importer.to_string_lossy())
            .expect("the hook runs")
            .is_none(),
        "a non-server specifier falls through"
    );

    // The load arm: the resolved id loads the generated stub, both envs.
    let module_path = "src/echo.server.rs";
    let secret = "stub-secret";
    for (env, expect_ids_export) in [(CompilerEnv::Client, false), (CompilerEnv::Ssr, true)] {
        let load = server_rs_load_hook(root, &nc, secret, env).expect("the hook");
        let id = &load
            .call(&dir.join("src/echo.server.rs").to_string_lossy())
            .expect("the hook runs")
            .expect("a .server.rs id loads the stub");
        let expected = rust_fn_stub_source(module_path, &["echo"], env, secret).0;
        assert_eq!(*id, expected, "the generator's exact bytes, {env:?}");
        assert!(id.contains("__oxide_call_fn"), "the call-through shape");
        assert!(id.contains("@oxide-fw/core/fn"), "the runtime import");
        assert_eq!(
            id.contains("__oxide_fn_ids"),
            expect_ids_export,
            "the SSR/Edge id list rides the server envs only"
        );
        assert!(
            id.contains(&server_fn_id(module_path, "echo", secret)),
            "the §9.4 id the consult answers"
        );
    }
    // A non-server id falls through to the next hook in the chain.
    let load = server_rs_load_hook(root, &nc, secret, CompilerEnv::Client).expect("the hook");
    assert!(
        load.call(&root.join("src/page.tsx").to_string_lossy())
            .expect("the hook runs")
            .is_none(),
        "a non-server id falls through"
    );

    // The composition: the server-rs arm first, the JS plugin arm second.
    let composed = compose_load_hooks(
        server_rs_load_hook(root, &nc, secret, CompilerEnv::Client),
        None,
    )
    .expect("one hook survives");
    assert!(
        composed
            .call(&dir.join("src/echo.server.rs").to_string_lossy())
            .expect("the hook runs")
            .is_some(),
        "the composed chain answers the server id"
    );
    let resolve_composed =
        compose_resolve_hooks(server_rs_resolve_hook(&nc), None).expect("one hook survives");
    assert!(
        resolve_composed
            .call("./echo.server", &importer.to_string_lossy())
            .expect("the hook runs")
            .is_some(),
        "the composed resolve chain answers"
    );
}

#[test]
fn server_fn_sources_without_a_crate_refuse_loud() {
    // The `RFN-005` arm: `.server.rs` sources under the app root with no
    // discoverable cdylib crate — the functions would 404 at runtime, so
    // the build refuses instead of emitting stubs with no server behind
    // them.
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src/routes")).expect("dirs");
    std::fs::write(root.join("src/routes/thumb.server.rs"), "fn t() {}").expect("source");
    let e = ensure_native_crate_consistency(root).expect_err("the mismatch refuses");
    assert_eq!(e.0.code, "RFN-005", "the coded refusal");
    assert!(
        e.0.message.contains("thumb.server.rs"),
        "the source is named"
    );

    // The consistent shapes pass: no sources at all, and sources with a
    // crate beside them.
    let tmp = tempfile::tempdir().expect("tempdir");
    assert!(
        ensure_native_crate_consistency(tmp.path()).is_ok(),
        "no sources, no crate — consistent"
    );
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let _dir = write_native_crate(root);
    assert!(
        ensure_native_crate_consistency(root).is_ok(),
        "sources with a crate — consistent"
    );
}

/// Block on the consult's async call from the sync test body (the house
/// single-threaded tokio recipe).
fn futures_block_on<T>(fut: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(fut)
}

// The `NativeCrate` name import keeps the discover pin's type honest.
const _: fn(&Path) -> Option<NativeCrate> = discover_native_crate;
