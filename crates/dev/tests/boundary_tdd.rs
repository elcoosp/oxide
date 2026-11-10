//! TDD pins for the V2-E01 HMR boundary planner (spec §V2-E01, §13.3).
//!
//! These tests are the spec-verbatim contract of `oxide_fw_dev::plan` and
//! its diagnostics seam. They exercise ONLY the public API of `oxide-fw-dev`
//! so the substrate can land before any consumer is wired: the algorithm is
//! the pure breadth-first walk-up over `importers` that stops at
//! self-accepting boundaries and falls back to a `FullReload` with an
//! actionable reason when a path reaches an entry module (or an orphan)
//! without one.

use oxide_fw_dev::{Plan, full_reload_diagnostic, plan, source_self_accepts};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Shorthand: `src/`-rooted path builders so the pins read like the dev
/// tree they model.
fn p(rel: &str) -> PathBuf {
    PathBuf::from(format!("src/{rel}"))
}

/// A graph with no boundaries at all (the degenerate default).
fn graph(
    importers: &[(&str, &[&str])],
    accepting: &[&str],
    entries: &[&str],
) -> (
    HashMap<PathBuf, Vec<PathBuf>>,
    HashSet<PathBuf>,
    HashSet<PathBuf>,
) {
    let mut imps: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    for (module, targets) in importers {
        imps.insert(
            p(module),
            targets.iter().map(|t| p(t)).collect::<Vec<_>>(),
        );
    }
    let acc = accepting.iter().map(|a| p(a)).collect::<HashSet<_>>();
    let ent = entries.iter().map(|e| p(e)).collect::<HashSet<_>>();
    (imps, acc, ent)
}

// ---------------- §V2-E01: the boundary walk ----------------

#[test]
fn changed_module_that_self_accepts_is_its_own_boundary() {
    // `import.meta.hot.accept` marks the changed module itself; the walk
    // stops immediately and the update set is exactly the changed file.
    let (imps, acc, ent) = graph(
        &[("islands/counter.tsx", &["routes/index/page.tsx"])],
        &["islands/counter.tsx"],
        &[],
    );
    let plan = plan(&p("islands/counter.tsx"), &imps, &acc, &ent);
    assert_eq!(plan, Plan::Update(vec![p("islands/counter.tsx")]));
}

#[test]
fn walk_stops_at_the_first_self_accepting_importer() {
    // util.ts ← helper.ts (self-accepting): the boundary is helper.ts; the
    // changed util is NOT in the update set (the boundary's re-import
    // re-fetches it).
    let (imps, acc, ent) = graph(
        &[("lib/util.ts", &["lib/helper.ts"])],
        &["lib/helper.ts"],
        &[],
    );
    let plan = plan(&p("lib/util.ts"), &imps, &acc, &ent);
    assert_eq!(plan, Plan::Update(vec![p("lib/helper.ts")]));
}

#[test]
fn walk_continues_through_non_boundary_importers() {
    // The §13.3 gap this substrate closes: util.ts ← util2.ts (no boundary)
    // ← page.tsx (component boundary). The one-hop simplification stopped at
    // util2 and left the page's import binding stale; the walk-up reaches
    // the boundary past it.
    let (imps, acc, ent) = graph(
        &[
            ("lib/util.ts", &["lib/util2.ts"]),
            ("lib/util2.ts", &["routes/index/page.tsx"]),
        ],
        &["routes/index/page.tsx"],
        &[],
    );
    let plan = plan(&p("lib/util.ts"), &imps, &acc, &ent);
    assert_eq!(plan, Plan::Update(vec![p("routes/index/page.tsx")]));
}

#[test]
fn multiple_boundaries_are_sorted_and_deduped() {
    // A diamond: util.ts ← a.tsx / b.tsx (both boundaries), and both also
    // reachable through a shared mid module. The update set is sorted by
    // path with duplicates collapsed — the deterministic batch order the
    // §3.2 coalescing publishes.
    let (imps, acc, ent) = graph(
        &[
            ("lib/util.ts", &["comp/a.tsx", "comp/b.tsx"]),
            ("comp/a.tsx", &["routes/index/page.tsx"]),
            ("comp/b.tsx", &["routes/index/page.tsx"]),
        ],
        &["comp/a.tsx", "comp/b.tsx", "routes/index/page.tsx"],
        &["routes/index/page.tsx"],
    );
    let plan = plan(&p("lib/util.ts"), &imps, &acc, &ent);
    assert_eq!(
        plan,
        Plan::Update(vec![p("comp/a.tsx"), p("comp/b.tsx")])
    );
}

