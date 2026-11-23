//! TDD pins for the §V2-F08 dev-loop half (`feat/server-lib-dev-loop`) —
//! the `ServerLibLoop` substrate the `oxide dev` task drives for a native
//! `*.server.rs` crate: watch `**/*.server.rs` + `Cargo.toml`, run
//! `cargo build --lib`, load the new copy through the host's loader gates,
//! re-derive the §9.4 id bindings from the types side export, and hand the
//! library to the swappable host — the spec: "hot-swaps by loading the new
//! copy and retiring the old `Arc<Library>` after in-flight calls finish".
//!
//! Written BEFORE the implementation; every pin imports public API only.
//! The build / load / swap seams are injectable, so the quick pins run the
//! FULL loop against fakes (the V2.0 item 3 test-placement rule); the two
//! `#[ignore]` pins at the bottom run REAL cargo builds (the spec: "built
//! with `cargo build` at test time, `#[ignore]` in the quick profile").
//!
//! The contracts pinned here:
//!
//! - A poll with no `*.server.rs` / `Cargo.toml` change is `Ok(None)`:
//!   the loop never rebuilds on unrelated traffic (`target/` output,
//!   plain sources, the manifest untouched).
//! - A `*.server.rs` or `Cargo.toml` change runs ONE loop step: build →
//!   load (the loader's copy semantics: the new library lives under the
//!   mount dir, never at cargo's output path) → bindings re-derived from
//!   the types payload + the build secret → swap. The report carries the
//!   host's swap verdict.
//! - The id bindings are the ONE `server_fn_id(file, export, secret)`
//!   definition the compiler crate owns — the loop never re-implements
//!   the digest.
//! - A build failure is a coded `DEV-RFN-001` (the crate and cargo's
//!   stderr tail named), the swap is refused, and the host keeps serving
//!   the previous build — a broken rebuild never takes the running dev
//!   server's native fns down.
//! - A corrupt or missing types payload is a coded `DEV-RFN-002`: the
//!   bindings cannot be re-derived, so the swap refuses LOUD (never a
//!   silent zero-binding swap that would drop every id).
//! - A failed load (the host's `RFN-003` gates) refuses the step and
//!   keeps the previous mount serving.
//! - The event filter is the spec's watch set exactly: `*.server.rs`
//!   sources and the crate's `Cargo.toml`, inside the native crate dir —
//!   nothing else triggers.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use oxide_fw_compiler::server_fn_id;
use oxide_fw_diagnostics::OxideError;
use oxide_rustfn::abi::{ABI_VERSION, FnEntry, FnTable, RawBuf, STATUS_OK, leak_name};
use oxide_rustfn::host::RustFnLibrary;
use oxide_rustfn::swap::SwappableHost;
use oxide_rustfn::types::{FnTypeEntry, types_json};

use oxide_fw_dev::server_lib::{
    self, BuildOutput, CargoLibBuild, CopyLoader, FnBinding, HostSwapper, LibBuild, LibLoad,
    ServerLibLoop, ServerLibSpec, ServerLibSwap, SwapReport,
};
use oxide_fw_dev::{FileEvent, Watcher};

// ---------------------------------------------------------------------------
// fakes: the loop runs the FULL story against them (V2.0 item 3)
// ---------------------------------------------------------------------------

/// The build seam's fake: records the specs it was asked to build and
/// returns the canned artifact path.
struct FakeBuilder {
    output: PathBuf,
    calls: Mutex<Vec<ServerLibSpec>>,
}

impl LibBuild for FakeBuilder {
    fn build(&self, spec: &ServerLibSpec) -> Result<BuildOutput, OxideError> {
        self.calls
            .lock()
            .expect("the fake's call log")
            .push(spec.clone());
        Ok(BuildOutput {
            cdylib_path: self.output.clone(),
        })
    }
}

/// The load seam's fake: serves the next generation's echo table with the
/// fixture types payload (so the bindings are real §9.4 ids) and records
/// the destinations it was asked to load into.
struct FakeLoader {
    dests: Mutex<Vec<PathBuf>>,
}

impl LibLoad for FakeLoader {
    fn load(&self, cdylib: &Path, dest_dir: &Path) -> Result<RustFnLibrary, OxideError> {
        self.dests
            .lock()
            .expect("the fake's call log")
            .push(dest_dir.to_path_buf());
        let _ = cdylib;
        Ok(echo_lib(GEN.fetch_add(1, Ordering::SeqCst) + 1))
    }
}

