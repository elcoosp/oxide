//! Consumer walkthrough for the V2-E01 HMR boundary planner: the pure
//! substrate `cmd_dev`'s adapter-HMR fan-out is built on (the last leg of
//! the §13.3 one-hop simplification retirement — see `docs/deviations.md`).
//!
//! Shows the three surfaces a consumer (the dev loop, a plugin host, a
//! test harness) touches:
//!
//! 1. [`oxide_fw_dev::plan`] — the spec-verbatim breadth-first walk-up:
//!    stop at self-accepting boundaries on every path, fall back to
//!    [`oxide_fw_dev::Plan::FullReload`] with the actionable reason when a
//!    path reaches an entry module (or an orphan) without a boundary.
//! 2. [`oxide_fw_dev::source_self_accepts`] — the `import.meta.hot.accept`
//!    probe that feeds the `accepting` set for sources the adapter compiled.
//! 3. [`oxide_fw_dev::full_reload_diagnostic`] — the `DEV-HMR-001` warning
//!    with the miette help text the §13.4 overlay renders.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-dev --example hmr_boundary
//! RUST_LOG=oxide_fw_dev=debug cargo run -p oxide-fw-dev --example hmr_boundary
//! ```

use oxide_fw_dev::{Plan, full_reload_diagnostic, plan, source_self_accepts};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

fn p(rel: &str) -> PathBuf {
    PathBuf::from(format!("src/{rel}"))
}

fn show(title: &str, changed: &Path, graph_plan: Plan) {
    println!("{title}");
    println!("  changed: {}", changed.display());
    match &graph_plan {
        Plan::Update(boundaries) => {
            println!(
                "  plan    : update {} boundar(ies), in batch order:",
                boundaries.len()
            );
            for b in boundaries {
                println!("    - {}", b.display());
            }
        }
        Plan::FullReload(reason) => {
            println!("  plan    : full reload ({reason})");
            let diag = full_reload_diagnostic(reason);
            println!("  overlay : [{}] {}", diag.code, diag.message);
            println!("    fix: {}", diag.fix.as_deref().unwrap_or_default());
        }
    }
    println!();
}

fn main() {
    // The dev-time module graph, as `oxide-fw-cli`'s per-tick harvest builds
    // it: reverse deps, the self-accepting set (component modules + hot-accept
    // registrations + server/CSS swaps) and the route-structural entries.
    let mut importers: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    // The §13.3 gap this substrate closes: a non-boundary chain whose top
    // component importer used to keep a stale import binding.
    importers.insert(p("lib/util.ts"), vec![p("lib/util2.ts")]);
    importers.insert(p("lib/util2.ts"), vec![p("routes/index/page.tsx")]);
    // A wide fan-out: two component boundaries off one util.
    importers.insert(p("lib/widget.ts"), vec![p("comp/a.tsx"), p("comp/b.tsx")]);
    // An endpoint dispatch table: a route-structural entry.
    importers.insert(p("lib/api.ts"), vec![p("routes/api/users/route.ts")]);
    importers.insert(p("lib/orphan.ts"), vec![]);

    let accepting: HashSet<PathBuf> =
        [p("routes/index/page.tsx"), p("comp/a.tsx"), p("comp/b.tsx")]
            .into_iter()
            .collect();
    let entries: HashSet<PathBuf> = [p("routes/index/page.tsx"), p("routes/api/users/route.ts")]
        .into_iter()
        .collect();

    // 1. The transitive chain: the planner walks PAST `util2.ts` (no
    //    boundary) and stops at the page boundary.
    show(
        "1) util.ts edited (chain: util.ts <- util2.ts <- page.tsx)",
        &p("lib/util.ts"),
        plan(&p("lib/util.ts"), &importers, &accepting, &entries),
    );

    // 2. The wide fan-out: sorted, deduplicated boundary batch.
    show(
        "2) widget.ts edited (imported by comp/a.tsx + comp/b.tsx)",
        &p("lib/widget.ts"),
        plan(&p("lib/widget.ts"), &importers, &accepting, &entries),
    );

    // 3. The entry fallback: the dispatch table's identity is load-bearing.
    show(
        "3) api.ts edited (imported only by route.ts)",
        &p("lib/api.ts"),
        plan(&p("lib/api.ts"), &importers, &accepting, &entries),
    );

    // 4. The orphan: nothing imports it — the `_ =>` arm of the walk.
    show(
        "4) orphan.ts edited (no importers)",
        &p("lib/orphan.ts"),
        plan(&p("lib/orphan.ts"), &importers, &accepting, &entries),
    );

    // 5. The self-accepting scanner: what the adapter's compile step feeds
    //    into `accepting` alongside the framework plugin's component marks.
    let source = "if (import.meta.hot) { import.meta.hot.accept(mod => {}); }";
    println!(
        "5) source_self_accepts(page.tsx source with hot.accept) = {}",
        source_self_accepts(source)
    );
}