#[test]
fn entry_module_reached_without_a_boundary_is_a_full_reload() {
    // util.ts ← route.ts (a route-structural entry, not self-accepting):
    // the dispatch table's identity is load-bearing, so the walk reports a
    // full reload with the spec's reason spelling.
    let (imps, acc, ent) = graph(
        &[("lib/util.ts", &["routes/api/users/route.ts"])],
        &[],
        &["routes/api/users/route.ts"],
    );
    let plan = plan(&p("lib/util.ts"), &imps, &acc, &ent);
    assert_eq!(
        plan,
        Plan::FullReload(format!("{} has no HMR boundary", p("routes/api/users/route.ts").display()))
    );
}

#[test]
fn an_orphan_module_is_a_full_reload_not_imported() {
    // A changed file nothing imports cannot reach any client; the spec's
    // `_ =>` arm reports it rather than silently updating nothing.
    let (imps, acc, ent) = graph(&[], &[], &[]);
    let plan = plan(&p("lib/orphan.ts"), &imps, &acc, &ent);
    assert_eq!(
        plan,
        Plan::FullReload(format!("{} is not imported", p("lib/orphan.ts").display()))
    );
}

#[test]
fn an_import_cycle_without_a_boundary_yields_an_empty_update() {
    // a.tsx ⇄ b.tsx, neither self-accepting, neither an entry: the visited
    // set terminates the walk and the (empty) boundary set is returned —
    // the cycle-safe outcome the spec's BFS produces. The CLI seam never
    // routes a boundary-less empty plan to the wire (see docs/deviations.md).
    let (imps, acc, ent) = graph(
        &[("comp/a.tsx", &["comp/b.tsx"]), ("comp/b.tsx", &["comp/a.tsx"])],
        &[],
        &[],
    );
    let plan = plan(&p("comp/a.tsx"), &imps, &acc, &ent);
    assert_eq!(plan, Plan::Update(Vec::new()));
}

#[test]
fn an_entry_that_is_also_self_accepting_stops_as_a_boundary() {
    // page.tsx is route-structural AND a component boundary: the boundary
    // check runs first (§V2-E01), so the page updates in place instead of
    // reloading.
    let (imps, acc, ent) = graph(
        &[("lib/util.ts", &["routes/index/page.tsx"])],
        &["routes/index/page.tsx"],
        &["routes/index/page.tsx"],
    );
    let plan = plan(&p("lib/util.ts"), &imps, &acc, &ent);
    assert_eq!(plan, Plan::Update(vec![p("routes/index/page.tsx")]));
}

#[test]
fn plan_is_deterministic_regardless_of_importer_order() {
    // The same diamond walked from two different HashMap iteration orders
    // must produce byte-identical plans — the planner sorts before it
    // returns (iteration over the map itself is unordered).
    let (imps, acc, ent) = graph(
        &[
            ("lib/util.ts", &["comp/b.tsx", "comp/a.tsx"]),
            ("comp/a.tsx", &["routes/index/page.tsx"]),
            ("comp/b.tsx", &["routes/index/page.tsx"]),
        ],
        &["comp/a.tsx", "comp/b.tsx", "routes/index/page.tsx"],
        &["routes/index/page.tsx"],
    );
    let a = plan(&p("lib/util.ts"), &imps, &acc, &ent);
    let b = plan(&p("lib/util.ts"), &imps, &acc, &ent);
    assert_eq!(a, b);
    assert_eq!(a, Plan::Update(vec![p("comp/a.tsx"), p("comp/b.tsx")]));
}

// ---------------- §V2-E01: the self-accepting scanner ----------------

#[test]
fn source_self_accepts_recognizes_the_hot_accept_registration() {
    assert!(source_self_accepts(
        "if (import.meta.hot) { import.meta.hot.accept(mod => {}); }"
    ));
    // A source without the registration probe is not self-accepting — the
    // scan is a substring probe, so the negative must genuinely lack the
    // token (a commented-out call still matches; the harvest may only feed
    // sources the adapter actually compiled).
    assert!(!source_self_accepts("import.meta.env.MODE;"));
    assert!(!source_self_accepts(""));
}

// ---------------- §V2-E01: the reload diagnostics seam ----------------

#[test]
fn full_reload_diagnostic_carries_code_and_fix_help() {
    let diag = full_reload_diagnostic("src/routes/api/users/route.ts has no HMR boundary");
    assert_eq!(diag.message, "HMR fell back to a full reload: src/routes/api/users/route.ts has no HMR boundary");
    assert!(diag.fix.as_deref().unwrap_or_default().contains("import.meta.hot.accept"));
}

#[test]
fn changed_path_types_flow_through_plan_unchanged() {
    // The planner is Path-native: the CLI hands it canonicalized PathBufs
    // and gets PathBufs back (no lossy string round-trips at the seam).
    let (imps, acc, ent) = graph(
        &[("lib/util.ts", &["lib/helper.ts"])],
        &["lib/helper.ts"],
        &[],
    );
    let changed: &Path = Path::new("src/lib/util.ts");
    let plan = plan(changed, &imps, &acc, &ent);
    assert_eq!(
        plan,
        Plan::Update(vec![PathBuf::from("src/lib/helper.ts")])
    );
}
