//! The §V2-F08 native server functions host — a walkthrough of the
//! seam end-to-end, entirely in-process (the [`RustFnLibrary`]
//! `from_static` seam stands in for the loaded cdylib; the
//! `#[ignore]`d fixture pin in `tests/rustfn_tdd.rs` drives the real
//! `libloading` path):
//!
//! ```text
//! cargo run -p oxide-rustfn --example rustfn
//! ```
//!
//! The loop the host drives: mount a library → register the §9.4 ids
//! → call through the registry (args JSON in, the fn's output out) →
//! hot-swap by mounting the new copy and re-registering (the old
//! `Arc` retires when the last in-flight call finishes).

use std::time::Duration;

use oxide_rustfn::abi::{ABI_VERSION, FnEntry, FnTable, RawBuf};
use oxide_rustfn::host::{RustFnHost, RustFnLibrary, RustFnRegistry};
use oxide_rustfn::shim::run_shim;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct GreetIn {
    name: String,
}

/// The "v1 library": a greet fn over the ODF wire (decode the args,
/// call the body, encode the result — what a `#[server_fn]` shim
/// generates).
extern "C" fn greet_shim_v1(inp: *const u8, len: usize, out: *mut RawBuf) -> i32 {
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
        match oxide_rustfn::bridge::encode_odf(&format!("hello, {} (v1)", args.name)) {
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

/// The "v2 library" — the hot-swap's new behavior.
extern "C" fn greet_shim_v2(inp: *const u8, len: usize, out: *mut RawBuf) -> i32 {
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
        let greeting = format!("hello, {} (v2 — hot-swapped)", args.name);
        match oxide_rustfn::bridge::encode_odf(&greeting) {
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

/// An fn that returns `Err` — the status-1 path the envelope maps to
/// 400 `FN-THREW`.
extern "C" fn refusing_shim(_inp: *const u8, _len: usize, out: *mut RawBuf) -> i32 {
    let mut buf = RawBuf::empty();
    let status = run_shim(b"", &mut buf, |_| {
        Err("the width must be positive".to_string())
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
        name: oxide_rustfn::abi::leak_name(name),
        call,
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    let v1 = leak_table(vec![
        entry("greet", greet_shim_v1),
        entry("refuse", refusing_shim),
    ]);
    let v2 = leak_table(vec![entry("greet", greet_shim_v2)]);

    let mut host = RustFnHost::new().with_timeout(Duration::from_secs(10));
    let lib_v1 = RustFnLibrary::from_static(v1, None)?;
    let index = host.mount(lib_v1);
    host.register("fn-greet", index, "greet");
    host.register("fn-refuse", index, "refuse");
    println!("registered ids: {:?}", host.ids());

    // The §9.4 route's args JSON → ODF at the boundary → the fn → ODF
    // back (the dispatcher consult hands this exact call the request).
    let out = rt.block_on(host.call("fn-greet", br#"{"name":"oxide"}"#))?;
    println!(
        "call fn-greet: {:?}",
        String::from_utf8(out).unwrap_or_else(|_| "<binary>".into())
    );

    // The error path: the fn's message rides status 1 → 400 FN-THREW.
    if let Err(e) = rt.block_on(host.call("fn-refuse", b"{}")) {
        println!(
            "call fn-refuse: {} {} ({})",
            e.status(),
            e.envelope_code(),
            e.message
        );
    }

    // The hot-swap: mount the new copy, re-register, the next call
    // serves the new behavior.
    let lib_v2 = RustFnLibrary::from_static(v2, None)?;
    let new_index = host.mount(lib_v2);
    host.register("fn-greet", new_index, "greet");
    let out = rt.block_on(host.call("fn-greet", br#"{"name":"oxide"}"#))?;
    println!(
        "call fn-greet after hot-swap: {:?}",
        String::from_utf8(out).unwrap_or_else(|_| "<binary>".into())
    );

    // The registry is the object-safe seam the server crate's
    // RustFirstDispatcher consults before the JS pool — the CLI wires
    // the loaded host in behind `Arc<dyn RustFnRegistry>`.
    let registry: std::sync::Arc<dyn RustFnRegistry> = std::sync::Arc::new(host);
    println!("registry owns fn-greet: {}", registry.has("fn-greet"));

    Ok(())
}
