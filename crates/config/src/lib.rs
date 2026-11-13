#![doc = include_str!("../README.md")]

//! oxide-fw-config — typed, validated, layered configuration (spec §14, D14).
//!
//! Layering (§14.2): defaults → preset → `oxide.config.ts` → mode file
//! (`oxide.config.production.ts`) → env (`OXIDE_*`) → CLI flags.
//! Env files: `.env`, `.env.local`, `.env.[mode]`, `.env.[mode].local`;
//! `PUBLIC_`-prefixed variables are exposed to client code.
// §14.2: the JSON schema `json!` literal is large enough to exceed the
// default macro recursion limit (128). Raise it for this crate.
#![recursion_limit = "512"]

use oxide_fw_diagnostics::{Diagnostic, OxideError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

mod env_file;
mod extract;
mod refusal;
pub mod schema;

pub use refusal::{FeatureReport, ProjectFacts};

/// Test-only wrapper around the minimal-config extractor. Production code
/// reaches the same result through `OxideConfig::load`; this exposes the
/// extractor's raw output so tests can assert on it directly.
#[doc(hidden)]
#[must_use]
pub fn extract_for_test(source: &str) -> Option<serde_json::Value> {
    extract::extract_public(source)
}

mod layer;
mod size;

pub type Result<T> = std::result::Result<T, OxideError>;

pub use schema::{CONFIG_SCHEMA_URI, config_schema, config_schema_pretty};

pub fn err(code: &'static str, msg: impl Into<String>) -> OxideError {
    OxideError(Box::new(Diagnostic::error(code, msg)))
}

/// §16.6: service worker configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SwConfig {
    /// Emit the service worker at build. Default: `true` when
    /// `manifest` is present; `sw.enabled = false` disables emission
    /// even with a manifest.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// The offline-fallback route the worker serves for a navigation
    /// miss. Default `/offline` when the build produced that route;
    /// otherwise `/` (the precached shell). `sw.offlineFallback`
    /// overrides it.
    #[serde(default)]
    pub offline_fallback: Option<String>,
}

fn default_true() -> bool {
    true
}

impl Default for SwConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            offline_fallback: None,
        }
    }
}

/// The single typed config (§14.1). Serde field names match the TS config keys.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OxideConfig {
    pub framework: FrameworkConfig,
    pub routes: RoutesConfig,
    pub server: ServerConfig,
    pub render: RenderConfig,
    pub cache: CacheConfig,
    pub images: ImagesConfig,
    pub fonts: FontsConfig,
    pub i18n: I18nConfig,
    pub security: SecurityConfig,
    pub rules: Vec<crate::native_rule::NativeRule>,
    pub budgets: BudgetsConfig,
    pub build: BuildConfig,
    pub adapter: String,
    /// Content collections (§16.5). Each entry maps to a directory of Markdown
    /// files with frontmatter. The build emits a search index per collection.
    #[serde(default)]
    pub content: ContentConfig,
    /// Static export settings (§10.5). `fallback` is emitted in place of any
    /// route that cannot be prerendered.
    #[serde(rename = "static", default)]
    pub static_export: StaticConfig,
    #[serde(default)]
    pub plugins: Vec<serde_json::Value>,
    #[serde(default)]
    pub modules: Vec<String>,
    #[serde(default)]
    pub extends: Vec<String>,
    #[serde(default)]
    pub environments: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub dev: DevConfig,
    /// §16.3: absolute site origin used by `sitemap.xml` and `robots.txt`.
    /// Example: `"https://example.com"`. When absent, the build tries
    /// `OXIDE_SITE_URL` from the environment; if both are missing, the
    /// sitemap is emitted with site-relative paths (still valid, just less
    /// useful to crawlers) and `robots.txt` omits the `Sitemap:` line.
    #[serde(default)]
    pub site_url: Option<String>,
    /// §16.3: web app manifest (PWA). Optional; when present, `oxide build`
    /// emits `manifest.webmanifest` and a `<link rel="manifest">` is
    /// available for the shell to include.
    #[serde(default)]
    pub manifest: Option<WebManifestConfig>,
    /// §16.6: the service worker configuration. `None` keeps the
    /// default (worker emitted when `manifest` is set, offline fallback
    /// from the shell). `sw.enabled = false` disables emission even when
    /// a manifest is present; `sw.offline_fallback` overrides the
    /// fallback route path.
    #[serde(default)]
    pub sw: SwConfig,
    /// S-09: the OG-image endpoint (`GET /_oxide/og`). **Off by default** —
    /// it rasterizes on demand, so leaving it unauthenticated and default-on
    /// is a CPU-exhaustion vector. Opt in with `og.enabled: true`.
    #[serde(default)]
    pub og: OgConfig,
    /// S-09: the on-demand image transform endpoint (`GET /_oxide/image`).
    /// On by default (it is the documented product feature) but gateable.
    #[serde(default)]
    pub images_endpoint: EndpointToggle,
    /// V2-F04 (§15.4): A/B experiments, evaluated natively per request
    /// before the L0 lookup so an experiment is a cache-key dimension
    /// instead of forcing the route dynamic. Off by default — with no
    /// `experiments` configured every L0 key stays byte-identical to the
    /// pre-F04 baseline (the WP's acceptance contract). Entries are
    /// validated at load: `traffic` 0–100 and at least two variants
    /// (`DSP-110`).
    #[serde(default)]
    pub experiments: Vec<ExperimentConfig>,
    /// V2-F05 (§V2-F05 step 8): the live-loader transport knobs (the SSE
    /// hub's capacity, cadence and transport). Defaults are inert: with no
    /// route opting in (`page.server.ts` `live: true`) nothing listens and
    /// no hub is constructed. Knobs are validated at load (`DSP-111`):
    /// the keepalive must be ≥ 1 s, the hub must hold at least one
    /// subscriber, and the per-IP cap cannot exceed the global cap.
    #[serde(default)]
    pub live: LiveConfig,
    /// V2-F07 (§V2-F07 wiring step 3): the resumable-stream bounds. The
    /// store only exists when the wiring has a resumable response to tee,
    /// so the defaults are inert the same way the `live` ones are. Knobs
    /// are validated at load (`DSP-112`): the ttl must be ≥ 1 s, the
    /// per-stream bound ≥ 1 byte, and the total bound cannot be below the
    /// per-stream bound; the size literals are the §14 grammar.
    #[serde(default)]
    pub stream: StreamConfig,
    /// V2-F12: the brownout-mode knobs (the governor's breach ladders, the
    /// stale ceiling, the opt-in hedger). Inert by default — with
    /// `enabled: false` no governor is constructed and the request path
    /// gains nothing. Knobs are validated at load (`DSP-113`): ascending
    /// threshold ladders, a `staleCeiling` multiplier ≥ 1, and a hedger
    /// `afterFactor` > 1.
    #[serde(default)]
    pub brownout: BrownoutConfig,
    /// V2-F03: the interaction-replay opt-out (`interactionReplay`,
    /// spec Appendix V2.L, default `true` — flag absent = enabled). When
    /// `true` (or absent) the pre-hydration interaction capture stays on
    /// and the emitted hydration bootstrap is byte-identical to the
    /// pre-F03 shapes; `false` makes the bootstrap advertise
    /// `interactionReplay:false` in `window.__oxide` so the boot runtime
    /// skips `startReplay`. A non-boolean value fails the load with
    /// `DSP-114` (the raw-`Value` validation pass runs before the typed
    /// parse).
    #[serde(default = "default_true")]
    pub interaction_replay: bool,
    /// V2-F09 (Appendix V2.L): the built-in search surface. Inert by
    /// default — with no `collections` configured nothing indexes, nothing
    /// serves `/_oxide/search`, and the §18 config-key refusal reports the
    /// surface as unconfigured. A non-empty `collections` list is the
    /// "search configured" fact the refusal consults (`AST-SRC-001` on a
    /// target without the `Search` capability); the index/serve engine
    /// itself is F09's remaining WP and consumes the query knobs
    /// (`fields`, `snippet`, `languages`) from this same typed key — no
    /// second schema churn.
    #[serde(default)]
    pub search: SearchConfig,
    /// V2-F13 (Appendix V2.L): the learned-prefetch knobs. Inert by
    /// default — with `learned: false` no nav graph is constructed, the
    /// data plane observes nothing, `GET /_oxide/nav` answers 404, and the
    /// hydration bootstrap stays byte-identical. When on, the server keeps
    /// the aggregate route-transition graph (`server::navgraph`) and the
    /// client prefetcher may consult it; `minProbability` is the client's
    /// threshold for prefetching an edge. A bad spelling or an
    /// out-of-range probability fails the load with `DSP-115` (the
    /// raw-`Value` validation pass runs before the typed parse).
    #[serde(default)]
    pub prefetch: PrefetchConfig,
}

