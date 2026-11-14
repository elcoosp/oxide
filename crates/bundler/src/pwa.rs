//! The PWA worker emitter (§16.6 + §V2-F14) — the deferred half of the
//! offline-first module: the precache plan, the service-worker source and
//! the `dist/sw.js` emission, as a pure module over the client manifest.
//!
//! Three pieces, each pinned by `tests/pwa_tdd.rs`:
//!
//! 1. [`ShellSource`] — the seam: the prerendered route shells this build
//!    produced (URL paths, `/` and `/about`). The CLI's prerender walk
//!    implements it at the build call site; tests and examples inject
//!    fakes, so the emitter stays V8-free.
//! 2. [`plan_precache`] — the pure plan: the build-id-versioned cache
//!    name, the deduped byte-sorted precache URL list (the breadth is the
//!    `offline.precache` key's, whose docs this module implements
//!    verbatim: `routes` = the prerendered route shells, `all` = every
//!    hashed asset the client manifest carries — the navigation shell
//!    rides both) and the resolved offline fallback (`sw.offlineFallback`
//!    override → `/offline` when the build produced it → `/`, the
//!    `SwConfig` comment's exact order).
//! 3. [`sw_source`] / [`emit_pwa`] — the deterministic worker source and
//!    its write to the dist root (`/sw.js`, the scope the worker needs to
//!    answer navigations).
//!
//! The worker's fetch contract is the spec's: `/__data` responses serve
//! stale-while-revalidate; navigations fall back to the resolved route
//! shell or the network; hashed `/client/` assets are cache-first. With
//! `offline.outbox` on, the activate step fans an `OXIDE_OUTBOX_FLUSH`
//! message out to every client — a build swap is a flush point, and the
//! client outbox listener (`packages/core/src/client/outbox.ts`) is the
//! consumer.
//!
//! Call-site contract: the CLI build emits when the build produced a
//! client manifest and `offline.enabled` + `sw.enabled` both hold —
//! `offline.enabled = false` "installs no service worker" (the
//! `OfflineConfig` docs), and `sw.enabled = false` disables emission even
//! with a manifest (the `SwConfig` docs). The bundler module itself is
//! total: the caller owns the gate.

use crate::ClientManifest;
use oxide_fw_config::{OfflineConfig, OfflinePrecache, SwConfig};
use std::collections::BTreeSet;
// The `sw_source` renderer writes through `write!` — `String`'s `Write`
// impl never fails, so every call is `let _ =`.
use std::fmt::Write as _;
use std::path::Path;

/// The prerendered route shells this build produced — the seam the CLI's
/// prerender walk implements (the emitter never touches the disk).
///
/// The values are the URL paths the worker should cache, exactly as a
/// browser navigation requests them (`/`, `/about`, `/offline`): a
/// cached navigation must match the request URL, so the shells are
/// precached under their route spelling, not their `prerendered/*.html`
/// file spelling.
pub trait ShellSource {
    /// The URL paths of the prerendered pages, byte-sorted by the
    /// `BTreeSet` the CLI's deterministic walk feeds.
    fn shell_urls(&self) -> BTreeSet<String>;
}

// The `&T` blanket impl (the `AcceptedGenerations` / `IdempotencyStore`
// pattern): a caller holding a concrete source by reference passes it
// where the plan wants `&dyn ShellSource` without re-wrapping.
impl<T: ShellSource + ?Sized> ShellSource for &T {
    fn shell_urls(&self) -> BTreeSet<String> {
        (**self).shell_urls()
    }
}

/// The emission plan ([`plan_precache`]'s output): everything
/// [`sw_source`] needs, nothing else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrecachePlan {
    /// The build id the manifest carries (D28: the cache-version
    /// dimension).
    pub build_id: String,
    /// The versioned cache name, `ox-<build_id>`: a new build swaps
    /// caches and the activate step deletes the stale ones.
    pub cache_name: String,
    /// The URLs to precache at install — deduped and byte-sorted (a
    /// stable diff across builds).
    pub precache: Vec<String>,
    /// The resolved navigation-fallback route: `sw.offlineFallback`
    /// override → `/offline` when the build produced it → `/`.
    pub offline_fallback: String,
    /// Whether the worker fans the `OXIDE_OUTBOX_FLUSH` message on
    /// activate (the `offline.outbox` key).
    pub outbox: bool,
}

