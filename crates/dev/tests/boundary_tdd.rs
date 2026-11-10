//! TDD pins for the V2-E01 HMR boundary planner (spec §V2-E01, §13.3) and
//! its phase-2 CSS boundary substrate.
//!
//! These tests are the spec-verbatim contract of `oxide_fw_dev::plan` and
//! its diagnostics seam. They exercise ONLY the public API of `oxide-fw-dev`
//! so the substrate can land before any consumer is wired: the algorithm is
//! the pure breadth-first walk-up over `importers` that stops at
//! self-accepting boundaries and falls back to a `FullReload` with an
//! actionable reason when a path reaches an entry module (or an orphan)
//! without one. The phase-2 section pins the CSS terminal: a stylesheet
//! classifies `kind: 'css'` and never reloads (§13.3 — the style swap),
//! whatever the graph beneath it says.

use oxide_fw_dev::boundary::{
    ClassifiedPlan, UpdateKind, css_orphan_diagnostic, css_self_accepts, plan_classified,
    update_kind,
};
use oxide_fw_dev::{ChangeKind, Plan, full_reload_diagnostic, plan, source_self_accepts};
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

// ---------------- §V2-E01 phase 2: the CSS boundary substrate ----------------

#[test]
fn update_kind_wire_spellings_are_the_spec_verbatim_kinds() {
    // The message schema's `kind` discriminant (§13.3): the TS client
    // switches on these exact strings, so the spellings are load-bearing.
    assert_eq!(UpdateKind::Js.as_str(), "js");
    assert_eq!(UpdateKind::Css.as_str(), "css");
    assert_eq!(UpdateKind::Data.as_str(), "data");
}

#[test]
fn the_watcher_change_kind_maps_onto_the_wire_kind() {
    // SPEC-14's `ChangeKind` is the watcher's classification; the wire kind
    // is the schema's. A page.server.ts loader change is a data update, a
    // stylesheet is css, everything else is a module update.
    assert_eq!(update_kind(ChangeKind::Css), UpdateKind::Css);
    assert_eq!(update_kind(ChangeKind::ServerLoader), UpdateKind::Data);
    assert_eq!(update_kind(ChangeKind::Component), UpdateKind::Js);
    assert_eq!(update_kind(ChangeKind::ServerModule), UpdateKind::Js);
    assert_eq!(update_kind(ChangeKind::ConfigOrRoutes), UpdateKind::Js);
}

#[test]
fn a_stylesheet_change_is_always_an_in_place_swap_even_orphaned() {
    // The §13.3 CSS terminal: an orphaned stylesheet (no importers row, no
    // accepting mark) produces a `FullReload` walk — and `plan_classified`
    // collapses it to `Update([changed])`. A stylesheet never reloads.
    let (imps, acc, ent) = graph(&[], &[], &[]);
    let changed = p("styles/main.css");
    let classified = plan_classified(ChangeKind::Css, &changed, &imps, &acc, &ent);
    assert_eq!(classified.kind, UpdateKind::Css);
    assert!(!classified.is_full_reload());
    assert_eq!(classified.plan, Plan::Update(vec![p("styles/main.css")]));
}

#[test]
fn a_stylesheet_in_the_accepting_set_plans_as_its_own_boundary() {
    // Post-harvest shape (phase 2 walks stylesheets too): the stylesheet is
    // marked self-accepting by `css_self_accepts`, so the walk stops at the
    // changed file itself — the same in-place swap, no collapse needed.
    let (imps, acc, ent) = graph(
        &[("styles/main.css", &["routes/index/page.tsx"])],
        &["styles/main.css"],
        &[],
    );
    let classified = plan_classified(ChangeKind::Css, &p("styles/main.css"), &imps, &acc, &ent);
    assert_eq!(classified.kind, UpdateKind::Css);
    assert_eq!(classified.plan, Plan::Update(vec![p("styles/main.css")]));
}

#[test]
fn a_component_change_through_the_classified_planner_keeps_the_walk() {
    // The Js kind is a pass-through: `plan_classified` must not alter the
    // boundary walk the plain `plan` produces for a component change.
    let (imps, acc, ent) = graph(
        &[("lib/util.ts", &["routes/index/page.tsx"])],
        &["routes/index/page.tsx"],
        &[],
    );
    let classified = plan_classified(ChangeKind::Component, &p("lib/util.ts"), &imps, &acc, &ent);
    assert_eq!(classified.kind, UpdateKind::Js);
    assert_eq!(
        classified,
        ClassifiedPlan {
            kind: UpdateKind::Js,
            plan: plan(&p("lib/util.ts"), &imps, &acc, &ent),
        }
    );
}

#[test]
fn a_loader_change_classifies_as_data_and_walks_normally() {
    // `ServerLoader` → `kind: 'data'` (the OxData / data-entry track); the walk
    // itself is unchanged — the planner still computes the surface for
    // consumers that fan out data updates through boundaries.
    let (imps, acc, ent) = graph(
        &[("blog/[slug]/page.server.ts", &["routes/blog/[slug]/page.tsx"])],
        &["routes/blog/[slug]/page.tsx"],
        &[],
    );
    let classified = plan_classified(
        ChangeKind::ServerLoader,
        &p("blog/[slug]/page.server.ts"),
        &imps,
        &acc,
        &ent,
    );
    assert_eq!(classified.kind, UpdateKind::Data);
    assert!(!classified.is_full_reload());
}

#[test]
fn css_self_accepts_covers_the_style_extensions_only() {
    // The probe is extension-level on purpose (the planner runs per watcher
    // tick and must not parse). Case-insensitive on the extension.
    assert!(css_self_accepts(Path::new("src/styles/main.css")));
    assert!(css_self_accepts(Path::new("src/styles/app.SCSS")));
    assert!(css_self_accepts(Path::new("src/styles/app.sass")));
    assert!(!css_self_accepts(Path::new("src/routes/index/page.tsx")));
    assert!(!css_self_accepts(Path::new("src/lib/util.ts")));
    assert!(!css_self_accepts(Path::new("src/app.server.ts")));
    assert!(!css_self_accepts(Path::new("src/styles/main")));
}

#[test]
fn css_orphan_diagnostic_is_a_warning_with_the_dev_hmr_002_code() {
    // The orphan-stylesheet warning: the swap still runs (a static `<link>`
    // may carry the sheet), but the graph has no edge for it — the dev
    // should know the fan-out will never include it.
    let diag = css_orphan_diagnostic(Path::new("src/styles/main.css"));
    assert_eq!(diag.code, "DEV-HMR-002");
    assert_eq!(diag.severity, oxide_fw_diagnostics::Severity::Warning);
    assert!(diag.message.contains("is not imported by any module"));
    assert!(diag.fix.as_deref().unwrap_or_default().contains("<link>"));
}

#[test]
fn plan_classified_is_deterministic_across_calls() {
    // Same graph, same answer — the classified plan inherits the planner's
    // determinism (sorted, deduplicated boundaries).
    let (imps, acc, ent) = graph(&[], &[], &[]);
    let a = plan_classified(ChangeKind::Css, &p("styles/main.css"), &imps, &acc, &ent);
    let b = plan_classified(ChangeKind::Css, &p("styles/main.css"), &imps, &acc, &ent);
    assert_eq!(a, b);
}