/// V2-F09 (Appendix V2.L): the `search` config key. The key shape is the
/// spec's (collections, field boosts, snippet length, languages); the
/// refusal seam reads `collections` today, the F09 engine reads the rest
/// from this same typed surface. Unknown keys are rejected
/// (`deny_unknown_fields`) so a typo cannot silently disable the feature.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields, rename_all = "camelCase", default)]
pub struct SearchConfig {
    /// The content collections (§16.5 names) the built-in search indexes.
    /// Empty = search not configured (the inert-defaults contract; the
    /// §18 refusal reads exactly this field).
    #[serde(default)]
    pub collections: Vec<String>,
    /// Query-time field boosts (F09's documented defaults: `title` 3,
    /// `body` 1). `None` = the engine default applies.
    #[serde(default)]
    pub fields: SearchFieldsConfig,
    /// Snippet length in characters (F09's documented default: 160).
    /// `None` = the engine default applies.
    #[serde(default)]
    pub snippet: Option<u32>,
    /// Index languages (F09's documented default: `["en"]`).
    #[serde(default)]
    pub languages: Vec<String>,
}

/// V2-F09: the `search.fields` query-time boosts. `None` = the engine
/// default applies (the spec's `title: 3`, `body: 1`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields, rename_all = "camelCase", default)]
pub struct SearchFieldsConfig {
    /// The title-field boost (F09: title matches outrank body matches).
    #[serde(default)]
    pub title: Option<f32>,
    /// The body-field boost.
    #[serde(default)]
    pub body: Option<f32>,
}

/// V2-F13 (Appendix V2.L): the `prefetch` config key. `learned` switches
/// the server's nav-graph substrate on (off by default — the
/// inert-defaults contract the `live`/`stream`/`brownout` sections set);
/// `minProbability` is the client prefetcher's threshold on the
/// `/_oxide/nav` probabilities (the spec's 0.35 default). Unknown keys are
/// rejected (`deny_unknown_fields`) so a typo cannot silently disable the
/// feature; bounds are validated at load (`DSP-115`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase", default)]
pub struct PrefetchConfig {
    /// Master switch. `false` (the default) means no `NavGraph` is
    /// constructed — the request path reads nothing new.
    pub learned: bool,
    /// The client prefetches an edge only at or above this probability
    /// (the spec's default 0.35). Validated to `0.0 ..= 1.0` at load.
    pub min_probability: f64,
}

impl Default for PrefetchConfig {
    fn default() -> Self {
        Self {
            learned: false,
            min_probability: 0.35,
        }
    }
}

/// S-09: opt-in toggle for the OG rasterization endpoint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
#[derive(Default)]
pub struct OgConfig {
    pub enabled: bool,
    /// A3: per-peer token-bucket limit applied before rasterization.
    /// Defaults to 10 renders / 60s.
    pub rate_limit: OgRateLimit,
}

/// A3: the `og.rateLimit` knob. `max` renders per `windowSecs` per peer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OgRateLimit {
    pub max: u32,
    pub window_secs: u64,
}

impl Default for OgRateLimit {
    fn default() -> Self {
        Self {
            max: 10,
            window_secs: 60,
        }
    }
}

/// S-09: a minimal enable flag shared by native endpoints (`enabled` defaults
/// true so existing projects keep the feature).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EndpointToggle {
    pub enabled: bool,
}

impl Default for EndpointToggle {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// V2-F04 (§15.4): one A/B experiment. Evaluated natively per request before
/// the L0 lookup, so an experiment is a cache-key dimension (`|exp:` vary
/// suffix) instead of forcing the route dynamic. Validation: `traffic` must
/// be a 0–100 percentage and the experiment needs at least two variants
/// (`DSP-110`, checked at load in `layer::validate`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ExperimentConfig {
    /// The experiment id — also the locals key and the vary-suffix name.
    pub id: String,
    /// Percentage of eligible identities in the experiment, 0–100 (`DSP-110`).
    pub traffic: u8,
    /// The identity strategy.
    pub sticky: StickyConfig,
    /// Variant names, in config order; at least two (`DSP-110`).
    pub variants: Vec<String>,
    /// URL-pattern prefixes the experiment applies to; empty = all routes.
    #[serde(default)]
    pub routes: Vec<String>,
}

