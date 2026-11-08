//! V2-B02 allocation budget (spec V2-B09 step 3): matching a static route on a
//! 1,000-route tree must allocate **zero** heap blocks.
//!
//! Own test binary because dhat allows one profiler per process, and one test
//! function because concurrent test threads would allocate inside the
//! snapshot windows. This is a plain integration test (no feature gate) so
//! CI's default `cargo test` runs it; the synthetic generator is duplicated
//! here instead of reusing `testing::synthetic_entries`, which is behind
//! `bench-util`.

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

use oxide_fw_router::{RouteEntry, RouteId, RouteKind, RouteTree};
use std::hint::black_box;

/// Deterministic 1,000-route tree (same shape as `testing::synthetic_entries`).
fn synthetic_tree(n: usize) -> RouteTree {
    let mut t = RouteTree::new();
    for i in 0..n {
        let segs: Vec<String> = if i % 10 == 0 {
            vec![format!("s{}", i % 50), ":id".into()]
        } else {
            vec![format!("s{}", i % 50), format!("p{i}")]
        };
        t.insert(RouteEntry {
            id: RouteId(format!(
                "routes/{}/page",
                segs.join("/").replace(":id", "[id]")
            )),
            kind: RouteKind::Page,
            segments: segs,
            file: std::path::PathBuf::from("page.tsx"),
            render_mode: None,
            tags: vec![],
            has_server_module: false,
            needs_parent: false,
            live: None,
            slot: None,
        });
    }
    t
}

/// Heap blocks allocated across 1,000 `f()` calls (snapshot delta).
fn blocks_for(mut f: impl FnMut()) -> u64 {
    let before = dhat::HeapStats::get();
    f();
    let after = dhat::HeapStats::get();
    after.total_blocks - before.total_blocks
}

#[test]
fn match_ref_allocation_budgets() {
    let _prof = dhat::Profiler::builder().testing().build();

    // Warm the derived caches (candidate index) — setup allocations are fine;
    // only the steady-state match loop is budgeted.
    let tree = synthetic_tree(1000);
    assert!(tree.match_ref("/s7/p7").is_some(), "fixture must match");
    assert!(
        tree.match_ref("/s0/42").is_some(),
        "param fixture must match"
    );
    assert!(
        tree.match_ref("/zzz/nothing").is_none(),
        "miss fixture must miss"
    );

    // Static hit: zero allocations (spec §8.2 / V2-B02).
    let blocks = blocks_for(|| {
        for _ in 0..1000 {
            let _ = black_box(tree.match_ref(black_box("/s7/p7")));
        }
    });
    dhat::assert_eq!(blocks, 0, "static match must not allocate");

    // Param hit with borrowed captures (no '%' in the URL): still zero.
    // NOTE: the budget applies while the candidate list fits the inline
    // SmallVec (16 entries). The 1,000-route fixture fans out 20 param
    // candidates per first segment (`s0`, `s10`, ...), so the param budget
    // uses a smaller tree whose fan-out (2) stays inline.
    let small = synthetic_tree(100);
    assert!(
        small.match_ref("/s0/42").is_some(),
        "param fixture must match"
    );
    let blocks = blocks_for(|| {
        for _ in 0..1000 {
            let _ = black_box(small.match_ref(black_box("/s0/42")));
        }
    });
    dhat::assert_eq!(blocks, 0, "borrowed param match must not allocate");

    // Miss walks the index and finds no candidate: zero (owned trees trust
    // the index — no linear fallback).
    let blocks = blocks_for(|| {
        for _ in 0..1000 {
            let _ = black_box(tree.match_ref(black_box("/zzz/nothing")));
        }
    });
    dhat::assert_eq!(blocks, 0, "miss must not allocate");
}
