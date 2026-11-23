//! TDD pins for the multi-host consult adapter — the `ChainedRegistry`
//! architectural seam the spec names but the substrate left unbuilt (spec
//! §V2-F08 "Host": "one host per crate; the consult adapter consults each
//! host in turn"; `crates/rustfn/src/swap.rs` and `host.rs` both point at
//! the adapter as the missing piece over the swappable-host M1/M2).
//!
//! Written BEFORE the implementation; every pin imports public API only.
//!
//! The contracts pinned here:
//!
//! - `ChainedRegistry` IS-A `RustFnRegistry`: `Arc<ChainedRegistry>`
//!   upcasts to `Arc<dyn RustFnRegistry>`, so the dispatcher consults ONE
//!   chain through the same object-safe seam it already wires for the
//!   boot-time and dev-loop hosts — no fork per native crate.
//! - `has(id)` is true when ANY member owns the id; `call(id, args)`
//!   consults the members in push order and delegates to the FIRST that
//!   `has(id)` — the deterministic consult order the spec names ("each
//!   host in turn"). An id no member owns refuses `FN-NOT-FOUND`.
//! - When two members both own the same id, the FIRST (push order) wins:
//!   the consult never reaches the second, so the second's behavior is
//!   never served for that id (the deterministic consult the dispatcher
//!   relies on — no surprises from member order).
//! - `ids()` returns the SORTED, deduplicated union of every member's
//!   registered ids (the trait surface the chain's report renders).
//! - `types_json()` MERGES every member's types payload: the `fns` arrays
//!   concatenate, the `decls` maps union (first contribution wins, the
//!   same dedup the `types::types_json` house rule already holds). A
//!   member with no payload contributes nothing; a malformed payload is
//!   skipped (logged, never panics) so one bad crate does not blank the
//!   whole `.d.ts`.
//! - `report()` is ONE consult over the chain: `member_count`, the
//!   sorted union `ids`, the union `total_ids`, and whether ANY member
//!   carries a types payload. `ChainedReport::is_empty()` is true when no
//!   member owns any id (the overlay's "is anything native-mounted?"
//!   sentinel).
//! - `RustFnRegistry::ids()` lands on the trait: the boot-time host and
//!   the dev-loop host both answer their sorted registered ids through
//!   the seam (the surface the chain enumerates the union through).
//! - The state is `Send + Sync` (`Arc<ChainedRegistry>` is the shape the
//!   dispatcher holds behind `Arc<dyn RustFnRegistry>`).

use std::sync::Arc;

use oxide_rustfn::abi::{ABI_VERSION, FnEntry, FnTable, RawBuf, leak_name};
use oxide_rustfn::bridge::{decode_odf, encode_odf};
use oxide_rustfn::chain::{ChainedRegistry, ChainedReport};
use oxide_rustfn::host::{RegistryErrorKind, RustFnHost, RustFnLibrary, RustFnRegistry};
use oxide_rustfn::shim::run_shim;

