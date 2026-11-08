//! V2-B01: `RouteContextTable` — every answer the dispatcher needs that is a
//! pure function of the matched route id, computed once at boot (D22).
//!
//! Per request the old dispatch path scanned all entries for middleware,
//! error and layout modules, and ran `.filter(|p| p.is_file())` per layout —
//! a `stat` syscall per layout per request. This table replaces those scans
//! with one `HashMap` lookup (consumers: `dispatch`, `prerender`).

use crate::{RouteEntry, RouteId, RouteKind};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// A layout's server-data module (`layout.server.ts`), precomputed §9.2.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutLoad {
    pub id: RouteId,
    pub server_file: PathBuf,
    pub needs_parent: bool,
}

/// Everything the dispatcher needs for one route id.
#[derive(Debug, Clone, PartialEq)]
pub struct RouteContext {
    pub route_id: RouteId,
    /// Precomputed metrics label (replaces the per-request `pattern_of`).
    pub pattern: Arc<str>,
    /// Layout chain ids, outer to inner (same order as `matcher::layout_chain`).
    pub layout_chain: Vec<RouteId>,
    /// Layouts that have `layout.server.ts` (flag-driven, no `stat` call).
    pub layout_loads: Vec<LayoutLoad>,
    /// Middleware module files, outer to inner.
    pub middleware_chain: Vec<PathBuf>,
    /// Innermost `error.tsx` route id.
    pub error_route: Option<RouteId>,
    /// Innermost `loading.tsx` route id.
    pub loading_route: Option<RouteId>,
}

/// One scoped module slot per directory base.
#[derive(Default)]
struct Scoped<'a> {
    layout: Option<&'a RouteEntry>,
    middleware: Option<&'a RouteEntry>,
    error: Option<&'a RouteEntry>,
    loading: Option<&'a RouteEntry>,
}

/// Precomputed per-route dispatch answers (D22). Build once at boot from the
/// route tree; lookups are `HashMap` hits.
pub struct RouteContextTable {
    by_id: HashMap<String, Arc<RouteContext>>,
    unmatched: Option<Arc<RouteContext>>,
}

/// Directory prefixes of a route id at path boundaries, outermost first, always
/// starting with the empty base: `routes/blog/page` gives `""`, `routes`,
/// `routes/blog`, `routes/blog/page`.
fn prefixes(id: &str) -> Vec<&str> {
    let mut v = vec![""];
    for (i, c) in id.char_indices() {
        if c == '/' {
            v.push(&id[..i]);
        }
    }
    v.push(id);
    v
}

/// The base of a scoped module id: `routes/blog/layout` → `routes/blog`.
#[must_use]
fn base<'a>(id: &'a str, suffix: &str) -> &'a str {
    id.strip_suffix(suffix).unwrap_or("")
}

/// The metrics pattern for an entry — matches the CLI's `pattern_of` exactly
/// (§20): `/` for the root, `/<segments joined by />` otherwise.
#[must_use]
pub fn default_pattern_of(e: &RouteEntry) -> String {
    if e.segments.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", e.segments.join("/"))
    }
}

impl RouteContextTable {
    /// Build the table over `entries`. `pattern_of` produces the metrics label
    /// for an entry (pass [`default_pattern_of`] or the CLI's own).
    pub fn build(entries: &[RouteEntry], pattern_of: impl Fn(&RouteEntry) -> String) -> Self {
        // Determinism: when two scoped modules share a base, the
        // lexicographically first id wins the slot.
        let mut sorted: Vec<&RouteEntry> = entries.iter().collect();
        sorted.sort_by(|a, b| a.id.0.cmp(&b.id.0));
        let mut scoped: HashMap<&str, Scoped> = HashMap::new();
        for e in &sorted {
            /// Which scoped slot a module kind occupies.
            enum Slot {
                Layout,
                Middleware,
                Error,
                Loading,
            }
            let (b, slot): (&str, Slot) = match e.kind {
                RouteKind::Layout => (base(&e.id.0, "/layout"), Slot::Layout),
                RouteKind::Middleware => (base(&e.id.0, "/middleware"), Slot::Middleware),
                RouteKind::Error => (base(&e.id.0, "/error"), Slot::Error),
                RouteKind::Loading => (base(&e.id.0, "/loading"), Slot::Loading),
                _ => continue,
            };
            let s = scoped.entry(b).or_default();
            let cell: &mut Option<&RouteEntry> = match slot {
                Slot::Layout => &mut s.layout,
                Slot::Middleware => &mut s.middleware,
                Slot::Error => &mut s.error,
                Slot::Loading => &mut s.loading,
            };
            if cell.is_none() {
                *cell = Some(e);
            }
        }
        let mut by_id = HashMap::with_capacity(entries.len());
        for e in entries.iter().filter(|e| {
            !matches!(
                e.kind,
                RouteKind::Layout | RouteKind::Middleware | RouteKind::Error | RouteKind::Loading
            )
        }) {
            by_id.insert(
                e.id.0.clone(),
                Arc::new(Self::context_for(e, &scoped, &pattern_of)),
            );
        }
        // Not-found: the FIRST NotFound entry in tree order (as the old
        // dispatch scans did), not the sorted order.
        let unmatched = entries
            .iter()
            .find(|e| matches!(e.kind, RouteKind::NotFound))
            .map(|e| Arc::new(Self::context_for(e, &scoped, &pattern_of)));
        Self { by_id, unmatched }
    }

