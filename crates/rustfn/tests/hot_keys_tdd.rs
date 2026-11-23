//! TDD pins for the per-id hot-key attribution decorator — the
//! `HotKeyRegistry` architectural seam the cache plane's
//! `HotKeyObserver` (F38) already lands for cache events and the
//! rustfn registry story names as the missing attribution arm beside
//! the `StatsRegistry` aggregate counters (M4): `StatsRegistry`
//! answers "how many native calls succeeded vs fell back" (one counter
//! set over every id), but NO decorator answers "which native fn ids
//! dominate the registry's traffic" — the per-id ledger a traffic
//! dashboard opens with. The cache plane's answer is `HotKeyObserver`
//! (a bounded per-key event ledger, admission by first sight, the
//! honest saturation posture); this module is that arm for the
//! registry plane.
//!
//! Written BEFORE the implementation; every pin imports public API only.
//!
//! The contracts pinned here:
//!
//! - `HotKeyRegistry` IS-A `RustFnRegistry`: `Arc<HotKeyRegistry>`
//!   upcasts to `Arc<dyn RustFnRegistry>`, so the dispatcher consults
//!   ONE attributed registry through the same object-safe seam it
//!   already wires for the boot-time host, the dev-loop host, the
//!   chain, the stats decorator and the tracing decorator.
//! - `call` attributes every call to its id: the per-id counter bumps
//!   once per call (served OR error — the ledger is per-id traffic,
//!   not a per-outcome split, the same posture `HotKeyObserver` holds
//!   for cache keys). The inner call's result passes through
//!   UNCHANGED (the "observe, never alter" rule `StatsRegistry` and
//!   `TracingRegistry` hold).
//! - `has` / `ids` / `types_json` delegate to the inner registry
//!   UNCOUNTED (consult reads, not calls — the same carve-out
//!   `StatsRegistry` holds).
//! - The map is bounded (`HotKeyRegistry::bounded`); admission is
//!   FIRST SIGHT (a key admits while there is room; once resident it
//!   counts past the bound forever). A NEW key past the bound is
//!   refused: its calls count toward `unattributed` (the honest
//!   saturation posture — the score is "how much of the stream the
//!   ledger covers"). The default is unbounded (every id admits).
//! - `top(n)` is the deterministic read: count desc, id asc on ties,
//!   at most `n` rows. Two reads over the same state are equal.
//! - `reset()` clears the map and zeroes `unattributed` (the dev
//!   overlay's "refresh the window" verb).
//! - Counts saturate at `u64::MAX`.
//! - The state is `Send + Sync`.

use std::sync::Arc;

use oxide_rustfn::abi::{ABI_VERSION, FnEntry, FnTable, RawBuf, leak_name};
use oxide_rustfn::bridge::{decode_odf, encode_odf};
use oxide_rustfn::host::{RustFnHost, RustFnLibrary, RustFnRegistry};
use oxide_rustfn::hot_keys::{HotKeyRegistry, HotKeyRow};
use oxide_rustfn::shim::run_shim;

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

/// The echo shim: decodes the ODF string, prefixes `v1:`, re-encodes.
extern "C" fn echo_v1(inp: *const u8, len: usize, out: *mut RawBuf) -> i32 {
    let input: &[u8] = if inp.is_null() || len == 0 {
        &[]
    } else {
        // SAFETY: the host passes a live `len`-byte buffer.
        unsafe { std::slice::from_raw_parts(inp, len) }
    };
    let mut buf = RawBuf::empty();
    let status = run_shim(input, &mut buf, |bytes| {
        let text: String = decode_odf(bytes).map_err(|e| e.to_string())?;
        encode_odf(&format!("v1:{text}")).map_err(|e| e.to_string())
    });
    // SAFETY: the host always passes a writable slot.
    unsafe {
        if !out.is_null() {
            *out = buf;
        }
    }
    status
}

/// A fn that returns `Err` — the `FnError` arm (still attributed to its
/// id; the ledger is per-id traffic, not a per-outcome split).
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

fn echo_lib() -> RustFnLibrary {
    let entries: &'static [FnEntry] =
        Box::leak(vec![entry_named("echo", echo_v1)].into_boxed_slice());
    RustFnLibrary::from_static(table(entries), None).expect("a current-ABI table loads")
}

fn refusing_lib() -> RustFnLibrary {
    let entries: &'static [FnEntry] =
        Box::leak(vec![entry_named("refuse", refusing_shim)].into_boxed_slice());
    RustFnLibrary::from_static(table(entries), None).expect("a current-ABI table loads")
}

/// A boot-time host with the echo fn mounted, two ids registered — the
/// inner registry the decorator wraps. Two ids exercise the sort +
/// tie-break contract.
fn echo_host_two_ids() -> Arc<RustFnHost> {
    let mut host = RustFnHost::new();
    let index = host.mount(echo_lib());
    host.register("id-alpha", index, "echo");
    host.register("id-beta", index, "echo");
    Arc::new(host)
}

