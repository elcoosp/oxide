//! Moved verbatim from lib.rs by scripts/split_cli.py (V2-A01).

use crate::*;

/// The build's prerendered route shells, in the URL spelling a navigation
/// requests — the call-site implementation of the emitter's
/// [`oxide_fw_bundler::pwa::ShellSource`] seam (the emitter never touches
/// the disk; the walk lives here where the files are).
///
/// `dist/prerendered/**.html` is the ground truth for "which URL exists"
/// (the sitemap walk's rule, shared verbatim): `url_path_from_rel` maps the
/// file spelling to the route spelling (`prerendered/about.html` →
/// `/about`, `prerendered/index.html` → `/`), so a cached navigation
/// matches its request URL. A missing root — a pure-SPA build with no
/// prerendered routes — is an empty shell set; the worker still serves the
/// fallback chain.
pub(crate) struct PrerenderedShells {
    pub(crate) urls: std::collections::BTreeSet<String>,
}

impl PrerenderedShells {
    pub(crate) fn collect(prerendered_root: &Path) -> anyhow::Result<Self> {
        let mut files = Vec::new();
        if prerendered_root.is_dir() {
            walk_files(prerendered_root, &mut files);
        }
        let mut urls = std::collections::BTreeSet::new();
        for f in files {
            if f.extension().is_none_or(|x| x != "html") {
                continue;
            }
            let Ok(rel) = f.strip_prefix(prerendered_root) else {
                continue;
            };
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            urls.insert(url_path_from_rel(&format!("prerendered/{rel_str}")));
        }
        Ok(Self { urls })
    }
}

pub(crate) fn url_path_from_rel(rel: &str) -> String {
    let stripped = rel.strip_prefix("prerendered/").unwrap_or(rel);
    let stripped = stripped.strip_suffix(".html").unwrap_or(stripped);
    if stripped == "index" || stripped.is_empty() {
        "/".to_string()
    } else {
        format!("/{stripped}")
    }
}

