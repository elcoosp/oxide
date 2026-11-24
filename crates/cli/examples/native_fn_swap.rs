//! The §V2-F08 finish's CLI walkthrough: the consult adapter holds the
//! LIVE host, the dev loop swaps under it, and the same source object
//! answers the new generation — plus the bundle-time stub hooks and the
//! boot-mount shape.
//!
//! The walkthrough runs the injectable seams (a fake build pass serving a
//! second generation, a fake loader mirroring the host's copy semantics);
//! the `native_finish_tdd` pins prove the same story, and the dev crate's
//! `#[ignore]` real-cargo pins prove the cargo halves.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-cli --no-default-features --example native_fn_swap
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use oxide_fw_cli::native_fns::{
    DevNativeHandle, dev_native_consult_with, discover_native_crate, server_rs_load_hook,
    swappable_native_fn_source,
};
use oxide_fw_compiler::{Env as CompilerEnv, server_fn_id};
use oxide_fw_dev::server_lib::{BuildOutput, LibBuild, LibLoad};
use oxide_fw_diagnostics::OxideError;
use oxide_rustfn::abi::{FnEntry, FnTable, RawBuf, leak_name};
use oxide_rustfn::host::RustFnLibrary;
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

struct FakeBuild {
    generation: Arc<AtomicU64>,
}

impl LibBuild for FakeBuild {
    fn build(
        &self,
        _spec: &oxide_fw_dev::server_lib::ServerLibSpec,
    ) -> Result<BuildOutput, OxideError> {
        let n = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(BuildOutput {
            cdylib_path: PathBuf::from(format!("fake://target/debug/echo_g{n}.so")),
        })
    }
}

struct FakeLoad {
    generation: Arc<AtomicU64>,
}

impl LibLoad for FakeLoad {
    fn load(&self, cdylib: &Path, dest_dir: &Path) -> Result<RustFnLibrary, OxideError> {
        let n = self.generation.load(Ordering::SeqCst);
        std::fs::create_dir_all(dest_dir).expect("mount dir");
        std::fs::write(
            dest_dir.join(format!("lib-{n}.so")),
            cdylib.to_string_lossy().as_bytes(),
        )
        .expect("mount copy");
        let call = if n % 2 == 1 { echo_v1 } else { echo_v2 };
        Ok(
            RustFnLibrary::from_static(table(echo_entries(call)), Some(echo_types()))
                .expect("a current-ABI table loads"),
        )
    }
}

fn main() {
    println!("— the F08 finish: the consult holds the live host, the dev loop swaps under it —\n");

    // 1. The fixture app: a native server-fn crate beside the app (the
    //    layout discovery scans: the app root, then one level beside it).
    let app = tempfile::tempdir().expect("tempdir");
    let root = app.path().to_path_buf();
    let native = root.join("native");
    std::fs::create_dir_all(native.join("src")).expect("crate dirs");
    std::fs::write(
        native.join("Cargo.toml"),
        "[package]\nname = \"native-fns\"\nversion = \"0.1.0\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n",
    )
    .expect("manifest");
    std::fs::write(native.join("src/echo.server.rs"), "// the watched module\n").expect("module");
    let nc = discover_native_crate(&root).expect("the crate is discovered");
    println!("1. discovered crate `{}` at {}", nc.name, nc.dir.display());

    // 2. The dev wiring: the loop task swaps under the swappable host; the
    //    consult handle it returns is what `cmd_dev` threads into
    //    `start_dev_server_full` (and so into the dispatcher).
    let generation = Arc::new(AtomicU64::new(0));
    let handle: DevNativeHandle = dev_native_consult_with(
        &root,
        "walkthrough-secret",
        Box::new(FakeBuild {
            generation: Arc::clone(&generation),
        }),
        Box::new(FakeLoad {
            generation: Arc::clone(&generation),
        }),
        Duration::from_millis(20),
    )
    .expect("the wiring starts");
    std::thread::sleep(Duration::from_millis(80));
    println!("2. dev consult live (the boot poll reported no change)");

    // 3. The consult answers generation 1 through the held source.
    let id = server_fn_id("src/echo.server.rs", "echo", "walkthrough-secret");
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let out = rt
        .block_on(handle.source.call(&id, br#""hello""#))
        .expect("the consult answers");
    let text: String = oxide_rustfn::bridge::decode_odf(&out).expect("decode");
    println!("3. consult: echo(\"hello\") = {text:?}");

    // 4. The dev loop's swap: an edit fires watch → build → load → swap;
    //    the SAME source object answers the new generation.
    std::fs::write(native.join("src/echo.server.rs"), "// an edit\n").expect("edit");
    for _ in 0..50 {
        if generation.load(Ordering::SeqCst) >= 1 {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let out = rt
        .block_on(handle.source.call(&id, br#""hello""#))
        .expect("the consult answers after the swap");
    let text: String = oxide_rustfn::bridge::decode_odf(&out).expect("decode");
    println!("4. after the swap: echo(\"hello\") = {text:?} — no restart, no rebuilt consult");
    handle.shutdown();

    // 5. The bundle-time stub: the load arm's bytes for the module id (the
    //    payload the emission phase writes feeds the exports).
    std::fs::create_dir_all(root.join(".oxide/rust")).expect("mount dir");
    std::fs::write(root.join(".oxide/rust/types.json"), echo_types()).expect("payload");
    let load = server_rs_load_hook(&root, &nc, "walkthrough-secret", CompilerEnv::Client)
        .expect("the hook");
    let stub = load
        .call(&native.join("src/echo.server.rs").to_string_lossy())
        .expect("the hook runs")
        .expect("a .server.rs id loads the stub");
    println!(
        "5. bundle-time stub for src/echo.server.rs:\n{}\n",
        stub.trim()
    );

    // 6. The boot-mount shape over a swapped host (what a serve consult
    //    holds after `oxide build`'s emission copied the library).
    let host = Arc::new(oxide_rustfn::swap::SwappableHost::new());
    let lib = RustFnLibrary::from_static(table(echo_entries(echo_v1)), Some(echo_types()))
        .expect("the fixture table loads");
    host.swap(
        lib,
        &[oxide_rustfn::swap::FnBinding::new(id.clone(), "echo")],
    )
    .expect("the mount");
    let boot_source = swappable_native_fn_source(host).expect("the host serves");
    let out = rt
        .block_on(boot_source.call(&id, br#""boot""#))
        .expect("the boot consult answers");
    let text: String = oxide_rustfn::bridge::decode_odf(&out).expect("decode");
    println!("6. boot-mount consult: echo(\"boot\") = {text:?}");
}
