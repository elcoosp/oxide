//! Route scanner (spec §8.1): walks a routes directory and produces the deterministic
//! [`RouteEntry`] list consumed by the matcher, archiver and typegen (spec §8, D8).

use std::fs;
use std::path::{Path, PathBuf};

use crate::{Result, RouteEntry, RouteId, RouteKind, RouteTree, ferr};

/// Conventional file bases recognized by the scanner (extension-agnostic, §8.1).
const CONVENTION_BASES: &[&str] = &[
    "page",
    "layout",
    "loading",
    "error",
    "not-found",
    "template",
    "route",
    "middleware",
    "socket",
];
/// Filename segments that disqualify a conventional-looking file (tests, CSS modules…).
const DENYLIST_EXTS: &[&str] = &["test", "spec", "stories", "module", "d", "snap", "mock"];

pub(crate) fn scan_impl(dir: &Path) -> Result<RouteTree> {
    let root = resolve_root(dir);
    if !root.is_dir() {
        return Err(ferr(
            "RTR-001",
            format!("routes directory not found: {}", root.display()),
            "point `routes_dir` at the directory containing your route modules (default src/routes)",
        ));
    }
    // Route ids are file-derived and carry the canonical root name (§8.1):
    // `routes/blog/[slug]/page` under the Oxide conventions root, `app/...` in
    // Next-compat mode (`app/**/page.tsx`).
    let id_prefix = if root.file_name().map(|s| s == "app").unwrap_or(false) {
        "app"
    } else {
        "routes"
    };
    let mut walk = Walk {
        id_prefix: id_prefix.to_string(),
        entries: Vec::new(),
    };
    walk_dir(&mut walk, &root, &[], &[], None)?;
    sort_entries(&mut walk.entries);
    let mut tree = RouteTree::new();
    tree.entries = walk.entries;
    Ok(tree)
}

pub(crate) fn scan_with_config_impl(
    dir: &Path,
    routes_dir_override: Option<&Path>,
) -> Result<RouteTree> {
    match routes_dir_override {
        Some(p) if p.is_absolute() => scan_impl(p),
        Some(p) => scan_impl(&dir.join(p)),
        None => {
            let conventional = dir.join("src").join("routes");
            if conventional.is_dir() {
                scan_impl(&conventional)
            } else {
                // Fall through to scan()'s Next-compat detection.
                scan_impl(dir)
            }
        }
    }
}

struct Walk {
    id_prefix: String,
    entries: Vec<RouteEntry>,
}

/// Next-compat mode (§8.1): when `dir` contains an `app/` directory with app-level
/// route modules and `dir` has no top-level route modules of its own, the scanner
/// rebases into `dir/app` so `app/**/page.tsx` maps identically to Oxide conventions.
fn resolve_root(dir: &Path) -> PathBuf {
    let app = dir.join("app");
    if app.is_dir() && !has_top_level_module(dir) && has_top_level_module(&app) {
        app
    } else {
        dir.to_path_buf()
    }
}

