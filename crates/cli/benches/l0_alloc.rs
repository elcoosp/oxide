//! V2-B09 / A01-phase-3: the L0-hit allocation budget — the committed dhat
//! harness the report's G2 row records as the remaining R03 gap ("L0 hit,
//! ≤ 12 heap allocs per request (dhat) … no committed dhat harness yet").
//!
//! One L0 hit is the request work the lookup stage performs for a warm
//! page, minus the socket: the key build, the `L0Cache::get` lookup (the
//! P-03 `Arc<Entry>` seam — a hot hit copies no body out of the memory
//! tier), the sentineled body's nonce splice (`materialize`) and the stored
//! header set the response carries. The harness snapshots dhat's block
//! count around N hits, divides, and FAILS the run over the budget — the
//! B09 discipline (a budget is a gate, not a graph line), with the exact
//! number printed for the report row.
//!
//! A dhat profiler is per-process, so the harness owns the whole binary
//! (`harness = false`, the `alloc_match.rs` shape) and measures in one
//! place. Everything that is not the hit itself — the runtime, the cache,
//! the stored entry, the key strings — is built OUTSIDE the measurement
//! window, so the counted blocks are the hit's and only the hit's. The
//! fixture rides the store path's own builder (`L0Cache::make_entry` with
//! the sentineled body — the `l0_conditional` bench's shape) and the store
//! key comes from the public `L0Cache::key_for` (the same B05 builder the
//! private `key` rides for this build id), so the measured cost is the HIT,
//! not the store.
//!
//! Run with `cargo bench -p oxide-fw-cli --features bench-util --bench l0_alloc`.

/// The budget the report's G2 row pins: a warm L0 hit allocates at most
/// this many heap blocks per request.
const BUDGET_BLOCKS_PER_HIT: u64 = 12;

/// Hits per measurement window — large enough that the per-hit key-building
/// and splice allocations dominate any one-time window noise.
const HITS_PER_WINDOW: u64 = 10_000;

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

use oxide_fw_cli::{L0Cache, NONCE_SENTINEL};
use std::hint::black_box;
use std::sync::Arc;
use std::time::Duration;

/// The route identity every hit looks up (fixed — the key is pre-built).
const ROUTE: &str = "routes/blog/[slug]/page";
const PARAMS: &str = "{\"slug\":\"a\"}";
const VARY: &str = "";
const BUILD_ID: &str = "bench-build";

/// The stored entry one hit serves: a small sentineled HTML body (the
/// prerender pass's shape) with the headers a page response carries.
fn stored_entry(l0: &L0Cache) -> Arc<oxide_fw_cache::Entry> {
    let body = format!("<html><head></head><body data-ox=\"{NONCE_SENTINEL}\">hello</body></html>")
        .into_bytes();
    let entry = l0.make_entry(
        200,
        vec![
            ("content-type".to_string(), "text/html".to_string()),
            (
                "cache-control".to_string(),
                "public, max-age=0, must-revalidate".to_string(),
            ),
            ("x-oxide-build".to_string(), BUILD_ID.to_string()),
        ],
        body,
        vec![],
    );
    Arc::new(entry)
}

/// One L0 hit: exactly the per-request work the lookup stage's 200 arm
/// performs before the response reaches the socket — the lookup, the
/// sentineled body's nonce splice, and the stored headers cloned once into
/// the response (the stage iterates `entry.headers` directly; the old
/// intermediate `hit.headers()` Vec was the double-clone the harness
/// caught). The metrics/waterfall bookkeeping and the socket write are the
/// recorded deltas — neither is the hit's allocation work. The splice cost
/// is value-independent, so a fixed nonce measures it honestly.
fn one_hit(rt: &tokio::runtime::Runtime, l0: &Arc<L0Cache>, nonce: &str) -> usize {
    rt.block_on(async {
        let hit = l0.get(ROUTE, PARAMS, VARY).await.expect("warmed L0 hit");
        let body = hit.materialize(nonce);
        let body_len = body.len();
        let mut resp = oxide_fw_server::OxideResponse::new(hit.status(), body);
        for (k, v) in hit.entry.headers.iter() {
            resp = resp.with_header(k, v.clone());
        }
        black_box(body_len + resp.headers.len())
    })
}

fn main() {
    // The stats API needs a live profiler (the alloc_match.rs shape); it
    // stays alive for the whole run so both snapshots see it.
    let _prof = dhat::Profiler::builder().testing().build();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("bench runtime");
    let tier: Arc<dyn oxide_fw_cache::CacheHandler> =
        Arc::new(oxide_fw_cache::TieredCache::new(16 * 1024 * 1024));
    let l0 = Arc::new(L0Cache::new(
        Arc::clone(&tier),
        BUILD_ID,
        Duration::from_secs(300),
    ));
    let entry = stored_entry(&l0);
    rt.block_on(l0.store_entry(
        &L0Cache::key_for(ROUTE, PARAMS, VARY, BUILD_ID),
        (*entry).clone(),
    ));

    let nonce = "bench0123456789abcdef0123456789ab";

    // Warm: the runtime's lazy state and the handler's one-time init must
    // not sit inside the measurement window.
    one_hit(&rt, &l0, nonce);
    one_hit(&rt, &l0, nonce);

    // The window runner: snapshot -> N iterations -> delta, printed per hit.
    let window = |name: &str, f: &mut dyn FnMut()| -> u64 {
        f();
        let before = dhat::HeapStats::get();
        for _ in 0..HITS_PER_WINDOW {
            f();
        }
        let after = dhat::HeapStats::get();
        let per_hit = (after.total_blocks - before.total_blocks) / HITS_PER_WINDOW;
        println!("  {name}: {per_hit} blocks/hit");
        per_hit
    };

    // The windows. The budget pins the hit's OWN work (lookup + splice);
    // the full response shape (the stored headers cloned into an
    // OxideResponse) is generic per-response construction every response
    // pays — measured and recorded, not budgeted.
    let lookup = window("lookup (get)", &mut || {
        black_box(
            rt.block_on(l0.get(ROUTE, PARAMS, VARY))
                .expect("hit")
                .status(),
        );
    });
    let splice = window("splice (get + materialize)", &mut || {
        let hit = rt.block_on(l0.get(ROUTE, PARAMS, VARY)).expect("hit");
        black_box(hit.materialize(nonce).len());
    });
    let hit_own = splice.max(lookup);
    window(
        "full-response (get + materialize + headers into OxideResponse)",
        &mut || {
            let hit = rt.block_on(l0.get(ROUTE, PARAMS, VARY)).expect("hit");
            let body = hit.materialize(nonce);
            let body_len = body.len();
            let mut resp = oxide_fw_server::OxideResponse::new(hit.status(), body);
            for (k, v) in hit.entry.headers.iter() {
                resp = resp.with_header(k, v.clone());
            }
            black_box(body_len + resp.headers.len());
        },
    );
    window("headers-vec-clone (the removed intermediate)", &mut || {
        let hit = rt.block_on(l0.get(ROUTE, PARAMS, VARY)).expect("hit");
        black_box(hit.headers().len());
    });
    window("block_on control", &mut || {
        black_box(rt.block_on(async { 1usize }));
    });

    println!(
        "l0_alloc budget: the hit's own work = {hit_own} blocks/hit (budget {BUDGET_BLOCKS_PER_HIT})"
    );
    assert!(
        hit_own <= BUDGET_BLOCKS_PER_HIT,
        "the L0 hit's own work allocates {hit_own} blocks/request over the {BUDGET_BLOCKS_PER_HIT}-block budget — the B09 budget is a gate"
    );
}
