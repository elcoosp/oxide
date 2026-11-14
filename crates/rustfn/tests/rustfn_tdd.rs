//! TDD pins for the §V2-F08 native server functions HOST half — the
//! `feat/rustfn-host-seam` substrate (the ABI, the ODF bridge, the
//! panic-safe shim, the loader + id-mapped registry and the `.d.ts`
//! renderer; the `#[server_fn]`/`TsType`/`export_table!` macros are
//! exercised end-to-end by the `#[ignore]` fixture pins at the bottom,
//! which build `tests/fixture` with cargo at test time).
//!
//! Written BEFORE the implementation; every pin imports public API only.
//!
//! The contracts pinned here (spec §V2-F08 verbatim where marked):
//!
//! - The C ABI is the spec-verbatim `RawBuf`/`FnEntry`/`FnTable` shape
//!   with `ABI_VERSION = 1`; the shim statuses are `0` (ok), `1`
//!   (error) and `2` (panic) — panics never unwind across the boundary.
//! - The library boundary carries ODF bytes (decision D33): a typed
//!   round trip through `encode_odf`/`decode_odf` starts with the ODF1
//!   magic, and a payload that does not decode is a coded `RFN-004`.
//! - A `#[server_fn]` call maps Ok → status 0 with the encoded value,
//!   `Err` → status 1 with the encoded message, a panic → status 2
//!   with no payload.
//! - `RustFnLibrary::load` copies the cdylib to `lib-<n>.<ext>` under
//!   the destination (never loads the file cargo wrote — Windows
//!   locks it), refuses a foreign `abi_version` with `RFN-001` naming
//!   saw-and-expected, refuses an unloadable file with `RFN-003`, and
//!   refuses duplicate entry names with `RFN-003`.
//! - The in-process table seam (`from_static`) drives the same gates —
//!   the embedded-host and test path never touches `libloading`.
//! - The host registry is keyed by the §9.4 id; `call` runs on
//!   `spawn_blocking` under a bounded semaphore with a per-function
//!   timeout (default 30 s): ok → the encoded ODF bytes, fn error →
//!   400 `FN-THREW`, panic → 500 `FN-INTERNAL`, garbage args JSON →
//!   400 `FN-BAD-REQUEST`, timeout → 504 `FN-TIMEOUT`, unknown id →
//!   404 `FN-NOT-FOUND`.
//! - Hot-swap: re-registering an id under a newly mounted library
//!   serves the new behavior on the next call.
//! - The optional `oxide_fn_types_json` side export rides the library
//!   when present and renders the `.d.ts` (`export declare function
//!   name(input): Out;` plus the interface blocks, sorted) — absent,
//!   `types_json()` is `None`.

use std::time::Duration;

use oxide_rustfn::TsType;
use oxide_rustfn::abi::{
    ABI_VERSION, FnEntry, FnTable, RawBuf, STATUS_ERROR, STATUS_OK, STATUS_PANIC,
};
use oxide_rustfn::bridge::{decode_error_message, decode_odf, encode_error_message, encode_odf};
use oxide_rustfn::host::{
    RUSTFN_TIMEOUT_SECS, RegistryErrorKind, RustFnHost, RustFnLibrary, RustFnRegistry,
};
use oxide_rustfn::shim::run_shim;
use oxide_rustfn::types::{render_types_dts, write_types};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// abi: the spec-verbatim shapes
// ---------------------------------------------------------------------------

#[test]
fn abi_version_and_statuses_are_the_spec_values() {
    assert_eq!(ABI_VERSION, 1);
    assert_eq!(STATUS_OK, 0);
    assert_eq!(STATUS_ERROR, 1);
    assert_eq!(STATUS_PANIC, 2);
}

/// The collector the release pin reads (a static: `release_via` takes
/// the table's `extern "C" fn(RawBuf)`, not a closure).
static RELEASED: std::sync::Mutex<Vec<u8>> = std::sync::Mutex::new(Vec::new());

