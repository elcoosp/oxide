//! §19-B: codemod `oxide migrate <source>` (§19.2).
//!
//! Converts a project from Next.js / Nuxt / SvelteKit / Vite to Oxide
//! conventions. Dry-run by default; with `--write`, every file is either
//! **converted** or **reported unconverted with an explicit reason** (the
//! §19.2 requirement). A dry-run never writes.
//!
//! ## Conversion table
//!
//! | Source | Marker | Files | Transform |
//! |---|---|---|---|
//! | Next.js (pages) | `next.config.*` | `pages/**/*.{tsx,jsx,ts,js}` | `pages/index.tsx`→`src/routes/page.tsx`, `pages/x.tsx`→`src/routes/x/page.tsx`. `getServerSideProps`/`getStaticProps` become `load`. `pages/api/*`→`src/routes/api/*/route.{ext}`. `_app`/`_document`/`_error` reported. |
//! | Next.js (app) | `next.config.*` | `app/**/{page,layout,route,loading,error,not-found,template}.{tsx,jsx,ts,js}` | near 1:1 path rewrite |
//! | Nuxt | `nuxt.config.*` | `pages/**/*.vue` | `pages/index.vue`→`src/routes/page.vue`, `pages/x.vue`→`src/routes/x/page.vue` |
//! | SvelteKit | `svelte.config.*` | `src/routes/**/+{page,layout,error}{,.server}.{svelte,ts,js}` | `+page.svelte`→`page.svelte` etc (path rename only — the routing convention is identical) |
//! | Vite | `vite.config.*` | `index.html` + `src/main.{tsx,ts,jsx,js}` | `main.*` body's mount call stripped, rest emitted as `src/routes/page.tsx` |
//!
//! ## What is NOT converted (reported, not silently dropped)
//!
//! - Next `_app` / `_document` / `_error` — port by hand (layout/error).
//! - `getServerSideProps` returning `{ redirect }` / `{ notFound }` — a
//!   `load` cannot express those shape (`throw` / `redirect()` is the Oxide
//!   equivalent). Reported per file.
//! - Nuxt `useFetch` / `useAsyncData` inside a `.vue` — moves the data
//!   fetch into a sibling `page.server.ts`. Not auto-rewritten: it is a
//!   cross-file move, not a local edit. Reported per file.
//! - Vite `index.html`'s DOM contract (`<div id="root">`) — no page-level
//!   equivalent; the mount call's component is preserved.
//!
//! ## Overwrite policy
//!
//! `apply` refuses to overwrite an existing destination file. A stale
//! Oxide project that has already-migrated paths gets a per-file
//! `refusing to overwrite …` error — the file on disk wins, never clobbered.
//!
//! ## Env
//!
//! `OXIDE_MIGRATE_ROUTES_DIR` overrides the destination routes directory
//! (default: `<root>/src/routes`). Used by tests to redirect writes into
//! a temp dir without changing the fixture root.

use std::path::{Path, PathBuf};

/// One planned conversion. `to` is `None` for a file that is NOT converted
/// — `note` explains why (a reported follow-up).
#[derive(Debug, Clone)]
pub struct Conversion {
    pub from: PathBuf,
    pub to: Option<PathBuf>,
    pub note: Option<String>,
    /// When `Some`, `apply` writes these bytes instead of copying `from`.
    pub body: Option<String>,
}

impl Conversion {
    fn rename(from: PathBuf, to: PathBuf) -> Self {
        Self {
            from,
            to: Some(to),
            note: None,
            body: None,
        }
    }
    fn rename_with_note(from: PathBuf, to: PathBuf, note: String) -> Self {
        Self {
            from,
            to: Some(to),
            note: Some(note),
            body: None,
        }
    }
    fn rewrite(from: PathBuf, to: PathBuf, body: String) -> Self {
        Self {
            from,
            to: Some(to),
            note: None,
            body: Some(body),
        }
    }
    fn skip(from: PathBuf, note: String) -> Self {
        Self {
            from,
            to: None,
            note: Some(note),
            body: None,
        }
    }
    pub fn converted(&self) -> bool {
        self.to.is_some()
    }
}