/// The boot `<script type=module>` tag, shared by the prerender injector
/// and the live SSR dispatch arm — one definition site for the attribute
/// order (nonce, integrity, crossorigin).
///
/// V2-E04: when the client manifest carries an integrity digest for the
/// boot chunk and `security.sri` is on, the tag gains `integrity` +
/// `crossorigin="anonymous"` — a CDN serving a modified chunk then fails
/// closed instead of executing. `None` (dev's `/@boot/` virtual module, or
/// `security.sri: false`) keeps the pre-E04 bytes.
pub(crate) fn boot_script_tag(boot_url: &str, nonce: &str, integrity: Option<&str>) -> String {
    let src = if boot_url.starts_with('/') {
        boot_url.to_string()
    } else {
        format!("/{boot_url}")
    };
    match integrity {
        Some(digest) => format!(
            r#"<script type="module" src="{src}" integrity="{digest}" crossorigin="anonymous" nonce="{nonce}"></script>"#
        ),
        None => format!(r#"<script type="module" src="{src}" nonce="{nonce}"></script>"#),
    }
}

/// V2-F17: the dev-token page wire — `window.__oxide.devToken` — so the
/// cost overlay (`packages/core/src/client/cost-overlay.ts`) can speak the
/// dev-token-guarded `GET /__oxide/cost` endpoint. The token is the SEC-09
/// dev session token the serve boot mints when the inspector is attached;
/// the tag carries the PAGE nonce (a nonce'd CSP executes only nonce'd
/// inline scripts), and the payload is JSON-encoded with the deferred-tag
/// builder's `<` escape — a value that could break out of the script
/// element cannot, whatever the minting path does someday.
///
/// `None` — production, or a dev boot with the inspector off — emits
/// nothing: the tag only exists on the dev wire, and every page stays
/// byte-identical without it.
pub(crate) fn dev_token_tag(token: Option<&str>, nonce: &str) -> String {
    let Some(token) = token else {
        return String::new();
    };
    // serde_json for the payload (no untrusted string concatenation), then
    // the `<` escape that keeps a JSON string from terminating the tag.
    let json = serde_json::to_string(&token).unwrap_or_else(|_| "\"\"".into());
    let safe = json.replace('<', "\\u003c");
    format!(
        r#"<script type="application/oxide-dev-token" nonce="{nonce}">window.__oxide=window.__oxide||{{}};window.__oxide.devToken={safe}</script>"#
    )
}

/// V2-E04: the manifest `integrity` digest for a boot URL of the form
/// `client/<file>` (or `/client/<file>`). `integrity_node` is the
/// manifest's top-level `integrity` object; a pre-E04 manifest (no key), a
/// dev URL (`/@boot/…`) or an unbundled chunk yields `None`.
pub(crate) fn manifest_integrity_for(
    integrity_node: Option<&serde_json::Value>,
    boot_url: &str,
) -> Option<String> {
    let file = boot_url
        .strip_prefix("client/")
        .or_else(|| boot_url.strip_prefix("/client/"))?;
    integrity_node
        .and_then(|i| i.get(file))
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// P-20: one prerender target (a route + one `generateParams` entry).
/// §16.4 / O2: locale set to prerender for. A project without i18n
/// config returns a single empty-string locale; one with i18n returns
/// one entry per declared locale.
pub(crate) fn prerender_locales(cfg: &oxide_fw_config::OxideConfig) -> Vec<String> {
    if cfg.i18n.locales.is_empty() || cfg.i18n.strategy == "none" {
        vec![String::new()]
    } else {
        cfg.i18n.locales.clone()
    }
}

/// §16.4 / O2: on-disk relative path and URL path for a locale's variant.
/// `base_rel` is a `.html`-suffixed path like `index.html` or
/// `nested/page.html`. Returns the path under `prerendered/` (which the
/// caller joins) and the request URL path.
pub(crate) fn prefixed_prerender_path(
    base_rel: &str,
    locale: &str,
    cfg: &oxide_fw_config::OxideConfig,
) -> (String, String) {
    if locale.is_empty() {
        return (
            format!("prerendered/{base_rel}"),
            url_path_from_rel(base_rel),
        );
    }
    let should_prefix = match cfg.i18n.strategy.as_str() {
        "prefix-always" => true,
        "prefix-never" => false,
        _ => locale != cfg.i18n.default_locale, // prefix-except-default
    };
    if should_prefix {
        let url = url_path_from_rel(&format!("{locale}/{base_rel}"));
        (format!("prerendered/{locale}/{base_rel}"), url)
    } else {
        (
            format!("prerendered/{base_rel}"),
            url_path_from_rel(base_rel),
        )
    }
}

pub struct PrerenderTarget {
    pub(crate) rel: String,
    pub(crate) url_path: String,
    pub(crate) params: BTreeMap<String, String>,
    /// §16.4 / O2: the locale this target renders for. Empty when the
    /// project has no i18n config.
    pub(crate) locale: String,
}

/// P-20: render a route's `generateParams` targets with bounded parallelism.
pub struct PrerenderTargetsArgs<'a> {
    pub dist: &'a Path,
    pub route_id: &'a str,
    pub pattern: &'a str,
    pub targets: Vec<PrerenderTarget>,
    pub nonce: &'a str,
    pub nonce_for: &'a str,
    pub page_renderer: Option<&'a std::sync::Arc<dyn oxide_fw_server::PageRenderer>>,
    pub loader_runner: Option<&'a std::sync::Arc<dyn oxide_fw_server::LoaderRunner>>,
    pub module_path: Option<&'a Path>,
    pub boot_script_url: Option<&'a str>,
    /// V2-E04: the manifest's `sha384-` digest for the boot chunk, when
    /// `security.sri` is on and the manifest carries one. `None` keeps the
    /// tag byte-identical (dev's `/@boot/` virtual module, SRI off).
    pub boot_script_integrity: Option<&'a str>,
    /// V2-E04: the manifest's whole `integrity` map, attached to the
    /// Injector so any modulepreload/stylesheet link whose URL resolves to
    /// a digest emits `integrity` + `crossorigin` (empty map = pre-E04
    /// bytes). Keyed by the client-dir-relative file name.
    pub manifest_integrity: &'a std::collections::BTreeMap<String, String>,
    /// V2-E04 tail: the adapter-level head producer's links for this route
    /// (modulepreload + stylesheet URLs the shell should carry). `None`
    /// keeps the Injector's fields untouched — the pre-milestone bytes.
    pub head_links: Option<&'a oxide_fw_bundler::head::RouteHeadLinks>,
    /// V2-F13: the learned-prefetch advertisement (`prefetch.learned`)
    /// passed through to every [`write_prerender`] call.
    pub prefetch: bool,
    pub layout_ids: &'a [String],
    pub layout_modules: &'a [(String, std::path::PathBuf)],
    pub error_route: Option<&'a str>,
    pub plugin_head: &'a serde_json::Value,
    pub prev_prerenders: &'a std::collections::BTreeMap<
        (String, String),
        &'a crate::build_graph::RoutePrerenderRecord,
    >,
    pub route_src_hash: &'a str,
}

///
/// Reuse (an unchanged source hash + an existing output file) is cheap and
/// stays synchronous. Only the targets that must actually render go through
/// the worker pool, at most `render_concurrency()` at a time. Results are
/// returned in target order, so seed manifests and graph records stay
/// deterministic regardless of completion order.
pub(crate) async fn prerender_targets(
    args: PrerenderTargetsArgs<'_>,
) -> anyhow::Result<(Vec<(PrerenderTarget, String, u16)>, usize)> {
    let PrerenderTargetsArgs {
        dist,
        route_id,
        pattern,
        targets,
        nonce,
        nonce_for,
        page_renderer,
        loader_runner,
        module_path,
        boot_script_url,
        boot_script_integrity,
        manifest_integrity,
        head_links,
        prefetch,
        layout_ids,
        layout_modules,
        error_route,
        plugin_head,
        prev_prerenders,
        route_src_hash,
    } = args;

    use futures::StreamExt as _;

    let mut results: Vec<Option<(String, u16)>> = (0..targets.len()).map(|_| None).collect();
    let mut to_render: Vec<usize> = Vec::new();
    let mut reused = 0usize;
    for (i, t) in targets.iter().enumerate() {
        match try_reuse_prerender(
            &dist.join(&t.rel),
            route_id,
            &t.url_path,
            route_src_hash,
            prev_prerenders,
        ) {
            Some((saved, status, old_nonce)) => {
                let fresh = saved.replace(&old_nonce, nonce);
                if std::fs::write(dist.join(&t.rel), &fresh).is_ok() {
                    reused += 1;
                    // V2-A07: the reused page keeps its sidecar current —
                    // write-or-clear by the record's status, so a previous
                    // build's error status neither vanishes nor outlives a
                    // return to 200.
                    if let Err(e) =
                        oxide_fw_server::write_status_sidecar(&dist.join(&t.rel), status)
                    {
                        tracing::warn!(
                            rel = %t.rel,
                            error = %e,
                            "prerender reuse: status sidecar persist failed"
                        );
                    }
                    results[i] = Some((fresh, status));
                } else {
                    to_render.push(i);
                }
            }
            None => to_render.push(i),
        }
    }

    let concurrency = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(2, 8);
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(concurrency));
    let mut renders = futures::stream::iter(to_render.into_iter().map(|i| {
        let sem = std::sync::Arc::clone(&sem);
        let t = &targets[i];
        async move {
            let _permit = sem.acquire().await;
            let out = write_prerender(PrerenderArgs {
                dist,
                rel: &t.rel,
                route_id,
                pattern,
                url_path: &t.url_path,
                params: &t.params,
                locale: &t.locale,
                nonce: nonce_for,
                page_renderer,
                loader_runner,
                module_path,
                boot_script_url,
                boot_script_integrity,
                manifest_integrity,
                head_links,
                prefetch,
                layouts: layout_ids,
                layout_module_paths: layout_modules,
                error_route_id: error_route,
                plugin_head,
            })
            .await;
            (i, out)
        }
    }))
    .buffer_unordered(concurrency);
    while let Some((i, out)) = renders.next().await {
        results[i] = Some(out?);
    }
    drop(renders);

    let mut ordered = Vec::with_capacity(targets.len());
    for (t, r) in targets.into_iter().zip(results) {
        let Some((html, status)) = r else {
            continue;
        };
        ordered.push((t, html, status));
    }
    Ok((ordered, reused))
}

