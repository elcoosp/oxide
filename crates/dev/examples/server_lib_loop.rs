//! The server-lib dev-loop walkthrough (the §V2-F08 dev-loop half,
//! `feat/server-lib-dev-loop`): the watch → build → load → swap loop that
//! makes "edit a `*.server.rs` module, the running dev server's native
//! functions change" a substrate contract — the rebuilt cdylib loads
//! through the host's copy semantics, the §9.4 ids re-derive from the
//! types side export, the swap lands without waiting on in-flight calls,
//! and the orphaned mount retires once the last call drops it.
//!
//! The walkthrough runs the loop against the injectable seams (a fake
//! build pass serving alternating generations, a fake loader mirroring the
//! host's copy semantics): the two `#[ignore]` pins in
//! `crates/dev/tests/server_lib_tdd.rs` prove the same story with REAL
//! cargo builds. The CLI adoption wires the loop into `cmd_dev`'s task set
//! on a V8-capable host — see `docs/deviations.md`.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-dev --example server_lib_loop
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use oxide_fw_dev::Watcher;
use oxide_fw_dev::server_lib::{
    BuildOutput, HostSwapper, LibBuild, LibLoad, ServerLibLoop, ServerLibSpec,
};
use oxide_fw_diagnostics::OxideError;
use oxide_rustfn::abi::{ABI_VERSION, FnEntry, FnTable, RawBuf, leak_name};
use oxide_rustfn::host::RustFnLibrary;
use oxide_rustfn::swap::SwappableHost;
use oxide_rustfn::types::{FnTypeEntry, types_json};

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
        file: "src/counter.server.rs".to_string(),
        args: vec!["string".to_string()],
        ret: "string".to_string(),
        decls: Vec::new(),
    }]);
    Box::leak(payload.into_boxed_str())
}

/// The build seam's fake: generation N's table (the walkthrough's stand-in
/// for `cargo build --lib` — the real pass is `CargoLibBuild`, proven by
/// the ignored e2e pins).
struct FakeBuild {
    generation: Arc<AtomicU64>,
}

impl LibBuild for FakeBuild {
    fn build(&self, _spec: &ServerLibSpec) -> Result<BuildOutput, OxideError> {
        let n = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let name: &'static str = Box::leak(format!("echo_g{n}").into_boxed_str());
        Ok(BuildOutput {
            cdylib_path: PathBuf::from(format!(
                "fake://target/debug/lib{name}.so (generation {n})"
            )),
        })
    }
}

/// The load seam's fake: the generation's library with the fixture types
/// payload (mirroring `CopyLoader`'s copy-then-load contract).
struct FakeLoad {
    generation: Arc<AtomicU64>,
}

impl LibLoad for FakeLoad {
    fn load(&self, _cdylib: &Path, _dest_dir: &Path) -> Result<RustFnLibrary, OxideError> {
        let n = self.generation.load(Ordering::SeqCst);
        let call = if n % 2 == 1 { echo_v1 } else { echo_v2 };
        let entries: &'static [FnEntry] = Box::leak(
            vec![FnEntry {
                name: leak_name("echo"),
                call,
            }]
            .into_boxed_slice(),
        );
        Ok(
            RustFnLibrary::from_static(table(entries), Some(echo_types()))
                .expect("a current-ABI table loads"),
        )
    }
}