fn has_top_level_module(dir: &Path) -> bool {
    let Ok(rd) = fs::read_dir(dir) else {
        return false;
    };
    let mut names: Vec<String> = rd
        .flatten()
        .filter(|e| e.path().is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names.iter().any(|n| file_role(n).is_some())
}

fn walk_dir(
    walk: &mut Walk,
    dir: &Path,
    rel: &[String],
    url_segs: &[String],
    slot: Option<&str>,
) -> Result<()> {
    let rd = fs::read_dir(dir).map_err(|e| {
        ferr(
            "RTR-001",
            format!("cannot read routes directory {}: {e}", dir.display()),
            "check that the routes directory exists and is readable",
        )
    })?;
    let mut names: Vec<String> = rd
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let mut subdirs = Vec::new();
    let mut files = Vec::new();
    for name in &names {
        let p = dir.join(name);
        if p.is_dir() {
            subdirs.push(name.clone());
        } else if p.is_file() {
            files.push(name.clone());
        }
    }

    // `*.server.ts` modules adjacent to pages/layouts mark `has_server_module` (§8.1, D10).
    let page_server = files.iter().find(|f| is_server_module(f, "page")).cloned();
    let layout_server = files
        .iter()
        .find(|f| is_server_module(f, "layout"))
        .cloned();

    for name in &files {
        let Some(role) = file_role(name) else {
            continue;
        };
        let base = name.split('.').next().unwrap_or(name);
        let kind = match role {
            FileRole::Page => RouteKind::Page,
            FileRole::Layout => RouteKind::Layout,
            FileRole::Loading => RouteKind::Loading,
            FileRole::Error => RouteKind::Error,
            FileRole::NotFound => RouteKind::NotFound,
            FileRole::Template => RouteKind::Template,
            FileRole::Endpoint => RouteKind::Endpoint,
            FileRole::Middleware => RouteKind::Middleware,
            FileRole::Socket => RouteKind::Socket,
        };
        let mut id = walk.id_prefix.clone();
        for r in rel {
            id.push('/');
            id.push_str(r);
        }
        id.push('/');
        id.push_str(base);
        let mut file_rel = rel.to_vec();
        file_rel.push(name.clone());
        let mut render_mode = None;
        let mut tags = Vec::new();
        let has_server_module = match kind {
            RouteKind::Page => {
                if let Some(server) = &page_server {
                    read_page_config(&dir.join(server), &mut render_mode, &mut tags);
                }
                page_server.is_some()
            }
            RouteKind::Layout => layout_server.is_some(),
            _ => false,
        };
        let needs_parent = if matches!(kind, RouteKind::Page) {
            page_server
                .as_ref()
                .and_then(|s| std::fs::read_to_string(dir.join(s)).ok())
                .map(|c| c.contains("parent"))
                .unwrap_or(false)
        } else {
            false
        };
        walk.entries.push(RouteEntry {
            id: RouteId(id),
            kind,
            segments: url_segs.to_vec(),
            file: file_rel.iter().collect(),
            render_mode,
            tags,
            has_server_module,
            slot: slot.map(str::to_string),
            needs_parent,
        });
    }

    for name in &subdirs {
        let mut next_rel = rel.to_vec();
        next_rel.push(name.clone());
        match dir_kind(name) {
            DirKind::Group | DirKind::Intercepting => {
                // Groups and intercepting routes (§8.5, post-MLP) never appear in the URL.
                walk_dir(walk, &dir.join(name), &next_rel, url_segs, slot)?;
            }
            DirKind::Slot(marker) => {
                walk_dir(walk, &dir.join(name), &next_rel, url_segs, Some(&marker))?;
            }
            DirKind::Segment(seg) => {
                let mut next_segs = url_segs.to_vec();
                next_segs.push(seg);
                walk_dir(walk, &dir.join(name), &next_rel, &next_segs, slot)?;
            }
            DirKind::Invalid(msg) => {
                return Err(ferr(
                    "RTR-002",
                    format!("invalid route directory `{name}`: {msg}"),
                    "use [param], [param]{regex}, [...rest], [[...optional]], (group) or @slot",
                ));
            }
        }
    }
    Ok(())
}

enum FileRole {
    Page,
    Layout,
    Loading,
    Error,
    NotFound,
    Template,
    Endpoint,
    Middleware,
    Socket,
}

/// Classify a conventional file name (extension-agnostic, lowercase, §8.1).
/// Test/spec/module/declaration files never become routes.
fn file_role(name: &str) -> Option<FileRole> {
    let parts: Vec<&str> = name.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    let (base, exts) = (parts[0], &parts[1..]);
    if exts.iter().any(|e| DENYLIST_EXTS.contains(e)) {
        return None;
    }
    // `*.server.ts` files are data modules (§8.1, D10), not route modules; they only
    // mark `has_server_module` via `is_server_module`.
    if exts.contains(&"server") {
        return None;
    }
    if !CONVENTION_BASES.contains(&base) {
        return None;
    }
    match base {
        "page" => Some(FileRole::Page),
        "layout" => Some(FileRole::Layout),
        "loading" => Some(FileRole::Loading),
        "error" => Some(FileRole::Error),
        "not-found" => Some(FileRole::NotFound),
        "template" => Some(FileRole::Template),
        "route" => Some(FileRole::Endpoint),
        "middleware" => Some(FileRole::Middleware),
        "socket" => Some(FileRole::Socket),
        _ => None,
    }
}

fn is_server_module(name: &str, base: &str) -> bool {
    let parts: Vec<&str> = name.split('.').collect();
    parts.len() >= 3
        && parts[0] == base
        && parts[1] == "server"
        && !parts[2..].iter().any(|e| DENYLIST_EXTS.contains(e))
}

enum DirKind {
    /// `(group)` — not in the URL.
    Group,
    /// `@slot` — parallel routes (§8.5): reserved identity, not in the URL.
    Slot(String),
    /// `(.)name` and friends — intercepting routes (§8.5, post-MLP): reserved, not in the URL.
    Intercepting,
    /// An encoded URL segment (static, `:param`, `*rest`, `*?opt`).
    Segment(String),
    Invalid(String),
}

fn dir_kind(name: &str) -> DirKind {
    if name.starts_with('(') && name.ends_with(')') && name.len() >= 2 {
        let inner = &name[1..name.len() - 1];
        // Intercepting prefixes: (.) (..) (...) (..)(..)
        return if inner.starts_with('.') {
            DirKind::Intercepting
        } else {
            DirKind::Group
        };
    }
    if let Some(rest) = name.strip_prefix('@') {
        return if rest.is_empty() {
            DirKind::Invalid("slot name must not be empty".to_string())
        } else {
            DirKind::Slot(name.to_string())
        };
    }
    if let Some(rest) = name.strip_prefix("[[...") {
        return match rest.strip_suffix("]]") {
            Some(base) if valid_param_name(base) => DirKind::Segment(format!("*?{base}")),
            Some(_) => DirKind::Invalid(
                "optional catch-all name must be non-empty without '/' or braces".to_string(),
            ),
            None => DirKind::Invalid("unclosed `[[...name]]`".to_string()),
        };
    }
    if let Some(rest) = name.strip_prefix("[...") {
        return match rest.strip_suffix(']') {
            Some(base) if valid_param_name(base) => DirKind::Segment(format!("*{base}")),
            Some(_) => DirKind::Invalid(
                "catch-all name must be non-empty without '/' or braces".to_string(),
            ),
            None => DirKind::Invalid("unclosed `[...name]`".to_string()),
        };
    }
    if name.starts_with('[') {
        // Canonical constraint form: `[name]{regex}` (spec §8.1 choice).
        if name.ends_with('}') {
            let Some(pos) = name.find("]{") else {
                return DirKind::Invalid("malformed `[name]{regex}`".to_string());
            };
            let inner = &name[1..pos];
            let re = &name[pos + 2..name.len() - 1];
            if !valid_param_name(inner) {
                return DirKind::Invalid(
                    "param name must be non-empty without '/' or braces".to_string(),
                );
            }
            if let Err(e) = regex::Regex::new(&format!("^(?:{re})$")) {
                return DirKind::Invalid(format!("invalid regex constraint `{re}`: {e}"));
            }
            return DirKind::Segment(format!(":{inner}{{{re}}}"));
        }
        let Some(close) = name.find(']') else {
            return DirKind::Invalid("unclosed `[...]`".to_string());
        };
        if close != name.len() - 1 {
            return DirKind::Invalid("unexpected text after `]`".to_string());
        }
        let inner = &name[1..close];
        if let Some((n, re)) = inner.split_once('=') {
            // Alternative constraint spelling: `[name=regex]`.
            if !valid_param_name(n) {
                return DirKind::Invalid(
                    "param name must be non-empty without '/' or braces".to_string(),
                );
            }
            if let Err(e) = regex::Regex::new(&format!("^(?:{re})$")) {
                return DirKind::Invalid(format!("invalid regex constraint `{re}`: {e}"));
            }
            return DirKind::Segment(format!(":{n}{{{re}}}"));
        }
        if !valid_param_name(inner) {
            return DirKind::Invalid(
                "param name must be non-empty without '/' or braces".to_string(),
            );
        }
        return DirKind::Segment(format!(":{inner}"));
    }
    DirKind::Segment(name.to_string())
}

fn valid_param_name(name: &str) -> bool {
    !name.is_empty() && !name.contains('/') && !name.contains('{') && !name.contains('}')
}

/// Best-effort config extraction from `page.server.ts` (full parsing is the
/// compiler's job; the scanner reads the obvious literals for `oxide routes`).
///
/// §10.1 mode inference runs here when no explicit `config.render` is
/// declared. A route is `static` unless its loader body reads a
/// request-specific value: `cookies`, `request.headers`, `locals`, or
/// `searchParams`. The full-fidelity version is the compiler's job (with
/// source spans and diagnostics); this heuristic keeps `oxide routes`,
/// `oxide build`, and the dev inspector in agreement today. An explicit
/// `config.render` always wins.
fn read_page_config(path: &Path, render_mode: &mut Option<String>, tags: &mut Vec<String>) {
    let Ok(text) = fs::read_to_string(path) else {
        return;
    };
    static RENDER: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RENDER.get_or_init(|| {
        regex::Regex::new(r#"(?:render|renderMode|render_mode)\s*:\s*["']([A-Za-z0-9_-]+)["']"#)
            .expect("render regex")
    });
    if let Some(c) = re.captures(&text) {
        *render_mode = Some(c[1].to_string());
    } else if has_request_specific_read(&text) {
        // §10.1: no explicit config, and the loader reads from the request.
        *render_mode = Some("dynamic".to_string());
    } else {
        // §10.1: no explicit config, and no request-specific reads. The
        // route is eligible for prerendering at build time.
        *render_mode = Some("static".to_string());
    }
    static TAGS: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re =
        TAGS.get_or_init(|| regex::Regex::new(r#"tags\s*:\s*\[([^\]]*)\]"#).expect("tags regex"));
    if let Some(c) = re.captures(&text) {
        static QUOTED: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        let q = QUOTED
            .get_or_init(|| regex::Regex::new(r#"[\"']([^\"']+)[\"']"#).expect("quoted regex"));
        for m in q.captures_iter(&c[1]) {
            tags.push(m[1].to_string());
        }
    }
}

/// §10.1: does the loader body read a request-scoped value?
///
/// Reads the three shapes the spec names plus the loader-event accessor
/// spelling:
///
/// * `cookies` — request cookies
/// * `request.headers` — request headers
/// * `locals` — middleware-injected request state
/// * `searchParams` — the search-param accessor (the load event exposes it
///   as `searchParams`; the underlying `URL` may also be read via
///   `request.url`, which is deliberately NOT treated as a marker — a
///   route can inspect the URL for canonicalisation without becoming
///   per-request).
///
/// The scan is line-oriented: it strips line comments so a `// uses cookies`
/// note does not flip the mode. Block comments and string literals are not
/// handled — the compiler pass (a follow-up) will be authoritative.
fn has_request_specific_read(source: &str) -> bool {
    for raw in source.lines() {
        let line = raw.split("//").next().unwrap_or("");
        // Only look at the loader/actions bodies? For now, a whole-file scan
        // is fine: a top-level import of `cookies` from `oxide/server`
        // without a loader body would also mark the route dynamic, which is
        // conservative (fails closed on the safe side).
        if line.contains("cookies") {
            return true;
        }
        if line.contains("locals") {
            return true;
        }
        if line.contains("request.headers") {
            return true;
        }
        if line.contains("searchParams") {
            return true;
        }
    }
    false
}

/// Deterministic ordering (§8.1): (depth, per-segment priority static > param >
/// optional > catch-all, id).
fn sort_entries(entries: &mut [RouteEntry]) {
    entries.sort_by(|a, b| {
        a.segments
            .len()
            .cmp(&b.segments.len())
            .then_with(|| prio_key(&a.segments).cmp(&prio_key(&b.segments)))
            .then_with(|| a.id.0.cmp(&b.id.0))
    });
}

fn prio_key(segments: &[String]) -> Vec<u8> {
    segments
        .iter()
        .map(|s| crate::segment::seg_priority(s))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Write a file (creating parent dirs) inside a tempdir fixture.
    fn put(root: &Path, rel: &str, contents: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, contents).unwrap();
    }

    fn segs_of<'a>(tree: &'a RouteTree, id: &str) -> &'a [String] {
        tree.entries
            .iter()
            .find(|e| e.id.0 == id)
            .unwrap_or_else(|| panic!("entry {id} not found"))
            .segments
            .as_slice()
    }

    fn kind_of(tree: &RouteTree, id: &str) -> RouteKind {
        tree.entries
            .iter()
            .find(|e| e.id.0 == id)
            .expect("entry")
            .kind
            .clone()
    }

    #[test]
    fn scan_recognizes_all_conventional_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for f in [
            "layout.tsx",
            "page.tsx",
            "loading.tsx",
            "error.tsx",
            "not-found.tsx",
            "template.tsx",
            "route.ts",
            "middleware.ts",
        ] {
            put(root, f, "export default null;");
        }
        put(root, "page.server.ts", "export const load = () => ({});");
        let tree = RouteTree::scan(root).unwrap();
        assert_eq!(tree.entries.len(), 8);
        assert_eq!(kind_of(&tree, "routes/layout"), RouteKind::Layout);
        assert_eq!(kind_of(&tree, "routes/page"), RouteKind::Page);
        assert_eq!(kind_of(&tree, "routes/loading"), RouteKind::Loading);
        assert_eq!(kind_of(&tree, "routes/error"), RouteKind::Error);
        assert_eq!(kind_of(&tree, "routes/not-found"), RouteKind::NotFound);
        assert_eq!(kind_of(&tree, "routes/template"), RouteKind::Template);
        assert_eq!(kind_of(&tree, "routes/route"), RouteKind::Endpoint);
        assert_eq!(kind_of(&tree, "routes/middleware"), RouteKind::Middleware);
        let page = tree
            .entries
            .iter()
            .find(|e| e.id.0 == "routes/page")
            .unwrap();
        assert!(
            page.has_server_module,
            "page.server.ts marks has_server_module"
        );
        assert_eq!(page.segments.len(), 0, "root page matches /");
        // test files and random assets are ignored
        put(root, "page.test.ts", "it('works')");
        put(root, "README.md", "# hi");
        put(root, "page.module.css", ".x {}");
        let tree2 = RouteTree::scan(root).unwrap();
        assert_eq!(
            tree2.entries.len(),
            8,
            "denylisted/unconventional files are ignored"
        );
    }

    #[test]
    fn groups_and_slots_are_reserved_but_not_url_segments() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        put(root, "(marketing)/page.tsx", "p");
        put(root, "(marketing)/layout.tsx", "l");
        put(root, "@modal/page.tsx", "m");
        let tree = RouteTree::scan(root).unwrap();
        assert!(
            segs_of(&tree, "routes/(marketing)/page").is_empty(),
            "(group) is not in the URL"
        );
        let modal = tree
            .entries
            .iter()
            .find(|e| e.id.0 == "routes/@modal/page")
            .unwrap();
        assert_eq!(
            modal.slot.as_deref(),
            Some("@modal"),
            "slot identity is reserved"
        );
        // The marketing page is URL-addressable; the slot page is not.
        let m = tree.match_path("/").unwrap();
        assert_eq!(m.route.id.0, "routes/(marketing)/page");
        // Nested group layout is part of the chain.
        let m = tree.match_path("/").unwrap();
        assert_eq!(
            m.layout_chain,
            vec![RouteId("routes/(marketing)/layout".to_string())]
        );
    }

    #[test]
    fn scan_accepts_all_param_directory_forms() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        put(root, "blog/[slug]/page.tsx", "p");
        put(root, "digits/[n]{\\d+}/page.tsx", "p");
        put(root, "legacy/[id=\\d+]/page.tsx", "p");
        put(root, "docs/[...rest]/page.tsx", "p");
        put(root, "[[...path]]/page.tsx", "p");
        let tree = RouteTree::scan(root).unwrap();
        assert_eq!(segs_of(&tree, "routes/blog/[slug]/page"), ["blog", ":slug"]);
        assert_eq!(
            segs_of(&tree, "routes/digits/[n]{\\d+}/page"),
            ["digits", ":n{\\d+}"]
        );
        assert_eq!(
            segs_of(&tree, "routes/legacy/[id=\\d+]/page"),
            ["legacy", ":id{\\d+}"],
            "`=` form normalizes to `{{}}`"
        );
        assert_eq!(
            segs_of(&tree, "routes/docs/[...rest]/page"),
            ["docs", "*rest"]
        );
        assert_eq!(segs_of(&tree, "routes/[[...path]]/page"), ["*?path"]);
    }

    #[test]
    fn scan_rejects_malformed_segments_and_regexes() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        put(root, "[slug/page.tsx", "p");
        let err = RouteTree::scan(root).unwrap_err();
        assert_eq!(err.0.code, "RTR-002");

        let tmp2 = tempfile::tempdir().unwrap();
        put(tmp2.path(), "[n]{(/}/page.tsx", "p");
        let err = RouteTree::scan(tmp2.path()).unwrap_err();
        assert_eq!(
            err.0.code, "RTR-002",
            "invalid regex constraint must fail at scan"
        );
    }

    #[test]
    fn scan_reads_tags_and_render_mode_from_page_server_ts() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        put(root, "blog/page.tsx", "p");
        put(
            root,
            "blog/page.server.ts",
            "export const config = { render: 'isr', tags: [\"post:a\", 'post:b'] };",
        );
        let tree = RouteTree::scan(root).unwrap();
        let page = tree
            .entries
            .iter()
            .find(|e| e.id.0 == "routes/blog/page")
            .unwrap();
        assert_eq!(page.render_mode.as_deref(), Some("isr"));
        assert_eq!(page.tags, vec!["post:a".to_string(), "post:b".to_string()]);
    }

    #[test]
    fn scan_is_deterministic_and_sorted() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        put(root, "blog/page.tsx", "p");
        put(root, "blog/[slug]/page.tsx", "p");
        put(root, "blog/new/page.tsx", "p");
        put(root, "docs/[...rest]/page.tsx", "p");
        put(root, "layout.tsx", "l");
        put(root, "[[...path]]/page.tsx", "p");
        let a = RouteTree::scan(root).unwrap();
        let b = RouteTree::scan(root).unwrap();
        assert_eq!(a.entries, b.entries, "two scans are equal");
        // Sorted by (depth, per-segment priority static>param>optional>catch-all, id).
        for w in a.entries.windows(2) {
            let ka = (w[0].segments.len(), prio(&w[0].segments), &w[0].id);
            let kb = (w[1].segments.len(), prio(&w[1].segments), &w[1].id);
            assert!(ka <= kb, "entries not sorted: {:?} then {:?}", ka, kb);
        }
        // Depth-first ordering: root layout first, then 1-deep, then 2-deep.
        assert_eq!(a.entries[0].id.0, "routes/layout");
    }

    fn prio(segs: &[String]) -> Vec<u8> {
        segs.iter()
            .map(|s| crate::segment::seg_priority(s))
            .collect()
    }

    #[test]
    fn scan_missing_dir_is_rtr_001() {
        let err = RouteTree::scan(Path::new("/nonexistent-oxide-routes-xyz")).unwrap_err();
        assert_eq!(err.0.code, "RTR-001");
        assert!(err.0.fix.is_some(), "known fix attached (§19.4)");
    }

    #[test]
    fn next_compat_rebases_into_app_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        put(root, "app/layout.tsx", "l");
        put(root, "app/blog/page.tsx", "p");
        put(root, "app/api/users/route.ts", "r");
        let tree = RouteTree::scan(root).unwrap();
        assert_eq!(
            segs_of(&tree, "app/blog/page"),
            ["blog"],
            "app/**/page.tsx maps identically"
        );
        assert!(tree.match_path("/blog").is_some());
        assert!(tree.match_path("/api/users").is_some());
        // Scanning the app dir directly yields the same routes.
        let tree2 = RouteTree::scan(&root.join("app")).unwrap();
        assert_eq!(tree2.entries, tree.entries);
    }

    #[test]
    fn scan_with_config_override_wins_over_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        put(root, "web/routes/hello/page.tsx", "p");
        // Explicit (relative) override.
        let tree = RouteTree::scan_with_config(root, Some(Path::new("web/routes"))).unwrap();
        assert!(tree.match_path("/hello").is_some());
        // Absolute override.
        let tree = RouteTree::scan_with_config(root, Some(&root.join("web/routes"))).unwrap();
        assert!(tree.match_path("/hello").is_some());
        // Default resolution: src/routes when present…
        put(root, "src/routes/alt/page.tsx", "p");
        let tree = RouteTree::scan_with_config(root, None).unwrap();
        assert!(tree.match_path("/alt").is_some());
        assert!(
            tree.match_path("/hello").is_none(),
            "override absent by default"
        );
    }

    #[test]
    fn nested_layouts_three_levels_are_scanned() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        put(root, "layout.tsx", "l0");
        put(root, "blog/layout.tsx", "l1");
        put(root, "blog/[slug]/layout.tsx", "l2");
        put(root, "blog/[slug]/page.tsx", "p");
        let tree = RouteTree::scan(root).unwrap();
        assert_eq!(
            kind_of(&tree, "routes/blog/[slug]/layout"),
            RouteKind::Layout
        );
        // matcher attaches the full chain (root -> leaf)
        let m = tree.match_path("/blog/rust").unwrap();
        let ids: Vec<&str> = m.layout_chain.iter().map(|i| i.0.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "routes/layout",
                "routes/blog/layout",
                "routes/blog/[slug]/layout"
            ],
            "outermost layout first"
        );
    }
}