pub struct PrerenderArgs<'a> {
    pub dist: &'a Path,
    pub rel: &'a str,
    pub route_id: &'a str,
    pub pattern: &'a str,
    pub url_path: &'a str,
    pub params: &'a BTreeMap<String, String>,
    pub locale: &'a str,
    pub nonce: &'a str,
    pub page_renderer: Option<&'a std::sync::Arc<dyn oxide_fw_server::PageRenderer>>,
    pub loader_runner: Option<&'a std::sync::Arc<dyn oxide_fw_server::LoaderRunner>>,
    pub module_path: Option<&'a Path>,
    pub boot_script_url: Option<&'a str>,
    /// V2-E04: the manifest's `sha384-` digest for the boot chunk, when
    /// `security.sri` is on and the manifest carries one. `None` keeps the
    /// tag byte-identical (dev's `/@boot/` virtual module, SRI off).
    pub boot_script_integrity: Option<&'a str>,
    /// V2-E04: the manifest's whole `integrity` map, attached to the
    /// Injector (see `PrerenderTargetsArgs.manifest_integrity`).
    pub manifest_integrity: &'a std::collections::BTreeMap<String, String>,
    /// V2-E04 tail: the adapter-level head producer's links for this route
    /// (see `PrerenderTargetsArgs.head_links`).
    pub head_links: Option<&'a oxide_fw_bundler::head::RouteHeadLinks>,
    pub layouts: &'a [String],
    pub layout_module_paths: &'a [(String, std::path::PathBuf)],
    pub error_route_id: Option<&'a str>,
    pub plugin_head: &'a serde_json::Value,
    /// V2-F13: the learned-prefetch advertisement from the typed config
    /// (`prefetch.learned`). `true` makes the injector's hydration
    /// bootstrap advertise `prefetch:true` (the seventh `window.__oxide`
    /// field) so the client's learned prefetcher may consult
    /// `GET /_oxide/nav`; `false` — the default — keeps the bootstrap
    /// byte-identical to the pre-F13 shapes.
    pub prefetch: bool,
}

/// V2-E04 tail: keep only the producer's links whose href the shell does
/// not already carry. Matching is the literal `href="<url>"` the
/// renderers emit — manifest URLs are `/client/<file>` and carry no
/// quotes, so a plain substring test cannot false-positive on an
/// attribute boundary. The order both lists keep is the manifest's
/// (critical first for `preload`, §10.2 step 4).
pub(crate) fn filter_head_links(
    links: &oxide_fw_bundler::head::RouteHeadLinks,
    shell: &str,
) -> (Vec<String>, Vec<String>) {
    let keep = |url: &String| !shell.contains(&format!("href=\"{url}\""));
    (
        links.preload.iter().filter(|u| keep(u)).cloned().collect(),
        links.css.iter().filter(|u| keep(u)).cloned().collect(),
    )
}