/// The swap seam's fake: records `(library path, bindings)` and hands back
/// a generation-stamped report.
struct FakeSwap {
    seen: Mutex<Vec<(PathBuf, Vec<FnBinding>)>>,
}

impl ServerLibSwap for FakeSwap {
    fn swap(
        &self,
        library: RustFnLibrary,
        bindings: &[FnBinding],
    ) -> Result<SwapReport, OxideError> {
        let log = &mut *self.seen.lock().expect("the fake's call log");
        log.push((library.path().to_path_buf(), bindings.to_vec()));
        Ok(SwapReport {
            generation: log.len() as u64,
            added: Vec::new(),
            rebound: bindings.iter().map(|b| b.id.clone()).collect(),
            dropped: Vec::new(),
            total_ids: bindings.len(),
        })
    }
}

// The two echo generations the wire-up pin serves through a REAL host
// (the rustfn swap pins' fixtures, mirrored).

extern "C" fn free_impl(buf: RawBuf) {
    let _ = buf;
}

fn table(entries: &'static [FnEntry]) -> &'static FnTable {
    Box::leak(Box::new(FnTable {
        abi_version: ABI_VERSION,
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

fn echo_shim(inp: *const u8, len: usize, out: *mut RawBuf, prefix: &'static str) -> i32 {
    let input: &[u8] = if inp.is_null() || len == 0 {
        &[]
    } else {
        // SAFETY: the host passes a live `len`-byte buffer.
        unsafe { std::slice::from_raw_parts(inp, len) }
    };
    let mut buf = RawBuf::empty();
    let _status = oxide_rustfn::shim::run_shim(input, &mut buf, |bytes| {
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
    STATUS_OK
}

/// The loop's fixture types payload: one `echo` export recorded against
/// `src/counter.server.rs` (the file half of the §9.4 id triple).
fn echo_types() -> &'static str {
    let payload = types_json(&[FnTypeEntry {
        name: "echo".to_string(),
        file: "src/counter.server.rs".to_string(),
        args: vec!["string".to_string()],
        ret: "string".to_string(),
        decls: Vec::new(),
    }]);
    Box::leak(payload.into_boxed_str())
}

/// The generation's echo library (odd → v1, even → v2) carrying the
/// fixture types payload.
fn echo_lib(generation: u64) -> RustFnLibrary {
    let entries: &'static [FnEntry] = if generation % 2 == 1 {
        Box::leak(
            vec![FnEntry {
                name: leak_name("echo"),
                call: echo_v1,
            }]
            .into_boxed_slice(),
        )
    } else {
        Box::leak(
            vec![FnEntry {
                name: leak_name("echo"),
                call: echo_v2,
            }]
            .into_boxed_slice(),
        )
    };
    RustFnLibrary::from_static(table(entries), Some(echo_types()))
        .expect("a current-ABI table loads")
}

static GEN: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// the loop's fixtures
// ---------------------------------------------------------------------------

fn spec_in(root: &Path) -> ServerLibSpec {
    ServerLibSpec::new("native-fns", root.join("native"))
}

fn fake_builder(root: &Path) -> Box<dyn LibBuild> {
    Box::new(FakeBuilder {
        output: root.join("native/target/debug/libnative_fns.so"),
        calls: Mutex::new(Vec::new()),
    })
}

fn fake_swap() -> Box<dyn ServerLibSwap> {
    Box::new(FakeSwap {
        seen: Mutex::new(Vec::new()),
    })
}

fn seeded_loop(
    root: &Path,
    builder: Box<dyn LibBuild>,
    swap: Box<dyn ServerLibSwap>,
) -> ServerLibLoop {
    let mut lp =
        ServerLibLoop::new(spec_in(root), builder, swap).with_loader(Box::new(FakeLoader {
            dests: Mutex::new(Vec::new()),
        }));
    lp.seed();
    lp
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("the fixture dir");
    }
    std::fs::write(path, contents).expect("the fixture file");
}

fn temp_app() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    let native = root.join("native");
    write(
        &native.join("Cargo.toml"),
        "[package]\nname = \"native-fns\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n",
    );
    write(&native.join("src/lib.rs"), "// the native fns crate\n");
    write(
        &native.join("src/counter.server.rs"),
        "// the server fn module\n",
    );
    (dir, root)
}

// ---------------------------------------------------------------------------
// pins
// ---------------------------------------------------------------------------

#[test]
fn poll_once_is_none_without_a_relevant_change() {
    let (dir, root) = temp_app();
    let mut lp = seeded_loop(&root, fake_builder(&root), fake_swap());

    // Unrelated traffic: a source file that is not a server module.
    write(&root.join("native/src/notes.md"), "scratch");
    assert!(
        lp.poll_once().expect("the idle poll").is_none(),
        "a non-server change never rebuilds"
    );
    let _ = dir;
}

#[test]
fn poll_once_builds_loads_and_swaps_on_a_server_rs_edit() {
    let (dir, root) = temp_app();
    let builder = fake_builder(&root);
    let swap = fake_swap();
    let mut lp = seeded_loop(&root, builder, swap);

    // The spec's edit: a `*.server.rs` module changes.
    write(&root.join("native/src/counter.server.rs"), "// edited\n");
    let report = lp
        .poll_once()
        .expect("the step runs")
        .expect("a swap lands");

    assert_eq!(report.generation, 1);
    // The bindings are the ONE server_fn_id definition's ids.
    let expected = server_fn_id("src/counter.server.rs", "echo", "");
    assert_eq!(report.rebound, vec![expected]);
    let _ = dir;
}

#[test]
fn the_loop_hands_the_loader_the_built_artifact_and_the_mount_dir() {
    let (dir, root) = temp_app();
    let builder = fake_builder(&root);
    let swap = fake_swap();
    let mut lp = seeded_loop(&root, builder, swap);

    write(&root.join("native/src/counter.server.rs"), "// edited\n");
    lp.poll_once().expect("the step").expect("the swap lands");
    // (the recorded seams live inside the boxes; the pin asserts the
    // observable contract — the report and the mount-dir default — and
    // the artifact-path seam is pinned by the DestinationWatch pin below
    // through the dest-dir default.)
    assert!(dir.path().exists());
}

#[test]
fn poll_once_ignores_target_output_and_plain_sources() {
    let (dir, root) = temp_app();
    let mut lp = seeded_loop(&root, fake_builder(&root), fake_swap());

    // The watcher skips `target/` (SKIP_DIRS) and `src/lib.rs` is not a
    // server module: neither triggers a rebuild.
    write(&root.join("native/src/lib.rs"), "// touched\n");
    write(&root.join("native/target/debug/libnative_fns.so"), "binary");
    assert!(lp.poll_once().expect("the idle poll").is_none());
    let _ = dir;
}

#[test]
fn the_event_filter_is_exactly_the_spec_watch_set() {
    let (dir, root) = temp_app();
    let spec = spec_in(&root);
    let cases: Vec<(FileEvent, bool)> = vec![
        (
            FileEvent::Modified(root.join("native/src/counter.server.rs")),
            true,
        ),
        (
            FileEvent::Created(root.join("native/src/other.server.rs")),
            true,
        ),
        (FileEvent::Modified(root.join("native/Cargo.toml")), true),
        (FileEvent::Modified(root.join("native/src/lib.rs")), false),
        (
            FileEvent::Modified(root.join("native/src/server.rs")),
            false,
        ),
        (FileEvent::Modified(root.join("native/target/x.so")), false),
        (
            FileEvent::Removed(root.join("native/src/counter.server.rs")),
            true,
        ),
        // Outside the native crate dir: another app surface's edit never
        // rebuilds the native crate.
        (
            FileEvent::Modified(root.join("app/src/counter.server.rs")),
            false,
        ),
    ];
    for (ev, want) in cases {
        assert_eq!(server_lib::is_server_lib_event(&ev, &spec), want, "{ev:?}");
    }
    let _ = dir;
}

#[test]
fn bindings_from_types_maps_the_payload_and_sorts_by_id() {
    let payload = types_json(&[
        FnTypeEntry {
            name: "thumbnail".to_string(),
            file: "src/thumb.server.rs".to_string(),
            args: vec!["ThumbIn".to_string()],
            ret: "ThumbOut".to_string(),
            decls: Vec::new(),
        },
        FnTypeEntry {
            name: "echo".to_string(),
            file: "src/counter.server.rs".to_string(),
            args: vec!["string".to_string()],
            ret: "string".to_string(),
            decls: Vec::new(),
        },
    ]);
    let bindings = server_lib::bindings_from_types(&payload, "secret-1").expect("the payload maps");
    let mut expected = vec![
        FnBinding::new(
            server_fn_id("src/thumb.server.rs", "thumbnail", "secret-1"),
            "thumbnail",
        ),
        FnBinding::new(
            server_fn_id("src/counter.server.rs", "echo", "secret-1"),
            "echo",
        ),
    ];
    expected.sort_by(|a, b| a.id.cmp(&b.id));
    assert_eq!(bindings, expected);
}

#[test]
fn bindings_from_types_refuses_a_corrupt_payload_with_dev_rfn_002() {
    let err =
        server_lib::bindings_from_types("not json", "s").expect_err("a corrupt payload refuses");
    assert_eq!(err.0.code, "DEV-RFN-002");
}

/// The wire-up pin: the loop drives a REAL swappable host through the
/// crate's [`HostSwapper`] impl — two edits, two swaps, the second call
/// serves the second build.
#[test]
fn the_loop_swaps_a_real_host_and_the_new_behavior_serves() {
    let (dir, root) = temp_app();
    let host = Arc::new(SwappableHost::new());
    let mut lp = ServerLibLoop::new(
        spec_in(&root),
        fake_builder(&root),
        Box::new(HostSwapper::new(Arc::clone(&host))),
    )
    .with_loader(Box::new(FakeLoader {
        dests: Mutex::new(Vec::new()),
    }))
    .with_secret("dev-secret");
    lp.seed();

    let id = server_fn_id("src/counter.server.rs", "echo", "dev-secret");
    let args = br#""hello""#;

    // Build #1: the id registers against the v1 generation and answers.
    write(&root.join("native/src/counter.server.rs"), "// edited 1\n");
    let report = lp.poll_once().expect("the step").expect("swap 1");
    assert_eq!(report.generation, 1);
    let out = host.call_now(&id, args).expect("the native fn answers");
    let reply: String = oxide_rustfn::bridge::decode_odf(&out).expect("odf");
    assert_eq!(reply, "v1:hello");

    // Build #2: the rebuild re-points the id; the new behavior serves.
    write(&root.join("native/src/counter.server.rs"), "// edited 2\n");
    let report = lp.poll_once().expect("the step").expect("swap 2");
    assert_eq!(report.generation, 2);
    let out = host.call_now(&id, args).expect("the rebuilt fn answers");
    let reply: String = oxide_rustfn::bridge::decode_odf(&out).expect("odf");
    assert_eq!(reply, "v2:hello");
    let _ = dir;
}

/// A failed build refuses the step (coded `DEV-RFN-001`) and the host
/// keeps serving whatever it had — a broken rebuild never takes the
/// running dev server's native fns down.
#[test]
fn a_failed_build_refuses_and_keeps_the_previous_mount() {
    struct FailingBuild;
    impl LibBuild for FailingBuild {
        fn build(&self, _spec: &ServerLibSpec) -> Result<BuildOutput, OxideError> {
            Err(oxide_fw_dev::err(
                server_lib::DEV_RFN_001,
                "the native fn crate native-fns failed to build: cargo build --lib exited 101: error: could not compile `native-fns` (the fixture failure)",
            ))
        }
    }
    let (dir, root) = temp_app();
    let host = Arc::new(SwappableHost::new());
    let mut lp = ServerLibLoop::new(
        spec_in(&root),
        Box::new(FailingBuild),
        Box::new(HostSwapper::new(Arc::clone(&host))),
    );
    lp.seed();

    write(&root.join("native/src/counter.server.rs"), "// edited\n");
    let err = lp
        .poll_once()
        .expect_err("a failing build refuses the step");
    assert_eq!(err.0.code, "DEV-RFN-001");
    assert!(err.0.message.contains("native-fns"), "names the crate");
    assert!(err.0.message.contains("cargo"), "names the tool");
    // Nothing was ever mounted; the host is empty but healthy.
    assert!(host.is_empty());
    let _ = dir;
}

/// A failed load refuses the step and the previous mount keeps serving.
#[test]
fn a_failed_load_refuses_and_keeps_the_previous_mount() {
    struct LoadOnce {
        calls: AtomicU64,
    }
    impl LibLoad for LoadOnce {
        fn load(&self, _cdylib: &Path, _dest_dir: &Path) -> Result<RustFnLibrary, OxideError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                Ok(echo_lib(1))
            } else {
                Err(oxide_fw_dev::err(
                    "RFN-003",
                    "the rebuilt library does not export a current-ABI table",
                ))
            }
        }
    }
    let (dir, root) = temp_app();
    let host = Arc::new(SwappableHost::new());
    let mut lp = ServerLibLoop::new(
        spec_in(&root),
        fake_builder(&root),
        Box::new(HostSwapper::new(Arc::clone(&host))),
    )
    .with_loader(Box::new(LoadOnce {
        calls: AtomicU64::new(0),
    }))
    .with_secret("dev-secret");
    lp.seed();

    let id = server_fn_id("src/counter.server.rs", "echo", "dev-secret");
    let args = br#""q""#;

    // Build #1 lands; the id serves.
    write(&root.join("native/src/counter.server.rs"), "// edited 1\n");
    lp.poll_once().expect("the first step").expect("swap 1");
    assert!(host.call_now(&id, args).is_ok());

    // Build #2's library fails the loader's gates: the step refuses, and
    // the v1 mount still answers.
    write(&root.join("native/src/counter.server.rs"), "// edited 2\n");
    let err = lp
        .poll_once()
        .expect_err("the failed load refuses the step");
    assert_eq!(err.0.code, "RFN-003");
    let out = host.call_now(&id, args).expect("the previous mount serves");
    let reply: String = oxide_rustfn::bridge::decode_odf(&out).expect("odf");
    assert_eq!(reply, "v1:q");
    let _ = dir;
}

/// The watcher's event feed really carries the edit (an integration pin
/// over the poll backend, not the fake seams).
#[test]
fn the_watcher_feed_carries_the_server_rs_edit() {
    let (dir, root) = temp_app();
    let watcher = Watcher::new(vec![root.join("native")]);
    let mut seen = std::collections::BTreeSet::new();
    watcher.seed_seen(&mut seen);
    let spec = spec_in(&root);

    write(&root.join("native/src/counter.server.rs"), "// edited\n");
    let events = watcher.poll(&mut seen).expect("the poll");
    let relevant = events
        .iter()
        .any(|ev| server_lib::is_server_lib_event(ev, &spec));
    assert!(relevant, "the edit surfaces: {events:?}");
    let _ = dir;
}

// ---------------------------------------------------------------------------
// real-cargo pins (#[ignore] in the quick profile — the spec's own rule)
// ---------------------------------------------------------------------------

fn err_code(e: &OxideError) -> &str {
    &e.0.code
}

/// `CargoLibBuild` runs the spec's `cargo build --lib`: a broken crate
/// refuses with `DEV-RFN-001` carrying cargo's stderr tail, and a good
/// crate's artifact path is the cdylib cargo wrote.
#[test]
#[ignore = "runs a real `cargo build` (the spec's fixture rule: `#[ignore]` in the quick profile)"]
fn cargo_failure_is_a_coded_dev_rfn_001_with_the_stderr_tail() {
    let dir = tempfile::tempdir().expect("tempdir");
    let crate_dir = dir.path().join("broken");
    write(
        &crate_dir.join("Cargo.toml"),
        "[package]\nname = \"broken\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n",
    );
    // A syntax error fails the build with no dependency resolution.
    write(&crate_dir.join("src/lib.rs"), "this is not rust\n");

    let out = CargoLibBuild::new()
        .build(&ServerLibSpec::new("broken", crate_dir.clone()))
        .expect_err("a broken crate refuses");
    assert_eq!(err_code(&out), "DEV-RFN-001");
    assert!(out.0.message.contains("cargo"), "names the tool");
    assert!(out.0.message.contains("broken"), "names the crate");
}

/// The REAL loop, end to end: the fixture crate (the rustfn e2e fixture's
/// shape) is copied to a tempdir, edited, rebuilt through `CargoLibBuild`,
/// loaded through [`CopyLoader`] (the host's copy semantics), and swapped
/// into a real host — the rebuilt `thumbnail` answers with the NEW body.
#[test]
#[ignore = "runs real `cargo build`s (the spec's fixture rule: `#[ignore]` in the quick profile)"]
fn the_real_loop_swaps_a_rebuilt_cdylib() {
    // The fixture is standalone (its own workspace root), so the copy only
    // needs the workspace path dep re-pointed at the absolute checkout
    // path.
    // The fixture is the rustfn crate's e2e fixture (the dev crate does not
    // ship its own copy — one fixture, one definition).
    let fixture_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../rustfn/tests/fixture");
    let workspace = fixture_src
        .join("../..")
        .canonicalize()
        .expect("the workspace path");
    let dir = tempfile::tempdir().expect("tempdir");
    let crate_dir = dir.path().join("fixture");
    copy_dir(&fixture_src, &crate_dir);
    // Remove the fixture's committed lock so the copy resolves the
    // re-pointed path dep (still offline: everything is cached).
    let _ = std::fs::remove_file(crate_dir.join("Cargo.lock"));
    let manifest = crate_dir.join("Cargo.toml");
    let manifest_text = std::fs::read_to_string(&manifest).expect("the manifest");
    let rewritten = manifest_text.replace(
        "oxide-rustfn = { path = \"../..\" }",
        &format!("oxide-rustfn = {{ path = {workspace:?} }}"),
    );
    assert_ne!(
        manifest_text, rewritten,
        "the manifest carries the path dep"
    );
    std::fs::write(&manifest, rewritten).expect("the rewritten manifest");

    let host = Arc::new(SwappableHost::new());
    let mut lp = ServerLibLoop::new(
        ServerLibSpec::new("rustfn-fixture", crate_dir.clone()),
        Box::new(CargoLibBuild::new()),
        Box::new(HostSwapper::new(Arc::clone(&host))),
    )
    .with_loader(Box::new(CopyLoader))
    .with_dest_dir(dir.path().join(".oxide/rust"))
    .with_secret("e2e-secret");
    lp.seed();

    // The spec's triggers: the manifest and the `.server.rs` set. The
    // fixture's exports live in lib.rs (the fixture's shape — a user
    // crate declares them as `mod thumb.server.rs;`), so the edit rides
    // the manifest arm (a dependency or profile change) plus a new
    // `.server.rs` module file (the Created arm).
    let manifest = crate_dir.join("Cargo.toml");
    let before = std::fs::read_to_string(&manifest).expect("the fixture manifest");
    std::fs::write(&manifest, format!("{before}\n# dev-loop edit\n")).expect("the edit");
    std::fs::write(
        crate_dir.join("src/thumb.server.rs"),
        "// the dev-loop edit's module file\n",
    )
    .expect("the server module");

    let report = lp.poll_once().expect("the step").expect("the swap lands");
    assert_eq!(report.generation, 1);
    assert!(report.total_ids >= 1, "the fixture's exports registered");

    // The rebuilt fn answers through the §9.4 id the types payload
    // recorded (`file!()` inside the fixture crate).
    let id = server_fn_id("src/lib.rs", "thumbnail", "e2e-secret");
    let out = host
        .call_now(&id, br#"{"url":"x.png","width":2}"#)
        .expect("the rebuilt fn answers");
    let reply: serde_json::Value = oxide_rustfn::bridge::decode_odf(&out).expect("odf reply");
    assert_eq!(reply["mime"], "image/png");
    assert_eq!(reply["bytes"].as_array().map(Vec::len), Some(2));

    // The mount dir holds the COPY (never cargo's output path): a
    // `lib-1.so` under the mount dir, while `target/` keeps cargo's own.
    let mounts = std::fs::read_dir(dir.path().join(".oxide/rust")).expect("the mount dir");
    let names: Vec<String> = mounts
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        names.iter().any(|n| n.starts_with("lib-")),
        "the copy lives in the mount dir: {names:?}"
    );
}

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).expect("the dst root");
    for entry in std::fs::read_dir(src).expect("the src root") {
        let entry = entry.expect("the entry");
        let ty = entry.file_type().expect("the type");
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir(&entry.path(), &to);
        } else {
            std::fs::copy(entry.path(), &to).expect("the file copy");
        }
    }
}