fn main() {
    println!("— the F08 server-lib dev loop: watch → build → load → swap —\n");

    // 1. The fixture app: a native server-fn crate beside the app (the
    //    layout `oxide add rust` scaffolds; the CLI adoption points the
    //    loop at it and passes the app's build secret).
    let app = tempfile::tempdir().expect("tempdir");
    let root = app.path().to_path_buf();
    let native = root.join("native");
    std::fs::create_dir_all(native.join("src")).expect("native crate");
    std::fs::write(
        native.join("Cargo.toml"),
        "[package]\nname = \"native-fns\"\nversion = \"0.1.0\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n",
    )
    .expect("manifest");
    std::fs::write(native.join("src/lib.rs"), "// the native fns crate\n").expect("lib");
    std::fs::write(
        native.join("src/counter.server.rs"),
        "// the watched server fn module\n",
    )
    .expect("server module");

    // 2. The swappable host (one per native crate — the consult adapter
    //    holds the same Arc and answers the §9.4 route through it).
    let host = Arc::new(SwappableHost::new());
    let generation = Arc::new(AtomicU64::new(0));

    // 3. The loop: watcher rooted at the crate dir, the fake build/load
    //    passes, the host as the hand-off, the mount dir under the app.
    let mut lp = ServerLibLoop::new(
        ServerLibSpec::new("native-fns", native.clone()),
        Box::new(FakeBuild {
            generation: Arc::clone(&generation),
        }),
        Box::new(HostSwapper::new(Arc::clone(&host))),
    )
    .with_loader(Box::new(FakeLoad {
        generation: Arc::clone(&generation),
    }))
    .with_dest_dir(root.join(".oxide/rust"))
    .with_secret("walkthrough-secret");
    lp.seed();
    println!(
        "watch set: {{**/*.server.rs, Cargo.toml}} under {}",
        native.display()
    );
    println!("mount dir: {}", root.join(".oxide/rust").display());

    // 4. First rebuild: the edit lands, the loop builds, loads and swaps.
    let id = oxide_fw_compiler::server_fn_id("src/counter.server.rs", "echo", "walkthrough-secret");
    println!("\n[edit 1] src/counter.server.rs changes");
    std::fs::write(native.join("src/counter.server.rs"), "// edited once\n").expect("edit 1");
    let report = lp
        .poll_once()
        .expect("the first step")
        .expect("the swap lands");
    println!(
        "swap #{}: rebound={:?} added={:?} dropped={:?}",
        report.generation, report.rebound, report.added, report.dropped
    );
    let args = br#""hello""#;
    let out = host.call_now(&id, args).expect("the native fn answers");
    let reply: String = oxide_rustfn::bridge::decode_odf(&out).expect("odf reply");
    println!("call {id}: {reply}");

    // 5. Second rebuild: the id re-points, the NEW behavior serves, and
    //    the old mount retires only once nothing holds it (an in-flight
    //    call would have kept it alive until it finished).
    println!("\n[edit 2] src/counter.server.rs changes again");
    std::fs::write(native.join("src/counter.server.rs"), "// edited twice\n").expect("edit 2");
    let report = lp
        .poll_once()
        .expect("the second step")
        .expect("the swap lands");
    println!(
        "swap #{}: rebound={:?} (the previous mount retires when idle)",
        report.generation, report.rebound
    );
    let out = host.call_now(&id, args).expect("the rebuilt fn answers");
    let reply: String = oxide_rustfn::bridge::decode_odf(&out).expect("odf reply");
    println!("call {id}: {reply}  <- the new build serves");
    let retire = host.retire_idle();
    println!(
        "retire: {} orphaned mount(s) reclaimed, {} kept",
        retire.retired, retire.kept
    );

    // 6. Quiet tick: no relevant change, no rebuild — the loop's per-tick
    //    cost is a walk over the crate tree, not a build.
    std::fs::write(native.join("src/notes.md"), "scratch\n").expect("the distractor");
    match lp.poll_once().expect("the quiet tick") {
        None => println!("\n[quiet tick] src/notes.md changed: no rebuild (not in the watch set)"),
        Some(_) => println!("\n[quiet tick] unexpected rebuild"),
    }

    println!("\nThe CLI adoption wires this loop into `cmd_dev` (the build");
    println!("secret and the mount dir come from the app config; the consult");
    println!("adapter holds the same Arc<SwappableHost>).");
    let _ = Watcher::new(vec![]); // the loop owns its watcher; keep the import for the story
}