extern "C" fn test_free(b: RawBuf) {
    if b.ptr.is_null() || b.cap == 0 {
        return;
    }
    // SAFETY: this test allocated with `RawBuf::alloc` (the global
    // allocator), so the reconstruction is sound.
    let v = unsafe { Vec::from_raw_parts(b.ptr, b.len, b.cap) };
    RELEASED
        .lock()
        .expect("the release collector lock")
        .extend_from_slice(&v);
}

#[test]
fn raw_buf_alloc_carries_the_exact_bytes() {
    let buf = RawBuf::alloc(&[1, 2, 3]);
    assert_eq!(buf.len, 3);
    assert_eq!(buf.cap, 3);
    // SAFETY: the buffer was just allocated from these exact bytes.
    assert_eq!(unsafe { buf.as_slice() }, &[1, 2, 3]);
    // SAFETY: the buffer is released exactly once, to its allocator.
    unsafe {
        buf.release_via(test_free);
    }
    assert_eq!(
        &*RELEASED.lock().expect("the release collector lock"),
        &[1, 2, 3]
    );
}

#[test]
fn raw_buf_empty_is_a_valid_zero_length_buffer() {
    let buf = RawBuf::empty();
    assert_eq!(buf.len, 0);
    assert_eq!(buf.cap, 0);
    // SAFETY: a zero-length buffer has no dereferenceable bytes.
    assert_eq!(unsafe { buf.as_slice() }.len(), 0);
}

// ---------------------------------------------------------------------------
// bridge: ODF is the library wire (decision D33)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TsType)]
pub struct Point {
    pub x: f64,
    pub y: f64,
    pub label: String,
    pub tags: Vec<String>,
}

#[test]
fn typed_odf_round_trip_starts_with_the_odf_magic() {
    let bytes = encode_odf(&Point {
        x: 1.5,
        y: -2.0,
        label: "origin".into(),
        tags: vec!["a".into(), "b".into()],
    })
    .expect("a serializable value always encodes");
    assert_eq!(&bytes[..4], b"ODF1");
    let back: Point = decode_odf(&bytes).expect("the same crate decodes its own bytes");
    assert_eq!(back.label, "origin");
    assert_eq!(back.tags.len(), 2);
}

#[test]
fn garbage_payload_refuses_with_rfn_004() {
    let err = decode_odf::<Point>(b"not odf at all").expect_err("garbage is not ODF");
    assert_eq!(err.0.code, "RFN-004");
}

#[test]
fn error_message_round_trip_is_an_odf_string() {
    let bytes =
        encode_error_message("boom: the width must be positive").expect("a message encodes");
    let back = decode_error_message(&bytes).expect("the message decodes");
    assert_eq!(back, "boom: the width must be positive");
}

// ---------------------------------------------------------------------------
// shim: Ok / Err / panic over the C ABI
// ---------------------------------------------------------------------------

fn take_out(out: &RawBuf) -> Vec<u8> {
    // SAFETY: the shim wrote exactly its own allocation into `out`.
    unsafe { out.as_slice() }.to_vec()
}

#[test]
fn shim_ok_maps_to_status_zero_and_the_encoded_value() {
    let mut out = RawBuf::empty();
    let status = run_shim(b"irrelevant", &mut out, |input| {
        assert_eq!(input, b"irrelevant");
        Ok(b"encoded-out".to_vec())
    });
    assert_eq!(status, STATUS_OK);
    assert_eq!(take_out(&out), b"encoded-out");
}

#[test]
fn shim_err_maps_to_status_one_and_an_encoded_message() {
    let mut out = RawBuf::empty();
    let status = run_shim(b"", &mut out, |_| Err("width must be positive".to_string()));
    assert_eq!(status, STATUS_ERROR);
    let msg = decode_error_message(&take_out(&out)).expect("the error payload is an ODF string");
    assert_eq!(msg, "width must be positive");
}

#[test]
fn shim_panic_maps_to_status_two_with_no_payload() {
    let mut out = RawBuf::empty();
    let status = run_shim(b"", &mut out, |_| -> Result<Vec<u8>, String> {
        panic!("the user body exploded");
    });
    assert_eq!(status, STATUS_PANIC);
    assert_eq!(take_out(&out).len(), 0);
}

// ---------------------------------------------------------------------------
// host: the in-process table seam drives the loader's gates
// ---------------------------------------------------------------------------

