#![doc = include_str!("../README.md")]

//! oxide-fw-router — route tree, matcher, typegen, rules engine (spec §8, D8).
//!
//! The route tree is compiled at build into a radix tree (static > param > optional >
//! catch-all priority), stored as an `rkyv` archive, memory-mapped at boot; match is
//! allocation-free on the hot path. `URLPattern` semantics preserved for the edge build.
//!
//! This file is the crate contract: public types and their signatures live here; the
//! implementations live in the private submodules (`scan`, `matcher`, `archive`,
//! `typegen`, `rules`, `segment`, `slot`).
//!
//! Segment encoding (path-to-regexp-lite, see `segment`): static names as-is,
//! `[param]` → `:param`, `[param]{regex}` / `[param=regex]` → `:param{regex}`
//! (the brace form is canonical), `[...rest]` → `*rest`, `[[...optional]]` → `*?optional`.

pub mod slot;
pub use slot::{
    InterceptLevel, InterceptSpec, SlotMatch, SlotSpec, parse_intercept_marker, parse_slot_name,
    resolve_slots,
};

use oxide_fw_diagnostics::{Diagnostic, OxideError};
use rkyv::{Archive, Deserialize as RkyvDeserialize, Serialize as RkyvSerialize};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub type Result<T> = std::result::Result<T, OxideError>;

pub fn err(code: &'static str, msg: impl Into<String>) -> OxideError {
    OxideError(Box::new(Diagnostic::error(code, msg)))
}

/// V2-B02 test oracle: the original linear matcher, retained (D23) so the
/// candidate-index matcher can be pinned to identical behavior by property
/// tests (`tests/index_oracle.rs`). `#[doc(hidden)]`: not part of the
/// documented API surface; additive and behaviorally unchanged.
#[doc(hidden)]
#[must_use]
pub fn match_in_linear(
    entries: &[RouteEntry],
    path: &str,
) -> Option<(usize, BTreeMap<String, String>, i64)> {
    matcher::match_in(entries, path)
}

/// Internal helper: an error with a known fix attached (§19.4).
pub(crate) fn ferr(
    code: &'static str,
    msg: impl Into<String>,
    fix: impl Into<String>,
) -> OxideError {
    OxideError(Box::new(err(code, msg).0.with_fix(fix)))
}

mod archive;
mod index;
mod matcher;
mod rules;
mod scan;
mod segment;
mod typegen;

/// V2-B01: precomputed per-route dispatch answers (`RouteContextTable`, D22).
pub mod context;
pub use context::{LayoutLoad, RouteContext, RouteContextTable, default_pattern_of};

/// V2-B09: synthetic route sets for benches + allocation-budget tests.
/// Compiled only under `cfg(any(test, feature = "bench-util"))`.
pub mod testing;

/// Lowercase kind name used by `table()` and the routes manifest (§19.1).
pub(crate) fn kind_str(k: &RouteKind) -> &'static str {
    match k {
        RouteKind::Page => "page",
        RouteKind::Layout => "layout",
        RouteKind::Loading => "loading",
        RouteKind::Error => "error",
        RouteKind::NotFound => "not-found",
        RouteKind::Template => "template",
        RouteKind::Endpoint => "endpoint",
        RouteKind::Middleware => "middleware",
        RouteKind::Socket => "socket",
    }
}

/// Route entry kinds, from the file conventions (spec §8.1).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Archive, RkyvSerialize, RkyvDeserialize,
)]
pub enum RouteKind {
    Page,
    Layout,
    Loading,
    Error,
    NotFound,
    Template,
    Endpoint,
    Middleware,
    /// §9.6: a `socket.ts` WebSocket route (per-phase handlers).
    Socket,
}

/// A route id is its file-derived path, e.g. `routes/blog/[slug]/page`.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    Hash,
    Archive,
    RkyvSerialize,
    RkyvDeserialize,
)]
pub struct RouteId(pub String);