    /// Convenience: [`Self::build`] with the crate-default pattern function.
    pub fn build_with_default_pattern(entries: &[RouteEntry]) -> Self {
        Self::build(entries, default_pattern_of)
    }

    fn context_for(
        e: &RouteEntry,
        scoped: &HashMap<&str, Scoped<'_>>,
        pattern_of: &impl Fn(&RouteEntry) -> String,
    ) -> RouteContext {
        let mut ctx = RouteContext {
            route_id: e.id.clone(),
            pattern: pattern_of(e).into(),
            layout_chain: vec![],
            layout_loads: vec![],
            middleware_chain: vec![],
            error_route: None,
            loading_route: None,
        };
        // Walk the prefixes outermost-first so chains come out in dispatch
        // order; inner scoped modules overwrite outer error/loading slots.
        for p in prefixes(&e.id.0) {
            let Some(s) = scoped.get(p) else { continue };
            if let Some(l) = s.layout {
                ctx.layout_chain.push(l.id.clone());
                if l.has_server_module {
                    ctx.layout_loads.push(LayoutLoad {
                        id: l.id.clone(),
                        server_file: l.file.with_file_name("layout.server.ts"),
                        needs_parent: l.needs_parent,
                    });
                }
            }
            if let Some(m) = s.middleware {
                ctx.middleware_chain.push(m.file.clone());
            }
            if let Some(x) = s.error {
                ctx.error_route = Some(x.id.clone()); // inner overwrites outer
            }
            if let Some(x) = s.loading {
                ctx.loading_route = Some(x.id.clone());
            }
        }
        ctx
    }

    /// The context for a matched route id.
    #[must_use]
    pub fn get(&self, route_id: &str) -> Option<&Arc<RouteContext>> {
        self.by_id.get(route_id)
    }