/// Plan the worker's precache surface from the client manifest plus the
/// typed `sw:` / `offline:` config keys.
///
/// The breadth is `offline.precache`'s, per that key's own docs:
/// `routes` precaches exactly the [`ShellSource`] shells (the
/// navigation-fallback surface — never client chunks); `all` unions in
/// every hashed asset the manifest carries (`/client/<file>` — the head
/// producer's URL spelling, so a precached chunk is the one the manifest's
/// integrity map digests).
#[must_use]
pub fn plan_precache(
    manifest: &ClientManifest,
    sw: &SwConfig,
    offline: &OfflineConfig,
    shells: &dyn ShellSource,
) -> PrecachePlan {
    let shell_urls = shells.shell_urls();
    let offline_fallback = resolve_offline_fallback(sw, &shell_urls);
    // `routes` = the shells; `all` = the shells + the hashed asset graph.
    let mut precache = shell_urls;
    if offline.precache == OfflinePrecache::All {
        for route in manifest.routes.values() {
            precache.insert(client_url(&route.entry_chunk));
            for file in route.preload.iter().chain(route.css.iter()) {
                precache.insert(client_url(file));
            }
        }
        for file in manifest.assets.values() {
            precache.insert(client_url(file));
        }
    }
    PrecachePlan {
        build_id: manifest.build_id.clone(),
        cache_name: format!("ox-{}", manifest.build_id),
        precache: precache.into_iter().collect(),
        offline_fallback,
        outbox: offline.outbox,
    }
}

/// The `SwConfig` comment's contract, enforced: `sw.offlineFallback`
/// overrides; otherwise `/offline` when the build produced that route;
/// otherwise `/` (the precached shell).
fn resolve_offline_fallback(sw: &SwConfig, shells: &BTreeSet<String>) -> String {
    if let Some(over) = &sw.offline_fallback {
        return over.clone();
    }
    if shells.contains("/offline") {
        "/offline".to_string()
    } else {
        "/".to_string()
    }
}

/// The client URL for an emitted file name — the head producer's
/// `/client/<file>` spelling, the one the manifest's integrity map keys.
fn client_url(file: &str) -> String {
    format!("/client/{file}")
}

