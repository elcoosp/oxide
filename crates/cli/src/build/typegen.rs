//! Moved verbatim from build/cmd.rs by scripts/build_phase_split.py (V2-A01 phase 2).
//!
//! phase_typegen — routes.d.ts + i18n message catalogs
//! (spec WP-A01 phase 2). Regenerates `.oxide/types/routes.d.ts` (locale-aware
//! overloads under i18n) and the `messages.d.ts` + per-locale/per-route catalog
//! emission of §16-D.

use crate::*;

// >>> phase-wrapper (not part of the moved body; split_selfcheck --phases strips this block)
pub(crate) fn phase_typegen(cx: &mut BuildCtx<'_>) -> anyhow::Result<()> {
    let rootp = &cx.rootp;
    let dist = &cx.dist;
    let cfg = &cx.cfg;
    let tree = &cx.tree;
    let w = &mut *cx.w;
    // <<< phase-wrapper
    // SPEC-09 / §8.3: regenerate `.oxide/types/routes.d.ts` from the build's
    // route tree so `href()`/`PageProps` stay in sync with the routes without
    // a separate `oxide routes` run. A typegen failure is a warning, not a
    // build failure — the app still ships.
    // SPEC-16.4: when i18n is configured, typegen emits the locale-aware
    // `href` overloads (route, params, locale) + a Locale union.
    let i18n_for_typegen: Option<&[String]> = match cfg.i18n.strategy.as_str() {
        "none" => None,
        _ if cfg.i18n.locales.is_empty() => None,
        _ => Some(&cfg.i18n.locales),
    };
    let default_for_typegen: Option<&str> = if i18n_for_typegen.is_some() {
        Some(cfg.i18n.default_locale.as_str())
    } else {
        None
    };
    if let Err(e) = tree.typegen_with_root_and_i18n(
        rootp,
        &rootp.join(&cfg.routes.dir),
        i18n_for_typegen,
        default_for_typegen,
    ) {
        outln!(
            w,
            "warn: typegen failed ({} {}); routes.d.ts is stale",
            e.0.code,
            e.0.message
        );
    }
    // SPEC-16.4 / §16-D: load the message catalogs from `locales/` and
    // write `.oxide/types/messages.d.ts` (typed MessageKey union + per-
    // locale LocaleMessages + DefaultLocale). No-op when there's no
    // `locales/` dir; a parse error is a warning (same convention as
    // routes.d.ts).
    {
        let locales_root = rootp.join("locales");
        match oxide_fw_assets::i18n::load_catalog_from_dir(&locales_root) {
            Ok(cat) if !cat.entries.is_empty() => {
                let out_path = rootp.join(".oxide/types/messages.d.ts");
                if let Err(e) = oxide_fw_assets::i18n::write_messages_dts(&cat, &out_path) {
                    outln!(
                        w,
                        "warn: messages.d.ts typegen failed ({} {}); stale",
                        e.0.code,
                        e.0.message,
                    );
                }
                // §16-D: ship the catalog. Without this a client island has
                // no messages at all — the typed keys exist, the values do
                // not reach the browser. One JSON file per locale under
                // `dist/client/messages/`; the client loader fetches the
                // active locale's file lazily.
                //
                // Per-route splitting (only the keys a route uses) is a
                // size optimization on top: a route that names its keys can
                // ship a subset. The full-catalog file is the baseline that
                // makes the feature work; splitting lands when a route's
                // key usage is recorded at scan time.
                let messages_dir = dist.join("client/messages");
                if let Err(e) = std::fs::create_dir_all(&messages_dir) {
                    outln!(w, "warn: cannot create dist/client/messages: {e}");
                } else {
                    let mut emitted = 0usize;
                    for (locale, entries) in &cat.entries {
                        let body = serde_json::json!({
                            "locale": locale,
                            "messages": entries,
                        });
                        let path = messages_dir.join(format!("{locale}.json"));
                        if std::fs::write(&path, serde_json::to_vec(&body).unwrap_or_default())
                            .is_ok()
                        {
                            emitted += 1;
                        }
                    }
                    outln!(
                        w,
                        "i18n: emitted {emitted} message catalog(s) -> dist/client/messages/"
                    );
                }
                // §16-D per-route splitting: scan each route's sources for
                // `t('key')` calls and emit only the keys that route names.
                // The client loader prefers `/client/messages/<locale>/<route>.json`
                // and falls back to the full catalog, so a route that names
                // no keys costs nothing extra and one that names a few ships
                // a few.
                let per_route_dir = messages_dir.clone();
                let mut routes_with_keys = 0usize;
                for e in &tree.entries {
                    if !matches!(e.kind, oxide_fw_router::RouteKind::Page) {
                        continue;
                    }
                    let route_file = rootp.join(&cfg.routes.dir).join(&e.file);
                    let server_file = route_file.with_file_name("page.server.ts");
                    let mut keys: std::collections::BTreeSet<String> =
                        std::collections::BTreeSet::new();
                    for f in [&route_file, &server_file] {
                        if let Ok(src) = std::fs::read_to_string(f) {
                            for k in scan_message_keys(&src) {
                                keys.insert(k);
                            }
                        }
                    }
                    if keys.is_empty() {
                        continue;
                    }
                    // The route id used by the client loader is the same
                    // `componentFor` key the adapter sees: the route
                    // directory with any trailing `/page` stripped.
                    let route_key =
                        e.id.0
                            .strip_suffix("/page")
                            .unwrap_or(&e.id.0)
                            .trim_start_matches("routes/")
                            .replace('/', "_");
                    if route_key.is_empty() {
                        continue;
                    }
                    routes_with_keys += 1;
                    for (locale, entries) in &cat.entries {
                        // The catalog keys are namespaced by the file they
                        // came from (`locales/en/common.json` → `common.`),
                        // but an author writes `t('blog.title')` without the
                        // namespace — it is a file-location artifact, not
                        // part of the message id. Match on the suffix: a
                        // route key `blog.title` selects `common.blog.title`.
                        let subset: std::collections::BTreeMap<&String, &String> = entries
                            .iter()
                            .filter(|(k, _)| {
                                keys.contains(*k)
                                    || keys.iter().any(|want| {
                                        k.as_str() == want.as_str()
                                            || k.ends_with(&format!(".{want}"))
                                    })
                            })
                            .collect();
                        if subset.is_empty() {
                            continue;
                        }
                        let dir = per_route_dir.join(locale);
                        let _ = std::fs::create_dir_all(&dir);
                        let body = serde_json::json!({
                            "locale": locale,
                            "route": route_key,
                            "messages": subset,
                        });
                        let _ = std::fs::write(
                            dir.join(format!("{route_key}.json")),
                            serde_json::to_vec(&body).unwrap_or_default(),
                        );
                    }
                }
                if routes_with_keys > 0 {
                    outln!(
                        w,
                        "i18n: per-route message subsets for {routes_with_keys} route(s)"
                    );
                }
            }
            Ok(_) => { /* no locales/ dir — nothing to typegen */ }
            Err(e) => outln!(
                w,
                "warn: catalog load failed ({} {}); messages.d.ts skipped",
                e.0.code,
                e.0.message,
            ),
        }
    }

    // >>> phase-wrapper (not part of the moved body; split_selfcheck --phases strips this block)
    Ok(())
    // <<< phase-wrapper
}
