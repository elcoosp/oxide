//! TDD pins for the §V2-F08 `.server.rs` import stubs — the
//! `rust_fn_stub_source` generator the build resolves `import { x }
//! from './mod.server.rs'` through (the same wire shapes the
//! `.server.ts` transform emits, computed from the host's export
//! names instead of a JS AST scan).
//!
//! Written BEFORE the implementation; every pin imports public API only.
//!
//! The contracts pinned here:
//!
//! - Every export becomes a call-through stub: `export const name =
//!   (async (...args) => __oxide_call_fn("<id>", args));` — the id is
//!   the §9.4 `server_fn_id(file, export, secret)` triple, identical to
//!   what a `.server.ts` module of the same path would register.
//! - The stub module is self-contained: it carries the
//!   `__oxide_call_fn` import itself (the transform's tail-import rule).
//! - SSR/Edge additionally export `__oxide_fn_ids` (the manifest's id
//!   list) — there is no JS function to register: the native fn runs
//!   in the host, never in the isolate.
//! - An empty export list renders the header comment only (the module
//!   imports nothing and exports nothing; a missing-name import fails
//!   at bundle time with the bundler's own error).

use oxide_fw_compiler::{Env, rust_fn_stub_source, server_fn_id};

#[test]
fn client_stub_is_a_self_contained_call_through_module() {
    let (code, stubs) =
        rust_fn_stub_source("src/thumb.server.rs", &["thumbnail"], Env::Client, "s1");
    let id = server_fn_id("src/thumb.server.rs", "thumbnail", "s1");
    assert!(code.contains("import { __oxide_call_fn } from \"@oxide-fw/core/fn\";"));
    assert!(
        code.contains(&format!(
            "export const thumbnail = (async (...args) => __oxide_call_fn(\"{id}\", args));"
        )),
        "the call-through stub, got: {code}"
    );
    assert_eq!(stubs.len(), 1);
    assert_eq!(stubs[0].export_name, "thumbnail");
    assert_eq!(stubs[0].id, id);
}

#[test]
fn stub_ids_match_the_server_ts_transform_for_the_same_triple() {
    // The parity pin: a `.server.rs` file and a `.server.ts` file at
    // the same module path with the same export produce the SAME id —
    // the dispatcher consult and the HTTP route cannot tell them apart.
    let (code, _) =
        rust_fn_stub_source("src/thumb.server.rs", &["thumbnail"], Env::Client, "secret");
    let (ts_code, ts_stubs) = oxide_fw_compiler::transform_server_fn_stubs(
        "export const thumbnail = serverFn(async (i: number) => i);",
        "src/thumb.server.rs",
        Env::Client,
        "secret",
    )
    .expect("the ts transform runs");
    let id = server_fn_id("src/thumb.server.rs", "thumbnail", "secret");
    assert!(ts_code.contains(&id));
    assert_eq!(ts_stubs[0].id, id);
    assert!(code.contains(&id));
}

#[test]
fn ssr_stub_adds_the_fn_ids_export_without_registering() {
    let (code, stubs) = rust_fn_stub_source("src/thumb.server.rs", &["thumbnail"], Env::Ssr, "s1");
    let id = server_fn_id("src/thumb.server.rs", "thumbnail", "s1");
    // No `__oxide_register_fn` line: the native fn never runs in the
    // isolate, so there is no JS function to register.
    assert!(!code.contains("__oxide_register_fn"));
    assert!(
        code.contains(&format!("export const __oxide_fn_ids = [\"{id}\"];")),
        "the ids manifest export, got: {code}"
    );
    // The call-through still rides the SSR stub (an isolate-side caller
    // reaches the native fn through the same fn route).
    assert!(code.contains("__oxide_call_fn"));
    assert_eq!(stubs.len(), 1);
}

#[test]
fn edge_stub_matches_the_ssr_shape() {
    let (edge, _) = rust_fn_stub_source("src/thumb.server.rs", &["thumbnail"], Env::Edge, "s1");
    let (ssr, _) = rust_fn_stub_source("src/thumb.server.rs", &["thumbnail"], Env::Ssr, "s1");
    assert_eq!(edge, ssr);
}

#[test]
fn two_exports_produce_two_distinct_ids() {
    let (code, stubs) = rust_fn_stub_source(
        "src/media.server.rs",
        &["thumbnail", "scale"],
        Env::Client,
        "s1",
    );
    let a = server_fn_id("src/media.server.rs", "thumbnail", "s1");
    let b = server_fn_id("src/media.server.rs", "scale", "s1");
    assert_ne!(a, b);
    assert_eq!(stubs.len(), 2);
    assert!(code.contains(&format!(
        "export const thumbnail = (async (...args) => __oxide_call_fn(\"{a}\", args));"
    )));
    assert!(code.contains(&format!(
        "export const scale = (async (...args) => __oxide_call_fn(\"{b}\", args));"
    )));
}

#[test]
fn empty_exports_render_the_header_only() {
    let (code, stubs) = rust_fn_stub_source("src/empty.server.rs", &[], Env::Client, "s1");
    assert_eq!(stubs.len(), 0);
    assert!(!code.contains("__oxide_call_fn"));
    assert!(!code.contains("import {"));
    assert!(
        code.contains("empty.server.rs"),
        "the header names the source file"
    );
}