/// The fixture table's function shape: the generated shim signature.
extern "C" fn echo_shim(inp: *const u8, len: usize, out: *mut RawBuf) -> i32 {
    let input: &[u8] = if inp.is_null() || len == 0 {
        &[]
    } else {
        // SAFETY: the host passes a live `len`-byte buffer.
        unsafe { std::slice::from_raw_parts(inp, len) }
    };
    let mut buf = RawBuf::empty();
    let status = run_shim(input, &mut buf, |bytes| {
        // The wire into a shim is ODF: the host converted the args JSON
        // before the call (the bridge's job), so the shim decodes it.
        let text: String = match oxide_rustfn::bridge::decode_odf(bytes) {
            Ok(text) => text,
            Err(e) => return Err(e.to_string()),
        };
        Ok(format!("echo:{text}").into_bytes())
    });
    // SAFETY: the host always passes a writable slot.
    unsafe {
        if !out.is_null() {
            *out = buf;
        }
    }
    status
}

extern "C" fn fail_shim(_inp: *const u8, _len: usize, out: *mut RawBuf) -> i32 {
    let mut buf = RawBuf::empty();
    let status = run_shim(b"", &mut buf, |_| Err("nope".to_string()));
    // SAFETY: the host always passes a writable slot.
    unsafe {
        if !out.is_null() {
            *out = buf;
        }
    }
    status
}

extern "C" fn panic_shim(_inp: *const u8, _len: usize, out: *mut RawBuf) -> i32 {
    let mut buf = RawBuf::empty();
    let status = run_shim(b"", &mut buf, |_| -> Result<Vec<u8>, String> {
        panic!("kaboom")
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
    // SAFETY: every buffer this table hands out came from `RawBuf::alloc`.
    drop(unsafe { Vec::from_raw_parts(buf.ptr, buf.len, buf.cap) });
}

fn table(entries: &'static [FnEntry]) -> &'static FnTable {
    // Leaked per call: the tables are test fixtures and the host keeps
    // the pointer for the library's lifetime.
    Box::leak(Box::new(FnTable {
        abi_version: ABI_VERSION,
        len: entries.len(),
        entries: entries.as_ptr(),
        free: free_impl,
    }))
}

fn entry_named(
    name: &'static str,
    call: extern "C" fn(*const u8, usize, *mut RawBuf) -> i32,
) -> FnEntry {
    FnEntry {
        name: oxide_rustfn::abi::leak_name(name),
        call,
    }
}

#[test]
fn from_static_indexes_the_entries_by_name() {
    let entries: &'static [FnEntry] = Box::leak(
        vec![
            entry_named("echo", echo_shim),
            entry_named("boom", panic_shim),
        ]
        .into_boxed_slice(),
    );
    let lib = RustFnLibrary::from_static(table(entries), None).expect("a current-ABI table loads");
    assert_eq!(lib.names(), vec!["boom".to_string(), "echo".to_string()]);
    assert_eq!(lib.abi_version(), ABI_VERSION);
    assert!(lib.types_json().is_none());
}

#[test]
fn from_static_refuses_a_foreign_abi_with_rfn_001_naming_saw_and_expected() {
    let entries: &'static [FnEntry] =
        Box::leak(vec![entry_named("echo", echo_shim)].into_boxed_slice());
    let foreign = FnTable {
        abi_version: 99,
        len: entries.len(),
        entries: entries.as_ptr(),
        free: free_impl,
    };
    let leaked: &'static FnTable = Box::leak(Box::new(foreign));
    let err = RustFnLibrary::from_static(leaked, None).expect_err("a foreign ABI refuses");
    assert_eq!(err.0.code, "RFN-001");
    assert!(err.0.message.contains("99"), "names the saw version");
    assert!(
        err.0.message.contains(&ABI_VERSION.to_string()),
        "names the expected version"
    );
}

#[test]
fn duplicate_entry_names_refuse_with_rfn_003() {
    let entries: &'static [FnEntry] = Box::leak(
        vec![
            entry_named("echo", echo_shim),
            entry_named("echo", echo_shim),
        ]
        .into_boxed_slice(),
    );
    let err =
        RustFnLibrary::from_static(table(entries), None).expect_err("a duplicate name refuses");
    assert_eq!(err.0.code, "RFN-003");
}

#[test]
fn call_raw_maps_the_three_statuses() {
    let entries: &'static [FnEntry] = Box::leak(
        vec![
            entry_named("echo", echo_shim),
            entry_named("fail", fail_shim),
            entry_named("boom", panic_shim),
        ]
        .into_boxed_slice(),
    );
    let lib = RustFnLibrary::from_static(table(entries), None).expect("loads");

    let out = lib
        .call_raw("echo", &encode_odf("hi").expect("the input encodes"))
        .expect("ok status decodes");
    assert_eq!(out, b"echo:hi");

    let err = lib
        .call_raw("fail", b"")
        .expect_err("status 1 is a fn error");
    assert!(matches!(err, oxide_rustfn::host::RawCallError::FnError(ref m) if m == "nope"));

    let err = lib.call_raw("boom", b"").expect_err("status 2 is a panic");
    assert!(matches!(err, oxide_rustfn::host::RawCallError::Panic));

    let err = lib
        .call_raw("missing", b"")
        .expect_err("unknown names refuse");
    assert!(matches!(err, oxide_rustfn::host::RawCallError::NotFound(_)));
}

