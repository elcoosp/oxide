//! TDD pins for the §V2-E08 inspector module-graph search (item 2's last
//! missing inspector surface): the graph wire-up on
//! [`RouteTableInspector`], the deterministic [`search_module_graph`]
//! match, and the two HTML shapes (the full page and the `?q=` filtered
//! render the server's `/__oxide?q=…` arm serves).
//!
//! The contract (§V2-E08 item 2 — "module graph search and `why`"):
//! - the caller maps the harvest's reverse-deps records (module →
//!   importers) into the state one-to-one — the dev crate carries the
//!   schema and the search, never the harvest (the E08 convention: the
//!   producers return their own records, the mapping is the caller's);
//! - a search matches the module path OR any importer path,
//!   case-insensitively, as a substring; a blank query is every row;
//! - the page renders the modules table only from wired rows — nothing
//!   wired, nothing rendered, no fabricated rows (the live/generation
//!   honesty rule).

use oxide_fw_dev::{
    InspectorState, RouteTableInspector, render_inspector_html, render_inspector_html_with_query,
    search_module_graph,
};

/// The harvest-shaped graph (module → importers), sorted as the CLI's
/// `BTreeMap` walk produces it.
fn graph() -> Vec<(String, Vec<String>)> {
    vec![
        (
            "src/app.tsx".to_string(),
            vec!["src/routes/index/page.tsx".to_string()],
        ),
        (
            "src/components/counter.tsx".to_string(),
            vec![
                "src/app.tsx".to_string(),
                "src/routes/about/page.tsx".to_string(),
            ],
        ),
        (
            "src/styles.css".to_string(),
            vec!["src/app.tsx".to_string()],
        ),
    ]
}

// ------------------------------------------------------------- the wire-up

#[test]
fn with_module_graph_wires_the_rows_into_the_state() {
    let inspector = RouteTableInspector::from_router_table(Vec::new()).with_module_graph(graph());
    let state = inspector.state();
    assert_eq!(state.module_graph.len(), 3);
    assert_eq!(state.module_graph[0].0, "src/app.tsx");
    assert_eq!(state.module_graph[1].1.len(), 2);
}

#[test]
fn the_default_state_stays_honest_with_no_graph() {
    // Nothing wired, nothing rendered — the schema's pre-E08 shape is
    // unchanged (the `#[serde(default)]`-era consumers keep parsing).
    let state = RouteTableInspector::from_router_table(Vec::new()).state();
    assert!(state.module_graph.is_empty());
    let html = render_inspector_html(&state);
    assert!(html.contains("Modules (0)"), "{html}");
}

// ---------------------------------------------------------------- search

#[test]
fn search_matches_the_module_path_substring() {
    let g = graph();
    let rows = search_module_graph(&g, "counter");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "src/components/counter.tsx");
}

#[test]
fn search_matches_an_importer_path_substring() {
    // `about` names no module — it matches only through the importer edge,
    // which IS the `why` view (who pulls this graph node in).
    let g = graph();
    let rows = search_module_graph(&g, "about");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "src/components/counter.tsx");
    assert!(rows[0].1.iter().any(|i| i.contains("about")));
}

#[test]
fn search_is_case_insensitive() {
    let g = graph();
    let rows = search_module_graph(&g, "COUNTER");
    assert_eq!(rows.len(), 1);
    let rows = search_module_graph(&g, "STYLES");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "src/styles.css");
}

#[test]
fn a_blank_query_is_every_row_and_a_miss_is_no_rows() {
    let g = graph();
    assert_eq!(search_module_graph(&g, "").len(), 3);
    assert_eq!(search_module_graph(&g, "   ").len(), 3);
    assert!(search_module_graph(&g, "zzz-no-such-module").is_empty());
}

#[test]
fn search_preserves_the_sorted_input_order() {
    // The harvest feeds a BTreeMap walk; the search never re-sorts — the
    // caller's order is the render's order, so the page is deterministic.
    let g = graph();
    let rows = search_module_graph(&g, "src/");
    let ids: Vec<&str> = rows.iter().map(|(m, _)| m.as_str()).collect();
    assert_eq!(
        ids,
        [
            "src/app.tsx",
            "src/components/counter.tsx",
            "src/styles.css"
        ]
    );
}