pub(crate) async fn write_prerender(args: PrerenderArgs<'_>) -> anyhow::Result<(String, u16)> {
    let PrerenderArgs {
        dist,
        rel,
        route_id,
        pattern,
        url_path,
        params,
        locale,
        nonce,
        page_renderer,
        loader_runner,
        module_path,
        boot_script_url,
        boot_script_integrity,
        manifest_integrity,
        head_links,
        layouts,
        layout_module_paths,
        error_route_id,
        plugin_head,
        prefetch,
    } = args;

    // §9.1: run the loader for this route (if the module exists and a
    // loader runner is attached) to obtain the data the render phase
    // receives. Prerendered static pages previously always wrote `{}`
    // into PageRenderInput.data; they now run the real `load()`.
    let mut data = serde_json::Value::Object(Default::default());
    // 16-G: the loader's `meta` contribution, if its server module exports
    // one. Merged into the render input's head below.
    let mut route_meta: Option<serde_json::Value> = None;
    // §7.1 / §10.2 step 5: resolved defer() values from the load. Prerender
    // used to discard these — the generated page had defer slots that never
    // resolved. They are injected as the same nonce'd script the live path
    // emits (below).
    let mut deferred_values: std::collections::BTreeMap<String, serde_json::Value> =
        std::collections::BTreeMap::new();
    // §9.2: run the layout chain (outer → inner) first, threading
    // `parent_data`, so the page's `await parent()` sees the real layout
    // data — the same sequence the live and /__data paths run.
    let mut parent_data: serde_json::Value = serde_json::Value::Null;
    if let Some(loader) = loader_runner {
        for (lid, lpath) in layout_module_paths {
            let mut linput = oxide_fw_server::LoaderRunInput::new(
                lpath.to_string_lossy().to_string(),
                lid.clone(),
                url_path,
            );
            linput.params = params.clone();
            linput.parent_data = parent_data.clone();
            linput.locale = locale.to_string();
            // Build time has no request middleware; `locals` is `{}`.
            linput.locals = serde_json::json!({});
            match loader.run(linput).await {
                Ok(out) if out.status == 200 => {
                    parent_data = out.data;
                }
                Ok(out) => {
                    // A layout short-circuit during prerender is the same as
                    // a page short-circuit: write a diagnostic file.
                    let body = format!(
                        "<!doctype html><html><body>\
                         <h1>layout loader short-circuit: {}</h1><pre>{}</pre>\
                         </body></html>",
                        out.status, out.data
                    );
                    let dest = dist.join(rel);
                    if let Some(parent) = dest.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(&dest, &body)?;
                    // V2-A07: the status sidecar — the serve-time scan turns
                    // this file into a real `out.status` instead of a 200.
                    oxide_fw_server::write_status_sidecar(&dest, out.status)?;
                    return Ok((body, out.status));
                }
                Err(e) => {
                    let body = format!(
                        "<!doctype html><html><body>\
                         <h1>layout loader error</h1><pre>{}: {}</pre>\
                         </body></html>",
                        oxide_fw_render::escape_text(&e.0.code),
                        oxide_fw_render::escape_text(&e.0.message)
                    );
                    let dest = dist.join(rel);
                    if let Some(parent) = dest.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(&dest, &body)?;
                    // V2-A07: the layout-error page is a 500 on disk too.
                    oxide_fw_server::write_status_sidecar(&dest, 500)?;
                    return Ok((body, 500u16));
                }
            }
        }
    }

    if let (Some(loader), Some(mp)) = (loader_runner, module_path) {
        // Match the route-id normalization the renderer will do so user
        // loaders see the same string in `event.route`.
        let route_for_loader = route_id
            .strip_suffix("/page")
            .unwrap_or(route_id)
            .to_string();
        let mut input = oxide_fw_server::LoaderRunInput::new(
            mp.to_string_lossy().to_string(),
            route_for_loader,
            url_path,
        );
        input.params = params.clone();
        // §9.2: the innermost layout's data — what `parent()` reads.
        input.parent_data = parent_data.clone();
        input.locale = locale.to_string();
        match loader.run(input).await {
            Ok(out) if out.status == 200 => {
                data = out.data;
                deferred_values = out.deferred;
                // 16-G: a `meta = defineMeta(fn)` export, evaluated by the
                // worker against this load's data.
                route_meta = out.meta;
            }
            Ok(out) => {
                // Non-200 during build: emit an explicit diagnostic HTML
                // file rather than a silently-empty page. The dev/serve
                // path short-circuits with a real HTTP response; at build
                // time the closest equivalent is a file with the error body.
                let body = format!(
                    "<!doctype html><html><body>\
                     <h1>loader short-circuit: {}</h1><pre>{}</pre>\
                     </body></html>",
                    out.status, out.data
                );
                let dest = dist.join(rel);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&dest, &body)?;
                // V2-A07: the status sidecar — the serve-time scan turns
                // this file into a real `out.status` instead of a 200.
                oxide_fw_server::write_status_sidecar(&dest, out.status)?;
                return Ok((body, out.status));
            }
            Err(e) => {
                // SEC-07: escape worker error text (HTML injection).
                let body = format!(
                    "<!doctype html><html><body>\
                     <h1>loader error</h1><pre>{}: {}</pre>\
                     </body></html>",
                    oxide_fw_render::escape_text(&e.0.code),
                    oxide_fw_render::escape_text(&e.0.message)
                );
                let dest = dist.join(rel);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&dest, &body)?;
                // V2-A07: the worker-error page is a 500 on disk too.
                oxide_fw_server::write_status_sidecar(&dest, 500)?;
                return Ok((body, 500u16));
            }
        }
    }

    let shell = if let Some(renderer) = page_renderer {
        // §7: the adapter's `componentFor` is keyed on the route directory
        // (`routes/index`), not the router's file-role id (`routes/index/page`).
        // Strip the trailing `/page` so the lookup matches — the same
        // normalization build_dispatch performs for the request path.
        let route_for_render = route_id
            .strip_suffix("/page")
            .unwrap_or(route_id)
            .to_string();
        let mut input = oxide_fw_server::PageRenderInput::new(route_for_render, url_path);
        for (k, v) in params {
            input.params.insert(k.clone(), v.clone());
        }
        input.data = data.clone();
        // §8.2: hand the layout chain to the adapter so it can nest the page.
        // The prerender path is separate from the live-dispatch path; both
        // populate this field from the same `MatchInfo.layout_chain`.
        input.layouts = layouts.to_vec();
        // §10.2 step 5: mirror the live-dispatch resolution on the prerender
        // path so a throw in `page.tsx` renders the project's `error.tsx`.
        input.error_route_id = error_route_id.map(str::to_string);
        // §16.4 / O2: the resolved locale this prerender targets. The
        // adapter forwards it to the render, so a page calling `t()` /
        // `getLocale()` knows its language even at build time.
        if !locale.is_empty() {
            input.locale = Some(locale.to_string());
        }
        // §7.1 / §9.2: hand the loader's resolved `defer()` values to the
        // adapter here too. The live-dispatch path populates this; the
        // prerender path collected the same values for the `<script>` tag
        // but never gave them to the render, so a prerendered deferred
        // boundary resolved against `{__deferred: id}` instead of the value.
        input.deferred = deferred_values.clone();
        // §17.1 / §7.1: the build-time nonce this prerender already stamps
        // on its own injected tags. The adapter needs it for the deferred
        // swapper; a prerendered page served under `strict-dynamic` must
        // carry the same nonce in both places.
        if !nonce.is_empty() {
            input.nonce = Some(nonce.to_string());
        }
        // §15.2: merge the plugin's `renderHead` contribution into the
        // neutral model. The adapter's own `head()` output and the plugin's
        // both land in `PageRenderInput.head`, which the adapter's head
        // merge walks.
        if plugin_head
            .as_object()
            .map(|o| {
                o.values()
                    .any(|v| v.as_array().is_some_and(|a| !a.is_empty()))
            })
            .unwrap_or(false)
        {
            input.head = plugin_head.clone();
        }
        // 16-G: the route's own `meta` (from `page.server.ts`) joins the
        // same neutral model. Route-level head is the more specific intent,
        // so its entries land after a plugin's.
        if let Some(meta) = route_meta.as_ref() {
            input.head = merge_neutral_head(input.head, normalize_route_meta(meta.clone()));
        }
        renderer
            .render(input)
            .await
            .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?
    } else {
        // No framework renderer attached (a project without a
        // `componentFor`). The placeholder is the whole document — but both
        // a plugin's `renderHead` and the route's own `meta` contribution
        // belong in it, so the head is built from their merged model rather
        // than hardcoded empty.
        let head_model = match route_meta.as_ref() {
            Some(meta) => {
                merge_neutral_head(plugin_head.clone(), normalize_route_meta(meta.clone()))
            }
            None => plugin_head.clone(),
        };
        format!(
            "<!doctype html><html><head>{}</head><body>\
             <main data-route=\"{pattern}\">placeholder</main></body></html>",
            neutral_head_to_html(&head_model),
        )
    };
    // §10.3: `<Island chunk="X">` emits `data-ox-chunk="X"` — a logical island
    // id, not a runtime URL. The client boot module we generated registers
    // each island under its name, so on the wire the id and the URL are the
    // same string. (If a future slice adds per-island chunks, the rewrite
    // hook lives here.)
    // §10.2 step 5 (prerender): the adapter's buffered output may carry
    // the same wire-level error-status markers the streaming path peeks
    // for. Detect them here (before the injector mutates `shell`) so the
    // status the adapter intended rides the L0 seed and is served as a
    // real 500 by `oxide start` — fixing what used to be a baked 200.
    let prerender_status: u16 = if str_signals_error(&shell) { 500 } else { 200 };
    let mut shell = shell;
    if let Some(url) = boot_script_url {
        shell = inject_island_boot_script(&shell, url, nonce, boot_script_integrity);
    }

    let mut injector = oxide_fw_render::Injector::new(
        oxide_fw_render::HeadTags::default(),
        nonce.to_string(),
        route_id.to_string(),
    )
    // V2-E04: the manifest's integrity map rides the Injector — any
    // modulepreload/stylesheet link a future shell carries whose URL
    // resolves to a digest emits `integrity` + `crossorigin`; an empty map
    // (no manifest entry, SRI off) is the pre-E04 bytes.
    .with_integrity(manifest_integrity.clone())
    // V2-F13: the learned-prefetch advertisement rides the typed config —
    // `prefetch.learned: true` adds the seventh `window.__oxide` field so
    // the client's prefetcher consults `GET /_oxide/nav`; `false` keeps
    // the bootstrap byte-identical (the injector's additive contract).
    .with_prefetch(prefetch);
    // V2-E04 tail: the adapter-level head producer's links — the
    // manifest's critical-first modulepreload list and the route's
    // stylesheet files, spelled `/client/<file>` so every URL resolves to
    // a digest in the map above (`security.sri` gates the ATTRIBUTES, not
    // the links: modulepreload is a §10.2 step 4 perf surface, not a
    // security knob). The filter drops the hrefs the shell already
    // carries — the adapter's own head emission (the shim's
    // `inject_route_css` through the neutral model) stays authoritative
    // for the stylesheets it renders, and the producer completes the head
    // instead of duplicating it.
    if let Some(links) = head_links {
        let (preload, css) = filter_head_links(links, &shell);
        injector.preload = preload;
        injector.css_links = css;
    }
    // PERF-01 / §9.5: embed the page's loader data as an ODF payload so the
    // client's hydration bootstrap has it without a second request. Empty
    // data still encodes (an empty object), matching the live path.
    {
        let payload = serde_json::json!({
            "route": route_id.strip_suffix("/page").unwrap_or(route_id),
            "url": url_path,
            "params": params,
            "data": data,
            // §8.2: the layout chain the client must re-nest on hydration
            // (SSR rendered `<RootLayout><Page/></RootLayout>`; without
            // this, `hydrateRoot` sees the layout markup as unexpected and
            // throws React #418).
            "layouts": layouts,
        });
        let odf = oxide_fw_odf::encode_to_bytes(&oxide_fw_odf::Value::from_json(&payload))
            .unwrap_or_default();
        use base64::Engine as _;
        injector.odf_payload_b64 = Some(base64::engine::general_purpose::STANDARD.encode(&odf));
    }
    // §7.1 / §10.2 step 5: the resolved-deferred script, identical to the
    // live path's (deferred_tag in build_dispatch). Serialized through
    // serde_json (no untrusted concatenation) with `<` escaped so the
    // content cannot break out of the tag. Without this, a prerendered
    // page's DeferredClient slots stayed pending forever (the glue had a
    // payload but no resolution script).
    if !deferred_values.is_empty() {
        let json = serde_json::to_string(&deferred_values).unwrap_or_else(|_| "{}".into());
        // `</script>` and `<!--` inside a JSON string would terminate the
        // tag; `<` escaping is the OWASP-recommended fix and produces valid
        // JS (`\u003c`).
        let safe = json.replace('<', "\\u003c");
        injector.body_extra = format!(
            r#"<script type="application/oxide-deferred" nonce="{nonce}">{safe}</script>"#,
        );
    }
    let html = injector
        .inject_into_html(&shell)
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
    let dest = dist.join(rel);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&dest, &html)?;
    // V2-A07: the status sidecar rides the final page — a non-200 prerender
    // (the adapter's error-status markers or a loader short-circuit) writes
    // `<dest>.status`; a 200 write clears a stale sidecar from a previous
    // build so the status survives incremental rebuilds in BOTH directions.
    oxide_fw_server::write_status_sidecar(&dest, prerender_status)?;
    // PERF-04 / §10.2 step 5: return (html, status) so the caller can seed
    // the L0 entry with the status the adapter's marker indicated.
    Ok((html, prerender_status))
}

