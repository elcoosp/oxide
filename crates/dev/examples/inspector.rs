//! The §V2-E08 inspector walkthrough: the live-subscriber (F05) and
//! generation (F10) tables riding the `/__oxide` state — the schema the
//! dev crate carries, the rows the callers map from the server hub's
//! `tag_snapshot` and the dispatch substrate's `GenerationSet::snapshot`,
//! and the HTML the pipeline renders.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-dev --example inspector
//! ```

use oxide_fw_dev::{GenerationRow, LiveRow, RouteTableInspector, render_inspector_html};

fn main() {
    // 1. The boot-shaped route table (§8.3 typegen output — the shape
    //    `from_router_table` takes in the serve wiring).
    let routes = vec![
        ("/".to_string(), "static".to_string(), Vec::new()),
        (
            "/blog/[slug]".to_string(),
            "dynamic".to_string(),
            vec!["no-cache".to_string()],
        ),
    ];

    // 2. The live-subscriber rows (F05): the caller maps the server hub's
    //    `LiveTagCount` records one-to-one — an exact tag, a wildcard
    //    pool, sorted by key.
    let live = vec![
        LiveRow {
            tag: "post:7".to_string(),
            wildcard: false,
            subscribers: 1,
        },
        LiveRow {
            tag: "comments:".to_string(),
            wildcard: true,
            subscribers: 16,
        },
    ];

    // 3. The generation rows (F10): the caller maps the dispatch
    //    substrate's `GenerationInfo` records — the current build always,
    //    the demoted previous while its skew window is open (30s left).
    let generations = vec![
        GenerationRow {
            id: "deadbeef0000000f".to_string(),
            role: "current".to_string(),
            window_ms_remaining: None,
        },
        GenerationRow {
            id: "deadbeef0000000e".to_string(),
            role: "previous".to_string(),
            window_ms_remaining: Some(30_000),
        },
    ];

    let inspector = RouteTableInspector::from_router_table(routes)
        .with_live(live)
        .with_generations(generations);
    let state = inspector.state();

    // 4. `/__oxide/state` — the JSON the pipeline serves from the
    //    injected `InspectorSnapshot` (live + generations ride the same
    //    record; a pre-E08 consumer that ignores unknown keys is
    //    unaffected).
    println!(
        "__oxide/state:\n{}",
        serde_json::to_string_pretty(&state).unwrap_or_default()
    );

    // 5. `/__oxide` — the self-contained HTML (the two new tables render
    //    with counts and escaped cells; the em dash marks the current
    //    generation's absent window).
    println!("\n__oxide html (tables only):\n");
    for line in render_inspector_html(&state).lines() {
        if line.contains("<h2>") || line.contains("<tr>") {
            println!("{line}");
        }
    }

    // 6. The no-source default stays honest: nothing wired, nothing
    //    rendered — no fabricated rows.
    let bare = RouteTableInspector::from_router_table(Vec::new()).state();
    println!(
        "\nbare state: live={} generations={}",
        bare.live.len(),
        bare.generations.len()
    );
}