#[test]
fn search_over_an_empty_graph_is_empty() {
    assert!(search_module_graph(&[], "anything").is_empty());
}

// ------------------------------------------------------------------ html

#[test]
fn html_renders_the_modules_table_with_counts_and_escape() {
    let mut g = graph();
    // An HTML-bearing module path: the cell escapes, the raw spelling never
    // rides the page.
    g[2].0 = "src/weird<b>.css".to_string();
    let state = RouteTableInspector::from_router_table(Vec::new())
        .with_module_graph(g)
        .state();
    let html = render_inspector_html(&state);
    assert!(html.contains("<h2>Modules (3)</h2>"), "{html}");
    assert!(html.contains("src/app.tsx"), "{html}");
    assert!(html.contains("&lt;b&gt;"), "the malicious module escaped");
    assert!(!html.contains("<b>"), "no raw tag rode the page");
    // The importer edges render inline — the row IS the why answer.
    assert!(html.contains("src/routes/index/page.tsx"), "{html}");
    // The search form targets the same endpoint the arm serves.
    assert!(html.contains("action=\"/__oxide\""), "{html}");
    assert!(html.contains("name=\"q\""), "{html}");
}

#[test]
fn html_with_a_query_renders_only_the_matching_rows_and_the_count_note() {
    let state = RouteTableInspector::from_router_table(Vec::new())
        .with_module_graph(graph())
        .state();
    let html = render_inspector_html_with_query(&state, Some("counter"));
    assert!(
        html.contains("Modules (3)"),
        "the heading keeps the total: {html}"
    );
    assert!(html.contains("src/components/counter.tsx"), "{html}");
    assert!(
        !html.contains("src/styles.css"),
        "the non-match left the table"
    );
    assert!(html.contains("1 of 3"), "the k-of-n note: {html}");
    assert!(html.contains("counter"), "the query echoed into the note");
}

#[test]
fn html_without_a_query_renders_every_module_row() {
    let state = RouteTableInspector::from_router_table(Vec::new())
        .with_module_graph(graph())
        .state();
    let html = render_inspector_html_with_query(&state, None);
    assert!(html.contains("src/app.tsx"), "{html}");
    assert!(html.contains("src/components/counter.tsx"), "{html}");
    assert!(html.contains("src/styles.css"), "{html}");
    assert!(!html.contains(" of 3"), "no k-of-n note without a query");
}

#[test]
fn html_treats_a_blank_query_as_no_query() {
    // `?q=` (an empty submit) renders the unfiltered page — a blank search
    // is not a filtered one.
    let state = RouteTableInspector::from_router_table(Vec::new())
        .with_module_graph(graph())
        .state();
    let html = render_inspector_html_with_query(&state, Some("   "));
    assert!(html.contains("src/styles.css"), "{html}");
    assert!(!html.contains(" of 3"), "no k-of-n note for a blank query");
}

#[test]
fn render_inspector_html_is_the_no_query_render() {
    let state = RouteTableInspector::from_router_table(Vec::new())
        .with_module_graph(graph())
        .state();
    assert_eq!(
        render_inspector_html(&state),
        render_inspector_html_with_query(&state, None),
    );
}

#[test]
fn the_searched_state_shape_is_unchanged_on_the_wire() {
    // The graph rides the SAME `InspectorState` JSON (the field the schema
    // carried since §13.4) — a search changes the HTML view, never the
    // `/__oxide/state` contract.
    let state: InspectorState = serde_json::from_str(
        "{\"module_graph\":[[\"src/a.tsx\",[\"src/b.tsx\"]]],\"why\":{},\"routes\":[],\
         \"cache\":[],\"waterfalls\":[],\"pool\":{\"workers\":0,\"in_flight\":0,\"recycles\":0}}",
    )
    .expect("the pre-E08 wire shape");
    assert_eq!(state.module_graph.len(), 1);
    let html = render_inspector_html_with_query(&state, Some("b.tsx"));
    assert!(html.contains("1 of 1"), "{html}");
}
