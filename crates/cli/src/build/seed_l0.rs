//! Moved verbatim from build/cmd.rs by scripts/build_phase_split.py (V2-A01 phase 2).
//!
//! phase_seed_l0 — route search index + L0 seed manifest
//! (spec WP-A01 phase 2). Emits the §16.5 route search index and the
//! PERF-04 L0 seed manifest (default-locale variants only) so `oxide start`
//! serves seeded routes as first-request hits.

use crate::*;

// >>> phase-wrapper (not part of the moved body; split_selfcheck --phases strips this block)
pub(crate) fn phase_seed_l0(cx: &mut BuildCtx<'_>) -> anyhow::Result<()> {
    let dist = &cx.dist;
    let cfg = &cx.cfg;
    let w = &mut *cx.w;
    let build_id = cx
        .build_id
        .as_ref()
        .expect("phase_bundle_server sets build_id");
    let seed_bodies = cx
        .seed_bodies
        .take()
        .expect("phase_prerender sets seed_bodies");
    let prerendered = cx
        .prerendered
        .as_ref()
        .expect("phase_prerender sets prerendered");
    // <<< phase-wrapper
    // §16.5: emit the route search index. Same source as the L0 seed
    // manifest — the `prerendered` map (route_id → output paths) — but a
    // different shape and consumer: a client-side island reads it to
    // answer a query. Extracting from the emitted HTML (not the source
    // AST) means a loader-injected `<title>`, a plugin-added heading, or
    // a `defineMeta` override all reach the index as the user sees them.
    if cfg.build.route_search && !prerendered.is_empty() {
        let search_index =
            oxide_fw_assets::route_search::build_route_search_index(prerendered, dist);
        if let Ok(json) = oxide_fw_assets::route_search::route_search_index_to_json(&search_index) {
            let path = dist.join("client/search-index.json");
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let _ = std::fs::write(&path, json);
            outln!(
                w,
                "search index: {} route(s) -> dist/client/search-index.json",
                search_index.entries.len()
            );
        }
    }

    // PERF-04: write the L0 seed manifest. Each prerendered page's sentineled
    // HTML is stored so `oxide start` can insert it into L0 at boot — a
    // seeded route is an L0 hit on the very first request (no warm-up miss).
    if !seed_bodies.is_empty() {
        let mut entries = Vec::with_capacity(seed_bodies.len());
        for (rid, url, sentineled, status, locale) in &seed_bodies {
            // §16.4 / O2: only seed the *default-locale* variant. The L0 key
            // is route + params + vary; the runtime's `l0_vary` is header-only
            // today (accept-language), so a locale-prefixed URL like `/fr/about`
            // and `/about` collapse to the same key. Seeding a non-default
            // locale would write an entry the runtime never hits, and would
            // collide with the default-locale entry. The non-default pages
            // serve from `prerendered/<locale>/<route>.html` via the normal
            // static-asset path. Making per-locale L0 seeding useful needs a
            // dispatch change (append the resolved locale to `l0_vary`);
            // tracked as a §21 follow-up.
            let is_default_variant = locale.is_empty() || locale == &cfg.i18n.default_locale;
            if !is_default_variant {
                continue;
            }
            // The key matches what the live server computes for a request
            // with no `accept-language`: route id (no `/page` suffix), no
            // params, vary `lang=`.
            let rid = rid.strip_suffix("/page").unwrap_or(rid);
            let key = l0_cache::L0Cache::key_for(rid, "", "lang=", build_id);
            entries.push(serde_json::json!({
                "key": key,
                "url": url,
                "body": sentineled,
                "status": status,
            }));
        }
        let seed = serde_json::json!({ "build_id": build_id, "entries": entries });
        let path = dist.join("server/l0-seed.json");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(&seed) {
            // Micro-debt retirement (the precompile/Watcher class): a seed
            // write failure used to vanish (`let _ =`), so `oxide start`
            // booted without L0 seeding and nobody could tell why the first
            // request was a miss. Warn with the path and the error.
            if let Err(e) = std::fs::write(&path, bytes) {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "l0 seed manifest write failed; first requests will be L0 misses"
                );
            }
        } else {
            tracing::warn!(
                path = %path.display(),
                "l0 seed manifest serialize failed; first requests will be L0 misses"
            );
        }
    }

    // >>> phase-wrapper (not part of the moved body; split_selfcheck --phases strips this block)
    Ok(())
    // <<< phase-wrapper
}
