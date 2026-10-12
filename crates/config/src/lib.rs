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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ServerConfig {
    pub port: u16,
    pub host: String,
    pub compression: Vec<String>,
    pub http3: bool,
}
impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: 3000,
            host: "localhost".into(),
            compression: vec!["zstd".into(), "br".into(), "gzip".into()],
            http3: false,
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
}
impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            target: "baseline-widely-available".into(),
            sourcemap: "hidden".into(),
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
