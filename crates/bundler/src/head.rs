//! The adapter-level head producer — the V2-E04 tail (spec §10.2 step 4 +
//! §V2-E04). The manifest already records everything a prerendered shell's
//! `<head>` needs: `RouteBundle.preload` is the route's modulepreload list
//! in critical-first order and `RouteBundle.css` its stylesheet files, both
//! keyed by the client-dir-relative file name the `integrity` map (and the
//! render `Injector`'s `integrity_file_for_url`) resolves. What was missing
//! is the seam that hands those URLs to the render layer: the E04 record
//! left the `Injector.preload`/`css_links` fields wired-and-ready with no
//! producer feeding them.
//!
//! The seam is a two-method trait (the `PageRenderer`/`LoaderRunner`
//! conventions: `name()` for the log surfaces, one projection method), so
//! the CLI can hold it behind `&dyn` and a future adapter-native producer
//! — one that reads build output the manifest does not carry — substitutes
//! without touching the wire-up. [`ManifestHeadProducer`] is the
//! manifest-driven implementation; it resolves route ids the same way the
//! CLI shim's `inject_route_css` does (exact key, `+ /page`, and the
//! `_param_` sanitized dynamic form), so both halves of the head pipeline
//! agree on one route-key spelling.
//!
//! Division of labor with the existing head emission (the design decision
//! the wire-up encodes): the adapter's own head output — the shim's
//! `inject_route_css` through the neutral model — stays authoritative for
//! the stylesheets it already renders into the shell; the wire-up site
//! dedupes by href, so the producer **completes** the shell's head instead
//! of duplicating it. `modulepreload` links are producer-owned: nothing
//! else emits them. The `integrity` + `crossorigin` attributes on every
//! emitted link come from the Injector's digest map (`security.sri` gates
//! the map upstream); the producer itself is a pure projection and stays
//! knob-free.

use crate::{ClientManifest, RouteBundle};
use std::collections::HashSet;

/// The head links a route's shell should carry: the manifest-derived
/// `modulepreload` URLs (critical first, §10.2 step 4) and the stylesheet
/// URLs, spelled the way a served page fetches them (`/client/<file>` —
/// the same shape `Injector::integrity_file_for_url` resolves to a digest,
/// so every link the producer emits is one the Injector can stamp).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RouteHeadLinks {
    /// `modulepreload` URLs, manifest (critical-first) order, deduplicated.
    pub preload: Vec<String>,
    /// stylesheet URLs, manifest order, deduplicated.
    pub css: Vec<String>,
}

/// The adapter-level head producer: given a route id, the manifest-derived
/// head links its shell should carry.
///
/// Object safe by design — the pipeline holds producers behind
/// `&dyn HeadProducer` (the `PageRenderer` trait conventions), and fixture
/// implementations are the seam's test vocabulary.
pub trait HeadProducer: Send + Sync {
    /// The producer's name (the inspector/log surface convention). A
    /// `&'static str` — producers are named by their construction, never
    /// dynamically.
    fn name(&self) -> &'static str;

    /// The head links for `route`. `None` when the producer has no bundle
    /// for the route — the wire-up leaves the render `Injector`'s
    /// `preload`/`css_links` untouched, i.e. the pre-milestone bytes. A
    /// route WITH a bundle but no recorded links yields `Some` with empty
    /// vectors: "this route has a bundle, it simply carries no links".
    fn head_for(&self, route: &str) -> Option<RouteHeadLinks>;
}

/// The manifest-driven [`HeadProducer`]: a typed `ClientManifest`
/// projection. The CLI constructs it from the parsed
/// `client.manifest.json` at build time; a pre-E04 manifest that fails the
/// typed parse degrades to no producer (no links), never a build failure.
///
/// Route-key resolution mirrors the CLI shim's `inject_route_css` exactly —
/// three shapes, in order of specificity:
/// 1. `<route>` — the exact key (`__boot__`, or a manifest already keyed
///    canonically),
/// 2. `<route>/page` — the static route form the bundler writes,
/// 3. `<route-with-_param_>/page` — the dynamic route form (`[slug]`
///    normalized to `_slug_`, the bundler's sanitized chunk name).
#[derive(Debug, Clone)]
pub struct ManifestHeadProducer {
    manifest: ClientManifest,
    base_url: String,
}

impl ManifestHeadProducer {
    /// Produce from an owned typed manifest; URLs are emitted under the
    /// default `/client/` prefix the bundler's output layout serves.
    #[must_use]
    pub fn new(manifest: ClientManifest) -> Self {
        Self {
            manifest,
            base_url: "/client/".to_string(),
        }
    }

    /// Re-route the emitted URLs under another prefix (a deploy that
    /// serves the client dir from a CDN path, say). A missing trailing
    /// slash is normalized so the file name always lands on its own
    /// segment.
    #[must_use]
    pub fn with_base_url(mut self, base: impl Into<String>) -> Self {
        let mut base = base.into();
        if !base.ends_with('/') {
            base.push('/');
        }
        self.base_url = base;
        self
    }

    fn client_url(&self, file: &str) -> String {
        format!("{}{file}", self.base_url)
    }

    /// The three-shape route-key resolution (see the type's docs).
    fn route_bundle_for(&self, route: &str) -> Option<&RouteBundle> {
        let routes = &self.manifest.routes;
        if let Some(bundle) = routes.get(route) {
            return Some(bundle);
        }
        if let Some(bundle) = routes.get(&format!("{route}/page")) {
            return Some(bundle);
        }
        let normalized: String = route
            .split('/')
            .map(|seg| {
                if seg.len() >= 2 && seg.starts_with('[') && seg.ends_with(']') {
                    format!("_{}_", &seg[1..seg.len() - 1])
                } else {
                    seg.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("/");
        routes.get(&format!("{normalized}/page"))
    }
}

impl HeadProducer for ManifestHeadProducer {
    fn name(&self) -> &'static str {
        "manifest-head-producer"
    }

    fn head_for(&self, route: &str) -> Option<RouteHeadLinks> {
        let bundle = self.route_bundle_for(route)?;
        Some(RouteHeadLinks {
            preload: dedup_preserving_order(&bundle.preload)
                .into_iter()
                .map(|f| self.client_url(&f))
                .collect(),
            css: dedup_preserving_order(&bundle.css)
                .into_iter()
                .map(|f| self.client_url(&f))
                .collect(),
        })
    }
}

/// First-seen-order dedup: the manifest's preload order is contractual
/// (critical first, §10.2 step 4), so a repeated chunk drops out rather
/// than resorting the list.
fn dedup_preserving_order(files: &[String]) -> Vec<String> {
    let mut seen = HashSet::with_capacity(files.len());
    let mut out = Vec::with_capacity(files.len());
    for file in files {
        if seen.insert(file.as_str()) {
            out.push(file.clone());
        }
    }
    out
}