/// Hash every source that fed this route's render: the serialized config,
/// the page file, its `page.server.ts`, every layout in the chain (both
/// `layout.*` and `layout.server.*`), and the error boundary when present.
pub(crate) fn route_source_hash(
    rootp: &Path,
    cfg: &OxideConfig,
    tree: &oxide_fw_router::RouteTree,
    entry: &oxide_fw_router::RouteEntry,
    layout_ids: &[String],
    layout_modules: &[(String, std::path::PathBuf)],
    error_route_id: Option<&str>,
) -> String {
    const HASH_VERSION: &str = "route-source-hash-v1";
    let mut h = blake3::Hasher::new();
    h.update(HASH_VERSION.as_bytes());
    h.update(&[0]);
    // The config is hashed as a whole — a config change is a flush.
    if let Ok(cfg_bytes) = serde_json::to_vec(cfg) {
        h.update(&cfg_bytes);
    }
    h.update(&[0]);
    let routes_root = rootp.join(&cfg.routes.dir);
    // Page source + page server module.
    let page_path = routes_root.join(&entry.file);
    hash_file_into(&mut h, &page_path);
    let page_server_path = routes_root.join(entry.file.with_file_name("page.server.ts"));
    hash_file_into(&mut h, &page_server_path);
    // Layout chain (both the layout file and its server module).
    for lid in layout_ids {
        if let Some(le) = tree.entries.iter().find(|x| &x.id.0 == lid) {
            let lfile = routes_root.join(&le.file);
            hash_file_into(&mut h, &lfile);
            let lserver = routes_root.join(le.file.with_file_name("layout.server.ts"));
            hash_file_into(&mut h, &lserver);
        }
    }
    // `layout_modules` also has the on-disk `layout.server.ts` paths — include
    // them so a rename of a layout module (which the id lookup may miss) still
    // invalidates.
    for (_, p) in layout_modules {
        hash_file_into(&mut h, p);
    }
    // Error boundary (if any).
    if let Some(eid) = error_route_id
        && let Some(ee) = tree.entries.iter().find(|x| x.id.0 == eid)
    {
        let efile = routes_root.join(&ee.file);
        hash_file_into(&mut h, &efile);
    }
    h.finalize().to_hex().to_string()
}

