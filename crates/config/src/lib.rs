//! oxide-config — typed, validated, layered configuration (spec §14, D14).
//!
//! Layering (§14.2): defaults → preset → `oxide.config.ts` → mode file
//! (`oxide.config.production.ts`) → env (`OXIDE_*`) → CLI flags.
//! Env files: `.env`, `.env.local`, `.env.[mode]`, `.env.[mode].local`;
//! `PUBLIC_`-prefixed variables are exposed to client code.

use oxide_diagnostics::{Diagnostic, OxideError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

mod env_file;
pub mod schema;
mod extract;

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

pub use schema::{config_schema, config_schema_pretty, CONFIG_SCHEMA_URI};

pub fn err(code: &'static str, msg: impl Into<String>) -> OxideError {
    OxideError(Diagnostic::error(code, msg))
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
#[serde(rename_all = "camelCase", default)]
pub struct ServerConfig {
    pub port: u16,
    pub host: String,
    pub compression: Vec<String>,
    pub http3: bool,
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
            backend: JsBackend::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RenderConfig {
    /// `auto | static | isr | dynamic | spa | islands` (§10.1)
    pub default: String,
}
impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            default: "auto".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CacheConfig {
    pub handler: CacheHandlerConfig,
    /// e.g. "512mb" — parsed by `parse_size`.
    pub l0_max_size: String,
}
impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            handler: CacheHandlerConfig::default(),
            l0_max_size: "512mb".into(),
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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ImagesConfig {
    pub formats: Vec<String>,
    pub remote_patterns: Vec<RemotePattern>,
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
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
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
#[serde(rename_all = "camelCase", default)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SecurityConfig {
    /// `strict | hash | off`
    pub csp: String,
    /// `origin | token | off`
    pub csrf: String,
}
impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            csp: "strict".into(),
            csrf: "origin".into(),
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
#[serde(rename_all = "camelCase", default)]
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
}
impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            target: "baseline-widely-available".into(),
            sourcemap: "hidden".into(),
            ssr_externals: Vec::new(),
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
