//! The typed-search-params walkthrough (V2-E02): a four-route fixture
//! project — one route declaring `export const searchSchema`, one with a
//! plain server module, one with a `function` export (not a schema
//! value), one with no server module — run through `typegen_with_root`,
//! printing the emitted delegation the way the build would.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-router --example typed_search
//! ```

use std::path::PathBuf;

use oxide_fw_router::{RouteEntry, RouteId, RouteKind, RouteTree};

/// The schema-carrying server module (any Standard Schema value — typegen
/// cannot statically infer its output, so it delegates).
const SCHEMA_SRC: &str = "import * as s from '@oxide/schema';\n\
export const searchSchema = s.object({ slug: s.string(), tag: s.optional(s.string()) });\n\
export const load = () => ({});\n";

fn main() {
    let root = std::env::temp_dir().join("oxide-typed-search-walkthrough");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("the fixture root is creatable");

    // Route 1: the declaring route (`/blog/[slug]` — the WP's example).
    write_route(
        &root,
        "routes/blog/[slug]/page",
        &["blog", ":slug"],
        Some(SCHEMA_SRC),
    );
    // Route 2: a server module WITHOUT the declaration.
    write_route(
        &root,
        "routes/blog/page",
        &["blog"],
        Some("export const load = () => ({});\n"),
    );
    // Route 3: a `function` export — NOT a Standard Schema value.
    write_route(
        &root,
        "routes/docs/page",
        &["docs"],
        Some("export function searchSchema() { return null; }\n"),
    );
    // Route 4: no server module at all.
    write_route(&root, "routes/page", &[], None);

    let mut tree = RouteTree::new();
    for id in [
        "routes/blog/[slug]/page",
        "routes/blog/page",
        "routes/docs/page",
        "routes/page",
    ] {
        let (kind, segments) = match id {
            "routes/blog/[slug]/page" => (RouteKind::Page, vec!["blog", ":slug"]),
            "routes/blog/page" => (RouteKind::Page, vec!["blog"]),
            "routes/docs/page" => (RouteKind::Page, vec!["docs"]),
            _ => (RouteKind::Page, vec![]),
        };
        tree.insert(RouteEntry {
            id: RouteId(id.to_string()),
            kind,
            segments: segments.into_iter().map(String::from).collect(),
            file: PathBuf::from(format!("{id}.tsx")),
            render_mode: None,
            tags: Vec::new(),
            has_server_module: true,
            slot: None,
            needs_parent: false,
            live: None,
        });
    }

    let out = root.join("dist");
    let dts_path = tree
        .typegen_with_root(&out, &root)
        .expect("typegen runs over the fixture tree");
    let dts = std::fs::read_to_string(&dts_path).expect("the emitted file reads back");

    println!("== the scan ==");
    println!(
        "  declaring const export : {}",
        oxide_fw_router::declares_search_schema(SCHEMA_SRC)
    );
    println!(
        "  function export        : {} (not a schema value)",
        oxide_fw_router::declares_search_schema("export function searchSchema() {{ }}")
    );

    println!("\n== the emitted header (delegation imports) ==");
    for line in dts.lines().take(4) {
        println!("  {line}");
    }

    println!("\n== the typed LoaderEvent + RouteSearch entries ==");
    for line in dts.lines() {
        if line.contains("search: S.InferOutput") || line.contains("RouteSearch") {
            println!("  {line}");
        }
    }

    println!("\n== the routes.json manifest flags ==");
    let json = std::fs::read_to_string(out.join(".oxide/types/routes.json")).expect("manifest");
    for row in serde_json::from_str::<serde_json::Value>(&json)
        .expect("manifest parses")
        .as_array()
        .expect("rows")
    {
        println!(
            "  {:<14} hasSearchSchema={}",
            row["pattern"].as_str().unwrap_or("?"),
            row["hasSearchSchema"]
        );
    }

    let _ = std::fs::remove_dir_all(&root);
    println!("\nwalkthrough complete");
}

/// Write a route's `page.server.ts` fixture (the `.tsx` file itself is
/// never read by typegen — the server module is what the scan walks).
fn write_route(root: &std::path::Path, id: &str, _segments: &[&str], server_source: Option<&str>) {
    let server = root
        .join(id)
        .parent()
        .expect("route dir")
        .join("page.server.ts");
    if let Some(src) = server_source {
        std::fs::create_dir_all(server.parent().expect("server dir")).expect("fixture dir");
        std::fs::write(server, src).expect("fixture server module");
    }
}