    /// The context for unmatched requests (the first `NotFound` entry).
    #[must_use]
    pub fn unmatched(&self) -> Option<&Arc<RouteContext>> {
        self.unmatched.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RouteTree;
    use crate::matcher::layout_chain;

    fn entry(id: &str, kind: RouteKind, segments: &[&str]) -> RouteEntry {
        RouteEntry {
            id: RouteId(id.to_string()),
            kind,
            segments: segments
                .iter()
                .map(std::string::ToString::to_string)
                .collect(),
            file: format!("{id}.tsx").into(),
            render_mode: None,
            tags: Vec::new(),
            has_server_module: false,
            needs_parent: false,
            live: None,
            slot: None,
        }
    }

    fn page(id: &str, segments: &[&str]) -> RouteEntry {
        entry(id, RouteKind::Page, segments)
    }

    fn layout(id: &str) -> RouteEntry {
        entry(id, RouteKind::Layout, &[])
    }

    /// V2-B01 test 1: for every fixture tree, the table's layout chain equals
    /// the `matcher::layout_chain` oracle for every non-layout entry.
    #[test]
    fn matches_layout_chain_oracle() {
        // Nested fixture.
        let entries = vec![
            layout("routes/layout"),
            layout("routes/blog/layout"),
            layout("routes/blog/[slug]/layout"),
            page("routes/blog/[slug]/page", &["blog", ":slug"]),
            page("routes/page", &[]),
        ];
        let table = RouteContextTable::build_with_default_pattern(&entries);
        for e in &entries {
            if matches!(e.kind, RouteKind::Layout) {
                assert!(table.get(&e.id.0).is_none(), "layouts have no context");
                continue;
            }
            let ctx = table.get(&e.id.0).expect("page context");
            assert_eq!(
                ctx.layout_chain,
                layout_chain(&entries, &e.id.0),
                "layout chain oracle mismatch for {}",
                e.id.0
            );
        }
        // Root layout wraps everything.
        let ctx = table.get("routes/page").unwrap();
        assert_eq!(ctx.layout_chain, vec![RouteId("routes/layout".into())]);
        let ctx = table.get("routes/blog/[slug]/page").unwrap();
        assert_eq!(
            ctx.layout_chain,
            vec![
                RouteId("routes/layout".into()),
                RouteId("routes/blog/layout".into()),
                RouteId("routes/blog/[slug]/layout".into()),
            ]
        );
    }

    #[test]
    fn inner_error_shadows_outer() {
        let entries = vec![
            entry("routes/error", RouteKind::Error, &[]),
            entry("routes/blog/error", RouteKind::Error, &[]),
            page("routes/blog/page", &["blog"]),
            page("routes/other/page", &["other"]),
        ];
        let table = RouteContextTable::build_with_default_pattern(&entries);
        assert_eq!(
            table.get("routes/blog/page").unwrap().error_route,
            Some(RouteId("routes/blog/error".into())),
            "innermost error wins"
        );
        assert_eq!(
            table.get("routes/other/page").unwrap().error_route,
            Some(RouteId("routes/error".into()))
        );
    }

    #[test]
    fn middleware_chain_outer_to_inner() {
        let entries = vec![
            entry("routes/middleware", RouteKind::Middleware, &[]),
            entry("routes/blog/middleware", RouteKind::Middleware, &[]),
            page("routes/blog/page", &["blog"]),
            page("routes/x/page", &["x"]),
        ];
        let table = RouteContextTable::build_with_default_pattern(&entries);
        let ctx = table.get("routes/blog/page").unwrap();
        assert_eq!(
            ctx.middleware_chain,
            vec![
                PathBuf::from("routes/middleware.tsx"),
                PathBuf::from("routes/blog/middleware.tsx"),
            ],
            "root middleware must come before the inner one"
        );
        assert_eq!(
            table.get("routes/x/page").unwrap().middleware_chain.len(),
            1
        );
    }

    #[test]
    fn layout_loads_only_with_server_module() {
        let mut with = layout("routes/blog/layout");
        with.has_server_module = true;
        let entries = vec![
            layout("routes/layout"),
            with,
            page("routes/blog/page", &["blog"]),
        ];
        let table = RouteContextTable::build_with_default_pattern(&entries);
        let ctx = table.get("routes/blog/page").unwrap();
        assert_eq!(ctx.layout_chain.len(), 2);
        assert_eq!(
            ctx.layout_loads.len(),
            1,
            "only the layout with a server module loads"
        );
        assert_eq!(ctx.layout_loads[0].id, RouteId("routes/blog/layout".into()));
    }

    #[test]
    fn layout_loads_carry_needs_parent() {
        let mut l = layout("routes/blog/layout");
        l.has_server_module = true;
        l.needs_parent = true;
        let entries = vec![l, page("routes/blog/page", &["blog"])];
        let table = RouteContextTable::build_with_default_pattern(&entries);
        let ctx = table.get("routes/blog/page").unwrap();
        assert!(ctx.layout_loads[0].needs_parent);
        assert_eq!(
            ctx.layout_loads[0].server_file,
            PathBuf::from("routes/blog/layout.server.ts"),
            "server file derives from the layout module file"
        );
    }

    #[test]
    fn loading_route_and_pattern_labels() {
        let entries = vec![
            entry("routes/loading", RouteKind::Loading, &[]),
            entry("routes/blog/loading", RouteKind::Loading, &[]),
            page("routes/blog/page", &["blog"]),
            entry("routes/not-found", RouteKind::NotFound, &["not-found"]),
        ];
        let table = RouteContextTable::build_with_default_pattern(&entries);
        let ctx = table.get("routes/blog/page").unwrap();
        assert_eq!(
            ctx.loading_route,
            Some(RouteId("routes/blog/loading".into())),
            "innermost loading wins"
        );
        assert_eq!(
            &*ctx.pattern, "/blog",
            "pattern matches the CLI's pattern_of"
        );
        let unmatched = table.unmatched().expect("not-found context");
        assert_eq!(unmatched.route_id, RouteId("routes/not-found".into()));
        assert_eq!(&*unmatched.pattern, "/not-found");
    }

    #[test]
    fn unmatched_uses_first_not_found() {
        let entries = vec![
            entry("routes/aaa/not-found", RouteKind::NotFound, &[]),
            entry("routes/zzz/not-found", RouteKind::NotFound, &[]),
        ];
        let table = RouteContextTable::build_with_default_pattern(&entries);
        assert_eq!(
            table.unmatched().map(|c| c.route_id.clone()),
            Some(RouteId("routes/aaa/not-found".into())),
            "the FIRST NotFound in tree order wins (not the sorted order)"
        );
    }

    #[test]
    fn root_and_routes_base_wrap_everything() {
        // A layout whose id has no `routes/` prefix (base "") and one at the
        // `routes` base both wrap every route — the prefixes walk always
        // includes "" and "routes".
        let entries = vec![
            layout("layout"),
            layout("routes/layout"),
            page("routes/blog/page", &["blog"]),
        ];
        let table = RouteContextTable::build_with_default_pattern(&entries);
        let ctx = table.get("routes/blog/page").unwrap();
        assert_eq!(
            ctx.layout_chain,
            vec![RouteId("layout".into()), RouteId("routes/layout".into())]
        );
    }

    /// V2-B01 test 4: random directory trees — table equals the oracle.
    #[test]
    fn proptest_table_matches_layout_chain_oracle() {
        use proptest::prelude::*;

        fn tree_strategy() -> impl Strategy<Value = Vec<RouteEntry>> {
            // Random nested paths over a small alphabet, each possibly
            // carrying a layout/middleware/error/loading module.
            proptest::collection::vec(
                (
                    proptest::collection::vec(
                        proptest::sample::select(vec!["a", "b", "c", "d"]),
                        0..=4,
                    ),
                    proptest::sample::select(vec![
                        "page",
                        "layout",
                        "middleware",
                        "error",
                        "loading",
                        "not-found",
                    ]),
                ),
                1..=40,
            )
            .prop_map(|parts| {
                let mut out = Vec::new();
                let mut seen = std::collections::HashSet::new();
                for (dirs, module) in parts {
                    let id = match module {
                        "page" => format!("routes/{}/page", dirs.join("/")),
                        "layout" => format!("routes/{}/layout", dirs.join("/")),
                        "middleware" => format!("routes/{}/middleware", dirs.join("/")),
                        "error" => format!("routes/{}/error", dirs.join("/")),
                        "loading" => format!("routes/{}/loading", dirs.join("/")),
                        _ => format!("routes/{}/not-found", dirs.join("/")),
                    };
                    if seen.insert(id.clone()) {
                        let kind = match module {
                            "page" => RouteKind::Page,
                            "layout" => RouteKind::Layout,
                            "middleware" => RouteKind::Middleware,
                            "error" => RouteKind::Error,
                            "loading" => RouteKind::Loading,
                            _ => RouteKind::NotFound,
                        };
                        out.push(entry(&id, kind, &[]));
                    }
                }
                out
            })
        }

        proptest!(|(entries in tree_strategy())| {
            let table = RouteContextTable::build_with_default_pattern(&entries);
            for e in &entries {
                if matches!(e.kind, RouteKind::Layout | RouteKind::Middleware | RouteKind::Error | RouteKind::Loading) {
                    continue;
                }
                if let Some(ctx) = table.get(&e.id.0) {
                    prop_assert!(
                        ctx.layout_chain == layout_chain(&entries, &e.id.0),
                        "layout chain oracle mismatch for {}: table={:?} oracle={:?}",
                        e.id.0,
                        ctx.layout_chain,
                        layout_chain(&entries, &e.id.0)
                    );
                }
            }
        });
    }

    /// The table answers the same questions as the tree's P-06 dispatch index.
    #[test]
    fn table_agrees_with_tree_dispatch_index() {
        let mut t = RouteTree::new();
        let entries = vec![
            entry("routes/middleware", RouteKind::Middleware, &[]),
            entry("routes/blog/error", RouteKind::Error, &[]),
            entry("routes/not-found", RouteKind::NotFound, &[]),
            layout("routes/blog/layout"),
            page("routes/blog/page", &["blog"]),
        ];
        for e in entries.clone() {
            t.insert(e);
        }
        let table = RouteContextTable::build_with_default_pattern(&entries);
        let ctx = table.get("routes/blog/page").unwrap();
        let mw: Vec<PathBuf> = t.middleware_files_for("routes/blog/page");
        assert_eq!(ctx.middleware_chain, mw);
        assert_eq!(
            ctx.error_route.as_ref().map(|i| i.0.as_str()),
            t.error_route_for("routes/blog/page").as_deref()
        );
        assert_eq!(
            ctx.layout_chain,
            t.layout_chain_cached("routes/blog/page"),
            "P-02 cached chain and the table agree"
        );
    }
}
