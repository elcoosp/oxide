//! The M1 multi-host consult walkthrough — `ChainedRegistry`, the
//! "consult each host in turn" seam the spec names (§V2-F08 "Host": "one
//! host per crate; the consult adapter consults each host in turn"):
//!
//! ```text
//! cargo run -p oxide-rustfn --example chain
//! ```
//!
//! The loop this example walks: two native crates' worth of server fns
//! (two `SwappableHost`s, each owning ONE crate's §9.4 id set) → chain
//! them through `ChainedRegistry` → upcast to `Arc<dyn RustFnRegistry>`
//! (the consult surface the dispatcher already wires for a single host)
//! → consult each id through the chain (disjoint ids, each member
//! answers) → observe the deterministic first-has-wins on a cross-crate
//! id collision → render the MERGED `.d.ts` (every crate's types behind
//! one consult) → snapshot the chain's posture through `ChainedReport`.
//!
//! The contract this example lands: `Arc<ChainedRegistry>` IS-A
//! `Arc<dyn RustFnRegistry>` — the dispatcher consults ONE chain over
//! every native crate's host, no fork per crate, no per-host match in
//! the server crate's `RustFirstDispatcher`.

use std::sync::Arc;

use oxide_rustfn::abi::{ABI_VERSION, FnEntry, FnTable, RawBuf, leak_name};
use oxide_rustfn::chain::ChainedRegistry;
use oxide_rustfn::host::{RustFnLibrary, RustFnRegistry};
use oxide_rustfn::shim::run_shim;
use oxide_rustfn::swap::{FnBinding, SwappableHost};
use oxide_rustfn::types::render_types_dts;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct GreetIn {
    name: String,
}