/// Build a `&'static FnTable` over `entries` (the test-fixture pattern the
/// host and swap pins use: the table is leaked, the host keeps the pointer
/// for the library's lifetime).
fn table(entries: &'static [FnEntry]) -> &'static FnTable {
    Box::leak(Box::new(FnTable {
        abi_version: ABI_VERSION,
        len: entries.len(),
        entries: entries.as_ptr(),
        free: free_impl,
    }))
}

extern "C" fn free_impl(buf: RawBuf) {
    let _ = buf;
}

fn entry_named(
    name: &'static str,
    call: extern "C" fn(*const u8, usize, *mut RawBuf) -> i32,
) -> FnEntry {
    FnEntry {
        name: leak_name(name),
        call,
    }
}

/// The "crate A" shim: echoes the input prefixed `a:` so the consult pin
/// can tell WHICH member answered.
extern "C" fn echo_a(inp: *const u8, len: usize, out: *mut RawBuf) -> i32 {
    let input: &[u8] = if inp.is_null() || len == 0 {
        &[]
    } else {
        // SAFETY: the host passes a live `len`-byte buffer.
        unsafe { std::slice::from_raw_parts(inp, len) }
    };
    let mut buf = RawBuf::empty();
    let status = run_shim(input, &mut buf, |bytes| {
        let text: String = decode_odf(bytes).map_err(|e| e.to_string())?;
        encode_odf(&format!("a:{text}")).map_err(|e| e.to_string())
    });
    // SAFETY: the host always passes a writable slot.
    unsafe {
        if !out.is_null() {
            *out = buf;
        }
    }
    status
}

/// The "crate B" shim: echoes the input prefixed `b:`.
extern "C" fn echo_b(inp: *const u8, len: usize, out: *mut RawBuf) -> i32 {
    let input: &[u8] = if inp.is_null() || len == 0 {
        &[]
    } else {
        // SAFETY: the host passes a live `len`-byte buffer.
        unsafe { std::slice::from_raw_parts(inp, len) }
    };
    let mut buf = RawBuf::empty();
    let status = run_shim(input, &mut buf, |bytes| {
        let text: String = decode_odf(bytes).map_err(|e| e.to_string())?;
        encode_odf(&format!("b:{text}")).map_err(|e| e.to_string())
    });
    // SAFETY: the host always passes a writable slot.
    unsafe {
        if !out.is_null() {
            *out = buf;
        }
    }
    status
}

/// A typed library for "crate A" with one export `echo_a` and a decl
/// `TypeA` — the payload the chain's `types_json` merge concatenates.
fn lib_a(types: Option<&'static str>) -> RustFnLibrary {
    let entries: &'static [FnEntry] =
        Box::leak(vec![entry_named("echo_a", echo_a)].into_boxed_slice());
    RustFnLibrary::from_static(table(entries), types).expect("a current-ABI table loads")
}

/// A typed library for "crate B" with one export `echo_b` and a decl
/// `TypeB`.
fn lib_b(types: Option<&'static str>) -> RustFnLibrary {
    let entries: &'static [FnEntry] =
        Box::leak(vec![entry_named("echo_b", echo_b)].into_boxed_slice());
    RustFnLibrary::from_static(table(entries), types).expect("a current-ABI table loads")
}

/// A boot-time host with one mounted library, `id` registered at the
/// library's sole export. The chain consults these through the
/// `RustFnRegistry` trait.
fn host_with(lib: RustFnLibrary, id: &str, export: &str) -> Arc<RustFnHost> {
    let mut host = RustFnHost::new();
    let index = host.mount(lib);
    host.register(id, index, export);
    Arc::new(host)
}

/// The §9.4 route's args wire: `call` takes RAW JSON (the host converts
/// to the ODF bytes the boundary carries).
fn json_string(s: &str) -> Vec<u8> {
    format!("\"{s}\"").into_bytes()
}

/// A minimal valid `oxide_fn_types_json` payload for one export + one
/// decl — the shape `types::TypesPayload` serializes.
fn types_payload(export: &str, decl: &str) -> String {
    format!(
        r#"{{"fns":[{{"name":"{export}","file":"x.rs","args":["string"],"ret":"string","decls":[]}}],"decls":{{"{decl}":"// {decl} block"}}}}"#
    )
}

/// Leak a `String` to `'static` so `RustFnLibrary::from_static`'s
/// `Option<&'static str>` types slot accepts a runtime-built payload
/// (the test-fixture pattern: the library keeps the pointer for its
/// lifetime, the test never reclaims it).
fn leak_str(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

#[test]
fn an_empty_chain_has_nothing_and_no_types() {
    let chain = ChainedRegistry::new();
    assert!(chain.is_empty());
    assert_eq!(chain.len(), 0);
    assert!(!chain.has("anything"));
    assert!(chain.types_json().is_none());
    let report = chain.report();
    assert!(report.is_empty());
    assert_eq!(report.member_count, 0);
    assert_eq!(report.total_ids, 0);
    assert_eq!(report.ids, Vec::<String>::new());
    assert!(!report.has_types);
}

#[test]
fn from_members_records_the_member_count() {
    let a = host_with(lib_a(None), "id-a", "echo_a");
    let b = host_with(lib_b(None), "id-b", "echo_b");
    let chain = ChainedRegistry::from_members(vec![a, b]);
    assert_eq!(chain.len(), 2);
    assert!(!chain.is_empty());
}

#[test]
fn push_appends_a_member_and_grows_the_chain() {
    let a = host_with(lib_a(None), "id-a", "echo_a");
    let mut chain = ChainedRegistry::new();
    chain.push(a);
    assert_eq!(chain.len(), 1);
    let b = host_with(lib_b(None), "id-b", "echo_b");
    chain.push(b);
    assert_eq!(chain.len(), 2);
}

#[tokio::test]
async fn a_single_member_chain_delegates_has_and_call() {
    let a = host_with(lib_a(None), "id-a", "echo_a");
    let chain = ChainedRegistry::from_members(vec![a]);
    assert!(chain.has("id-a"));
    let out = chain
        .call("id-a", &json_string("hi"))
        .await
        .expect("the member answers");
    let reply: String = decode_odf(&out).expect("odf reply");
    assert_eq!(reply, "a:hi");
}

#[tokio::test]
async fn the_chain_consults_each_member_in_turn_for_disjoint_ids() {
    let a = host_with(lib_a(None), "id-a", "echo_a");
    let b = host_with(lib_b(None), "id-b", "echo_b");
    let chain = ChainedRegistry::from_members(vec![a, b]);
    assert!(chain.has("id-a"));
    assert!(chain.has("id-b"));
    let out_a = chain
        .call("id-a", &json_string("x"))
        .await
        .expect("crate A answers");
    assert_eq!(decode_odf::<String>(&out_a).expect("odf"), "a:x");
    let out_b = chain
        .call("id-b", &json_string("y"))
        .await
        .expect("crate B answers");
    assert_eq!(decode_odf::<String>(&out_b).expect("odf"), "b:y");
}

#[tokio::test]
async fn when_two_members_both_own_an_id_the_first_in_push_order_wins() {
    // Both members register `id-shared` (the cross-crate collision the
    // consult order resolves deterministically). Crate A is pushed first,
    // so its `a:` behavior is what the consult serves — crate B is never
    // reached for that id.
    let a = host_with(lib_a(None), "id-shared", "echo_a");
    let b = host_with(lib_b(None), "id-shared", "echo_b");
    let chain = ChainedRegistry::from_members(vec![a, b]);
    assert!(chain.has("id-shared"));
    let out = chain
        .call("id-shared", &json_string("z"))
        .await
        .expect("the first member answers");
    assert_eq!(decode_odf::<String>(&out).expect("odf"), "a:z");
}

#[tokio::test]
async fn call_on_an_id_no_member_owns_is_the_registry_not_found() {
    let a = host_with(lib_a(None), "id-a", "echo_a");
    let chain = ChainedRegistry::from_members(vec![a]);
    let err = chain
        .call("id-missing", b"{}")
        .await
        .expect_err("no member owns the id");
    assert!(matches!(err.kind, RegistryErrorKind::NotFound));
}

#[test]
fn ids_is_the_sorted_deduplicated_union_of_member_ids() {
    let a = host_with(lib_a(None), "zeta", "echo_a");
    let b = host_with(lib_b(None), "alpha", "echo_b");
    let chain = ChainedRegistry::from_members(vec![a, b]);
    assert_eq!(chain.ids(), vec!["alpha".to_string(), "zeta".to_string()]);
}

#[test]
fn ids_deduplicates_an_id_two_members_both_own() {
    let a = host_with(lib_a(None), "shared", "echo_a");
    let b = host_with(lib_b(None), "shared", "echo_b");
    let chain = ChainedRegistry::from_members(vec![a, b]);
    assert_eq!(chain.ids(), vec!["shared".to_string()]);
}

#[test]
fn types_json_merges_every_member_payload_into_one() {
    let a = host_with(
        lib_a(Some(leak_str(types_payload("echo_a", "TypeA")))),
        "id-a",
        "echo_a",
    );
    let b = host_with(
        lib_b(Some(leak_str(types_payload("echo_b", "TypeB")))),
        "id-b",
        "echo_b",
    );
    let chain = ChainedRegistry::from_members(vec![a, b]);
    let merged = chain.types_json().expect("the merge answers");
    let value: serde_json::Value =
        serde_json::from_str(&merged).expect("the merged payload is valid JSON");
    let fn_names: Vec<String> = value["fns"]
        .as_array()
        .expect("fns is an array")
        .iter()
        .map(|f| f["name"].as_str().expect("a fn name").to_string())
        .collect();
    assert!(
        fn_names.contains(&"echo_a".to_string()),
        "crate A's export survived the merge: {fn_names:?}"
    );
    assert!(
        fn_names.contains(&"echo_b".to_string()),
        "crate B's export survived the merge: {fn_names:?}"
    );
    let decl_keys: Vec<String> = value["decls"]
        .as_object()
        .expect("decls is a map")
        .keys()
        .cloned()
        .collect();
    assert!(
        decl_keys.contains(&"TypeA".to_string()),
        "crate A's decl survived the union: {decl_keys:?}"
    );
    assert!(
        decl_keys.contains(&"TypeB".to_string()),
        "crate B's decl survived the union: {decl_keys:?}"
    );
}

#[test]
fn types_json_passes_through_when_only_one_member_has_a_payload() {
    let a = host_with(
        lib_a(Some(leak_str(types_payload("echo_a", "TypeA")))),
        "id-a",
        "echo_a",
    );
    let b = host_with(lib_b(None), "id-b", "echo_b");
    let chain = ChainedRegistry::from_members(vec![a, b]);
    let merged = chain.types_json().expect("the typed member answers");
    let value: serde_json::Value = serde_json::from_str(&merged).expect("valid JSON");
    assert_eq!(
        value["fns"][0]["name"].as_str(),
        Some("echo_a"),
        "the untyped member contributed nothing"
    );
}

#[test]
fn types_json_skips_a_malformed_payload_and_keeps_the_valid_one() {
    // One member carries a malformed payload (a corrupt crate the loader
    // somehow mounted); the other carries a valid one. The merge drops
    // the malformed payload (logged, never panics) and serves the valid
    // one — one bad crate never blanks the whole `.d.ts`.
    let a = host_with(lib_a(Some("not-json{")), "id-a", "echo_a");
    let b = host_with(
        lib_b(Some(leak_str(types_payload("echo_b", "TypeB")))),
        "id-b",
        "echo_b",
    );
    let chain = ChainedRegistry::from_members(vec![a, b]);
    let merged = chain.types_json().expect("the valid member answers");
    let value: serde_json::Value = serde_json::from_str(&merged).expect("valid JSON");
    assert_eq!(
        value["fns"][0]["name"].as_str(),
        Some("echo_b"),
        "the malformed payload was skipped, not fatal"
    );
}

#[test]
fn types_json_is_none_when_no_member_carries_a_payload() {
    let a = host_with(lib_a(None), "id-a", "echo_a");
    let b = host_with(lib_b(None), "id-b", "echo_b");
    let chain = ChainedRegistry::from_members(vec![a, b]);
    assert!(chain.types_json().is_none());
}

#[test]
fn report_snapshots_the_chain_posture_in_one_consult() {
    let a = host_with(
        lib_a(Some(leak_str(types_payload("echo_a", "TypeA")))),
        "zeta",
        "echo_a",
    );
    let b = host_with(lib_b(None), "alpha", "echo_b");
    let chain = ChainedRegistry::from_members(vec![a, b]);
    let report: ChainedReport = chain.report();
    assert_eq!(report.member_count, 2);
    assert_eq!(report.total_ids, 2);
    assert_eq!(report.ids, vec!["alpha".to_string(), "zeta".to_string()]);
    assert!(report.has_types, "crate A carries a payload");
    assert!(!report.is_empty());
}

#[test]
fn report_is_empty_when_no_member_owns_any_id() {
    // Two hosts, neither registered an id: the chain reports empty even
    // though the member count is non-zero (the overlay's "is anything
    // native-mounted?" sentinel).
    let a = Arc::new(RustFnHost::new());
    let b = Arc::new(RustFnHost::new());
    let chain = ChainedRegistry::from_members(vec![a, b]);
    let report = chain.report();
    assert_eq!(report.member_count, 2);
    assert_eq!(report.total_ids, 0);
    assert!(report.is_empty());
}

#[test]
fn the_chain_upcasts_to_the_registry_consult_surface() {
    // `Arc<ChainedRegistry>` IS-A `Arc<dyn RustFnRegistry>` — the
    // dispatcher consults ONE chain through the same object-safe seam it
    // already wires for the boot-time and dev-loop hosts.
    let a = host_with(lib_a(None), "id-a", "echo_a");
    let chain = Arc::new(ChainedRegistry::from_members(vec![a]));
    let registry: Arc<dyn RustFnRegistry> = chain;
    assert!(registry.has("id-a"));
    assert!(!registry.has("id-missing"));
    assert_eq!(registry.ids(), vec!["id-a".to_string()]);
}

#[test]
fn the_chain_state_is_send_sync_and_shareable() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Arc<ChainedRegistry>>();
    let a = host_with(lib_a(None), "id-a", "echo_a");
    let chain = Arc::new(ChainedRegistry::from_members(vec![a]));
    std::thread::spawn(move || {
        assert!(chain.has("id-a"));
    })
    .join()
    .expect("the shared handle crosses threads");
}

#[test]
fn the_registry_trait_exposes_ids_on_the_boot_time_host() {
    // The trait extension `RustFnRegistry::ids` the chain enumerates the
    // union through — the boot-time host answers its sorted registered
    // ids via the seam, not just via the inherent method.
    let a = host_with(lib_a(None), "zeta", "echo_a");
    let registry: Arc<dyn RustFnRegistry> = a;
    assert_eq!(registry.ids(), vec!["zeta".to_string()]);
}

#[test]
fn the_registry_trait_exposes_ids_on_the_swappable_host() {
    use oxide_rustfn::swap::{FnBinding, SwappableHost};
    // The dev-loop host answers the same `ids` seam — the chain
    // enumerates the union over EITHER member shape.
    let host = Arc::new(SwappableHost::new());
    host.swap(lib_a(None), &[FnBinding::new("alpha", "echo_a")])
        .expect("the swap mounts");
    let registry: Arc<dyn RustFnRegistry> = host;
    assert_eq!(registry.ids(), vec!["alpha".to_string()]);
}

#[tokio::test]
async fn the_async_registry_call_delegates_through_the_chain() {
    // The trait's async `call` (the consult the dispatcher wires behind
    // `Arc<dyn RustFnRegistry>`) delegates through the chain the same way
    // the inherent consult does — the first member that `has(id)` answers.
    let a = host_with(lib_a(None), "id-a", "echo_a");
    let chain = Arc::new(ChainedRegistry::from_members(vec![a]));
    let registry: Arc<dyn RustFnRegistry> = chain;
    let out = registry
        .call("id-a", &json_string("async"))
        .await
        .expect("the chain delegates");
    assert_eq!(decode_odf::<String>(&out).expect("odf"), "a:async");
}