/// Hash `path` and the file's content into `h`. A missing file is a
/// well-defined sentinel rather than an error, so the source hash stays
/// stable when a file legitimately doesn't exist (e.g., a page with no
/// `page.server.ts`).
pub(crate) fn hash_file_into(h: &mut blake3::Hasher, path: &Path) {
    h.update(path.to_string_lossy().as_bytes());
    h.update(&[0]);
    match std::fs::read(path) {
        Ok(bytes) => {
            h.update(blake3::hash(&bytes).as_bytes());
        }
        Err(_) => {
            h.update(b"<missing>");
        }
    }
    h.update(&[0]);
}

/// If a prior build's record for `(route_id, url)` has the same source
/// hash and its saved output file exists, return `(html, status, prev_nonce)`.
/// The caller is responsible for replacing `prev_nonce` with the current
/// build's nonce (and rewriting the disk file to match, so a static-file
/// serve sees a matching CSP).
pub(crate) fn try_reuse_prerender(
    disk_path: &Path,
    route_id: &str,
    url: &str,
    src_hash: &str,
    prev_map: &std::collections::BTreeMap<
        (String, String),
        &crate::build_graph::RoutePrerenderRecord,
    >,
) -> Option<(String, u16, String)> {
    let prev = prev_map.get(&(route_id.to_string(), url.to_string()))?;
    if prev.source_hash != src_hash {
        return None;
    }
    if !disk_path.is_file() {
        return None;
    }
    let html = std::fs::read_to_string(disk_path).ok()?;
    Some((html, prev.status, prev.nonce.clone()))
}

/// Read each route's reachable chunk list from the client manifest and hash
/// every chunk file. The result is what a per-chunk incremental build would
/// diff against; recording it makes the question measurable even though
/// Rolldown 1.2.13's API does not expose a per-module rebuild.
pub(crate) fn collect_route_chunks(
    rootp: &Path,
    client_manifest: &serde_json::Value,
) -> Vec<crate::build_graph::RouteChunkRecord> {
    let client_dir = rootp.join("dist/client");
    let mut out = Vec::new();
    let Some(routes) = client_manifest.get("routes").and_then(|r| r.as_object()) else {
        return out;
    };
    for (route_id, row) in routes {
        let mut chunks: Vec<(String, String)> = Vec::new();
        if let Some(entry) = row.get("entry_chunk").and_then(|v| v.as_str()) {
            push_chunk_hash(&client_dir, entry, &mut chunks);
        }
        if let Some(pre) = row.get("preload").and_then(|v| v.as_array()) {
            for p in pre {
                if let Some(f) = p.as_str() {
                    push_chunk_hash(&client_dir, f, &mut chunks);
                }
            }
        }
        chunks.sort();
        chunks.dedup();
        out.push(crate::build_graph::RouteChunkRecord {
            route_id: route_id.clone(),
            chunks,
        });
    }
    out
}

pub(crate) fn push_chunk_hash(dir: &Path, file: &str, out: &mut Vec<(String, String)>) {
    let Ok(bytes) = std::fs::read(dir.join(file)) else {
        return;
    };
    out.push((file.to_string(), blake3::hash(&bytes).to_hex().to_string()));
}