/// One scanned route module (output of the route scanner over `src/routes`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteEntry {
    pub id: RouteId,
    pub kind: RouteKind,
    /// URL pattern segments, e.g. `["blog", ":slug"]`, catch-all as `["*rest"]`,
    /// optional catch-all as `["*?rest"]`, param constraint `["files/:name{\\.md}"]`.
    pub segments: Vec<String>,
    /// Directory group `(group)` does not appear in the URL.
    pub file: PathBuf,
    /// Render mode inferred or configured (spec §10.1): `static|isr|dynamic|spa|islands`.
    pub render_mode: Option<String>,
    /// Cache tags declared in `page.server.ts` config.
    pub tags: Vec<String>,
    /// Whether a `.server.ts` data module accompanies this entry.
    pub has_server_module: bool,
    /// §9.2: `true` when this page's `page.server.ts` references `parent()`,
    /// meaning the page load must await the innermost layout's data. `false`
    /// pages can have their load run concurrently with the layout chain.
    /// Set by the scanner's source-file heuristic; layouts carry `false`.
    pub needs_parent: bool,
    /// Parallel-route slot marker (§8.5): `Some("@modal")` for entries under an
    /// `@slot` directory. Slot entries reserve identity but are not URL-addressable.
    pub slot: Option<String>,
    /// V2-F05: the `live` opt-in from `page.server.ts` config (`live: true`
    /// or `live: { debounceMs: 100 }`). `None` (the default) keeps the route
    /// out of the live machinery entirely — no SSE endpoint, no bootstrap
    /// field, no hub registration. The hydration bootstrap advertises
    /// `live:true` for opted-in routes and the hub re-runs their loaders
    /// when one of their cache tags is revalidated.
    #[serde(default)]
    pub live: Option<RouteLive>,
}

/// V2-F05: the per-route live configuration read from `page.server.ts`.
/// `live: true` scans to the default (`debounce_ms: None` — the config's
/// `live.debounceMs` applies); `live: { debounceMs: 100 }` carries the
/// override. Serde is only used for the `routes.json` manifest round trip.
#[derive(
    Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Archive, RkyvSerialize, RkyvDeserialize,
)]
pub struct RouteLive {
    /// Per-route rerun debounce override in ms (`debounceMs`); `None` uses
    /// the config default (`live.debounceMs`, 50 ms).
    #[serde(default, rename = "debounceMs", alias = "debounce_ms")]
    pub debounce_ms: Option<u32>,
}

/// Compiled match result (allocation-light: params into a small vec).
#[derive(Debug, Clone, PartialEq)]
pub struct MatchResult {
    pub route: RouteEntry,
    /// Layout chain ids from root to leaf, outermost first.
    pub layout_chain: Vec<RouteId>,
    pub params: BTreeMap<String, String>,
    /// Match score for diagnostics/tests (higher wins).
    pub score: i64,
}

/// V2-B02: zero-copy match result — the winning entry by reference plus
/// borrowed captures. Params are `(Cow<str>, Cow<str>)`: borrowed from the
/// pattern and the URL on the common (no percent-escape) path, owned only for
/// catch-all joins and decoded escapes. Use [`RouteTree::match_ref`] on the
/// hot path; [`RouteTree::match_path`] stays for callers that need owned data.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchRef<'a> {
    pub route: &'a RouteEntry,
    pub params: smallvec::SmallVec<[(std::borrow::Cow<'a, str>, std::borrow::Cow<'a, str>); 4]>,
    /// Match score (higher wins), identical to [`MatchResult::score`].
    pub score: i64,
}

/// V2-B02: zero-copy match over a memory-mapped archive. Unlike the owned
/// [`MatchRef`], the winning entry is identified by index — archived entries
/// live in the mmap and are materialized on demand (see `entry_from_arch`),
/// keeping the mapped bytes the single source of truth.
#[derive(Debug, Clone, PartialEq)]
pub struct ArchivedMatchRef<'a> {
    /// Index into the archive's entry list.
    pub route_index: usize,
    pub params: smallvec::SmallVec<[(std::borrow::Cow<'a, str>, std::borrow::Cow<'a, str>); 4]>,
    pub score: i64,
}

/// Builder: scan a routes directory, build the tree, match, archive.
#[derive(Debug, Clone, Default)]
pub struct RouteTree {
    pub entries: Vec<RouteEntry>,
    /// P-02: per-route layout chains, computed once on first match instead of
    /// re-scanning every entry (and sorting) on every request. Shared across
    /// clones; cleared by [`RouteTree::insert`].
    layout_chains:
        std::sync::Arc<std::sync::RwLock<std::collections::HashMap<String, Vec<RouteId>>>>,
    /// P-06: middleware/error/not-found indexes, built once on first use.
    dispatch: std::sync::Arc<std::sync::RwLock<Option<std::sync::Arc<DispatchIndex>>>>,
    /// V2-B02: candidate index, built once on first match; cleared by
    /// [`RouteTree::insert`]. Cost of a match scales with URL depth, not with
    /// the number of routes (D23).
    index: std::sync::Arc<std::sync::RwLock<Option<std::sync::Arc<index::CandidateIndex>>>>,
}