/// V2-F04: the `sticky` identity strategy (§15.4) — internally tagged by the
/// `sticky` string, with the strategy's storage name alongside
/// (`cookieName` / `headerName`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "sticky", rename_all = "kebab-case")]
pub enum StickyConfig {
    /// Read (then mint) the named cookie.
    Cookie {
        /// The cookie storage key, e.g. `oxide_hero`.
        #[serde(default, rename = "cookieName")]
        cookie_name: String,
    },
    /// Read the named request header (edge/CDN cohorts).
    Header {
        /// The header storage key, e.g. `x-bucket`.
        #[serde(default, rename = "headerName")]
        header_name: String,
    },
    /// Hash the client IP directly — no storage, coarse-grained.
    IpHash,
}

/// §16.3: a minimal, honest web app manifest. The fields are the ones a
/// PWA needs to be installable; anything richer (shortcuts, screenshots,
/// `share_target`) can be added without a shape change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct WebManifestConfig {
    /// Display name. Defaults to `name` when omitted.
    pub name: Option<String>,
    /// Short name shown under the home-screen icon.
    pub short_name: Option<String>,
    /// Background colour (`#rrggbb`).
    pub background_color: Option<String>,
    /// Theme colour (`#rrggbb`).
    pub theme_color: Option<String>,
    /// `standalone` | `fullscreen` | `minimal-ui` | `browser`.
    pub display: Option<String>,
    /// Start URL, default `/`.
    pub start_url: Option<String>,
    /// Icons. `src` is resolved against the built site; the build copies it
    /// from `public/` or `dist/client/`.
    pub icons: Vec<ManifestIcon>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ManifestIcon {
    pub src: String,
    pub sizes: String,
    /// `image/png` etc. Inferred from the extension when omitted.
    pub r#type: Option<String>,
    pub purpose: Option<String>,
}

/// Content collections configuration (§16.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ContentConfig {
    /// One entry per collection.
    pub collections: Vec<CollectionDef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct CollectionDef {
    pub name: String,
    pub dir: String,
    /// Frontmatter fields (besides `title`) to include in the search index.
    pub indexed_fields: Vec<String>,
}

impl Default for OxideConfig {
    fn default() -> Self {
        Self {
            framework: FrameworkConfig::default(),
            routes: RoutesConfig::default(),
            server: ServerConfig::default(),
            render: RenderConfig::default(),
            cache: CacheConfig::default(),
            images: ImagesConfig::default(),
            fonts: FontsConfig::default(),
            i18n: I18nConfig::default(),
            security: SecurityConfig::default(),
            rules: Vec::new(),
            budgets: BudgetsConfig::default(),
            build: BuildConfig::default(),
            adapter: "binary".into(),
            content: ContentConfig::default(),
            static_export: StaticConfig::default(),
            plugins: Vec::new(),
            modules: Vec::new(),
            extends: Vec::new(),
            environments: BTreeMap::new(),
            dev: DevConfig::default(),
            site_url: None,
            manifest: None,
            sw: SwConfig::default(),
            og: OgConfig::default(),
            images_endpoint: EndpointToggle::default(),
            experiments: Vec::new(),
            live: LiveConfig::default(),
            stream: StreamConfig::default(),
            brownout: BrownoutConfig::default(),
            interaction_replay: true,
            search: SearchConfig::default(),
            prefetch: PrefetchConfig::default(),
        }
    }
}

/// V2-F05 (§V2-F05 step 8): the live-loader transport knobs. All fields
/// camelCase on the wire (`maxSubscribers`, `debounceMs`,
/// `keepaliveSeconds`, `maxRerunsPerSecond`, `perIpMax`). Unknown keys are
/// rejected (`deny_unknown_fields`) so a typo cannot silently disable a
/// production knob.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase", default)]
pub struct LiveConfig {
    /// The hub's total subscriber capacity; the endpoint answers `503` +
    /// `retry-after` above it (spec step 8: 5000).
    pub max_subscribers: u32,
    /// Per-subscriber rerun debounce in ms (spec step 8: 50).
    pub debounce_ms: u64,
    /// SSE keepalive `ping` period in seconds (spec step 8: 20; `DSP-111`
    /// rejects 0 — an immediate-tick keepalive busy-loops every
    /// connection).
    pub keepalive_seconds: u64,
    /// Per-subscriber rerun rate cap per second (spec step 8: 20).
    pub max_reruns_per_second: u32,
    /// Per-peer subscriber cap (spec security bullet: "per-IP subscriber
    /// cap (default 20)"); `DSP-111` rejects values above
    /// `max_subscribers`.
    pub per_ip_max: u32,
    /// The transport. `sse` pushes snapshots; `poll` is the serverless
    /// fallback (`capability_matrix(...).get(&Capability::Live)`).
    pub transport: LiveTransport,
}

impl Default for LiveConfig {
    fn default() -> Self {
        Self {
            max_subscribers: 5_000,
            debounce_ms: 50,
            keepalive_seconds: 20,
            max_reruns_per_second: 20,
            per_ip_max: 20,
            transport: LiveTransport::Sse,
        }
    }
}

/// V2-F05: the live transport. `sse` (default) streams snapshots over
/// `GET /__oxide/live`; `poll` makes the client fetch `/__data` on the
/// keepalive cadence instead (the serverless/edge fallback — the
/// capability matrix degrades `Live` to `Degraded` there).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LiveTransport {
    /// Server-sent events (the spec's default transport).
    #[default]
    Sse,
    /// HTTP polling fallback (`transport: 'poll'` in the spec table).
    Poll,
}

/// V2-F07 (§V2-F07 wiring step 3): the resumable-stream settings. The
/// spec's `stream.resume` object: a streamed response marked
/// `x-oxide-stream: <id>` stays replayable for the ttl, each buffer
/// holding at most `maxBytesEach`, all buffers together at most
/// `maxBytesTotal`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase", default)]
pub struct StreamConfig {
    /// The reattach/replay bounds.
    pub resume: StreamResumeConfig,
}

