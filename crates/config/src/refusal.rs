//! The config side of the §18 config-key refusal seam (V2-R02's config
//! half, spec §V2-R02): the typed `OxideConfig` mapped into the adapters
//! crate's [`ConfigFeatureReport`], plus the [`ProjectFacts`] the driver
//! supplies.
//!
//! The dependency direction is the recorded V2-R02 ruling: the adapters
//! crate never sees `oxide.config.ts`, so the CONFIG layer consults the
//! matrix (config → adapters), not the other way around. The mapping owns
//! the config-key vocabulary (`fonts.subset: ['used']`); the driver owns
//! the project facts (the route tree's `live` opt-ins, the `*.server.rs`
//! walk, the effective render modes) — this module carries the facts
//! verbatim and is never the judge of them.

use crate::OxideConfig;
use oxide_fw_adapters::ConfigFeatureReport;

/// The project facts the config-key refusal needs that no config key
/// carries. Computed by the driver (the CLI build, which scanned the
/// route tree and can walk the project sources) and carried verbatim by
/// [`OxideConfig::feature_report`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProjectFacts {
    /// Any route opted into the live loaders (`page.server.ts` config
    /// `live: true`, V2-F05 — the scanner reads it into
    /// `RouteEntry::live`).
    pub live_routes: bool,
    /// Native Rust server function sources (`**/*.server.rs`, V2-F08's
    /// project surface) are present.
    pub rust_fns: bool,
    /// Any routable page renders non-`static` (§10.1 — the effective
    /// render mode, explicit or inferred).
    pub non_static_routes: bool,
}

/// The [`ConfigFeatureReport`] over a typed config plus project facts.
/// Copyable, borrow-only (no I/O behind the trait methods — the refusal
/// seam is pure).
#[derive(Debug, Clone, Copy)]
pub struct FeatureReport<'a> {
    config: &'a OxideConfig,
    facts: ProjectFacts,
}

impl OxideConfig {
    /// The feature report the §18 config-key refusal consults: this
    /// config's keys plus the driver's project facts (spec §V2-R02, the
    /// V2-R02 config half).
    #[must_use]
    pub fn feature_report(&self, facts: ProjectFacts) -> FeatureReport<'_> {
        FeatureReport {
            config: self,
            facts,
        }
    }
}

impl ConfigFeatureReport for FeatureReport<'_> {
    fn live_routes_present(&self) -> bool {
        // A project fact (the route tree's `live` opt-ins) — not a config
        // key: the `live` config section is inert defaults until a route
        // opts in (the V2-F05 inert-defaults contract).
        self.facts.live_routes
    }

    fn rust_fns_present(&self) -> bool {
        // A project fact (the `**/*.server.rs` walk, V2-F08). F08's loader
        // is still todo; the refusal is the enforcement arm the registry
        // already names (RFN-002), live from day one — a project carrying
        // the files gets a correct refusal on every target that cannot
        // load them.
        self.facts.rust_fns
    }

    fn search_configured(&self) -> bool {
        // The F09 config key (Appendix V2.L): `search.collections` —
        // a non-empty list is the "search configured" surface. The §16.5
        // collection indexes are static JSON (the registry's own remedy
        // for unsupported targets), so their presence must NOT set this —
        // only the key does. The F09 engine consumes the same key's query
        // knobs (`fields`/`snippet`/`languages`) when it lands.
        !self.config.search.collections.is_empty()
    }

    fn font_subset_used(&self) -> bool {
        // The C08 config key: `fonts.subset: ['used']` — the glyph-usage
        // spelling is the `"used"` entry inside the subset list.
        self.config.fonts.subset.iter().any(|s| s == "used")
    }

    fn non_static_routes_present(&self) -> bool {
        // A project fact (the effective §10.1 render modes of the scanned
        // route tree).
        self.facts.non_static_routes
    }
}