/// B2: hold the document's closing tags until the deferred settles finish,
/// emitting one resolution script per settle as it arrives.
///
/// The render stream ends with `</body></html>`. This adapter withholds the
/// last `suffix_len` bytes, forwards everything before them immediately, then
/// drains the settle receiver (one `<script type="application/oxide-deferred"
/// data-id="…">` per settle, in settle order), and finally emits the
/// tail + the withheld closing bytes.
///
/// The point: the shell and the initial data reach the browser at once; each
/// deferred slot resolves when *its* promise settles, not when the slowest
/// one does. A no-defer route never enters this path (its receiver is empty
/// and the caller uses the buffered `deferred_tag`).
pub(crate) fn strip_close_and_append_settles(
    src: oxide_fw_server::BodyStream,
    mut settles: tokio::sync::mpsc::Receiver<serde_json::Value>,
    nonce: String,
    // §20 / O3b: waterfall context. When present, the spawned drain task
    // records a `deferred-settles` stage on the request's row after the
    // last settle lands. The stage's value is the time from the start of
    // the settle drain to the last delivered settle — i.e. the extra
    // time the settles added *after* the shell finished, not the total
    // request time. That is what the inspector bar means.
    waterfall: Option<oxide_fw_dev::WaterfallRecorder>,
    request_id: String,
    trace_id: Option<String>,
) -> oxide_fw_server::BodyStream {
    use bytes::Bytes;
    use futures::StreamExt as _;

    const CLOSE: &[u8] = b"</body></html>";
    let (tx, mut rx) = tokio::sync::mpsc::channel::<std::result::Result<Bytes, std::io::Error>>(16);
    tokio::spawn(async move {
        let mut src = src;
        // Hold back at least CLOSE.len() bytes so a `</body></html>` split
        // across chunks is still stripped. Keep one extra byte of margin so
        // a chunk boundary cannot defeat the match.
        let hold = CLOSE.len() + 1;
        let mut buf: Vec<u8> = Vec::new();
        while let Some(item) = src.next().await {
            match item {
                Ok(chunk) => {
                    buf.extend_from_slice(&chunk);
                    if buf.len() > hold {
                        let emit_upto = buf.len() - hold;
                        let head = buf[..emit_upto].to_vec();
                        buf.drain(..emit_upto);
                        if tx.send(Ok(Bytes::from(head))).await.is_err() {
                            return; // consumer gone
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(e)).await;
                    return;
                }
            }
        }
        // Strip the trailing close, if present (it always is for a full doc).
        let stripped = if buf.ends_with(CLOSE) {
            buf.truncate(buf.len() - CLOSE.len());
            std::mem::take(&mut buf)
        } else {
            std::mem::take(&mut buf)
        };
        if !stripped.is_empty() && tx.send(Ok(Bytes::from(stripped))).await.is_err() {
            return;
        }
        // §20 / O3b: mark the drain start so `deferred-settles` measures
        // the settle window only, not the whole request (which is already
        // recorded by the caller at dispatch time).
        let settles_started = std::time::Instant::now();
        // §20 / WP-r9 nit: count actual settles so we do not record a
        // ~0 ms "deferred-settles" row on a no-defer stream. An empty
        // record_staged is noise in the inspector.
        let mut settle_count: usize = 0;
        // One resolution script per settle, in settle order. A sender that
        // is never dropped would otherwise hold the document open forever;
        // 30s is far past any real defer and turns a leak into a closed page.
        let deadline = std::time::Duration::from_secs(30);
        loop {
            let v = match tokio::time::timeout(deadline, settles.recv()).await {
                Ok(Some(v)) => v,
                Ok(None) => break, // channel closed: all settles delivered
                Err(_) => {
                    // A settle hung: close the document rather than hold it.
                    tracing::warn!("deferred settle wait timed out; closing the document");
                    break;
                }
            };
            let Some(obj) = v.get("deferredSettle") else {
                continue;
            };
            settle_count += 1;
            let Some(id) = obj.get("id").and_then(|x| x.as_str()) else {
                continue;
            };
            let payload = match obj.get("value") {
                Some(value) => value.clone(),
                None => serde_json::json!({
                    "__oxide_deferred_error": obj
                        .get("error")
                        .and_then(|e| e.as_str())
                        .unwrap_or("deferred rejected")
                }),
            };
            let json = serde_json::to_string(&payload).unwrap_or_else(|_| "null".into());
            let safe = json.replace('<', "\\u003c");
            let tag = format!(
                r#"<script type="application/oxide-deferred" data-id="{id}" nonce="{nonce}">{safe}</script>"#
            );
            if tx.send(Ok(Bytes::from(tag))).await.is_err() {
                return;
            }
        }
        // §20 / O3b: record the settle window on the request's waterfall
        // row. Merges with the initial middleware/load/render record via
        // `record_staged` — the two calls produce one row per request.
        if settle_count > 0
            && let Some(rec) = waterfall
        {
            let settle_ms = settles_started.elapsed().as_secs_f64() * 1000.0;
            rec.record_staged(
                &request_id,
                vec![("deferred-settles".to_string(), settle_ms)],
                trace_id,
            );
        }
        // Close the document only after every settle has been delivered.
        let _ = tx.send(Ok(Bytes::from_static(CLOSE))).await;
        // `tx` drops, closing the body stream.
    });
    Box::pin(futures::stream::poll_fn(move |cx| rx.poll_recv(cx)))
}

/// Bounded LRU store for PPR shells. Insertion-order, not access-order: a
/// shell being served will be re-fetched if evicted, which re-inserts it.
/// A miss costs a tap, not a wrong answer.
pub(crate) struct PprShellEntry {
    html: String,
    /// Same tags the L0 entry carries (`route`, `path:<path>`), so a
    /// revalidate action or SWR refresh can drop the shell with it.
    tags: Vec<String>,
}

#[derive(Default)]
pub(crate) struct PprShellStore {
    map: std::collections::HashMap<String, PprShellEntry>,
    /// Insertion order for eviction. A re-insert appends a fresh occurrence;
    /// the older one is skipped on evict (the map lookup says which is live).
    order: std::collections::VecDeque<String>,
}

impl PprShellStore {
    pub(crate) fn get(&self, key: &str) -> Option<&String> {
        self.map.get(key).map(|e| &e.html)
    }

    /// Drop every shell carrying `tag` (hard invalidation).
    pub(crate) fn invalidate_tag(&mut self, tag: &str) -> usize {
        let doomed: Vec<String> = self
            .map
            .iter()
            .filter(|(_, e)| e.tags.iter().any(|t| t == tag))
            .map(|(k, _)| k.clone())
            .collect();
        for k in &doomed {
            self.map.remove(k);
            if let Some(pos) = self.order.iter().position(|o| o == k) {
                self.order.remove(pos);
            }
        }
        doomed.len()
    }

    /// Drop every shell carrying `path:<path>` (hard invalidation).
    pub(crate) fn invalidate_path(&mut self, path: &str) -> usize {
        self.invalidate_tag(&format!("path:{path}"))
    }

    pub(crate) fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
    }

    /// The live entry count (the invalidation log's dropped count; the
    /// pins assert it directly).
    pub(crate) fn len(&self) -> usize {
        self.map.len()
    }

    /// Test-only introspection: the live keys, sorted (the seed pin reads
    /// the TAP's key to prove the build-side manifest keys with the exact
    /// same identity).
    #[cfg(test)]
    pub(crate) fn keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.map.keys().cloned().collect();
        keys.sort();
        keys
    }

    /// Insert, evicting the oldest live key when at capacity.
    ///
    /// Re-insert of an existing key removes its old order entry first, so a
    /// refreshed shell is at the *back* of the eviction queue. The earlier
    /// form appended a duplicate order entry and relied on the map lookup
    /// to skip stale occurrences — but a re-insert keeps the key in the
    /// map, so the lookup could not distinguish the stale occurrence from
    /// the live one and evicted the freshly-refreshed key.
    ///
    /// `VecDeque::remove(pos)` is O(256); taps run once per route per
    /// process, so the scan is invisible next to the render itself.
    #[cfg(test)]
    pub(crate) fn insert(&mut self, key: String, shell: String) {
        self.insert_tagged(key, shell, Vec::new());
    }

    pub(crate) fn insert_tagged(&mut self, key: String, shell: String, tags: Vec<String>) {
        if self.map.contains_key(&key) {
            // Refresh: drop the stale order occurrence so age is reset.
            if let Some(pos) = self.order.iter().position(|k| k == &key) {
                self.order.remove(pos);
            }
        } else if self.map.len() >= PPR_SHELLS_MAX {
            // New key at capacity: evict the oldest live entry.
            while let Some(oldest) = self.order.pop_front() {
                if self.map.remove(&oldest).is_some() {
                    break;
                }
            }
        }
        self.map
            .insert(key.clone(), PprShellEntry { html: shell, tags });
        self.order.push_back(key);
    }
}

