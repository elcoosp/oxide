//! The §V2-E08 inspector module-graph search walkthrough (item 2's
//! search surface): the harvest-shaped rows wired onto the inspector,
//! the deterministic search (module path or importer edge,
//! case-insensitive), and the two HTML shapes — the full page and the
//! `?q=` filtered render the server arm serves.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-dev --example module_search
//! ```

use oxide_fw_dev::{
    RouteTableInspector, render_inspector_html, render_inspector_html_with_query,
    search_module_graph,
};

fn main() {
    // 1. The harvest shape (§V2-E08 item 2): one `(module, importers)` row
    //    per module, the CLI's `BTreeMap` walk order — the caller maps the
    //    reverse-deps records into the schema one-to-one at the dev boot
    //    (`with_module_graph`, beside the route table).
    let graph = vec![
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
    ];
    let state = RouteTableInspector::from_router_table(Vec::new())
        .with_module_graph(graph.clone())
        .state();

    // 2. The search (the `why` view): a module-name query matches the
    //    module path; a route-name query matches only through the importer
    //    edge — both answers carry the importer rows inline.
    for query in ["counter", "about", "COUNTER", "zzz"] {
        let rows = search_module_graph(&graph, query);
        println!(
            "search {query:?}: {} row(s) — {}",
            rows.len(),
            rows.iter()
                .map(|(m, i)| format!("{m} <- {}", i.join(", ")))
                .collect::<Vec<_>>()
                .join(" | ")
        );
    }

    // 3. `/__oxide?q=counter` — the filtered render the server arm serves:
    //    only the matching row, with the `k of n` note (the heading keeps
    //    the total).
    let html = render_inspector_html_with_query(&state, Some("counter"));
    println!("\n__oxide?q=counter (modules section):");
    for line in html.lines() {
        if line.contains("<h2>Modules") || line.contains("class=\"note\"") || line.contains("<tr>")
        {
            println!("  {line}");
        }
    }

    // 4. `/__oxide` — the unfiltered page renders every wired row and the
    //    search form (which a graph-less state hides — nothing wired,
    //    nothing searched, no fabrication).
    let bare = render_inspector_html(&RouteTableInspector::from_router_table(Vec::new()).state());
    println!(
        "\nunfiltered page carries the form: {} ; bare state hides it: {}",
        render_inspector_html(&state).contains("action=\"/__oxide\""),
        !bare.contains("action=\"/__oxide\""),
    );
}
