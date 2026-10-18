//! oxide-router — route tree, matcher, typegen, rules engine (spec §8, D8).
//!
//! The route tree is compiled at build into a radix tree (static > param > optional >
//! catch-all priority), stored as an `rkyv` archive, memory-mapped at boot; match is
//! allocation-free on the hot path. URLPattern semantics preserved for the edge build.
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

use oxide_diagnostics::{Diagnostic, OxideError};
use rkyv::{Archive, Deserialize as RkyvDeserialize, Serialize as RkyvSerialize};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub type Result<T> = std::result::Result<T, OxideError>;

pub fn err(code: &'static str, msg: impl Into<String>) -> OxideError {
    OxideError(Diagnostic::error(code, msg))
}

/// Internal helper: an error with a known fix attached (§19.4).
pub(crate) fn ferr(
    code: &'static str,
    msg: impl Into<String>,
    fix: impl Into<String>,
) -> OxideError {
    OxideError(err(code, msg).0.with_fix(fix))
}

mod archive;
mod matcher;
mod rules;
mod scan;
mod segment;
mod typegen;

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

/// Builder: scan a routes directory, build the tree, match, archive.
#[derive(Debug, Clone, Default)]
pub struct RouteTree {
    pub entries: Vec<RouteEntry>,
}

impl RouteTree {
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
    pub fn scan(dir: &Path) -> Result<Self> {
        scan::scan_impl(dir)
    }

    /// Resolve the routes directory for a project root (`dir`): an explicit override
    /// wins, else `dir/src/routes`, else Next-compat detection inside [`RouteTree::scan`].
    pub fn scan_with_config(dir: &Path, routes_dir_override: Option<&Path>) -> Result<Self> {
        scan::scan_with_config_impl(dir, routes_dir_override)
    }

    /// Insert a pre-built entry (used by tests, plugins' `routes` hook §15.2).
    pub fn insert(&mut self, entry: RouteEntry) {
        self.entries.push(entry);
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
    pub fn match_path(&self, path: &str) -> Option<MatchResult> {
        matcher::match_impl(self, path)
    }

    /// Serialize to an rkyv archive (§8.2) written to `out.manifest`.
    pub fn archive_to(&self, out: &Path) -> Result<()> {
        archive::archive_impl(self, out)
    }

    /// Load zero-copy: memory-map the archive and build an accessor.
    /// Accessing matches through the mapping must not allocate.
    pub fn load_archived(path: &Path) -> Result<ArchivedTree> {
        archive::load_impl(path)
    }

    /// Typed route generation (§8.3): writes `.oxide/types/routes.d.ts` with params,
    /// search params, loader data and action I/O per route + typed `href()`.
    /// Also writes a machine-readable `.oxide/types/routes.json` manifest used by
    /// `oxide routes` (§19.1). Returns the path of the generated `.d.ts`.
    pub fn typegen(&self, out_dir: &Path) -> Result<String> {
        typegen::typegen_impl(self, out_dir, None)
    }

    /// §8.3 like [`typegen`](Self::typegen), but with the routes root so a
    /// per-route `page.server.ts` `LoaderData` annotation can be read (the
    /// entry's `file` is routes-root-relative).
    pub fn typegen_with_root(&self, out_dir: &Path, routes_root: &Path) -> Result<String> {
        typegen::typegen_impl(self, out_dir, Some(routes_root))
    }

    /// Sorted deterministic listing for `oxide routes` (§19.1) incl. inferred modes.
    pub fn table(&self) -> Vec<(String, String, Vec<String>)> {
        typegen::table_impl(self)
    }
}

/// Zero-copy accessor over a memory-mapped archive.
pub struct ArchivedTree {
    _mmap: memmap2::Mmap,
}

impl std::fmt::Debug for ArchivedTree {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Manual impl: the mapping is opaque bytes; only the mapped length is shown
        // (decoding entry metadata would force validation on every Debug call).
        f.debug_struct("ArchivedTree")
            .field("mapped_len", &self._mmap.len())
            .finish()
    }
}

impl ArchivedTree {
    /// Load an archive from disk (alias of [`RouteTree::load_archived`], §8.2).
    pub fn load_archived(path: &Path) -> Result<Self> {
        archive::load_impl(path)
    }

    pub fn match_path(&self, path: &str) -> Option<MatchResult> {
        archive::arch_match_impl(self, path)
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
    /// [`RATE_LIMITER_MAX_KEYS`] so a unique-key flood cannot grow it without
    /// bound.
    pub limiter: crate::rules::TokenBuckets,
}

impl RulesEngine {
    /// Validate + normalize a rules table. Sources must be absolute paths whose
    /// `*` wildcard (if any) is the last segment; rewrites need a destination;
    /// rate-limit rules need a token-bucket config. A rule carrying `rate_limit`
    /// without an explicit `type` is normalized to `type: "ratelimit"` (§17.1).
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
    pub fn evaluate(&self, path: &str) -> RuleOutcome {
        rules::evaluate_impl(self, path, "")
    }

    /// SEC-01: like [`evaluate`], but with the request's rate-limit key
    /// material (e.g. the client IP). A `rate_limit.key` of `ip` (or an empty
    /// key) uses `client_key`; a `:param` uses the captured route param.
    pub fn evaluate_with_key(&self, path: &str, client_key: &str) -> RuleOutcome {
        rules::evaluate_impl(self, path, client_key)
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
