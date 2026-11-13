//! TDD suite for the V2-E02 typed-search-params substrate: typegen's
//! `search` dimension. A `page.server.ts` that declares
//! `export const searchSchema` (a Standard Schema value) gets its route
//! typed through **delegated inference** — the emitted `routes.d.ts`
//! imports the schema value and references
//! `S.InferOutput<typeof searchSchema>` — instead of the raw
//! `LoaderSearchParams` record. Routes without a declaration keep the
//! record; a `function` export is not a schema value and is never marked.
//!
//! Written BEFORE the implementation; every pin drives the public
//! `typegen_with_root` surface over fixture trees on disk, plus the
//! `declares_search_schema` scanner directly.

use oxide_fw_router::{RouteEntry, RouteId, RouteKind, RouteTree};

/// A fixture project: `route` builds a route entry whose files live under
/// `root` (the entry's `file` is routes-root-relative, per the scanner's
/// contract).
fn entry_with_root(
    root: &std::path::Path,
    id: &str,
    kind: RouteKind,
    segments: &[&str],
    server_source: Option<&str>,
) -> RouteEntry {
    let rel = format!("{id}.tsx");
    let file = std::path::PathBuf::from(&rel);
    let server = file.with_file_name("page.server.ts");
    if let Some(src) = server_source {
        let p = root.join(&server);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("fixture dir");
        }
        std::fs::write(p, src).expect("fixture server module");
    }
    RouteEntry {
        id: RouteId(id.to_string()),
        kind,
        segments: segments
            .iter()
            .map(std::string::ToString::to_string)
            .collect(),
        file,
        render_mode: None,
        tags: Vec::new(),
        // The scan is the source of truth, but `hasServerModule` is what
        // the manifest carries for the entry shape; mirror the fixture.
        has_server_module: server_source.is_some(),
        slot: None,
        needs_parent: false,
        live: None,
    }
}

/// The schema-carrying source the WP's example shows (any Standard Schema
/// value typegen cannot statically infer — it delegates).
const SCHEMA_SRC: &str = "import * as s from '@oxide/schema';\nexport const searchSchema = s.object({ slug: s.string(), tag: s.optional(s.string()) });\nexport const load = () => ({});\n";

/// Build the WP's four-route tree on disk; returns (tempdir guard, tree).
fn fixture_tree() -> (tempfile::TempDir, RouteTree) {
    let root = tempfile::tempdir().expect("fixture root");
    let mut t = RouteTree::new();
    // 1. the declaring route (the WP's `/blog/[slug]` example).
    t.insert(entry_with_root(
        root.path(),
        "routes/blog/[slug]/page",
        RouteKind::Page,
        &["blog", ":slug"],
        Some(SCHEMA_SRC),
    ));
    // 2. a server module WITHOUT the declaration.
    t.insert(entry_with_root(
        root.path(),
        "routes/blog/page",
        RouteKind::Page,
        &["blog"],
        Some("export const load = () => ({});\n"),
    ));
    // 3. a `function` export — NOT a Standard Schema value.
    t.insert(entry_with_root(
        root.path(),
        "routes/docs/page",
        RouteKind::Page,
        &["docs"],
        Some("export function searchSchema() { return null; }\n"),
    ));
    // 4. no server module at all.
    t.insert(entry_with_root(
        root.path(),
        "routes/page",
        RouteKind::Page,
        &[],
        None,
    ));
    (root, t)
}

fn typegen_dts(root: &std::path::Path, t: &RouteTree) -> String {
    // The out dir lives UNDER the fixture root (the build layout: the dist
    // tree is inside the project), so the delegation import resolves
    // relative to `.oxide/types` the way it does in a real project.
    let out = root.join(".oxide-out");
    let path = t.typegen_with_root(&out, root).expect("typegen runs");
    std::fs::read_to_string(&path).expect("dts readable")
}

fn routes_json(root: &std::path::Path, t: &RouteTree) -> serde_json::Value {
    let out = root.join(".oxide-out");
    t.typegen_with_root(&out, root).expect("typegen runs");
    let raw = std::fs::read_to_string(out.join(".oxide/types/routes.json")).expect("json");
    serde_json::from_str(&raw).expect("json parses")
}

#[test]
fn search_schema_delegation_is_emitted_for_declaring_routes() {
    let (root, t) = fixture_tree();
    let dts = typegen_dts(root.path(), &t);
    assert!(
        dts.contains("import type { StandardSchemaV1 as S } from '@standard-schema/spec';"),
        "the Standard-Schema import rides the header when any route declares: {dts}"
    );
    assert!(
        dts.contains("import type { searchSchema as s_blog_slug } from '"),
        "the schema value is imported under a per-route name: {dts}"
    );
    assert!(
        dts.contains("search: S.InferOutput<typeof s_blog_slug>;"),
        "the declaring route's LoaderEvent carries the typed search: {dts}"
    );
    assert!(
        dts.contains("searchParams: LoaderSearchParams;"),
        "non-declaring routes keep the raw record"
    );
}

