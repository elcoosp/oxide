//! Consumer walkthrough for the V2-E01 phase-2 CSS boundary substrate: the
//! classified planner `cmd_dev`'s style-swap fan-out is built on (the
//! deferred half of the §13.3 boundary work — see `docs/deviations.md`).
//!
//! Shows the four surfaces a consumer (the dev loop, a plugin host, a test
//! harness) touches:
//!
//! 1. [`oxide_fw_dev::boundary::plan_classified`] — the §13.3 wire kind plus
//!    the walk, with the CSS terminal applied: a stylesheet NEVER reloads.
//! 2. [`oxide_fw_dev::boundary::update_kind`] — the SPEC-14 bridge from the
//!    watcher's `ChangeKind` to the schema's `kind: 'js' | 'css' | 'data'`.
//! 3. [`oxide_fw_dev::boundary::css_self_accepts`] — the extension-level
//!    probe the harvest uses to mark stylesheets as boundaries.
//! 4. [`oxide_fw_dev::boundary::css_orphan_diagnostic`] — the `DEV-HMR-002`
//!    warning with the miette fix text for a stylesheet the graph cannot see.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-dev --example css_boundary
//! RUST_LOG=oxide_fw_dev=debug cargo run -p oxide-fw-dev --example css_boundary
//! ```

use oxide_fw_dev::ChangeKind;
use oxide_fw_dev::boundary::{
    ClassifiedPlan, UpdateKind, css_orphan_diagnostic, css_self_accepts, plan_classified,
};
use oxide_fw_dev::Plan;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

fn p(rel: &str) -> PathBuf {
    PathBuf::from(format!("src/{rel}"))
}

fn show(title: &str, changed: &Path, classified: &ClassifiedPlan) {
    println!("{title}");
    println!("  changed : {}", changed.display());
    println!("  kind    : {}", classified.kind.as_str());
    match &classified.plan {
        Plan::Update(boundaries) => {
            println!("  plan    : update {} boundar(ies), in batch order:", boundaries.len());
            for b in boundaries {
                println!("    - {}", b.display());
            }
        }
        Plan::FullReload(reason) => {
            println!("  plan    : full reload ({reason})");
        }
    }
    println!();
}

fn main() {
    // The dev-time module graph, as `oxide-fw-cli`'s per-tick harvest builds
    // it AFTER phase 2: stylesheets join the walk (`css`/`scss`/`sass`) and
    // self-accept through style injection, so a stylesheet change plans as
    // its own in-place swap.
    let mut importers: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    // A stylesheet the graph can see: `page.tsx` side-effect-imports it.
    importers.insert(p("styles/main.css"), vec![p("routes/index/page.tsx")]);
    // A component chain for the Js control.
    importers.insert(p("lib/util.ts"), vec![p("routes/index/page.tsx")]);
    // A page loader for the Data control.
    importers.insert(
        p("blog/[slug]/page.server.ts"),
        vec![p("routes/blog/[slug]/page.tsx")],
    );

    let accepting: HashSet<PathBuf> = [
        p("routes/index/page.tsx"),
        p("styles/main.css"),
    ]
    .into_iter()
    .collect();
    let entries: HashSet<PathBuf> = HashSet::new();

    // 1. The §13.3 CSS terminal, orphan shape: the stylesheet below has NO
    //    importers row — the plain walk reports a reload, and the terminal
    //    collapses it to the swap. A stylesheet never reloads; the orphan
    //    state is a DEV-HMR-002 warning, not user-visible noise.
    let orphan = p("styles/orphan.css");
    let classified = plan_classified(ChangeKind::Css, &orphan, &importers, &accepting, &entries);
    show("1) styles/orphan.css edited (nothing imports it)", &orphan, &classified);
    let diag = css_orphan_diagnostic(&orphan);
    println!("  overlay : [{}] {}", diag.code, diag.message);
    println!("    fix: {}", diag.fix.as_deref().unwrap_or_default());
    println!();

    // 2. The edge shape: the stylesheet is in the accepting set (the
    //    harvest's `css_self_accepts` mark), so the walk stops at the changed
    //    file itself — the same swap, no collapse needed.
    let edge = p("styles/main.css");
    let classified = plan_classified(ChangeKind::Css, &edge, &importers, &accepting, &entries);
    show("2) styles/main.css edited (imported by page.tsx)", &edge, &classified);

    // 3. The Js control: a component change passes through the walk
    //    unchanged — the classified planner is a pure composition.
    let js = p("lib/util.ts");
    let classified = plan_classified(ChangeKind::Component, &js, &importers, &accepting, &entries);
    show("3) lib/util.ts edited (imported by page.tsx)", &js, &classified);

    // 4. The Data control: a page loader change classifies `kind: 'data'`
    //    (the OxData / data-entry track); the walk stays available for consumers
    //    that fan data updates out through boundaries.
    let data = p("blog/[slug]/page.server.ts");
    let classified =
        plan_classified(ChangeKind::ServerLoader, &data, &importers, &accepting, &entries);
    show("4) blog/[slug]/page.server.ts edited", &data, &classified);

    // 5. The probe and the wire spellings: what the harvest feeds into
    //    `accepting`, and the exact strings the schema v1 `kind` field
    //    carries to the client.
    println!("5) css_self_accepts probes");
    for candidate in ["src/styles/main.css", "src/routes/index/page.tsx", "src/lib/util.ts"] {
        println!(
            "   css_self_accepts({candidate}) = {}",
            css_self_accepts(Path::new(candidate))
        );
    }
    println!(
        "   wire spellings: Js={} Css={} Data={}",
        UpdateKind::Js.as_str(),
        UpdateKind::Css.as_str(),
        UpdateKind::Data.as_str()
    );
}