/// V2-F07: the `stream.resume` knobs. The size knobs carry the §14 size
/// literal grammar (`"4mb"`, `"64kb"`, plain bytes) — parsed by
/// [`parse_size`] at the wiring, validated at load (`CFG-SIZE` on a bad
/// literal, `DSP-112` on a physically useless bound).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase", default)]
pub struct StreamResumeConfig {
    /// How long a buffer stays reattachable, in seconds (spec: `ttl:
    /// '60s'`; `DSP-112` rejects 0 — the buffer would expire before the
    /// reattach arrives).
    pub ttl_secs: u64,
    /// The per-stream replay cap, e.g. `"4mb"` (spec default). Past it the
    /// buffer loses replay instead of eating memory: the client falls back
    /// to a fresh fetch.
    pub max_bytes_each: String,
    /// The store-wide cap, e.g. `"256mb"` (spec default). The accounting
    /// evicts the oldest completed buffer first, then the oldest buffer.
    pub max_bytes_total: String,
}

impl Default for StreamResumeConfig {
    fn default() -> Self {
        Self {
            ttl_secs: 60,
            max_bytes_each: "4mb".into(),
            max_bytes_total: "256mb".into(),
        }
    }
}

/// V2-F12: the brownout-mode knobs. The spec's `brownout` object —
/// `enabled`, `staleCeiling`, the three breach ladders (`thresholds`) and
/// the opt-in hedger. Off by default: with `enabled: false` no governor is
/// constructed and the request path keeps its pre-F12 shape (the inert
/// defaults contract the `live`/`stream` sections set). Knobs are
/// validated at load (`DSP-113`): each threshold ladder must be ascending
/// (a higher rung below a lower one makes the hysteresis nonsensical), the
/// `staleCeiling` multiplier must parse and be ≥ 1, and the hedger's
/// `afterFactor` must be > 1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase", default)]
pub struct BrownoutConfig {
    /// Master switch. `false` (the default) means no governor is
    /// constructed — the request path reads nothing new.
    pub enabled: bool,
    /// How long past the TTL an L0 entry may still serve at level `Stale`,
    /// as a multiplier on the entry's TTL (`"10x"`, plain `"10"`, `"1.5x"`).
    /// Parsed by [`parse_stale_ceiling`]; the spec's default is 10×.
    pub stale_ceiling: String,
    /// The three breach ladders, in milliseconds for the latency knobs and
    /// MiB for `rssMb`. Rung `i` is the level-`i+1` threshold; a signal at
    /// or above it breaches that level.
    pub thresholds: BrownoutThresholdsConfig,
    /// The opt-in hedger (§V2-F12, off by default): a still-running
    /// idempotent GET render is duplicated after `afterFactor` × the
    /// route's recent p95, first response wins.
    pub hedge: HedgeConfig,
}

impl Default for BrownoutConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            stale_ceiling: "10x".into(),
            thresholds: BrownoutThresholdsConfig::default(),
            hedge: HedgeConfig::default(),
        }
    }
}

/// V2-F12: the three breach ladders. `0` on an `rssMb` rung is the
/// documented "no RSS gate" spelling (the wiring maps it to the spec's
/// never-breach `u64::MAX` sentinel — JSON cannot carry `u64::MAX` without
/// precision loss). The latency ladders default to the §V2-F12
/// `Thresholds::default` code block converted to whole milliseconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase", default)]
pub struct BrownoutThresholdsConfig {
    /// Pool queue-wait p99 breach rungs, ms (spec: 50 / 200 / 500).
    pub queue_wait_ms: [u64; 3],
    /// Event-loop lag breach rungs, ms (spec: 20 / 100 / 300).
    pub loop_lag_ms: [u64; 3],
    /// Resident-memory breach rungs, MiB (spec default: never).
    pub rss_mb: [u64; 3],
}

impl Default for BrownoutThresholdsConfig {
    fn default() -> Self {
        Self {
            queue_wait_ms: [50, 200, 500],
            loop_lag_ms: [20, 100, 300],
            rss_mb: [0, 0, 0],
        }
    }
}

/// V2-F12: the hedger's knobs (opt-in, off by default — the spec documents
/// the idempotency requirement an enabled hedger imposes).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase", default)]
pub struct HedgeConfig {
    /// `false` (the default) disables the hedger entirely.
    pub enabled: bool,
    /// Hedge a still-running render after this multiple of the route's
    /// recent p95 (`> 1`, spec default 2.0 — `DSP-113` rejects ≤ 1: a
    /// factor of 1 would hedge every render).
    pub after_factor: f64,
}

impl Default for HedgeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            after_factor: 2.0,
        }
    }
}

/// Parse a [`BrownoutConfig::stale_ceiling`] multiplier: `"10x"` (the
/// spec's default spelling), a plain `"10"`, or a fractional `"1.5x"`.
/// Returns `None` on an unparseable literal, a zero or negative
/// multiplier, or a non-finite value — the load validation turns `None`
/// into the `DSP-113` diagnostic.
#[must_use]
pub fn parse_stale_ceiling(s: &str) -> Option<f64> {
    let digits = s.strip_suffix('x').unwrap_or(s);
    if digits.is_empty() {
        return None;
    }
    let n: f64 = digits.parse().ok()?;
    if n.is_finite() && n >= 1.0 {
        Some(n)
    } else {
        None
    }
}

/// Static export settings (§10.5). When `fallback` is set, the static
/// target emits that HTML file for any dynamic route without a prerendered
/// output, instead of refusing the build.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct StaticConfig {
    /// Path to a fallback HTML file emitted for dynamic routes.
    pub fallback: Option<String>,
}