#[test]
fn the_delegation_import_resolves_from_the_types_dir() {
    let (root, t) = fixture_tree();
    let dts = typegen_dts(root.path(), &t);
    // `.oxide-out/.oxide/types/routes.d.ts` reaching
    // `<root>/routes/blog/[slug]/page.server.ts` is
    // `../../../routes/blog/[slug]/page.server` (extensionless).
    assert!(
        dts.contains("from '../../../routes/blog/[slug]/page.server'"),
        "the import path is extensionless and relative to the types dir: {dts}"
    );
}

#[test]
fn route_search_map_names_the_per_route_search_dimension() {
    let (root, t) = fixture_tree();
    let dts = typegen_dts(root.path(), &t);
    assert!(
        dts.contains("export type RouteSearch = {"),
        "the per-route search map is emitted"
    );
    assert!(
        dts.contains("`/blog/${string}`: S.InferOutput<typeof s_blog_slug>;"),
        "the declaring route maps to the inferred output"
    );
    assert!(
        dts.contains("\"/\": LoaderSearchParams;"),
        "routes without a declaration map to the record"
    );
    assert!(
        dts.contains("export type Search<R extends OxideRoutes> = RouteSearch[R];"),
        "the module augmentation exposes the Search accessor"
    );
}

#[test]
fn href_gains_the_search_options_overload_for_schema_routes() {
    let (root, t) = fixture_tree();
    let dts = typegen_dts(root.path(), &t);
    assert!(
        dts.contains(
            "opts?: { search?: Partial<S.InferOutput<typeof s_blog_slug>>; hash?: string }"
        ),
        "the declaring route's href carries the typed search options: {dts}"
    );
}

#[test]
fn a_function_export_is_not_a_schema_value() {
    let (root, t) = fixture_tree();
    let dts = typegen_dts(root.path(), &t);
    assert!(
        !dts.contains("s_docs"),
        "a `export function searchSchema` route is never delegated: {dts}"
    );
    let json = routes_json(root.path(), &t);
    let docs = json
        .as_array()
        .expect("rows")
        .iter()
        .find(|r| r["pattern"] == "/docs")
        .expect("the docs row");
    assert_eq!(
        docs["hasSearchSchema"],
        serde_json::json!(false),
        "the function export is not a schema value"
    );
}

#[test]
fn routes_json_marks_has_search_schema() {
    let (root, t) = fixture_tree();
    let json = routes_json(root.path(), &t);
    let blog = json
        .as_array()
        .expect("rows")
        .iter()
        .find(|r| r["pattern"] == "/blog/:slug")
        .expect("the blog row");
    assert_eq!(
        blog["hasSearchSchema"],
        serde_json::json!(true),
        "the const export is marked in the manifest"
    );
    let home = json
        .as_array()
        .expect("rows")
        .iter()
        .find(|r| r["pattern"] == "/")
        .expect("the home row");
    assert_eq!(home["hasSearchSchema"], serde_json::json!(false));
}

#[test]
fn schemaless_projects_emit_byte_compatible_output() {
    // No page.server.ts anywhere: no imports, no RouteSearch dimension —
    // the pre-E02 bytes (the compatibility contract for generated output).
    let root = tempfile::tempdir().expect("root");
    let mut t = RouteTree::new();
    t.insert(entry_with_root(
        root.path(),
        "routes/page",
        RouteKind::Page,
        &[],
        None,
    ));
    let dts = typegen_dts(root.path(), &t);
    assert!(
        !dts.contains("StandardSchemaV1"),
        "no schema import without a declaration"
    );
    assert!(
        !dts.contains("RouteSearch"),
        "no search map without a declaration"
    );
    assert!(dts.contains("export type OxideRoutes ="), "the base shape");
}

#[test]
fn declares_search_schema_is_token_safe() {
    // The scanner is public API (the CLI's docs/typegen consumers reuse
    // it); these pins hold it to the token boundary.
    assert!(oxide_fw_router::declares_search_schema(SCHEMA_SRC));
    assert!(oxide_fw_router::declares_search_schema(
        "export const searchSchema: S.StandardSchemaV1<Input> = s.object({});"
    ));
    assert!(!oxide_fw_router::declares_search_schema(
        "export function searchSchema() { return null; }"
    ));
    assert!(!oxide_fw_router::declares_search_schema(
        "export const searchSchemaV2 = s.object({});"
    ));
    assert!(!oxide_fw_router::declares_search_schema(
        "export const load = () => ({});"
    ));
    assert!(!oxide_fw_router::declares_search_schema(""));
}