#[test]
fn load_refuses_a_missing_file_with_rfn_003() {
    let dir = tempfile::tempdir().expect("a tempdir");
    let err = RustFnLibrary::load(&dir.path().join("absent.so"), dir.path())
        .expect_err("a missing file refuses");
    assert_eq!(err.0.code, "RFN-003");
}

#[test]
fn load_refuses_a_non_library_file_with_rfn_003() {
    let dir = tempfile::tempdir().expect("a tempdir");
    let path = dir.path().join("text.so");
    std::fs::write(&path, b"definitely not an elf or pe image").expect("writes");
    let err = RustFnLibrary::load(&path, dir.path()).expect_err("garbage refuses");
    assert_eq!(err.0.code, "RFN-003");
}

// ---------------------------------------------------------------------------
// host: the id-mapped registry (the §9.4 id is the HTTP contract)
// ---------------------------------------------------------------------------

fn host_with_echo() -> (RustFnHost, usize) {
    let entries: &'static [FnEntry] =
        Box::leak(vec![entry_named("echo", echo_shim)].into_boxed_slice());
    let mut host = RustFnHost::new();
    let lib = RustFnLibrary::from_static(table(entries), None).expect("loads");
    let index = host.mount(lib);
    host.register("id-echo", index, "echo");
    (host, index)
}

#[test]
fn host_registry_keys_by_id_and_reports_its_ids() {
    let (host, _) = host_with_echo();
    assert!(host.has("id-echo"));
    assert!(!host.has("id-other"));
    assert_eq!(host.ids(), vec!["id-echo".to_string()]);
}