/// P-06: per-prefix indexes for the middleware chain, error boundary and the
/// not-found route. Built once per tree instead of scanning all entries (and
/// formatting prefixes + sorting) on every request.
#[derive(Debug, Clone, Default)]
pub struct DispatchIndex {
    /// `(base_with_trailing_slash, file)`, sorted outermost → innermost.
    middleware: Vec<(String, std::path::PathBuf)>,
    /// `(base_with_trailing_slash, route_id)`, innermost first.
    errors: Vec<(String, String)>,
    /// `(route_id, layout_chain)` for the not-found route, if any.
    not_found: Option<(String, Vec<RouteId>)>,
}

impl RouteTree {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Scan a routes directory (spec §8.1 conventions: layout/page/loading/error/
    /// not-found/template/route/middleware, `(group)`, `[param]`, `[param]{regex}`,
    /// `[...rest]`, `[[...optional]]`, `@slot` reserved). Deterministic ordering.
    ///
    /// The routes root is `dir` itself, EXCEPT in Next-compat mode: when `dir` contains
    /// an `app/` directory with app-level route modules and `dir` has no top-level route
    /// modules of its own, the scanner rebases into `dir/app` so `app/**/page.tsx` maps
    /// identically. Route ids are file-derived: `routes/<relative path w/o ext>`
    /// (`app/...` in Next-compat mode).
    ///
    /// # Errors
    ///
    /// A scan/diagnostic error when the routes tree violates the §8
    /// conventions (colliding slots, unreadable modules, bad segment syntax).
    pub fn scan(dir: &Path) -> Result<Self> {
        scan::scan_impl(dir)
    }

    /// Resolve the routes directory for a project root (`dir`): an explicit override
    /// wins, else `dir/src/routes`, else Next-compat detection inside [`RouteTree::scan`].
    ///
    /// # Errors
    ///
    /// Same error class as [`Self::scan`]; the override path must exist.
    pub fn scan_with_config(dir: &Path, routes_dir_override: Option<&Path>) -> Result<Self> {
        scan::scan_with_config_impl(dir, routes_dir_override)
    }

    /// Insert a pre-built entry (used by tests, plugins' `routes` hook §15.2).
    pub fn insert(&mut self, entry: RouteEntry) {
        self.entries.push(entry);
        // P-02/P-06/V2-B02: derived caches; drop them on mutation.
        if let Ok(mut c) = self.layout_chains.write() {
            c.clear();
        }
        if let Ok(mut c) = self.dispatch.write() {
            *c = None;
        }
        if let Ok(mut c) = self.index.write() {
            *c = None;
        }
    }

    /// V2-B02: build (once) the candidate index over the current entries.
    pub(crate) fn candidate_index(&self) -> std::sync::Arc<index::CandidateIndex> {
        if let Ok(c) = self.index.read()
            && let Some(idx) = c.as_ref()
        {
            return std::sync::Arc::clone(idx);
        }
        let idx = std::sync::Arc::new(index::CandidateIndex::build(&self.entries));
        if let Ok(mut c) = self.index.write() {
            *c = Some(std::sync::Arc::clone(&idx));
        }
        idx
    }

    /// P-02: cached layout chain for `route_id` (outermost first). Computed
    /// once per id, then cloned per match — no O(entries) re-scan.
    pub(crate) fn layout_chain_cached(&self, route_id: &str) -> Vec<RouteId> {
        if let Ok(c) = self.layout_chains.read()
            && let Some(v) = c.get(route_id)
        {
            return v.clone();
        }
        let chain = matcher::layout_chain(&self.entries, route_id);
        if let Ok(mut c) = self.layout_chains.write() {
            c.insert(route_id.to_string(), chain.clone());
        }
        chain
    }

    /// P-06: build (once) the middleware/error/not-found prefix indexes.
    fn dispatch_index(&self) -> std::sync::Arc<DispatchIndex> {
        if let Ok(c) = self.dispatch.read()
            && let Some(idx) = c.as_ref()
        {
            return std::sync::Arc::clone(idx);
        }
        let mut middleware: Vec<(String, std::path::PathBuf, usize)> = Vec::new();
        let mut errors: Vec<(String, String, usize)> = Vec::new();
        let mut not_found: Option<String> = None;
        for e in &self.entries {
            match e.kind {
                RouteKind::Middleware => {
                    let base =
                        e.id.0
                            .strip_suffix("/middleware")
                            .unwrap_or(&e.id.0)
                            .to_string();
                    middleware.push((base, e.file.clone(), e.id.0.len()));
                }
                RouteKind::Error => {
                    let base = e.id.0.strip_suffix("/error").unwrap_or(&e.id.0).to_string();
                    errors.push((base, e.id.0.clone(), e.id.0.len()));
                }
                RouteKind::NotFound if not_found.is_none() => {
                    not_found = Some(e.id.0.clone());
                }
                _ => {}
            }
        }
        // Outermost middleware first (shorter base); nearest error first
        // (longer base).
        middleware.sort_by_key(|(_, _, len)| *len);
        errors.sort_by_key(|a| std::cmp::Reverse(a.2));
        let prefix = |base: &str| -> String {
            if base == "routes" || base.is_empty() {
                String::new() // matches everything
            } else {
                format!("{base}/")
            }
        };
        let idx = DispatchIndex {
            middleware: middleware
                .into_iter()
                .map(|(base, file, _)| (prefix(&base), file))
                .collect(),
            errors: errors
                .into_iter()
                .map(|(base, id, _)| (prefix(&base), id))
                .collect(),
            not_found: not_found.map(|id| {
                let chain = self.layout_chain_cached(&id);
                (id, chain)
            }),
        };
        let idx = std::sync::Arc::new(idx);
        if let Ok(mut c) = self.dispatch.write() {
            *c = Some(std::sync::Arc::clone(&idx));
        }
        idx
    }

    /// P-06: middleware files wrapping `route_id`, outermost first.
    #[must_use]
    pub fn middleware_files_for(&self, route_id: &str) -> Vec<std::path::PathBuf> {
        let idx = self.dispatch_index();
        idx.middleware
            .iter()
            .filter(|(k, _)| k.is_empty() || route_id.starts_with(k.as_str()))
            .map(|(_, f)| f.clone())
            .collect()
    }

    /// P-06: nearest ancestor `error.tsx` route id for `route_id`.
    #[must_use]
    pub fn error_route_for(&self, route_id: &str) -> Option<String> {
        let idx = self.dispatch_index();
        idx.errors
            .iter()
            .find(|(k, _)| k.is_empty() || route_id.starts_with(k.as_str()))
            .map(|(_, id)| id.clone())
    }

    /// P-06: the not-found route id and its layout chain, if declared.
    #[must_use]
    pub fn not_found_route(&self) -> Option<(String, Vec<RouteId>)> {
        self.dispatch_index().not_found.clone()
    }

    /// Match a URL path against the tree with priority: static > param > optional > catch-all.
    /// i18n prefixes (§16.4) are matched as leading static segments; because static
    /// segments score 4 vs 2 for params, a prefixed exact route (e.g. `/en/blog`) always
    /// beats a `[locale]`-style param fallback at the same position.
    ///
    /// Score: each matched URL segment contributes 4 (static), 2 (param), 1 (optional
    /// catch-all) or 0 (catch-all); ties break on per-position segment priority, then on
    /// longer patterns, then on scan order. Trailing slashes and query strings are
    /// tolerated; params are percent-decoded.
    #[must_use]
    pub fn match_path(&self, path: &str) -> Option<MatchResult> {
        matcher::match_impl(self, path)
    }

    /// V2-B02: zero-copy match over the candidate index — the hot path.
    /// Behaviorally identical to [`Self::match_path`] (pinned by the oracle
    /// property test in `tests/index_oracle.rs`), but the winning entry is
    /// borrowed and captures stay borrowed unless a catch-all join or a
    /// percent-escape forces an owned copy.
    #[must_use]
    pub fn match_ref<'a>(&'a self, path: &'a str) -> Option<MatchRef<'a>> {
        matcher::match_ref_impl(self, path)
    }

    /// Serialize to an rkyv archive (§8.2) written to `out.manifest`.
    ///
    /// # Errors
    ///
    /// When the archive cannot be written (unwritable path, serialization
    /// failure — a bug class, not user input).
    pub fn archive_to(&self, out: &Path) -> Result<()> {
        archive::archive_impl(self, out)
    }

    /// Load zero-copy: memory-map the archive and build an accessor.
    /// Accessing matches through the mapping must not allocate.
    ///
    /// # Errors
    ///
    /// When `path` cannot be opened/mapped, or the archive fails its one-time
    /// validation (corrupt or foreign manifest).
    pub fn load_archived(path: &Path) -> Result<ArchivedTree> {
        archive::load_impl(path)
    }

    /// Typed route generation (§8.3): writes `.oxide/types/routes.d.ts` with params,
    /// search params, loader data and action I/O per route + typed `href()`.
    /// Also writes a machine-readable `.oxide/types/routes.json` manifest used by
    /// `oxide routes` (§19.1). Returns the path of the generated `.d.ts`.
    ///
    /// # Errors
    ///
    /// When `out_dir` cannot be created or the generated files cannot be
    /// written.
    pub fn typegen(&self, out_dir: &Path) -> Result<String> {
        typegen::typegen_impl(self, out_dir, None, None, None)
    }

    /// §8.3 like `typegen`, but with the routes root so a
    /// per-route `page.server.ts` `LoaderData` annotation can be read (the
    /// entry's `file` is routes-root-relative).
    ///
    /// # Errors
    ///
    /// Same as [`Self::typegen`].
    pub fn typegen_with_root(&self, out_dir: &Path, routes_root: &Path) -> Result<String> {
        typegen::typegen_impl(self, out_dir, Some(routes_root), None, None)
    }

    /// SPEC-16.4 / §16-D: locale-aware typegen. When `locales` is `Some`,
    /// the emitted `href()` overloads gain an optional trailing locale
    /// parameter (`href(route, params, locale)`), and every `loc` annotation
    /// documents the supported set. When `None`, behavior matches
    /// [`Self::typegen_with_root`].
    ///
    /// # Errors
    ///
    /// Same as [`Self::typegen`].
    pub fn typegen_with_i18n(
        &self,
        out_dir: &Path,
        locales: Option<&[String]>,
        default_locale: Option<&str>,
    ) -> Result<String> {
        typegen::typegen_impl(self, out_dir, None, locales, default_locale)
    }

    /// Locale-aware variant of [`Self::typegen_with_root`].
    ///
    /// # Errors
    ///
    /// Same as [`Self::typegen`].
    pub fn typegen_with_root_and_i18n(
        &self,
        out_dir: &Path,
        routes_root: &Path,
        locales: Option<&[String]>,
        default_locale: Option<&str>,
    ) -> Result<String> {
        typegen::typegen_impl(self, out_dir, Some(routes_root), locales, default_locale)
    }

    /// Sorted deterministic listing for `oxide routes` (§19.1) incl. inferred modes.
    #[must_use]
    pub fn table(&self) -> Vec<(String, String, Vec<String>)> {
        typegen::table_impl(self)
    }
}

/// Zero-copy accessor over a memory-mapped archive.
///
/// P-01: the archive is validated **once** at load; `root` aliases the mapped
/// bytes for the process lifetime so a match is O(route-fanout), not
/// O(archive-bytes).
pub struct ArchivedTree {
    mmap: memmap2::Mmap,
    /// INVARIANT: points into `mmap`, which is read-only and was validated by
    /// `access()` at construction, so the pointee cannot change invalidly.
    root: *const archive::ArchivedArchiveRoot,
    /// V2-B02: candidate index over the archived entries, built once on first
    /// match (see `index` module docs for the archived-tree caveats: static
    /// prefixes are owned copies, and any incomplete walk falls back to the
    /// exact linear scan over the mapped bytes).
    index: OnceLock<index::CandidateIndex>,
}

// SAFETY: the mapping is read-only and the root pointer is immutable; the
// only access is through `&ArchivedArchiveRoot` (shared reads).
unsafe impl Send for ArchivedTree {}
unsafe impl Sync for ArchivedTree {}

impl std::fmt::Debug for ArchivedTree {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Manual impl: the mapping is opaque bytes — only the mapped length,
        // the root pointer and the index cell are shown (decoding entry
        // metadata would force validation on every Debug call).
        f.debug_struct("ArchivedTree")
            .field("mapped_len", &self.mmap.len())
            .field("root", &self.root)
            .field("index", &self.index)
            .finish()
    }
}