fn refusing_host() -> Arc<RustFnHost> {
    let mut host = RustFnHost::new();
    let index = host.mount(refusing_lib());
    host.register("id-refuse", index, "refuse");
    Arc::new(host)
}

fn json_string(s: &str) -> Vec<u8> {
    format!("\"{s}\"").into_bytes()
}

/// The decorator upcasts: `Arc<HotKeyRegistry>` IS-A `Arc<dyn
/// RustFnRegistry>`, the shape the dispatcher wires behind the same
/// object-safe seam it already consults. Without the impl this fails to
/// compile (the trait bound is unsatisfied).
#[test]
fn the_hot_key_registry_is_a_rust_fn_registry() {
    fn assert_registry<T: RustFnRegistry>() {}
    assert_registry::<HotKeyRegistry>();
    let decorated = Arc::new(HotKeyRegistry::new(echo_host_two_ids()));
    let registry: Arc<dyn RustFnRegistry> = decorated;
    assert!(registry.has("id-alpha"), "the decorator delegates has");
}

/// A fresh decorator has seen no traffic: `top(n)` is empty and
/// `unattributed` is zero.
#[test]
fn hot_keys_start_empty_on_a_fresh_decorator() {
    let decorated = Arc::new(HotKeyRegistry::new(echo_host_two_ids()));
    assert!(decorated.top(10).is_empty(), "no call was attributed yet");
    assert_eq!(decorated.unattributed(), 0);
}

/// A served call attributes to its id: `top(1)` returns one row naming
/// the id, count 1. The inner call's bytes pass through unchanged.
#[tokio::test]
async fn a_served_call_attributes_to_its_id() {
    let decorated = Arc::new(HotKeyRegistry::new(echo_host_two_ids()));
    let registry: Arc<dyn RustFnRegistry> = decorated.clone();
    let out = registry
        .call("id-alpha", &json_string("hello"))
        .await
        .expect("the attributed call serves");
    let text: String = decode_odf(&out).expect("the bytes decode");
    assert_eq!(text, "v1:hello", "the decorator did not alter the wire");
    let top = decorated.top(10);
    assert_eq!(top.len(), 1);
    assert_eq!(
        top[0],
        HotKeyRow {
            id: "id-alpha".into(),
            calls: 1
        }
    );
}

/// A refused call (the `FnError` arm) STILL attributes to its id — the
/// ledger is per-id traffic, not a per-outcome split (the same posture
/// the cache `HotKeyObserver` holds: every completed event counts one).
#[tokio::test]
async fn a_refused_call_attributes_to_its_id() {
    let decorated = Arc::new(HotKeyRegistry::new(refusing_host()));
    let registry: Arc<dyn RustFnRegistry> = decorated.clone();
    let _ = registry
        .call("id-refuse", b"{}")
        .await
        .expect_err("the attributed call refuses");
    let top = decorated.top(10);
    assert_eq!(top.len(), 1);
    assert_eq!(top[0].id, "id-refuse");
    assert_eq!(top[0].calls, 1);
}

/// `top(n)` is the deterministic read: count DESC, id ASC on ties, at
/// most `n` rows. Two ids called different counts sort by count; the
/// hotter id leads.
#[tokio::test]
async fn top_is_count_desc_id_asc_on_ties() {
    let decorated = Arc::new(HotKeyRegistry::new(echo_host_two_ids()));
    let registry: Arc<dyn RustFnRegistry> = decorated.clone();
    // id-beta called 3 times, id-alpha called 1 time.
    for _ in 0..3 {
        let _ = registry
            .call("id-beta", &json_string("b"))
            .await
            .expect("serves");
    }
    let _ = registry
        .call("id-alpha", &json_string("a"))
        .await
        .expect("serves");
    let top = decorated.top(10);
    assert_eq!(top.len(), 2);
    // count desc: beta (3) leads, alpha (1) trails.
    assert_eq!(top[0].id, "id-beta");
    assert_eq!(top[0].calls, 3);
    assert_eq!(top[1].id, "id-alpha");
    assert_eq!(top[1].calls, 1);
    // top(1) truncates to the hottest.
    let one = decorated.top(1);
    assert_eq!(one.len(), 1);
    assert_eq!(one[0].id, "id-beta");
    // A tie breaks id ASC: call both once more so beta=4 alpha=2 still not
    // tied; instead build a fresh decorator with a tie.
    let decorated2 = Arc::new(HotKeyRegistry::new(echo_host_two_ids()));
    let r2: Arc<dyn RustFnRegistry> = decorated2.clone();
    let _ = r2.call("id-beta", &json_string("b")).await.expect("serves");
    let _ = r2
        .call("id-alpha", &json_string("a"))
        .await
        .expect("serves");
    let tied = decorated2.top(10);
    assert_eq!(tied.len(), 2);
    // both count 1 → id asc: alpha before beta.
    assert_eq!(tied[0].id, "id-alpha");
    assert_eq!(tied[1].id, "id-beta");
}