#[tokio::test]
async fn registry_call_runs_the_native_fn_with_decoded_args() {
    let (host, _) = host_with_echo();
    let out = host.call("id-echo", br#""hi""#).await.expect("ok");
    // The echo shim received the ODF-decoded bytes: the args JSON went
    // in as ODF, the shim saw `hi`, and the reply carries the prefix.
    assert_eq!(String::from_utf8(out).expect("utf8"), "echo:hi");
}

#[tokio::test]
async fn registry_call_unknown_id_maps_to_404_fn_not_found() {
    let (host, _) = host_with_echo();
    let err = host.call("id-other", b"{}").await.expect_err("unknown id");
    assert_eq!(err.kind, RegistryErrorKind::NotFound);
    assert_eq!(err.status(), 404);
    assert_eq!(err.envelope_code(), "FN-NOT-FOUND");
}

#[tokio::test]
async fn registry_call_fn_error_maps_to_400_fn_threw() {
    let entries: &'static [FnEntry] =
        Box::leak(vec![entry_named("fail", fail_shim)].into_boxed_slice());
    let mut host = RustFnHost::new();
    let index = host.mount(RustFnLibrary::from_static(table(entries), None).expect("loads"));
    host.register("id-fail", index, "fail");
    let err = host.call("id-fail", b"{}").await.expect_err("fn error");
    assert_eq!(err.kind, RegistryErrorKind::FnError);
    assert_eq!(err.status(), 400);
    assert_eq!(err.envelope_code(), "FN-THREW");
    assert_eq!(err.message, "nope");
}

#[tokio::test]
async fn registry_call_panic_maps_to_500_fn_internal() {
    let entries: &'static [FnEntry] =
        Box::leak(vec![entry_named("boom", panic_shim)].into_boxed_slice());
    let mut host = RustFnHost::new();
    let index = host.mount(RustFnLibrary::from_static(table(entries), None).expect("loads"));
    host.register("id-boom", index, "boom");
    let err = host.call("id-boom", b"{}").await.expect_err("panic");
    assert_eq!(err.kind, RegistryErrorKind::Panic);
    assert_eq!(err.status(), 500);
    assert_eq!(err.envelope_code(), "FN-INTERNAL");
}

#[tokio::test]
async fn registry_call_garbage_args_json_maps_to_400_fn_bad_request() {
    let (host, _) = host_with_echo();
    let err = host.call("id-echo", b"not json").await.expect_err("wire");
    assert_eq!(err.kind, RegistryErrorKind::Wire);
    assert_eq!(err.status(), 400);
    assert_eq!(err.envelope_code(), "FN-BAD-REQUEST");
}

#[tokio::test]
async fn registry_call_timeout_maps_to_504_fn_timeout() {
    extern "C" fn slow_shim(_inp: *const u8, _len: usize, _out: *mut RawBuf) -> i32 {
        std::thread::sleep(Duration::from_millis(250));
        STATUS_OK
    }
    let entries: &'static [FnEntry] =
        Box::leak(vec![entry_named("slow", slow_shim)].into_boxed_slice());
    let mut host = RustFnHost::new().with_timeout(Duration::from_millis(50));
    let index = host.mount(RustFnLibrary::from_static(table(entries), None).expect("loads"));
    host.register("id-slow", index, "slow");
    let err = host.call("id-slow", b"{}").await.expect_err("timeout");
    assert_eq!(err.kind, RegistryErrorKind::Timeout);
    assert_eq!(err.status(), 504);
    assert_eq!(err.envelope_code(), "FN-TIMEOUT");
}

#[test]
fn the_default_timeout_is_the_spec_thirty_seconds() {
    assert_eq!(RUSTFN_TIMEOUT_SECS, 30);
}

#[test]
fn hot_swap_re_registration_serves_the_new_behavior() {
    extern "C" fn echo_v2_shim(inp: *const u8, len: usize, out: *mut RawBuf) -> i32 {
        let input: &[u8] = if inp.is_null() || len == 0 {
            &[]
        } else {
            // SAFETY: the host passes a live `len`-byte buffer.
            unsafe { std::slice::from_raw_parts(inp, len) }
        };
        let mut buf = RawBuf::empty();
        let status = run_shim(input, &mut buf, |bytes| {
            // The v2 body decodes the same ODF wire (the contract v1
            // followed), only the reply prefix changes.
            let text: String = match oxide_rustfn::bridge::decode_odf(bytes) {
                Ok(text) => text,
                Err(e) => return Err(e.to_string()),
            };
            Ok(format!("v2:{text}").into_bytes())
        });
        // SAFETY: the host always passes a writable slot.
        unsafe {
            if !out.is_null() {
                *out = buf;
            }
        }
        status
    }
    let v1: &'static [FnEntry] = Box::leak(vec![entry_named("echo", echo_shim)].into_boxed_slice());
    let v2: &'static [FnEntry] =
        Box::leak(vec![entry_named("echo", echo_v2_shim)].into_boxed_slice());
    let mut host = RustFnHost::new();
    let first = host.mount(RustFnLibrary::from_static(table(v1), None).expect("loads"));
    host.register("id-echo", first, "echo");

    let second = host.mount(RustFnLibrary::from_static(table(v2), None).expect("loads"));
    host.register("id-echo", second, "echo");

    let out = host.call_now("id-echo", br#""hi""#).expect("hot-swapped");
    assert_eq!(out, b"v2:hi");
}

// ---------------------------------------------------------------------------
// types: the optional `oxide_fn_types_json` side export renders the .d.ts
// ---------------------------------------------------------------------------

#[test]
fn types_json_renders_the_declare_functions_and_the_interface_blocks() {
    let json = r#"{"fns":[{"name":"thumbnail","file":"src/thumb.server.rs","args":["ThumbIn"],"ret":"ThumbOut"}],"decls":{"ThumbIn":"export interface ThumbIn { url: string; width: number }","ThumbOut":"export interface ThumbOut { bytes: string; mime: string }"}}"#;
    let lib = RustFnLibrary::from_static(table(&[]), Some(json)).expect("loads");
    let rendered = render_types_dts(lib.types_json().expect("the types ride the library"))
        .expect("a valid payload renders");
    assert!(
        rendered.contains("export declare function thumbnail(input: ThumbIn): ThumbOut;"),
        "the fn declaration, got: {rendered}"
    );
    assert!(rendered.contains("export interface ThumbIn { url: string; width: number }"));
    assert!(rendered.contains("export interface ThumbOut"));
    // Deterministic output: the interface blocks are sorted by name.
    let tin = rendered.find("ThumbIn {").expect("In block");
    let tout = rendered.find("ThumbOut {").expect("Out block");
    assert!(tin < tout, "decls sorted by type name");
}

#[test]
fn write_types_persists_the_rendered_d_ts() {
    let json = r#"{"fns":[{"name":"health","file":"src/health.server.rs","args":[],"ret":"Health"}],"decls":{"Health":"export interface Health { ok: boolean }"}}"#;
    let dir = tempfile::tempdir().expect("a tempdir");
    let path = dir.path().join("rust-fns.d.ts");
    write_types(json, &path).expect("writes");
    let content = std::fs::read_to_string(&path).expect("reads back");
    assert!(content.contains("export declare function health(): Health;"));
    assert!(content.contains("export interface Health { ok: boolean }"));
}

// ---------------------------------------------------------------------------
// fixture: the real cdylib end-to-end (built with cargo at test time)
// ---------------------------------------------------------------------------

/// The fixture crate (`tests/fixture`, its own workspace root) builds
/// with `cargo build` and loads through the real `libloading` path:
/// the `#[server_fn]` shims, the `export_table!` static and the types
/// export all come from the macros, not from a hand-built table.
#[test]
#[ignore = "builds the fixture cdylib with cargo (the quick profile skips it; run with --ignored)"]
fn fixture_library_loads_calls_errors_and_panics_end_to_end() {
    let manifest =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture/Cargo.toml");
    let status = std::process::Command::new("cargo")
        .args([
            "build",
            "--manifest-path",
            manifest.to_string_lossy().as_ref(),
            "--target-dir",
            "../../target/fixture",
        ])
        .status()
        .expect("cargo runs");
    assert!(status.success(), "the fixture crate builds");

    // The artifact: target/fixture/<debug>/librustfn_fixture.<ext>.
    let debug_dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/fixture/debug");
    let ext = if cfg!(target_os = "macos") {
        "dylib"
    } else if cfg!(target_os = "windows") {
        "dll"
    } else {
        "so"
    };
    let lib_path = debug_dir.join(format!("librustfn_fixture.{ext}"));
    assert!(
        lib_path.exists(),
        "the cdylib exists at {}",
        lib_path.display()
    );

    let dest = tempfile::tempdir().expect("a tempdir");
    let lib = RustFnLibrary::load(&lib_path, dest.path()).expect("the fixture loads");
    assert_eq!(
        lib.names(),
        vec!["boom".to_string(), "thumbnail".to_string()]
    );

    // Ok: the ODF-encoded args decode into the typed input, the result
    // encodes back.
    let out = lib
        .call_raw(
            "thumbnail",
            &encode_odf(&FixtureIn {
                url: "a.png".into(),
                width: 8,
            })
            .expect("encodes"),
        )
        .expect("ok");
    let thumb: FixtureOut = decode_odf(&out).expect("decodes");
    assert_eq!(thumb.mime, "image/png");
    assert!(thumb.bytes.len() == 8, "the fixture echoes `width` bytes");

    // Err: the ServerError message survives the boundary as status 1.
    let err = lib
        .call_raw(
            "boom",
            &encode_odf(&FixtureIn {
                url: String::new(),
                width: 0,
            })
            .expect("encodes"),
        )
        .expect_err("the fixture fn refuses");
    assert!(matches!(err, oxide_rustfn::host::RawCallError::FnError(ref m) if m.contains("boom")));

    // The types side export renders the .d.ts the compiler's stub
    // generator pairs with the client stubs.
    let types = lib.types_json().expect("the fixture exports its types");
    let rendered = render_types_dts(types).expect("renders");
    assert!(rendered.contains("export declare function thumbnail(input: FixtureIn): FixtureOut;"));
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TsType)]
pub struct FixtureIn {
    pub url: String,
    pub width: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TsType)]