impl ArchivedTree {
    /// Load an archive from disk (alias of [`RouteTree::load_archived`], §8.2).
    ///
    /// # Errors
    ///
    /// Same conditions as [`RouteTree::load_archived`].
    pub fn load_archived(path: &Path) -> Result<Self> {
        archive::load_impl(path)
    }

    pub fn match_path(&self, path: &str) -> Option<MatchResult> {
        archive::arch_match_impl(self, path)
    }

    /// V2-B02: zero-copy match over the memory-mapped archive (hot path).
    /// The winning entry is identified by [`ArchivedMatchRef::route_index`];
    /// materialize it with `archive::entry_from_arch` only if owned data is
    /// needed — matching itself reads the mapped bytes in place.
    pub fn match_ref<'a>(&'a self, path: &'a str) -> Option<ArchivedMatchRef<'a>> {
        archive::arch_match_ref_impl(self, path)
    }

    /// V2-B02: the lazily built candidate index over the archived entries.
    pub(crate) fn candidate_index(&self) -> &index::CandidateIndex {
        self.index
            .get_or_init(|| index::CandidateIndex::build(self.root().entries.as_slice()))
    }

    pub fn len(&self) -> usize {
        archive::arch_len_impl(self)
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Native rules table (spec §12.1 step 4, D13): redirects, rewrites, headers,
/// i18n locale resolution, basic auth gates, rate limiting (§17.1).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NativeRule {
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destination: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub rule_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub basic_auth: Option<(String, String)>,
    /// Token-bucket rate limit (§17.1): requests per key per window seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limit: Option<RateLimit>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RateLimit {
    pub key: String,
    pub max: u32,
    pub window_secs: u64,
}

/// Compiled rules evaluated natively in Rust before any JS (D13).
#[derive(Debug, Clone, Default)]
pub struct RulesEngine {
    pub rules: Vec<NativeRule>,
    /// SEC-01: the per-key token buckets. `Mutex` because `evaluate` takes
    /// `&self` (rules are shared read-only; the buckets are the mutable
    /// state). The bucket map is swept opportunistically when it grows past
    /// `RATE_LIMITER_MAX_KEYS` so a unique-key flood cannot grow it without
    /// bound.
    pub limiter: crate::rules::TokenBuckets,
}

impl RulesEngine {
    /// Validate + normalize a rules table. Sources must be absolute paths whose
    /// `*` wildcard (if any) is the last segment; rewrites need a destination;
    /// rate-limit rules need a token-bucket config. A rule carrying `rate_limit`
    /// without an explicit `type` is normalized to `type: "ratelimit"` (§17.1).
    ///
    /// # Errors
    ///
    /// A diagnostic naming the offending rule (relative index + reason):
    /// relative `source`, missing `destination` on a rewrite, a `rate_limit`
    /// rule without a bucket config.
    pub fn compile(rules: Vec<NativeRule>) -> Result<Self> {
        rules::compile_impl(rules)
    }