/// `reset()` clears the map and zeroes `unattributed` — the dev
/// overlay's "refresh the window" verb.
#[tokio::test]
async fn reset_clears_the_ledger() {
    let decorated = Arc::new(HotKeyRegistry::new(echo_host_two_ids()));
    let registry: Arc<dyn RustFnRegistry> = decorated.clone();
    let _ = registry
        .call("id-alpha", &json_string("a"))
        .await
        .expect("serves");
    assert_eq!(decorated.top(10).len(), 1);
    decorated.reset();
    assert_eq!(decorated.top(10).len(), 0);
    assert_eq!(decorated.unattributed(), 0);
}

/// A bounded ledger admits by first sight: ids that registered while
/// there was room count past the bound forever; a NEW id past the bound
/// is refused and counted toward `unattributed` (the honest saturation
/// posture — the score is "how much of the stream the ledger covers").
#[tokio::test]
async fn a_bounded_ledger_refuses_new_ids_past_the_bound() {
    // bound = 1: id-alpha admits (first sight), id-beta is past the
    // bound → unattributed.
    let decorated = Arc::new(HotKeyRegistry::bounded(echo_host_two_ids(), 1));
    let registry: Arc<dyn RustFnRegistry> = decorated.clone();
    let _ = registry
        .call("id-alpha", &json_string("a"))
        .await
        .expect("serves");
    let _ = registry
        .call("id-beta", &json_string("b"))
        .await
        .expect("serves");
    let top = decorated.top(10);
    assert_eq!(top.len(), 1, "only the admitted id is in the ledger");
    assert_eq!(top[0].id, "id-alpha");
    assert_eq!(top[0].calls, 1);
    assert_eq!(
        decorated.unattributed(),
        1,
        "the refused id counted as unattributed"
    );
    // id-alpha called again past the bound still counts (resident forever).
    let _ = registry
        .call("id-alpha", &json_string("a2"))
        .await
        .expect("serves");
    let top = decorated.top(10);
    assert_eq!(top[0].calls, 2);
    assert_eq!(
        decorated.unattributed(),
        1,
        "no new unattributed for a resident id"
    );
}

/// `has` / `ids` / `types_json` delegate UNCOUNTED — consult reads, not
/// calls (the same carve-out `StatsRegistry` holds).
#[test]
fn consult_reads_delegate_uncounted() {
    let decorated = Arc::new(HotKeyRegistry::new(echo_host_two_ids()));
    assert!(decorated.has("id-alpha"));
    assert!(!decorated.has("id-missing"));
    let mut ids = decorated.ids();
    ids.sort();
    assert_eq!(ids, vec!["id-alpha".to_string(), "id-beta".to_string()]);
    assert_eq!(decorated.types_json(), None);
    assert!(
        decorated.top(10).is_empty(),
        "consult reads did not attribute"
    );
}

/// The decorator composes with `StatsRegistry`: wrap an
/// `Arc<StatsRegistry>` and the consult STILL counts AND attributes —
/// every call lands in the aggregate counters AND the per-id ledger.
#[tokio::test]
async fn the_decorator_composes_with_stats_registry() {
    use oxide_rustfn::stats::StatsRegistry;
    let stats = Arc::new(StatsRegistry::new(echo_host_two_ids()));
    let attributed = Arc::new(HotKeyRegistry::new(stats.clone()));
    let registry: Arc<dyn RustFnRegistry> = attributed.clone();
    let _ = registry
        .call("id-alpha", &json_string("hi"))
        .await
        .expect("serves");
    // The stats decorator (inner) counted the call.
    assert_eq!(stats.stats().calls_served, 1);
    // The hot-key decorator attributed it.
    let top = attributed.top(10);
    assert_eq!(top.len(), 1);
    assert_eq!(top[0].id, "id-alpha");
}

/// The state is `Send + Sync`: `Arc<HotKeyRegistry>` is the shape the
/// dispatcher holds behind `Arc<dyn RustFnRegistry>`.
#[test]
fn the_hot_key_registry_state_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<HotKeyRegistry>();
    let decorated = Arc::new(HotKeyRegistry::new(echo_host_two_ids()));
    let _handle: Arc<dyn RustFnRegistry> = decorated;
}

/// A not-found call (no member owns the id) STILL attributes to the id
/// — the ledger records the ATTEMPT, not just the served calls. This is
/// the "which ids is the dispatcher being asked for" attribution the
/// aggregate counters cannot surface (they fold not-found into one
/// counter; the ledger keeps it per-id).
#[tokio::test]
async fn a_not_found_call_attributes_to_the_asked_id() {
    let decorated = Arc::new(HotKeyRegistry::new(echo_host_two_ids()));
    let registry: Arc<dyn RustFnRegistry> = decorated.clone();
    let _ = registry
        .call("id-missing", &json_string("x"))
        .await
        .expect_err("the call misses");
    let top = decorated.top(10);
    assert_eq!(top.len(), 1);
    assert_eq!(top[0].id, "id-missing");
    assert_eq!(top[0].calls, 1);
    // The error kind passed through unchanged.
    let _ = registry
        .call("id-missing", &json_string("y"))
        .await
        .expect_err("still misses");
    assert_eq!(decorated.top(10)[0].calls, 2);
}
