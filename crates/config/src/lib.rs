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
pub mod schema;

/// Test-only wrapper around the minimal-config extractor. Production code
/// reaches the same result through `OxideConfig::load`; this exposes the
/// extractor's raw output so tests can assert on it directly.
#[doc(hidden)]
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

/// §16.3: a minimal, honest web app manifest. The fields are the ones a
/// PWA needs to be installable; anything richer (shortcuts, screenshots,
/// share_target) can be added without a shape change.
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
        }
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct FrameworkConfig {
    pub name: Option<String>,
    /// `react | vue | svelte | solid`
    pub adapter: Option<String>,
    pub compiler: Option<bool>,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct DevConfig {
    pub bundle: bool,
    pub https: bool,
    pub proxy: BTreeMap<String, String>,
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
pub fn load(
    dir: &Path,
    mode: &str,
    cli_overrides: BTreeMap<String, serde_json::Value>,
) -> Result<LoadedConfig> {
    let process_env: BTreeMap<String, String> = std::env::vars().collect();
    layer::load_with_env(dir, mode, cli_overrides, &process_env)
}

/// [`load`] with an explicit process-env map (injectable for tests).
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
pub fn extract_static_config(ts_source: &str) -> Option<serde_json::Value> {
    extract::extract(ts_source)
}

/// Env file parsing (dotenv-compatible: KEY=VALUE, quotes, comments, escapes).
pub fn parse_env_file(content: &str) -> BTreeMap<String, String> {
    env_file::parse(content)
}

pub mod native_rule {
    //! Re-exported mirror of router::NativeRule for config deserialization without a dep cycle.
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
    /// node_modules.
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
        assert!(BuildConfig::default().ssr_externals.is_empty());
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
