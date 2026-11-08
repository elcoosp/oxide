//! V2-B09: synthetic route sets shared by the criterion benches and the
//! allocation-budget tests.
//!
//! Compiled only under `cfg(any(test, feature = "bench-util"))` so production
//! builds never pay for it. Benches and the `alloc_match` test target enable
//! the `bench-util` feature (see `benchmarks/README.md` for the commands).

#![cfg(any(test, feature = "bench-util"))]

use crate::{RouteEntry, RouteId, RouteKind};

/// Deterministic synthetic route set of `n` entries.
///
/// Every 10th route carries a `[id]` param segment (so `static` and `param`
/// URLs both have plenty of candidates at every scale); the rest are fully
/// static. Ids and segments are derived from the index, so the same `n` always
/// produces the same tree (bench comparability, V2-B09).
#[must_use]
pub fn synthetic_entries(n: usize) -> Vec<RouteEntry> {
    (0..n)
        .map(|i| {
            let segs: Vec<String> = if i % 10 == 0 {
                // Encoded param form (`:id`), matching what the scanner writes
                // into `RouteEntry.segments` (see `segment.rs`: `[param]` →
                // `:param`).
                vec![format!("s{}", i % 50), ":id".into()]
            } else {
                vec![format!("s{}", i % 50), format!("p{i}")]
            };
            RouteEntry {
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
                slot: None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RouteTree;

    #[test]
    fn synthetic_entries_are_deterministic_and_matchable() {
        let a = synthetic_entries(100);
        let b = synthetic_entries(100);
        assert_eq!(a, b, "same n must produce identical entries");
        assert_eq!(a.len(), 100);
        let mut t = RouteTree::new();
        for e in a {
            t.insert(e);
        }
        // Static route for i = 7: segments ["s7", "p7"].
        let m = t.match_path("/s7/p7").expect("static route must match");
        assert_eq!(m.route.id.0, "routes/s7/p7/page");
        // Param route for i = 0: segments ["s0", ":id"].
        let m = t.match_path("/s0/42").expect("param route must match");
        assert_eq!(m.route.id.0, "routes/s0/[id]/page");
        assert_eq!(m.params.get("id").map(String::as_str), Some("42"));
    }
}