/// "Crate A" — a greet fn that prefixes `a:`. The shim shape a
/// `#[server_fn]` body generates (decode ODF → call → encode ODF).
extern "C" fn greet_shim_a(inp: *const u8, len: usize, out: *mut RawBuf) -> i32 {
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
        match oxide_rustfn::bridge::encode_odf(&format!("a:hello, {}", args.name)) {
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

/// "Crate B" — a greet fn that prefixes `b:`.
extern "C" fn greet_shim_b(inp: *const u8, len: usize, out: *mut RawBuf) -> i32 {
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
        match oxide_rustfn::bridge::encode_odf(&format!("b:hello, {}", args.name)) {
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

/// A minimal `oxide_fn_types_json` payload for one export + one decl —
/// the shape `types::TypesPayload` serializes, leaked to `'static` so the
/// in-process library's types slot accepts it.
/// A minimal valid `oxide_fn_types_json` payload for one export + one
/// decl — the exact shape `types::TypesPayload` serializes (`args` is a
/// `Vec<String>` of inline TS annotations, `decls` the fn-level pairs,
/// the top-level `decls` the deduped name → block map `render_types_dts`
/// reads). Leaked to `'static` so the in-process library's types slot
/// accepts it.
fn types_payload(export: &str, arg: &str, decl_name: &str, decl_block: &str) -> &'static str {
    let s = format!(
        r#"{{"fns":[{{"name":"{export}","file":"x.rs","args":["{arg}"],"ret":"string","decls":[]}}],"decls":{{"{decl_name}":"{decl_block}"}}}}"#
    );
    Box::leak(s.into_boxed_str())
}

/// One native crate's dev-loop host: a `SwappableHost` with one library
/// mounted and one id registered. The chain consults these through the
/// `RustFnRegistry` trait.
fn crate_host(
    table: &'static FnTable,
    types: Option<&'static str>,
    id: &str,
    export: &str,
) -> Arc<SwappableHost> {
    let host = Arc::new(SwappableHost::new());
    host.swap(
        RustFnLibrary::from_static(table, types).expect("the library loads"),
        &[FnBinding::new(id, export)],
    )
    .expect("the swap mounts");
    host
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    // Two native crates' worth of server fns: crate A owns `fn-greet-a`,
    // crate B owns `fn-greet-b`. Each is a `SwappableHost` the dev task
    // would drive; here they stand in for the post-boot state.
    let crate_a = crate_host(
        leak_table(vec![entry("greet_a", greet_shim_a)]),
        Some(types_payload(
            "greet_a",
            "GreetIn",
            "GreetIn",
            "export interface GreetIn { name: string }",
        )),
        "fn-greet-a",
        "greet_a",
    );
    let crate_b = crate_host(
        leak_table(vec![entry("greet_b", greet_shim_b)]),
        Some(types_payload(
            "greet_b",
            "GreetIn",
            "GreetOut",
            "export interface GreetOut { greeting: string }",
        )),
        "fn-greet-b",
        "greet_b",
    );

    // The multi-host consult adapter: `ChainedRegistry` over the two
    // crates' hosts. `Arc<ChainedRegistry>` upcasts to
    // `Arc<dyn RustFnRegistry>`, so the dispatcher consults ONE chain
    // through the same object-safe seam it already wires for a single
    // host — no fork per crate.
    let chain = Arc::new(ChainedRegistry::from_members(vec![
        Arc::clone(&crate_a) as Arc<dyn RustFnRegistry>,
        Arc::clone(&crate_b) as Arc<dyn RustFnRegistry>,
    ]));
    let registry: Arc<dyn RustFnRegistry> = chain.clone();
    println!("chain: {} members, ids {:?}", chain.len(), registry.ids());

    // The consult walks the members in push order; each disjoint id lands
    // on its owning member.
    let out = rt.block_on(registry.call("fn-greet-a", &greet_args("oxide")))?;
    println!(
        "call fn-greet-a: {:?}",
        String::from_utf8(out).unwrap_or_else(|_| "<binary>".into())
    );
    let out = rt.block_on(registry.call("fn-greet-b", &greet_args("oxide")))?;
    println!(
        "call fn-greet-b: {:?}",
        String::from_utf8(out).unwrap_or_else(|_| "<binary>".into())
    );

    // The deterministic first-has-wins: register the SAME id on both
    // crates (a cross-crate collision), re-chain, and observe crate A's
    // `a:` behavior wins — crate B is never reached for that id. The
    // consult order is the push order, every time.
    let collision_a = crate_host(
        leak_table(vec![entry("greet_a", greet_shim_a)]),
        None,
        "fn-shared",
        "greet_a",
    );
    let collision_b = crate_host(
        leak_table(vec![entry("greet_b", greet_shim_b)]),
        None,
        "fn-shared",
        "greet_b",
    );
    let collision = Arc::new(ChainedRegistry::from_members(vec![
        Arc::clone(&collision_a) as Arc<dyn RustFnRegistry>,
        Arc::clone(&collision_b) as Arc<dyn RustFnRegistry>,
    ]));
    let out = rt.block_on(collision.call("fn-shared", &greet_args("collide")))?;
    println!(
        "call fn-shared (first-has-wins): {:?}",
        String::from_utf8(out).unwrap_or_else(|_| "<binary>".into())
    );

    // The merged `.d.ts`: `types_json` consults every member and merges
    // the `fns` arrays + `decls` maps into one payload, so the host
    // renders every native crate's declarations behind one consult. One
    // bad crate (a malformed payload) would be skipped, not fatal.
    if let Some(merged) = registry.types_json() {
        let dts = render_types_dts(&merged)?;
        println!("--- merged .d.ts ({} bytes) ---", dts.len());
        print!("{dts}");
        println!("--- end .d.ts ---");
    }

    // The chain's posture snapshot — ONE consult, not N: the member
    // count, the sorted union of ids, the union id count, and whether
    // ANY member carries a types payload. The dev overlay renders this
    // through `ChainedReport` the same way it renders a single host
    // through `RegistryReport`.
    let report = chain.report();
    println!(
        "report: members {}, ids {:?} (count {}), has_types {}",
        report.member_count, report.ids, report.total_ids, report.has_types,
    );
    assert!(!report.is_empty(), "the chain owns ids");

    Ok(())
}