/// A whole plan — every file under the source's convention.
#[derive(Debug, Clone, Default)]
pub struct MigrationPlan {
    pub source: String,
    pub label: String,
    pub conversions: Vec<Conversion>,
}

impl MigrationPlan {
    pub fn converted_count(&self) -> usize {
        self.conversions.iter().filter(|c| c.converted()).count()
    }
    pub fn skipped_count(&self) -> usize {
        self.conversions.iter().filter(|c| !c.converted()).count()
    }
}

/// Detect + plan. Never writes.
pub fn plan(root: &Path, source: &str) -> Result<MigrationPlan, String> {
    match source {
        "next" => plan_next(root),
        "nuxt" => plan_nuxt(root),
        "sveltekit" => plan_sveltekit(root),
        "vite" => plan_vite(root),
        other => Err(format!(
            "unknown source `{other}`; expected next|nuxt|sveltekit|vite"
        )),
    }
}

/// Apply a plan. Refuses to overwrite any existing destination file;
/// returns a multi-line error naming each conflict if any.
pub fn apply(plan: &MigrationPlan) -> Result<(), String> {
    let mut errors: Vec<String> = Vec::new();
    for c in &plan.conversions {
        let Some(ref dst) = c.to else { continue };
        if dst.exists() {
            errors.push(format!("refusing to overwrite {}", dst.display()));
            continue;
        }
        if let Some(parent) = dst.parent()
            && let Err(e) = std::fs::create_dir_all(parent) {
                errors.push(format!("cannot create {}: {e}", parent.display()));
                continue;
            }
        let bytes = match &c.body {
            Some(b) => b.as_bytes().to_vec(),
            None => match std::fs::read(&c.from) {
                Ok(b) => b,
                Err(e) => {
                    errors.push(format!("read {}: {e}", c.from.display()));
                    continue;
                }
            },
        };
        if let Err(e) = std::fs::write(dst, bytes) {
            errors.push(format!("write {}: {e}", dst.display()));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

// ─── helpers ───────────────────────────────────────────────────────────────

fn routes_root(root: &Path) -> PathBuf {
    std::env::var_os("OXIDE_MIGRATE_ROUTES_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("src/routes"))
}

fn ext_of(name: &str) -> Option<&str> {
    name.rsplit_once('.').map(|(_, e)| e)
}

fn strip_ext(name: &str) -> &str {
    name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name)
}

fn collect_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(dir, &mut out);
    out.sort();
    out
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        let Ok(meta) = std::fs::symlink_metadata(&p) else {
            continue;
        };
        if meta.is_dir() {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            // Never descend into vendor / build / VCS dirs.
            if matches!(
                name,
                "node_modules"
                    | "dist"
                    | ".git"
                    | "target"
                    | ".oxide"
                    | ".next"
                    | ".nuxt"
                    | ".svelte-kit"
                    | ".output"
            ) {
                continue;
            }
            walk(&p, out);
        } else if meta.is_file() {
            out.push(p);
        }
    }
}

// ─── Next.js ───────────────────────────────────────────────────────────────

fn plan_next(root: &Path) -> Result<MigrationPlan, String> {
    let markers = ["next.config.js", "next.config.mjs", "next.config.ts"];
    if !markers.iter().any(|m| root.join(m).is_file()) {
        return Err(format!(
            "no Next.js project detected (missing {})",
            markers.join(" / ")
        ));
    }
    let mut conv = Vec::new();
    if root.join("pages").is_dir() {
        for p in collect_files(root.join("pages").as_path()) {
            plan_next_page(root, &p, &mut conv);
        }
    }
    if root.join("app").is_dir() {
        for p in collect_files(root.join("app").as_path()) {
            plan_next_app(root, &p, &mut conv);
        }
    }
    Ok(MigrationPlan {
        source: "next".to_string(),
        label: "Next.js".to_string(),
        conversions: conv,
    })
}

fn plan_next_page(root: &Path, src: &Path, out: &mut Vec<Conversion>) {
    let pages = root.join("pages");
    let rel = match src.strip_prefix(&pages) {
        Ok(r) => r,
        Err(_) => return,
    };
    let name = rel.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let stem = strip_ext(name);
    let ext = ext_of(name).unwrap_or("tsx");

    // API routes. `rel` is relative to `pages/`, e.g. `api/hello.ts`.
    // Strip the `api/` prefix and drop the extension, so
    // `pages/api/hello.ts` → `src/routes/api/hello/route.ts`.
    // (Note: `PathBuf::join` with an absolute RHS replaces the whole path —
    // strip leading `/` explicitly, never `strip_prefix("api")` on a
    // `/`-separated relative path.)
    if let Ok(api_rel) = rel
        .strip_prefix("api/")
        .or_else(|_| rel.strip_prefix("api\\"))
    {
        let mut dst = routes_root(root).join("api");
        if let Some(parent) = api_rel.parent()
            && !parent.as_os_str().is_empty() {
                dst = dst.join(parent);
            }
        let dir_name = strip_ext(api_rel.file_name().and_then(|n| n.to_str()).unwrap_or(""));
        let dst = dst.join(dir_name).join(format!("route.{ext}"));
        out.push(Conversion::rename(src.to_path_buf(), dst));
        return;
    }
    // Next specials — reported.
    if matches!(stem, "_app" | "_document" | "_error") {
        out.push(Conversion::skip(
            src.to_path_buf(),
            format!("`{stem}` is a Next special file — port into an Oxide layout/error by hand"),
        ));
        return;
    }
    // A `.d.ts` or a `pages/**/*.test.ts` is not a route — report and skip.
    if !matches!(ext, "tsx" | "jsx" | "ts" | "js") {
        out.push(Conversion::skip(
            src.to_path_buf(),
            format!("non-route extension `{ext}` — left untouched"),
        ));
        return;
    }
    // pages/index.tsx → routes/page.tsx; pages/x/y.tsx → routes/x/y/page.tsx.
    let dir_part = rel.parent().unwrap_or(Path::new("")).to_path_buf();
    let dst_dir = if stem == "index" || stem == "index.page" {
        routes_root(root).join(&dir_part)
    } else if let Some(stripped) = stem.strip_suffix(".page") {
        // `pages/blog/post.page.tsx` → `routes/blog/post/page.tsx`.
        routes_root(root).join(&dir_part).join(stripped)
    } else {
        routes_root(root).join(&dir_part).join(stem)
    };
    let dst = dst_dir.join(format!("page.{ext}"));

    // Body rewrite for GSSP/GSFP when present.
    match rewrite_next_body(src) {
        Ok(Some((new_body, notes))) => {
            let body = format!(
                "// ─── oxide migrate: {} manual follow-up(s) ───\n// {}\n\n{}",
                notes.len(),
                notes.join("\n// "),
                new_body,
            );
            out.push(Conversion::rewrite(src.to_path_buf(), dst, body));
        }
        Ok(None) => {
            out.push(Conversion::rename(src.to_path_buf(), dst));
        }
        Err(e) => {
            out.push(Conversion::skip(
                src.to_path_buf(),
                format!("{e}; port by hand"),
            ));
        }
    }
}

fn plan_next_app(root: &Path, src: &Path, out: &mut Vec<Conversion>) {
    let app = root.join("app");
    let rel = match src.strip_prefix(&app) {
        Ok(r) => r,
        Err(_) => return,
    };
    let name = rel.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let stem = strip_ext(name);
    let ext = ext_of(name).unwrap_or("tsx");
    let dir_part = rel.parent().unwrap_or(Path::new("")).to_path_buf();

    if !matches!(ext, "tsx" | "jsx" | "ts" | "js") {
        out.push(Conversion::skip(
            src.to_path_buf(),
            format!("non-route extension `{ext}` — left untouched"),
        ));
        return;
    }
    let dst = match stem {
        "page" | "layout" | "route" | "loading" | "error" | "not-found" | "template" => {
            routes_root(root)
                .join(&dir_part)
                .join(format!("{stem}.{ext}"))
        }
        _ => {
            out.push(Conversion::skip(
                src.to_path_buf(),
                format!(
                    "`{}` is not a Next app-router special file; left untouched",
                    rel.display()
                ),
            ));
            return;
        }
    };
    out.push(Conversion::rename(src.to_path_buf(), dst));
}

/// Rewrite `getServerSideProps` / `getStaticProps` into an Oxide `load`.
/// Returns `Some((new_source, notes))` when at least one rewrite happened;
/// `None` when the file has neither. An `Err` for a GSSP that uses
/// `redirect`/`notFound` — the caller reports a skip.
fn rewrite_next_body(src: &Path) -> Result<Option<(String, Vec<String>)>, String> {
    let mut body = match std::fs::read_to_string(src) {
        Ok(b) => b,
        Err(e) => return Err(format!("read {}: {e}", src.display())),
    };
    let mut notes: Vec<String> = Vec::new();
    let mut changed = false;
    for (fn_name, static_mode) in [("getServerSideProps", false), ("getStaticProps", true)] {
        let Some((start, end)) = find_export_function(&body, fn_name) else {
            continue;
        };
        let original = body[start..end].to_string();
        if original.contains("redirect:") || original.contains("notFound:") {
            return Err(format!(
                "{fn_name} uses redirect/notFound — Oxide's load throws or returns a redirect; port the body by hand"
            ));
        }
        let Some(open) = original.find('{') else {
            return Err(format!("{fn_name} has no body"));
        };
        let Some(close) = original.rfind('}') else {
            return Err(format!("{fn_name} is malformed"));
        };
        let inner = &original[open + 1..close];
        let mut replacement = format!(
            "// oxide migrate: `{fn_name}` → `load`\n\
             export async function load(event) {{\n  \
               const __props = await (async (context) => {{{inner}}})(event);\n  \
               return (__props && __props.props) || {{}};\n\
             }}\n"
        );
        if static_mode {
            replacement.push_str("\nexport const config = { render: 'static' };\n");
        }
        body = format!("{}{}{}", &body[..start], replacement, &body[end..]);
        changed = true;
        notes.push(format!(
            "`{fn_name}` converted to `load` — verify any Request/Response shapes"
        ));
    }
    Ok(if changed { Some((body, notes)) } else { None })
}

/// Find `export [async] function NAME(...) { … }` and return the byte span
/// covering `export` through the matching closing brace. Returns `None` when
/// the export is absent.
fn find_export_function(src: &str, name: &str) -> Option<(usize, usize)> {
    let patterns = [
        format!("export async function {name}"),
        format!("export function {name}"),
    ];
    for pat in &patterns {
        let Some(idx) = src.find(pat.as_str()) else {
            continue;
        };
        let after = &src[idx..];
        let Some(open_rel) = after.find('{') else {
            continue;
        };
        let mut depth = 0i32;
        let mut end_rel = 0usize;
        for (i, c) in after[open_rel..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end_rel = open_rel + i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        if end_rel > 0 {
            let end = idx + end_rel;
            return Some((idx, end));
        }
    }
    None
}

// ─── Nuxt ──────────────────────────────────────────────────────────────────

fn plan_nuxt(root: &Path) -> Result<MigrationPlan, String> {
    let markers = ["nuxt.config.ts", "nuxt.config.js"];
    if !markers.iter().any(|m| root.join(m).is_file()) {
        return Err(format!(
            "no Nuxt project detected (missing {})",
            markers.join(" / ")
        ));
    }
    let pages = root.join("pages");
    let mut conv = Vec::new();
    if pages.is_dir() {
        for p in collect_files(&pages) {
            let rel = match p.strip_prefix(&pages) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let name = rel.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if ext_of(name) != Some("vue") {
                conv.push(Conversion::skip(
                    p.clone(),
                    format!("`{}` is not a `.vue` page; left untouched", rel.display()),
                ));
                continue;
            }
            let stem = strip_ext(name);
            let dir_part = rel.parent().unwrap_or(Path::new("")).to_path_buf();
            let dst_dir = if stem == "index" {
                routes_root(root).join(&dir_part)
            } else {
                routes_root(root).join(&dir_part).join(stem)
            };
            let dst = dst_dir.join("page.vue");
            // Nuxt `useFetch` / `useAsyncData` need a sibling `page.server.ts`
            // — a cross-file move this codemod does not perform. Attach the
            // follow-up as a note on the rename (rather than a phantom file),
            // so the report names the real source file and the real problem.
            let body = std::fs::read_to_string(&p).unwrap_or_default();
            let note = if body.contains("useFetch(") || body.contains("useAsyncData(") {
                "uses useFetch/useAsyncData — port the fetch into a sibling `page.server.ts`"
                    .to_string()
            } else {
                String::new()
            };
            if note.is_empty() {
                conv.push(Conversion::rename(p, dst));
            } else {
                conv.push(Conversion::rename_with_note(p, dst, note));
            }
        }
    }
    Ok(MigrationPlan {
        source: "nuxt".to_string(),
        label: "Nuxt".to_string(),
        conversions: conv,
    })
}

// ─── SvelteKit ─────────────────────────────────────────────────────────────

fn plan_sveltekit(root: &Path) -> Result<MigrationPlan, String> {
    let markers = ["svelte.config.js", "svelte.config.ts"];
    if !markers.iter().any(|m| root.join(m).is_file()) {
        return Err(format!(
            "no SvelteKit project detected (missing {})",
            markers.join(" / ")
        ));
    }
    let routes = root.join("src/routes");
    if !routes.is_dir() {
        return Err("SvelteKit project has no src/routes/".to_string());
    }
    let mut conv = Vec::new();
    for p in collect_files(&routes) {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let renamed = match name {
            "+page.svelte" => "page.svelte",
            "+page.ts" => "page.ts",
            "+page.server.ts" => "page.server.ts",
            "+page.js" => "page.js",
            "+page.server.js" => "page.server.js",
            "+layout.svelte" => "layout.svelte",
            "+layout.ts" => "layout.ts",
            "+layout.server.ts" => "layout.server.ts",
            "+layout.js" => "layout.js",
            "+layout.server.js" => "layout.server.js",
            "+error.svelte" => "error.svelte",
            other if other.starts_with('+') => {
                conv.push(Conversion::skip(
                    p.clone(),
                    format!("`{other}` is a SvelteKit special; port by hand"),
                ));
                continue;
            }
            _ => {
                // Not a SvelteKit route file — leave it in place (it might
                // be an imported component or a lib file).
                continue;
            }
        };
        let dst = p.with_file_name(renamed);
        conv.push(Conversion::rename(p, dst));
    }
    Ok(MigrationPlan {
        source: "sveltekit".to_string(),
        label: "SvelteKit".to_string(),
        conversions: conv,
    })
}

// ─── Vite ──────────────────────────────────────────────────────────────────

fn plan_vite(root: &Path) -> Result<MigrationPlan, String> {
    let markers = ["vite.config.ts", "vite.config.js", "vite.config.mjs"];
    if !markers.iter().any(|m| root.join(m).is_file()) {
        return Err(format!(
            "no Vite project detected (missing {})",
            markers.join(" / ")
        ));
    }
    let mut conv = Vec::new();
    // The Vite **config** is the marker (a library-mode Vite project has no
    // index.html at all — the entry is a package `main`/`exports`). The
    // absence of a conventional entry is a documented follow-up, not a
    // detection failure.
    let candidates = ["src/main.tsx", "src/main.ts", "src/main.jsx", "src/main.js"];
    let main = candidates
        .iter()
        .map(|p| root.join(p))
        .find(|p| p.is_file());
    match main {
        Some(ref main_path) => {
            let dst = routes_root(root).join("page.tsx");
            match rewrite_vite_main(main_path) {
                Ok((body, notes)) => {
                    let header = if notes.is_empty() {
                        String::new()
                    } else {
                        format!(
                            "// ─── oxide migrate: {} follow-up(s) ───\n// {}\n\n",
                            notes.len(),
                            notes.join("\n// "),
                        )
                    };
                    conv.push(Conversion::rewrite(
                        main_path.clone(),
                        dst,
                        format!("{header}{body}"),
                    ));
                }
                Err(e) => {
                    conv.push(Conversion::skip(
                        main_path.clone(),
                        format!("{e}; port the mount by hand"),
                    ));
                }
            }
        }
        None => {
            // Report against the config so the user sees a real source path;
            // `index.html` may or may not exist (a library-mode project has
            // none). Never a hard failure — a project with no app entry has
            // nothing to convert but is still a valid Vite project.
            let anchor = if root.join("index.html").is_file() {
                root.join("index.html")
            } else {
                root.join(markers[0])
            };
            conv.push(Conversion::skip(
                anchor,
                "no src/main.{tsx,ts,jsx,js} found — port the mount by hand".to_string(),
            ));
        }
    }
    Ok(MigrationPlan {
        source: "vite".to_string(),
        label: "Vite".to_string(),
        conversions: conv,
    })
}

/// Strip the `createRoot(...).render(...)` / `ReactDOM.render(...)` mount call
/// from a Vite main file. The component the mount used is preserved as the
/// page's default export (best-effort: if the mount referenced `<App/>`, we
/// emit `export default App;`; otherwise we emit a placeholder that reports
/// what to fill in).
fn rewrite_vite_main(src: &Path) -> Result<(String, Vec<String>), String> {
    let body = std::fs::read_to_string(src).map_err(|e| format!("read {}: {e}", src.display()))?;
    let mut notes: Vec<String> = Vec::new();

    // Find a `createRoot` or `ReactDOM.render` call and the identifier it
    // renders (`<App />` or `<App/>` or `createElement(App)`).
    let mut mounted: Option<String> = None;
    for marker in ["createRoot", "ReactDOM.render"] {
        if let Some(idx) = body.find(marker) {
            // Look for `render(<Name` within the next ~200 bytes.
            let window = &body[idx..body.len().min(idx + 500)];
            for opener in ["render(<", "render( <"] {
                if let Some(o) = window.find(opener) {
                    let rest = &window[o + opener.len()..];
                    let name: String = rest
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect();
                    if !name.is_empty() {
                        mounted = Some(name);
                    }
                    break;
                }
            }
            if mounted.is_some() {
                break;
            }
        }
    }

    // Strip every line that mentions createRoot / ReactDOM.render and the
    // whole `ReactDOM.render(...)` block. Simplest form: remove lines
    // containing those markers plus a trailing `);`.
    let stripped: String = body
        .lines()
        .filter(|line| {
            let t = line.trim();
            !(t.contains("createRoot(")
                || t.contains("ReactDOM.render(")
                || t.starts_with("root.render(")
                || (t == ");" && body.contains("createRoot(")))
        })
        .collect::<Vec<_>>()
        .join("\n");

    let mounted_name = mounted.unwrap_or_else(|| "App".to_string());
    if !body.contains(&format!("function {mounted_name}"))
        && !body.contains(&format!("const {mounted_name}"))
    {
        notes.push(format!(
            "could not find `{mounted_name}` in this file — the page's default export needs a component"
        ));
    }
    let rewritten = format!(
        "{stripped}\n\n// oxide migrate: the mount call was stripped — the component it rendered is the page.\nexport default {mounted_name};\n"
    );
    Ok((rewritten, notes))
}