/// Render the service worker source for the plan. Deterministic: the same
/// plan emits byte-identical source (the build's reproducibility contract).
#[must_use]
pub fn sw_source(plan: &PrecachePlan) -> String {
    let mut src = String::new();
    let _ = write!(
        src,
        "// Oxide service worker — generated by `oxide build` (§16.6, §V2-F14).\n\
         // Do not edit: this file is regenerated on every build.\n\
         //\n\
         // The cache name is versioned by build id (D28): a new build activates a\n\
         // fresh cache and deletes every stale `ox-` cache, so a deployed\n\
         // generation never serves another build's hashed assets.\n\
         const CACHE = '{}';\n\
         const PRECACHE = [\n",
        plan.cache_name,
    );
    for url in &plan.precache {
        let _ = writeln!(src, "  '{url}',");
    }
    let _ = write!(
        src,
        "];\n\
         // The navigation fallback: a navigation the cache cannot answer is\n\
         // served the shell this build prerendered for that route.\n\
         const FALLBACK_ROUTE = '{}';\n\n",
        plan.offline_fallback,
    );
    src.push_str("self.addEventListener('install', (event) => {\n");
    src.push_str(
        "  event.waitUntil(caches.open(CACHE).then((cache) => cache.addAll(PRECACHE)));\n",
    );
    src.push_str("});\n\n");
    src.push_str("self.addEventListener('activate', (event) => {\n");
    src.push_str("  event.waitUntil(\n");
    src.push_str("    caches.keys()\n");
    src.push_str("      .then((keys) => Promise.all(\n");
    src.push_str("        keys.filter((key) => key.startsWith('ox-') && key !== CACHE)\n");
    src.push_str("          .map((key) => caches.delete(key)),\n");
    src.push_str("      ))\n");
    if plan.outbox {
        src.push_str(
            "      // A build swap is a flush point: queued actions replay once the new\n",
        );
        src.push_str("      // generation is live. The clients answer with the outbox flush\n");
        src.push_str("      // (packages/core/src/client/outbox.ts listens for this message).\n");
        src.push_str("      .then(() => self.clients.matchAll({ includeUncontrolled: true }))\n");
        src.push_str("      .then((clients) => {\n");
        src.push_str("        for (const client of clients) client.postMessage({ type: 'OXIDE_OUTBOX_FLUSH' });\n");
        src.push_str("      })\n");
    }
    src.push_str("  );\n});\n\n");
    src.push_str("self.addEventListener('fetch', (event) => {\n");
    src.push_str("  const req = event.request;\n");
    src.push_str("  if (req.method !== 'GET') return;\n");
    src.push_str("  const url = new URL(req.url);\n");
    src.push_str("  if (url.origin !== self.location.origin) return;\n");
    src.push_str("  // Data responses serve stale-while-revalidate: the cached copy answers\n");
    src.push_str("  // immediately while the network copy refreshes the cache behind it.\n");
    src.push_str("  if (url.pathname === '/__data') {\n");
    src.push_str("    event.respondWith(\n");
    src.push_str("      caches.open(CACHE).then((cache) => {\n");
    src.push_str("        const refresh = fetch(req).then((res) => {\n");
    src.push_str("          if (res.ok) cache.put(req, res.clone());\n");
    src.push_str("          return res;\n");
    src.push_str("        });\n");
    src.push_str("        event.waitUntil(refresh);\n");
    src.push_str("        return cache.match(req).then((hit) => hit ?? refresh);\n");
    src.push_str("      }),\n");
    src.push_str("    );\n");
    src.push_str("    return;\n");
    src.push_str("  }\n");
    src.push_str("  // Navigations: the cached route shell first, the offline fallback\n");
    src.push_str("  // second, the network last.\n");
    src.push_str("  if (req.mode === 'navigate') {\n");
    src.push_str("    event.respondWith(\n");
    src.push_str("      caches.open(CACHE).then((cache) =>\n");
    src.push_str("        cache.match(req)\n");
    src.push_str("          .then((hit) => hit ?? cache.match(FALLBACK_ROUTE))\n");
    src.push_str("          .then((hit) => hit ?? fetch(req)),\n");
    src.push_str("      ),\n");
    src.push_str("    );\n");
    src.push_str("    return;\n");
    src.push_str("  }\n");
    src.push_str("  // Hashed client assets are immutable per build: cache-first, populating\n");
    src.push_str("  // the cache on the way through.\n");
    src.push_str("  if (url.pathname.startsWith('/client/')) {\n");
    src.push_str("    event.respondWith(\n");
    src.push_str("      caches.open(CACHE).then((cache) =>\n");
    src.push_str("        cache.match(req).then((hit) =>\n");
    src.push_str("          hit ??\n");
    src.push_str("          fetch(req).then((res) => {\n");
    src.push_str("            if (res.ok) cache.put(req, res.clone());\n");
    src.push_str("            return res;\n");
    src.push_str("          }),\n");
    src.push_str("        ),\n");
    src.push_str("      ),\n");
    src.push_str("    );\n");
    src.push_str("  }\n");
    src.push_str("});\n");
    src
}

/// Write the worker to the dist root (`<out_dir>/sw.js` — the root scope
/// a navigation-fallback worker needs). The write matches the client
/// manifest's convention (one plain write at the end of the build step).
///
/// # Errors
///
/// Propagates the `sw.js` write's I/O error.
pub fn emit_pwa(out_dir: &Path, plan: &PrecachePlan) -> std::io::Result<()> {
    std::fs::write(out_dir.join("sw.js"), sw_source(plan))
}