/// First request for a PPR route: the render is streamed through unchanged,
/// while the completed document is split into a shell (holes for each
/// `data-ox-b` boundary) and stored with the nonce replaced by
/// [`l0_cache::NONCE_SENTINEL`] so another request can reuse it.
pub(crate) fn ppr_tap_stream(
    src: oxide_fw_server::BodyStream,
    shells: PprShells,
    key: String,
    nonce: String,
    tags: Vec<String>,
) -> oxide_fw_server::BodyStream {
    use futures::StreamExt as _;
    let (tx, mut rx) =
        tokio::sync::mpsc::channel::<std::result::Result<bytes::Bytes, std::io::Error>>(8);
    tokio::spawn(async move {
        let mut src = src;
        let mut collected: Vec<u8> = Vec::new();
        while let Some(item) = src.next().await {
            match item {
                Ok(chunk) => {
                    collected.extend_from_slice(&chunk);
                    if tx.send(Ok(chunk)).await.is_err() {
                        return;
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(e)).await;
                    return;
                }
            }
        }
        // The client already saw EOF; storing after this point adds no latency.
        if let Ok(html) = String::from_utf8(collected)
            && let Ok(shell) = oxide_fw_render::ppr::split_shell(&html)
            && !shell.hole_ids.is_empty()
        {
            let stored = shell.html.replace(&nonce, l0_cache::NONCE_SENTINEL);
            if let Ok(mut store) = shells.lock() {
                store.insert_tagged(key, stored, tags);
            }
        }
    });
    Box::pin(futures::stream::poll_fn(move |cx| rx.poll_recv(cx)))
}

/// Later requests: emit the cached shell immediately, then forward only the
/// resolved boundary fragments from the live render (`<template data-ox-c>`),
/// dropping the duplicated static document. The resolver in the shell swaps
/// each fragment into its hole as it arrives.
pub(crate) fn ppr_filter_stream(
    shell: String,
    src: oxide_fw_server::BodyStream,
    nonce: String,
) -> oxide_fw_server::BodyStream {
    use futures::StreamExt as _;
    let (tx, mut rx) =
        tokio::sync::mpsc::channel::<std::result::Result<bytes::Bytes, std::io::Error>>(16);
    tokio::spawn(async move {
        let mut src = src;
        let prelude = format!("{shell}{}", oxide_fw_render::ppr::client_resolver(&nonce));
        if tx.send(Ok(bytes::Bytes::from(prelude))).await.is_err() {
            return;
        }
        const OPEN: &[u8] = b"<template data-ox-c=";
        const CLOSE: &[u8] = b"</template>";
        let mut buf: Vec<u8> = Vec::new();
        while let Some(item) = src.next().await {
            match item {
                Ok(chunk) => {
                    buf.extend_from_slice(&chunk);
                    loop {
                        let Some(rel_open) = find_subslice(&buf, OPEN) else {
                            // No boundary yet. Keep only a tail long enough to
                            // contain a split opening tag.
                            if buf.len() > OPEN.len() + 8 {
                                let keep = OPEN.len() + 8;
                                buf.drain(..buf.len() - keep);
                            }
                            break;
                        };
                        let Some(rel_close) = find_subslice(&buf[rel_open..], CLOSE) else {
                            // A boundary may span chunks; drop the prefix and
                            // wait for the rest.
                            buf.drain(..rel_open);
                            break;
                        };
                        let end = rel_open + rel_close + CLOSE.len();
                        let frag = bytes::Bytes::copy_from_slice(&buf[rel_open..end]);
                        buf.drain(..end);
                        if tx.send(Ok(frag)).await.is_err() {
                            return;
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(e)).await;
                    return;
                }
            }
        }
        // `tx` drops → EOF.
    });
    Box::pin(futures::stream::poll_fn(move |cx| rx.poll_recv(cx)))
}

pub(crate) fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}