pub struct FixtureOut {
    pub bytes: Vec<u8>,
    pub mime: String,
}

// The macro surface compiles inside a test crate too: the shim, the
// registration and the TsType impls all generate without a fixture
// build (the fixture pins the loaded-library behavior).
mod macro_surface {
    use super::{FixtureIn, FixtureOut, RawBuf, STATUS_OK, decode_odf, encode_odf, take_out};
    use oxide_rustfn::{ServerError, TsType, server_fn};

    /// A zero-cost body the macro wraps: the pin is that the generated
    /// shim compiles and the registry-side names resolve.
    #[server_fn]
    pub fn macro_checked(input: FixtureIn) -> Result<FixtureOut, ServerError> {
        if input.width == 0 {
            return Err(ServerError {
                message: format!("width must be positive (url: {})", input.url),
            });
        }
        // The body CONSUMES the input (the shim decodes into the call),
        // and the Err arm is real, so the macro's `Result` contract is
        // exercised rather than wrapped for ceremony.
        Ok(FixtureOut {
            bytes: vec![0; input.width as usize],
            mime: input.url,
        })
    }

    #[test]
    fn generated_shim_and_registration_resolve() {
        let shim =
            __oxide_shim_macro_checked as extern "C" fn(*const u8, usize, *mut RawBuf) -> i32;
        let mut out = RawBuf::empty();
        let args = encode_odf(&FixtureIn {
            url: String::new(),
            width: 3,
        })
        .expect("encodes");
        let status = shim(args.as_ptr(), args.len(), std::ptr::from_mut(&mut out));
        assert_eq!(status, STATUS_OK);
        let decoded: FixtureOut = decode_odf(&take_out(&out)).expect("the shim encoded the result");
        assert_eq!(decoded.bytes.len(), 3);

        let entry = __oxide_entry_macro_checked();
        assert_eq!(
            // SAFETY: the entry name is a leaked CString the macro made.
            unsafe { entry.name_str() },
            "macro_checked"
        );
        let types = __oxide_types_macro_checked();
        assert_eq!(types.name, "macro_checked");
        assert_eq!(types.args, vec!["FixtureIn"]);
        assert_eq!(types.ret, "FixtureOut");
        // `file!()` names the source as rustc saw it (package-relative
        // or workspace-relative depending on the invocation); the pin
        // is that the §9.4 id triple's file half is present.
        assert!(
            types.file.ends_with("rustfn_tdd.rs"),
            "file!() names the source: {}",
            types.file
        );
        // FixtureIn's own decls carry its interface (nested types ride
        // along); FixtureOut's block rides the FnTypeEntry, which
        // unions the input's and the output's contributions.
        let decls = <FixtureIn as TsType>::ts_decls();
        assert!(decls.iter().any(|d| d.1.contains("interface FixtureIn")));
        assert!(
            types
                .decls
                .iter()
                .any(|(n, b)| n == "FixtureOut" && b.contains("interface FixtureOut"))
        );
        let json = oxide_rustfn::types::types_json(std::slice::from_ref(&types));
        let rendered = oxide_rustfn::types::render_types_dts(&json).expect("the record renders");
        assert!(
            rendered
                .contains("export declare function macro_checked(input: FixtureIn): FixtureOut;")
        );
    }
}