/// V2-C01 (D26): the `framework.compilerBackend` switch — `auto` (the
/// default) picks a healthy native compiler when one is compiled in and
/// falls back to the Node tooling pool, `native` requires the native path
/// (`CMP-NAT-001` otherwise) and `node` always uses the pool. Serialized
/// lowercase to match the config-file spelling the TS side writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CompilerBackend {
    /// Pick a healthy compiled-in native compiler, else the Node pool.
    #[default]
    Auto,
    /// Require the native path; refuse with `CMP-NAT-001` when no healthy
    /// native compiler is available.
    Native,
    /// Always the Node tooling pool (the one-release escape hatch).
    Node,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct FrameworkConfig {
    pub name: Option<String>,
    /// `react | vue | svelte | solid`
    pub adapter: Option<String>,
    pub compiler: Option<bool>,
    /// V2-C01 (D26): the compiler backend the precompile pass consults
    /// per file. `auto` (the default) picks a native compiler when one is
    /// compiled in **and** healthy, and uses the Node tooling pool
    /// otherwise — a broken backend must never surface as a hard failure
    /// the config author did not ask for. `native` requires the native
    /// path and refuses with `CMP-NAT-001` when no healthy backend is
    /// available; `node` is the one-release escape hatch that always uses
    /// the pool. The engine-side copy of this switch lives in
    /// `oxide_fw_compiler::native::Backend`; the two shapes are pinned by
    /// their own crates and mapped at the call site (the recorded F04
    /// ruling: config → engine conversion is wiring, not a dependency).
    /// A bad spelling fails the load with `CFG-BACKEND` (the raw-`Value`
    /// validation pass runs before the typed parse).
    #[serde(default)]
    pub compiler_backend: CompilerBackend,
    /// §10.3 islands registry: component name → source path (relative to
    /// the project root). The build emits a synthetic `__boot__` client
    /// chunk that statically imports every island here and registers them
    /// with the framework adapter's `hydrateIsland`. A page writes
    /// `<Island component="Counter" chunk="Counter">` — the `chunk` prop is
    /// an *island id* (a key into this map), not a runtime URL.
    pub islands: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RoutesConfig {
    pub dir: String,
}
impl Default for RoutesConfig {
    fn default() -> Self {
        Self {
            dir: "src/routes".into(),
        }
    }
}

/// Which JavaScript host backs the render pool (§4.2, D2). `Node` is the
/// current default because it can resolve npm from the workspace; `V8` runs
/// `deno_core` in-process and is the spec's long-term default once
/// `deno_node` module loading lands. Selecting `v8` today is honest — the
/// pool starts, but user-module ops return a clear `JSH-NOKIND` error rather
/// than silently falling back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum JsBackend {
    #[default]
    Node,
    V8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase", default)]
pub struct ServerConfig {
    pub port: u16,
    pub host: String,
    pub compression: Vec<String>,
    pub http3: bool,
    /// Whether to advertise h3 via `Alt-Svc`. Default **true** now that the
    /// h3 request-body drain and response-streaming paths are implemented;
    /// `false` keeps a browser on h1/h2 without disabling the listener.
    pub h3_advertise: bool,
    /// §4.2: which JS host backend to use for the render pool.
    pub backend: JsBackend,
}
impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: 3000,
            host: "localhost".into(),
            compression: vec!["zstd".into(), "br".into(), "gzip".into()],
            http3: false,
            h3_advertise: true,
            backend: JsBackend::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase", default)]
pub struct RenderConfig {
    /// `auto | static | isr | dynamic | spa | islands` (§10.1)
    pub default: String,
    /// §10.8: partial prerendering. When true, the dispatcher caches the
    /// static shell of a streamed page and serves it immediately on later
    /// requests, streaming the dynamic holes in afterwards.
    #[serde(default)]
    pub ppr: bool,
}
impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            default: "auto".into(),
            ppr: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase", default)]
#[derive(Default)]
pub struct CacheConfig {
    pub handler: CacheHandlerConfig,
    /// §14.1: `cache: { l0: { maxSize: "512mb" } }`.
    #[serde(default)]
    pub l0: L0CacheConfig,
}

/// §14.1: `cache.l0` — the L0 response-cache settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase", default)]
pub struct L0CacheConfig {
    /// e.g. `"512mb"`, `"64kb"`, `"1gb"` — parsed by `parse_size`.
    pub max_size: String,
    /// §11.2 / §14.1: request headers the L0 key is fingerprinted by. A cached
    /// response that varies by a header not listed here is served to a
    /// different request — so this must name every header the app branches on
    /// (beyond the always-included `accept-language` when i18n is configured).
    /// Empty disables header variance. Matched case-insensitively.
    pub vary: Vec<String>,
    /// P-05: freshness window for an L0 entry, in seconds. Was hard-coded to
    /// 60 — long enough that every cached page fully re-renders on a TTL
    /// cliff, short enough that the SWR window is useless. Default 300.
    pub ttl_secs: Option<u64>,
}
impl Default for L0CacheConfig {
    fn default() -> Self {
        Self {
            max_size: "512mb".into(),
            vary: vec!["accept-language".into()],
            ttl_secs: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum CacheHandlerConfig {
    #[default]
    Memory,
    Disk,
    Redis {
        url: String,
    },
    S3 {
        bucket: String,
    },
    /// §11.2 / SPEC-12: a remote HTTP cache (`HttpCacheHandler`). The build
    /// driver serves `GET/PUT/DELETE /cache/<key>` + `POST /cache/invalidate`.
    Http {
        url: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase", default)]
pub struct ImagesConfig {
    pub formats: Vec<String>,
    pub remote_patterns: Vec<RemotePattern>,
    /// §16.1: maximum concurrent rasterizations. Each transform is CPU-bound;
    /// unbounded concurrency lets a few clients saturate every core.
    #[serde(default = "default_image_concurrency")]
    pub max_concurrency: usize,
    /// §16.1: largest accepted `w`/`h`. A user-supplied size is not trusted
    /// to allocate an arbitrarily large canvas.
    #[serde(default = "default_image_max_dimension")]
    pub max_dimension: u32,
    /// V2-C09: the AVIF encoder speed (ravif's `with_speed`, 1..=10 —
    /// higher is faster at lower quality; the spec default is 6, the
    /// documented middle rate). A value outside 1..=10 fails the load
    /// with `CFG-AVIF` (the raw-`Value` validation pass runs before the
    /// typed parse).
    #[serde(default = "default_avif_speed")]
    pub avif_speed: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemotePattern {
    pub host: String,
    #[serde(default)]
    pub protocol: Option<String>,
}

impl Default for ImagesConfig {
    fn default() -> Self {
        Self {
            formats: vec!["avif".into(), "webp".into()],
            remote_patterns: Vec::new(),
            avif_speed: 6,
            max_concurrency: default_image_concurrency(),
            max_dimension: default_image_max_dimension(),
        }
    }
}

fn default_image_concurrency() -> usize {
    4
}

fn default_image_max_dimension() -> u32 {
    8192
}

/// §V2-C09: the `images.avifSpeed` default — ravif's documented middle rate.
fn default_avif_speed() -> u8 {
    6
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields, rename_all = "camelCase", default)]
pub struct FontsConfig {
    pub providers: Vec<String>,
    pub subset: Vec<String>,
    /// §16.2: font families to fetch/subset/self-host at build time.
    pub families: Vec<FontFamilyConfig>,
}

/// One font family to bundle at build (§16.2): a family name, the weights and
/// styles to emit, and where the face files come from. Local paths are the
/// supported source today; a provider name reserves the fetch path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields, rename_all = "camelCase", default)]
pub struct FontFamilyConfig {
    pub family: String,
    /// `"local"` or a provider name (google, …). Local copies files verbatim.
    pub provider: String,
    pub weights: Vec<u32>,
    pub styles: Vec<String>,
    /// Filesystem paths to the face files, relative to the project root.
    pub local_paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct I18nConfig {
    pub locales: Vec<String>,
    #[serde(rename = "default")]
    pub default_locale: String,
    /// `prefix | prefix-except-default | none`
    pub strategy: String,
}
impl Default for I18nConfig {
    fn default() -> Self {
        Self {
            locales: vec!["en".into()],
            default_locale: "en".into(),
            strategy: "none".into(),
        }
    }
}

fn default_session_ttl_secs() -> u64 {
    1800
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase", default)]
pub struct SecurityConfig {
    /// `strict | hash | off`
    pub csp: String,
    /// `origin | token | off`
    pub csrf: String,
    /// SEC-05: hosts the server accepts in the `Host` header (421 otherwise).
    /// Empty = accept any host. A leading-dot entry (`.example.com`) matches
    /// subdomains; a bare entry matches exactly.
    pub host_allow_list: Vec<String>,
    /// SEC-05: hosts the SSRF check treats as allowed even in a blocked range.
    pub ssrf_allow_list: Vec<String>,
    /// SEC-05: CIDRs whose `X-Forwarded-For`/`-Proto`/`-Host` headers are
    /// trusted. A peer outside this set has those headers ignored entirely.
    /// Empty = trust no proxy.
    pub trusted_proxies: Vec<String>,
    /// SEC-05 / §16.6: cookie-signing keys (32+ bytes each, first signs).
    /// Empty = derive from `.oxide/secret`.
    pub cookie_keys: Vec<String>,
    /// §16.6: session-**encryption** keys for `SessionCodec` (ChaCha20-Poly1305;
    /// 32+ bytes each, first key seals, all tried on open). Empty = session
    /// cookies fall back to the signed-only `sign_cookie` primitive (payload
    /// visible). Set this to make session payloads confidential.
    pub session_keys: Vec<String>,
    /// §16.6 / WP-r9: session cookie lifetime in seconds. 1800 (30 min)
    /// matches the previous hardcoded value; explicit so a project can
    /// tighten or extend it without a rebuild of the CLI.
    #[serde(default = "default_session_ttl_secs")]
    pub session_ttl_secs: u64,
    /// S-21.3: emit the `x-oxide-fw-cache` status header (`L0-hit`/`miss`/…).
    /// Default `true`; a strict deployment can set it `false` to avoid
    /// telling clients whether a response came from cache.
    pub expose_cache_header: bool,
    /// V2-E04 (SRI): emit subresource-integrity digests (`sha384-<base64>`)
    /// for every emitted client JS/CSS chunk in `client.manifest.json`
    /// (`integrity`) and stamp the boot `<script type=module>` tag with
    /// `integrity` + `crossorigin="anonymous"`. Default `true` — "default on
    /// in production builds": the dev server's `/@boot/` virtual module has
    /// no manifest digest, so dev output is byte-identical without a
    /// dev/prod key split. The bundler always records the digests (cheap,
    /// deterministic, useful to `oxide verify`); this flag gates the
    /// head-tag injection.
    pub sri: bool,
    /// V2-E04: the Trusted Types mode — `off | report | enforce`. When
    /// `report`/`enforce`, the emitted CSP gains
    /// `require-trusted-types-for 'script'; trusted-types oxide default`;
    /// with `report` the directives ride an additional Report-Only policy
    /// (violations reported, nothing blocked) while `enforce` puts them on
    /// the enforced header. The boot-side `trustedTypes.createPolicy`
    /// wiring is the client half (see `docs/deviations.md`).
    pub trusted_types: String,
}
impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            csp: "strict".into(),
            csrf: "origin".into(),
            host_allow_list: Vec::new(),
            ssrf_allow_list: Vec::new(),
            trusted_proxies: Vec::new(),
            cookie_keys: Vec::new(),
            session_keys: Vec::new(),
            session_ttl_secs: 1800,
            expose_cache_header: true,
            sri: true,
            trusted_types: "off".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct BudgetsConfig {
    /// e.g. "150kb" (§21 client JS budget)
    pub client_js: Option<String>,
    pub route_js: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase", default)]
pub struct BuildConfig {
    /// `baseline-widely-available` default (§6.6)
    pub target: String,
    /// `hidden` default (§17.1)
    pub sourcemap: String,
    /// §6.1 / §21: module specifiers to leave **external** in the SSR bundle
    /// (never bundled). Empty — the default — produces a self-contained server
    /// bundle, which is what the `binary` adapter needs. A Node-target
    /// deployment that ships `node_modules` sets this to its framework
    /// runtimes (e.g. `["vue", "@vue/server-renderer"]`) to shrink the bundle
    /// from ~950 KB to a few KB. Externals are matched exactly on the
    /// specifier, so subpaths (`vue/server-renderer`) must be listed too.
    pub ssr_externals: Vec<String>,
    /// §18 / D18: specifiers left external in the **edge** bundle. The edge
    /// handler must be self-contained by default (no externals), so this is
    /// empty unless a project declares its runtime's node-compat layer — a
    /// Cloudflare Worker with `nodejs_compat` can `import 'node:crypto'`
    /// only if the build does not try to resolve it.
    ///
    /// Matched exactly on the specifier, like `ssrExternals`.
    pub edge_externals: Vec<String>,
    /// §5.4: a remote build-cache URL (`http://…`). When set, the precompile
    /// pass reads through it on a local miss and writes back on a store, so a
    /// CI job reuses another job's artifacts. `None` disables the remote.
    pub remote_cache: Option<String>,
    /// SPEC-5D: branches that may WRITE to the remote cache (§5.4
    /// trusted-branch write). Reads `GIT_BRANCH` at build time.
    #[serde(default)]
    pub write_branches: Option<Vec<String>>,
    /// §16.5: emit the route-search index (`dist/client/search-index.json`).
    /// On by default when prerendered pages exist; set `false` for projects
    /// that do not ship a client-side search UI.
    #[serde(default = "default_true")]
    pub route_search: bool,
}
impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            target: "baseline-widely-available".into(),
            sourcemap: "hidden".into(),
            ssr_externals: Vec::new(),
            edge_externals: Vec::new(),
            remote_cache: None,
            write_branches: None,
            route_search: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DevConfig {
    pub bundle: bool,
    pub https: bool,
    pub proxy: BTreeMap<String, String>,
    /// Native ESM dev server (Phase 1 of the HMR design in
    /// `docs/ideas/hmr.md`). When `true`, `oxide dev` skips the client
    /// bundler and serves `.tsx`/`.ts` source files directly as transformed
    /// ESM (Oxc, ~8ms per file). The client manifest points
    /// `componentFor` at `/src/*.tsx` URLs, which the existing client
    /// runtime already handles via `import(url)`.
    ///
    /// Default `true` since Phase 4 (React Refresh proven, see
    /// `docs/plan-esm-hmr.md`). Opt out per project with
    /// `dev: { esm: false }` in `oxide.config.ts` to fall back to the
    /// Rolldown bundle + full-reload HMR path.
    pub esm: bool,
    /// Packages to bundle once at dev startup from CJS to ESM. Each entry
    /// is served from `.oxide/esm-prebundle/<name>.js` and referenced by
    /// the importmap. `react`, `react-dom/client` and `react/jsx-runtime`
    /// must be prebundled because their published entries are CJS and the
    /// browser cannot execute them natively.
    pub prebundle: Vec<String>,
}

impl Default for DevConfig {
    fn default() -> Self {
        Self {
            bundle: false,
            https: false,
            proxy: BTreeMap::new(),
            // Phase 4: React Refresh is proven (Phase 3 tests green), so the
            // native ESM dev server is the default. Rollback:
            // `dev: { esm: false }` in `oxide.config.ts`.
            esm: true,
            prebundle: vec![
                // React 19 ships CJS-only. `prebundle_cjs_to_esm` bundles
                // each entry with Rolldown's CJS interop (entry-point mode),
                // which scans `exports.useState = ...` and emits real named
                // ESM exports. `react` must be a separate entry from
                // `react-dom/client` so that each has its own file, and
                // `react-dom-client.js` externalizes `react` (resolved by
                // the import map to `/@prebundle/react.js`) so the browser
                // has exactly one React instance with one dispatcher.
                "react".into(),
                "react-dom".into(),
                "react-dom/client".into(),
                "react/jsx-runtime".into(),
                "react-refresh/runtime".into(),
                "lucide-react".into(),
                "rangi".into(),
                "rangi/themes".into(),
            ],
        }
    }
}

/// SPEC-16 / §14.2: detect the nearest monorepo workspace root above `dir`.
///
/// Recognized markers (first found walking up, nearest wins):
/// - `pnpm-workspace.yaml`
/// - `package.json` with a `workspaces` field
/// - `Cargo.toml` with a `[workspace]` section
///
/// Returns the workspace directory, or `None` when `dir` is not inside one.
/// Used by `extends` resolution (a preset can name a workspace package) and
/// by the dev watcher (linked packages are watched too).
#[must_use]
pub fn detect_workspace_root(dir: &Path) -> Option<PathBuf> {
    let mut cur = Some(dir);
    while let Some(d) = cur {
        if d.join("pnpm-workspace.yaml").is_file() {
            return Some(d.to_path_buf());
        }
        if let Ok(pkg) = std::fs::read_to_string(d.join("package.json"))
            && (pkg.contains("\"workspaces\"") || pkg.contains("'workspaces'"))
        {
            return Some(d.to_path_buf());
        }
        if let Ok(cargo) = std::fs::read_to_string(d.join("Cargo.toml"))
            && cargo.contains("[workspace]")
        {
            return Some(d.to_path_buf());
        }
        cur = d.parent();
    }
    None
}

/// SPEC-16: resolve an `extends` spec against the workspace. A bare package
/// spec (`@scope/oxide-preset`) is looked up in the workspace's `node_modules`
/// when `dir` is inside one, so a shared preset installs once at the root.
#[must_use]
pub fn resolve_workspace_preset(dir: &Path, spec: &str) -> Option<PathBuf> {
    if spec.starts_with('.') || spec.starts_with('/') {
        return None; // relative/absolute — the normal resolver handles it
    }
    let ws = detect_workspace_root(dir)?;
    let base = ws.join("node_modules").join(spec);
    [
        base.join("oxide.preset.ts"),
        base.join("oxide.preset.json"),
        base.clone(),
    ]
    .into_iter()
    .find(|cand| cand.is_file())
}

/// Size parser: "512mb" | "64kb" | "1gb" | plain bytes → u64.
/// Binary units (kb = 1024). Invalid input → `CFG-SIZE`.
///
/// # Errors
///
/// Fails with `CFG-SIZE` on an empty string, a bad number, a negative or
/// non-finite value, or an unknown unit.
pub fn parse_size(s: &str) -> Result<u64> {
    size::parse(s)
}

/// Load + layer the full config for `dir` in `mode`.
/// Validates with typo suggestions (§14.2 "typed and validated").
///
/// Layering (§14.2): defaults → `extends` presets → `oxide.config.{ts,json}` →
/// `oxide.config.<mode>.{ts,json}` → `OXIDE_*` env (allow-list) → `cli_overrides`.
/// Environment files are loaded in the order `.env`, `.env.<mode>`, `.env.local`,
/// `.env.<mode>.local`; the process environment wins over file values.
///
/// # Errors
///
/// Fails with the layering diagnostics: `CFG-IO` (unreadable file),
/// `CFG-UNKNOWN-KEY` (unknown top-level key), `CFG-FRAMEWORK` (missing
/// adapter), `DSP-110` (invalid `experiments` entry) or `CFG-EXTENDS`
/// (unresolvable preset).
pub fn load(
    dir: &Path,
    mode: &str,
    cli_overrides: BTreeMap<String, serde_json::Value>,
) -> Result<LoadedConfig> {
    let process_env: BTreeMap<String, String> = std::env::vars().collect();
    layer::load_with_env(dir, mode, cli_overrides, &process_env)
}

/// [`load`] with an explicit process-env map (injectable for tests).
///
/// # Errors
///
/// Same diagnostics as [`load`] — the layering and validation failures.
pub fn load_with_env(
    dir: &Path,
    mode: &str,
    cli_overrides: BTreeMap<String, serde_json::Value>,
    process_env: &BTreeMap<String, String>,
) -> Result<LoadedConfig> {
    layer::load_with_env(dir, mode, cli_overrides, process_env)
}

#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub config: OxideConfig,
    /// Resolved env files in layering order.
    pub env_sources: Vec<PathBuf>,
    /// All variables; `PUBLIC_`-prefixed are client-exposed (§14.2).
    pub env: BTreeMap<String, String>,
    /// Whether V8/dynamic evaluation was needed (static extraction failed) — D14.
    pub used_dynamic_eval: bool,
}

impl LoadedConfig {
    /// Variables whose name starts with `PUBLIC_` — exposed to client code (§14.2).
    #[must_use]
    pub fn public_env(&self) -> BTreeMap<String, String> {
        self.env
            .iter()
            .filter(|(k, _)| k.starts_with("PUBLIC_"))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

/// Static extraction (D14): parse a literal `defineConfig({...})` object from TS source
/// with a scanner (balanced braces, string/template literals, comments, trailing commas,
/// spread of known adapter factory calls with literal args). Anything dynamic (functions,
/// `process.env` outside the allow-list, top-level await) returns `None` → caller falls
/// back to V8 evaluation, cached by hash of the file + tracked env.
#[must_use]
pub fn extract_static_config(ts_source: &str) -> Option<serde_json::Value> {
    extract::extract(ts_source)
}

/// Env file parsing (dotenv-compatible: KEY=VALUE, quotes, comments, escapes).
#[must_use]
pub fn parse_env_file(content: &str) -> BTreeMap<String, String> {
    env_file::parse(content)
}

pub mod native_rule {
    //! Re-exported mirror of `router::NativeRule` for config deserialization without a dep cycle.
    use serde::{Deserialize, Serialize};
    use std::collections::BTreeMap;

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
        #[serde(skip_serializing_if = "Option::is_none")]
        pub rate_limit: Option<RateLimitSpec>,
        /// V2-F12: the route's overload priority (`low | normal |
        /// critical`, default `normal`). The spelling is validated at load
        /// (`DSP-113`); the CLI passes it through to the router's
        /// `NativeRule` for the brownout enforcement stage.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub priority: Option<String>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct RateLimitSpec {
        pub key: String,
        pub max: u32,
        pub window_secs: u64,
    }
}

#[cfg(test)]
mod spec16_tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// SPEC-16: a pnpm workspace root is detected from a nested app dir.
    #[test]
    fn detects_pnpm_workspace_root() {
        let ws = tempdir().unwrap();
        fs::write(
            ws.path().join("pnpm-workspace.yaml"),
            "packages:\n  - apps/*\n",
        )
        .unwrap();
        let app = ws.path().join("apps/web");
        fs::create_dir_all(&app).unwrap();
        assert_eq!(detect_workspace_root(&app), Some(ws.path().to_path_buf()));
        // A dir outside any workspace → None.
        let solo = tempdir().unwrap();
        assert_eq!(detect_workspace_root(solo.path()), None);
    }

    #[test]
    fn detects_package_workspaces_and_cargo() {
        let ws = tempdir().unwrap();
        fs::write(
            ws.path().join("package.json"),
            r#"{"workspaces":["packages/*"]}"#,
        )
        .unwrap();
        let app = ws.path().join("packages/a");
        fs::create_dir_all(&app).unwrap();
        assert_eq!(detect_workspace_root(&app), Some(ws.path().to_path_buf()));

        let cws = tempdir().unwrap();
        fs::write(cws.path().join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
        assert_eq!(
            detect_workspace_root(cws.path()),
            Some(cws.path().to_path_buf())
        );
    }

    /// SPEC-16: a bare `extends` package resolves from the workspace root's
    /// `node_modules`.
    #[test]
    fn resolves_workspace_preset() {
        let ws = tempdir().unwrap();
        fs::write(
            ws.path().join("pnpm-workspace.yaml"),
            "packages:\n  - apps/*\n",
        )
        .unwrap();
        let app = ws.path().join("apps/web");
        fs::create_dir_all(&app).unwrap();
        let preset = ws.path().join("node_modules/@scope/oxide-preset");
        fs::create_dir_all(&preset).unwrap();
        fs::write(preset.join("oxide.preset.json"), "{}").unwrap();
        assert_eq!(
            resolve_workspace_preset(&app, "@scope/oxide-preset"),
            Some(preset.join("oxide.preset.json"))
        );
        // A relative spec is left to the normal resolver.
        assert_eq!(resolve_workspace_preset(&app, "./local.ts"), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssr_externals_default_is_empty() {
        assert_eq!(BuildConfig::default().ssr_externals, Vec::<String>::new());
    }

    #[test]
    fn ssr_externals_roundtrip_camel_case() {
        // The TS key is `ssrExternals`; serde's rename_all = "camelCase" must
        // accept it from a JS config object.
        let json = r#"{"target":"baseline-widely-available","sourcemap":"hidden","ssrExternals":["vue","@vue/server-renderer"]}"#;
        let b: BuildConfig = serde_json::from_str(json).unwrap();
        assert_eq!(b.ssr_externals, vec!["vue", "@vue/server-renderer"]);
        let out = serde_json::to_string(&b).unwrap();
        assert!(out.contains("ssrExternals"));
    }
}

#[cfg(test)]
mod security_cfg_tests {
    use super::*;
    #[test]
    fn security_config_parses_allow_lists() {
        // SEC-05: the new keys deserialize from camelCase JSON.
        let j = r#"{"csp":"strict","csrf":"origin",
            "hostAllowList":["app.example.com"],
            "ssrfAllowList":["10.0.0.5"],
            "trustedProxies":["10.0.0.0/8"],
            "cookieKeys":["key1"]}"#;
        let s: SecurityConfig = serde_json::from_str(j).unwrap();
        assert_eq!(s.host_allow_list, vec!["app.example.com"]);
        assert_eq!(s.ssrf_allow_list, vec!["10.0.0.5"]);
        assert_eq!(s.trusted_proxies, vec!["10.0.0.0/8"]);
        assert_eq!(s.cookie_keys, vec!["key1"]);
    }
}

pub use extract::extract_public;
