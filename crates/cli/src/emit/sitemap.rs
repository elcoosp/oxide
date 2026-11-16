//! Moved verbatim from lib.rs by scripts/split_cli.py (V2-A01).

use crate::*;

/// Derive the request URL from a prerender output path. `prerendered/index.html`
/// → `/`; `prerendered/blog/hello.html` → `/blog/hello`; `prerendered/foo.html`
/// → `/foo`. Used to give the framework renderer a plausible URL for a static
/// route (it may use it for canonical links, caching keys, etc.).
/// §16.3: write `sitemap.xml` and `robots.txt` into `dist/`.
///
/// The sitemap lists every URL-addressable page route. A dynamic route
/// (`[slug]`) is included only when the build resolved concrete paths for
/// it — the enumerations from `generateParams`, or the ones the
/// prerender pass already wrote under `dist/prerendered/`. Slots
/// (`@modal`) and API endpoints are not URLs a crawler visits and are
/// skipped.
///
/// Absolute URLs come from `config.siteUrl` (falling back to the
/// `OXIDE_SITE_URL` env var). Without one, the sitemap still emits — with
/// site-relative `<loc>` values — because a sitemap that is only missing
/// the origin is more useful than no sitemap, and robots.txt simply omits
/// the `Sitemap:` line rather than pointing at a broken URL.
pub(crate) fn emit_sitemap_and_robots(
    cfg: &oxide_fw_config::OxideConfig,
    tree: &oxide_fw_router::RouteTree,
    dist: &Path,
    w: Writer<'_>,
) -> anyhow::Result<()> {
    // Origin: config wins, then env. Normalized to no trailing slash.
    let origin = cfg
        .site_url
        .clone()
        .or_else(|| std::env::var("OXIDE_SITE_URL").ok())
        .map(|s| s.trim_end_matches('/').to_string());

    // Absolute URL for a path, or the path itself when no origin is set.
    let abs = |path: &str| -> String {
        match &origin {
            Some(o) => format!("{o}{path}"),
            None => path.to_string(),
        }
    };

    // Collect the URL list: static routes from segments, plus the dynamic
    // routes that were actually prerendered. `dist/prerendered/**.html` is
    // the ground truth for "which URL exists".
    let mut urls: Vec<String> = Vec::new();
    for e in &tree.entries {
        if !matches!(e.kind, oxide_fw_router::RouteKind::Page) {
            continue;
        }
        if e.slot.is_some() {
            continue;
        }
        // Only routes with no dynamic segment are unconditionally listed;
        // dynamic ones are covered by the prerendered-file walk below.
        let has_dynamic = e
            .segments
            .iter()
            .any(|s| s.contains(':') || s.contains('*') || s.contains('{'));
        if has_dynamic {
            continue;
        }
        let path = url_path_from_rel(&format!("prerendered/{}.html", e.segments.join("/")));
        urls.push(abs(&path));
    }

    // Prerendered files (including dynamic-route enumerations).
    let prerendered_root = dist.join("prerendered");
    let mut all_files: Vec<PathBuf> = Vec::new();
    walk_files(&prerendered_root, &mut all_files);
    for f in all_files {
        if f.extension().is_none_or(|x| x != "html") {
            continue;
        }
        let Ok(rel) = f.strip_prefix(&prerendered_root) else {
            continue;
        };
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        let path = url_path_from_rel(&format!("prerendered/{rel_str}"));
        let u = abs(&path);
        if !urls.contains(&u) {
            urls.push(u);
        }
    }
    urls.sort();
    urls.dedup();

    // Write the two files. Both live at `dist/` root so the static-asset
    // store serves them at `/sitemap.xml` and `/robots.txt`.
    if !urls.is_empty() {
        let refs: Vec<&str> = urls.iter().map(String::as_str).collect();
        let xml = oxide_fw_assets::sitemap_xml(&refs);
        std::fs::write(dist.join("sitemap.xml"), xml)
            .map_err(|e| anyhow::anyhow!("write sitemap.xml: {e}"))?;
    }
    let sitemap_url = origin.as_ref().map(|o| format!("{o}/sitemap.xml"));
    // `robots.txt` semantics: allow everything by default, which is what
    // an empty rule list means. A project that wants to disallow adds a
    // rule in the future `robots` config field; today the default is the
    // sensible one — you do not ship a sitemap you want ignored.
    let txt = oxide_fw_assets::robots_txt(&[], sitemap_url.as_deref());
    std::fs::write(dist.join("robots.txt"), txt)
        .map_err(|e| anyhow::anyhow!("write robots.txt: {e}"))?;

    outln!(
        w,
        "sitemap: {} url(s){}{}",
        urls.len(),
        if origin.is_some() {
            ""
        } else {
            " (relative — set siteUrl for absolute URLs)"
        },
        if sitemap_url.is_some() {
            ""
        } else {
            " (no siteUrl; robots has no Sitemap: line)"
        },
    );
    Ok(())
}