    /// Outcome of evaluating the rules for a request path.
    ///
    /// Rules are evaluated in declaration order. The first matching rule with a
    /// *terminal* outcome wins: `type: "ratelimit"` → [`RuleOutcome::RateLimited`],
    /// `basic_auth` → [`RuleOutcome::AuthRequired`], `type: "rewrite"` with a
    /// destination → [`RuleOutcome::Rewrite`], any other destination →
    /// [`RuleOutcome::Redirect`] (status defaults to 308). Header rules are
    /// non-terminal: their maps merge across all matching rules and are returned as
    /// [`RuleOutcome::Headers`] only when no terminal outcome fired. `:param` captures
    /// (and `:splat` for `*`) substitute into destinations.
    #[must_use]
    pub fn evaluate(&self, path: &str) -> RuleOutcome {
        rules::evaluate_impl(self, path, "", None)
    }

    /// SEC-01: like `evaluate`, but with the request's rate-limit key
    /// material (e.g. the client IP). A `rate_limit.key` of `ip` (or an empty
    /// key) uses `client_key`; a `:param` uses the captured route param.
    #[must_use]
    pub fn evaluate_with_key(&self, path: &str, client_key: &str) -> RuleOutcome {
        rules::evaluate_impl(self, path, client_key, None)
    }

    /// B-16: evaluate the rules with the request's `Authorization` header so a
    /// `basic_auth` rule verifies credentials in constant time instead of
    /// emitting a permanent 401. `authorization` is the raw header value.
    #[must_use]
    pub fn evaluate_with_auth(
        &self,
        path: &str,
        client_key: &str,
        authorization: Option<&str>,
    ) -> RuleOutcome {
        rules::evaluate_impl(self, path, client_key, authorization)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RuleOutcome {
    None,
    Redirect {
        destination: String,
        status: u16,
    },
    Rewrite(String),
    Headers(BTreeMap<String, String>),
    AuthRequired,
    /// SEC-01: the token bucket for this key is empty. `retry_after_secs` is
    /// how long until at least one token refills (the server sends it as the
    /// `Retry-After` header).
    RateLimited {
        retry_after_secs: u64,
    },
}
