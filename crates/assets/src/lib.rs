//! oxide-assets — CSS pipeline, images, fonts, OG, content (spec §6.6–6.7, §16, D5, D19).
//!
//! CSS: Lightning-CSS-shaped pipeline behind `CssBackend` (parse, nesting, modules,
//! prefixing, minify, source maps, browserslist targets). Images: pure-Rust default
//! backend behind `ImageBackend` (D19) — resize/encode to AVIF/WebP/JPEG/PNG with
//! `?w=&format=` queries, srcset/sizes, LQIP placeholders, intrinsic dimensions.
//! Fonts: provider fetch → subset → WOFF2 → preload + size-adjust metric overrides.

use blake3::Hasher;
use oxide_diagnostics::{Diagnostic, OxideError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

mod content;
pub mod content_collection;
pub use content::{
    parse_content_entry_with_format, parse_mdx_source, ContentFormat, MdxCompileRequest,
};
pub use content_collection::{
    build_search_index, check_references, scan_collection, search_index_to_json,
    validate_collection, Collection, CollectionConfig, CollectionEntry, FieldType,
    IndexedEntry, NativeSchema, SchemaKind, SearchIndex,
};
mod css;
mod images;
mod lightningcss_backend;
mod font_metrics;
mod og_backend;
pub mod i18n;

pub use lightningcss_backend::LightningCssBackend;
pub use font_metrics::{metric_overrides, read_metrics, FontMetric, MetricOverrides, TIMES_NEW_ROMAN};
pub use og_backend::SvgOgBackend;
pub use i18n::{href, locale_prefix, negotiate, parse_accept_language, strip_locale_prefix, Catalog, I18nConfig, LanguageRange};

pub type Result<T> = std::result::Result<T, OxideError>;

pub fn err(code: &'static str, msg: impl Into<String>) -> OxideError {
    OxideError(Diagnostic::error(code, msg))
}

/// Asset import queries (§6.7): `?url`, `?raw`, `?inline`, `?worker`,
/// `?w=640&format=avif` image queries, SVG-as-component per adapter.
///
/// Documented: an escaped `\?` inside the path is a literal question mark (the first
/// *unescaped* `?` starts the query); bare flags (`?url`) map to empty values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetQuery {
    pub path: String,
    pub query: BTreeMap<String, String>,
}

pub fn parse_asset_query(specifier: &str) -> Result<AssetQuery> {
    if specifier.is_empty() || specifier.starts_with('?') {
        return Err(err(
            "AST-QUERY",
            "asset specifier must be a non-empty path optionally followed by `?query`",
        ));
    }
    let bytes = specifier.as_bytes();
    let mut split = None;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'?' && bytes.get(i.wrapping_sub(1)) != Some(&b'\\') {
            split = Some(i);
            break;
        }
    }
    let (path_raw, query_str) = match split {
        Some(idx) => (&specifier[..idx], &specifier[idx + 1..]),
        None => (specifier, ""),
    };
    // Unescape `\?` → `?` in the path portion.
    let path = path_raw.replace("\\?", "?");
    let mut query = BTreeMap::new();
    for pair in query_str.split('&').filter(|p| !p.is_empty()) {
        match pair.split_once('=') {
            Some((k, v)) => {
                query.insert(k.to_string(), v.to_string());
            }
            None => {
                query.insert(pair.to_string(), String::new());
            }
        }
    }
    Ok(AssetQuery { path, query })
}

// ---------------- CSS (§6.6, D5) ----------------

/// CSS backend trait (Lightning CSS slots in; D5 isolation).
pub trait CssBackend: Send + Sync {
    fn transform(&self, input: &CssInput) -> Result<CssOutput>;
}

#[derive(Debug, Clone)]
pub struct CssInput {
    pub code: String,
    pub file: Option<String>,
    /// css | module
    pub kind: CssKind,
    pub minify: bool,
    /// browserslist targets, e.g. ["chrome 100", "safari 15"].
    pub targets: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CssKind {
    Global,
    Module,
}

#[derive(Debug, Clone, Default)]
pub struct CssOutput {
    pub code: String,
    /// CSS modules: original local name → generated scoped name.
    pub exports: BTreeMap<String, String>,
    pub diagnostics: Vec<Diagnostic>,
}


/// Tailwind v4 integration point (§6.6): native plugin using the Rust scanner against
/// graph-known files (no separate glob pass). This scanner extracts candidate utilities;
/// the engine itself is provided by the tailwind plugin (docs/deviations.md).
pub fn scan_tailwind_candidates(sources: &[&str]) -> Vec<String> {
    css::scan_tailwind_candidates(sources)
}

// ---------------- Images (§16.1, D19) ----------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageFormat {
    Avif,
    Webp,
    Jpeg,
    Png,
}

impl ImageFormat {
    pub fn mime(&self) -> &'static str {
        match self {
            ImageFormat::Avif => "image/avif",
            ImageFormat::Webp => "image/webp",
            ImageFormat::Jpeg => "image/jpeg",
            ImageFormat::Png => "image/png",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "avif" => ImageFormat::Avif,
            "webp" => ImageFormat::Webp,
            "jpeg" | "jpg" => ImageFormat::Jpeg,
            "png" => ImageFormat::Png,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ImageRequest {
    pub source: Vec<u8>,
    pub source_format: ImageFormat,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub format: ImageFormat,
    pub quality: u8,
    /// LQIP / blur placeholder (§16.1).
    pub placeholder: Option<PlaceholderKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaceholderKind {
    Lqip,
    Blur,
}

#[derive(Debug, Clone)]
pub struct ImageResult {
    pub bytes: Vec<u8>,
    pub format: ImageFormat,
    pub width: u32,
    pub height: u32,
    /// Tiny placeholder (base64 data URI) when requested.
    pub placeholder: Option<String>,
    /// Cache key: hash(source + params).
    pub cache_key: String,
    /// Non-fatal diagnostics (e.g. `AST-WEBP` when a WebP request fell back to PNG).
    /// Additive field (Task 9): no cross-crate constructor existed.
    pub diagnostics: Vec<Diagnostic>,
}

/// ImageBackend trait (D19): pure-Rust default; libvips optional backend slots in.
pub trait ImageBackend: Send + Sync {
    fn process(&self, req: ImageRequest) -> Result<ImageResult>;
}

/// Default pure-Rust backend (image crate): decode, resize (triangle filter),
/// encode PNG/JPEG/WebP; AVIF encodes feature-gated (see docs/deviations.md).
/// Implementation in `images.rs`.
pub struct RustImageBackend;

/// srcset/sizes generator (§16.1): `?w=` widths → responsive candidates, ascending,
/// e.g. `"<base>?w=640 640w, <base>?w=1024 1024w"`. Deduplicated; empty → `""`.
pub fn srcset(widths: &[u32], base_url: &str) -> String {
    let mut ws = widths.to_vec();
    ws.sort_unstable();
    ws.dedup();
    ws.iter()
        .map(|w| format!("{base_url}?w={w} {w}w"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `/_oxide/image` endpoint request signing (§16.1, signed URLs optional):
/// appends `s=<blake3(url||secret)[..16]>` with `&` when the URL already has a query.
pub fn sign_image_url(url: &str, secret: &str) -> String {
    let mut h = Hasher::new();
    h.update(url.as_bytes());
    h.update(secret.as_bytes());
    // SEC-08: the full 64-hex digest. The old 16-hex (64-bit) prefix was
    // forgeable by birthday search; 256 bits is not.
    let sig = h.finalize().to_hex().to_string();
    if url.contains('?') {
        format!("{url}&s={sig}")
    } else {
        format!("{url}?s={sig}")
    }
}

/// §16.1 / SEC-08: verify the signature `sign_image_url` produced.
/// Constant-time compare (see [`constant_time_eq_hex`]).
pub fn verify_image_signature(url: &str, secret: &str, sig: &str) -> bool {
    let mut h = Hasher::new();
    h.update(url.as_bytes());
    h.update(secret.as_bytes());
    let expected = h.finalize().to_hex().to_string();
    constant_time_eq_hex(&expected, sig)
}

/// SEC-08: constant-time hex-string compare. Length is allowed to leak (it
/// is public); the bytes are compared without an early exit.
pub fn constant_time_eq_hex(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// §16.1: an `/_oxide/image` request parsed from its query string. Every
/// field is optional except `url`; the endpoint validates and defaults the
/// rest. Errors carry a stable code so a response can name the reason.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageEndpointRequest {
    /// Source path or URL. A path starting with `/` resolves under the
    /// site's public asset root; an `http(s)://` URL is checked against
    /// `ImagesConfig.remotePatterns`.
    pub url: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Output format from `?format=`. `None` means "negotiate from the
    /// request's `Accept` header" — the endpoint picks WebP/AVIF when the
    /// client supports it and falls back to the source format otherwise.
    pub format: Option<ImageFormat>,
    /// Quality 1-100; the backend clamps. `None` uses the backend default.
    pub quality: Option<u8>,
    pub placeholder: Option<PlaceholderKind>,
    /// Request signature (`?s=`). Present only when the deployment
    /// configures a signing secret.
    pub signature: Option<String>,
}

/// Parse the query string of an `/_oxide/image` request. Tolerates missing
/// `?`, duplicate keys (last wins), and percent-encoded values in `url`.
pub fn parse_image_request(query: &str) -> Result<ImageEndpointRequest> {
    let mut url: Option<String> = None;
    let mut width = None;
    let mut height = None;
    let mut format = None;
    let mut quality = None;
    let mut placeholder = None;
    let mut signature = None;
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        match k {
            "url" => {
                if v.is_empty() {
                    return Err(err("AST-IMG-REQ", "`url` query parameter is empty"));
                }
                url = Some(percent_decode(v)?);
            }
            "w" => {
                width = Some(
                    v.parse::<u32>()
                        .map_err(|_| err("AST-IMG-REQ", format!("`w` is not an integer: {v:?}")))?,
                );
            }
            "h" => {
                height = Some(
                    v.parse::<u32>()
                        .map_err(|_| err("AST-IMG-REQ", format!("`h` is not an integer: {v:?}")))?,
                );
            }
            "format" => {
                format = Some(match v.to_ascii_lowercase().as_str() {
                    "avif" => ImageFormat::Avif,
                    "webp" => ImageFormat::Webp,
                    "jpeg" | "jpg" => ImageFormat::Jpeg,
                    "png" => ImageFormat::Png,
                    other => {
                        return Err(err(
                            "AST-IMG-REQ",
                            format!("unknown `format`: {other:?}"),
                        ))
                    }
                });
            }
            "q" | "quality" => {
                let q = v
                    .parse::<u32>()
                    .map_err(|_| err("AST-IMG-REQ", format!("`q` is not an integer: {v:?}")))?;
                if q == 0 || q > 100 {
                    return Err(err("AST-IMG-REQ", format!("`q` out of range: {q}")));
                }
                quality = Some(q as u8);
            }
            "placeholder" => {
                placeholder = Some(match v {
                    "lqip" => PlaceholderKind::Lqip,
                    "blur" => PlaceholderKind::Blur,
                    other => {
                        return Err(err(
                            "AST-IMG-REQ",
                            format!("unknown `placeholder`: {other:?}"),
                        ))
                    }
                });
            }
            "s" => signature = Some(v.to_string()),
            _ => {
                // Unknown query parameters are ignored, matching the asset
                // query convention in §6.7 (`?w=&format=` are the ones that
                // matter; anything else rides along harmlessly).
            }
        }
    }
    let url = url.ok_or_else(|| err("AST-IMG-REQ", "`url` query parameter is required"))?;
    Ok(ImageEndpointRequest {
        url,
        width,
        height,
        format,
        quality,
        placeholder,
        signature,
    })
}

/// Percent-decode a query value. `+` is treated as a literal plus (a URL in
/// the `url=` parameter carries its own encoding); only `%XX` sequences are
/// decoded. Invalid escapes are left untouched rather than erroring, which
/// keeps a hostile input from 500-ing the endpoint.
fn percent_decode(s: &str) -> Result<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = hex_val(bytes[i + 1]);
            let lo = hex_val(bytes[i + 2]);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).map_err(|_| err("AST-IMG-REQ", "`url` is not valid UTF-8"))
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

// ---------------- Fonts (§16.2) ----------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FontSpec {
    pub family: String,
    pub provider: String,
    pub weights: Vec<u32>,
    pub styles: Vec<String>,
    pub subset: Vec<String>,
    pub local_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FontBundle {
    /// WOFF2 files emitted (subset per §16.2 — pass-through in default backend).
    pub files: Vec<(String, Vec<u8>)>,
    /// @font-face CSS with preload hints + size-adjust metric overrides on fallbacks.
    pub css: String,
    /// URLs to preload (critical fonts).
    pub preload: Vec<String>,
}

/// FontBackend trait (D19): fontations/skrifa subsetting slots in.
pub trait FontBackend: Send + Sync {
    fn bundle(&self, spec: &FontSpec) -> Result<FontBundle>;
}

pub struct DefaultFontBackend;


/// Compute metric overrides from the first font file that parses (skrifa). WOFF2
/// input (compressed) or any parse failure returns `None`, so the caller falls back
/// to a neutral 100% face.
fn metric_overrides_for(spec: &FontSpec) -> Option<font_metrics::MetricOverrides> {
    for p in &spec.local_paths {
        if let Ok(bytes) = std::fs::read(p) {
            if let Ok(m) = font_metrics::read_metrics(&bytes) {
                return Some(font_metrics::metric_overrides(m, font_metrics::TIMES_NEW_ROMAN));
            }
        }
    }
    None
}

impl FontBackend for DefaultFontBackend {
    /// Local pass-through bundling (§16.2): files are copied verbatim (subsetting via
    /// fontations/skrifa slots in), one `@font-face` per weight×style with
    /// `font-display: swap`, preload = first file (the critical face), plus a
    /// `family-fallback` face with `size-adjust: 100%`.
    /// Documented: `size-adjust: 100%` is a placeholder — real metric math
    /// (ascent/descent/lineGap from the font tables) lands with fontations.
    fn bundle(&self, spec: &FontSpec) -> Result<FontBundle> {
        let mut files = Vec::new();
        for p in &spec.local_paths {
            let bytes = std::fs::read(p).map_err(|e| {
                err(
                    "AST-FONT",
                    format!("failed to read font file {}: {e}", p.display()),
                )
            })?;
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "font.woff2".to_string());
            files.push((name, bytes));
        }

        let weights: Vec<Option<u32>> = if spec.weights.is_empty() {
            vec![None]
        } else {
            spec.weights.iter().map(|w| Some(*w)).collect()
        };
        let styles: Vec<&str> = if spec.styles.is_empty() {
            vec!["normal"]
        } else {
            spec.styles.iter().map(String::as_str).collect()
        };

        let mut css = String::new();
        let mut face = 0usize;
        for weight in &weights {
            for style in &styles {
                css.push_str("@font-face {\n");
                css.push_str(&format!("  font-family: '{}';\n", spec.family));
                if let Some(w) = weight {
                    css.push_str(&format!("  font-weight: {w};\n"));
                }
                css.push_str(&format!("  font-style: {style};\n"));
                if !files.is_empty() {
                    let (name, _) = &files[face % files.len()];
                    // The `format()` hint must match the real container — a
                    // `.ttf` emitted with `format('woff2')` makes the browser
                    // skip the face. Derive it from the file extension and
                    // fall back to `woff2` (the spec's preferred container).
                    let fmt = match std::path::Path::new(name)
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(|e| e.to_ascii_lowercase())
                        .as_deref()
                    {
                        Some("woff2") => "woff2",
                        Some("woff") => "woff",
                        Some("ttf") => "truetype",
                        Some("otf") => "opentype",
                        _ => "woff2",
                    };
                    css.push_str(&format!("  src: url(/fonts/{name}) format('{fmt}');\n"));
                }
                css.push_str("  font-display: swap;\n");
                css.push_str("}\n");
                face += 1;
            }
        }
        // Metric-override fallback face (§16.2). The overrides are computed from
        // the target font's real hhea metrics (via skrifa) and Times New Roman's
        // canonical metrics, so the swap from web font to local fallback has
        // identical vertical metrics and no layout shift.
        css.push_str("@font-face {\n");
        css.push_str(&format!("  font-family: '{}-fallback';\n", spec.family));
        css.push_str("  src: local('Times New Roman');\n");
        match metric_overrides_for(spec) {
            Some(o) => {
                css.push_str(&format!("  size-adjust: {:.2}%;\n", o.size_adjust));
                css.push_str(&format!("  ascent-override: {:.2}%;\n", o.ascent_override));
                css.push_str(&format!("  descent-override: {:.2}%;\n", o.descent_override));
                css.push_str(&format!("  line-gap-override: {:.2}%;\n", o.line_gap_override));
            }
            None => {
                // WOFF2 (compressed) or unparseable input: keep the neutral fallback.
                css.push_str("  size-adjust: 100%;\n");
            }
        }
        css.push_str("  font-display: swap;\n");
        css.push_str("}\n");

        let preload = files
            .first()
            .map(|(name, _)| format!("/fonts/{name}"))
            .into_iter()
            .collect();

        Ok(FontBundle {
            files,
            css,
            preload,
        })
    }
}

// ---------------- Metadata / SEO (§16.3) ----------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Metadata {
    pub title: Option<String>,
    /// Title template, e.g. "%s · Acme".
    pub title_template: Option<String>,
    pub description: Option<String>,
    pub canonical: Option<String>,
    pub robots: Option<String>,
    pub open_graph: BTreeMap<String, String>,
    pub twitter: BTreeMap<String, String>,
    pub alternates: Vec<(String, String)>,
    pub json_ld: Vec<String>,
}

impl Metadata {
    /// Emit neutral head tags (consumed by render::HeadTags). Meta key convention
    /// mirrors render: `name:<n>`, `property:<p>`; `og:*` uses `property`,
    /// `twitter:*` uses `name`. The title template (`%s`) applies only when a title
    /// is set and contains `%s`.
    pub fn to_head(&self) -> crate::render_types::HeadContrib {
        let mut head = crate::render_types::HeadContrib::default();
        if let Some(t) = &self.title {
            let final_title = match &self.title_template {
                Some(tmpl) if tmpl.contains("%s") => tmpl.replacen("%s", t, 1),
                _ => t.clone(),
            };
            head.meta
                .push(("property:og:title".to_string(), final_title.clone()));
            head.title = Some(final_title);
        }
        if let Some(d) = &self.description {
            head.meta.push(("name:description".to_string(), d.clone()));
            head.meta
                .push(("property:og:description".to_string(), d.clone()));
        }
        if let Some(r) = &self.robots {
            head.meta.push(("name:robots".to_string(), r.clone()));
        }
        for (k, v) in &self.open_graph {
            head.meta.push((format!("property:og:{k}"), v.clone()));
        }
        for (k, v) in &self.twitter {
            head.meta.push((format!("name:twitter:{k}"), v.clone()));
        }
        if let Some(c) = &self.canonical {
            let mut attrs = BTreeMap::new();
            attrs.insert("href".to_string(), c.clone());
            head.links.push(("canonical".to_string(), attrs));
        }
        for (hreflang, href) in &self.alternates {
            let mut attrs = BTreeMap::new();
            attrs.insert("href".to_string(), href.clone());
            attrs.insert("hreflang".to_string(), hreflang.clone());
            head.links.push(("alternate".to_string(), attrs));
        }
        head.json_ld = self.json_ld.clone();
        head
    }
}

/// Minimal mirror of render::HeadTags to avoid a dep cycle.
pub mod render_types {
    #[derive(Debug, Clone, Default)]
    pub struct HeadContrib {
        pub title: Option<String>,
        pub meta: Vec<(String, String)>,
        pub links: Vec<(String, std::collections::BTreeMap<String, String>)>,
        pub json_ld: Vec<String>,
    }
}

/// sitemap.xml generation (§16.3): one `<url><loc>` per URL, XML-escaped.
pub fn sitemap_xml(urls: &[&str]) -> String {
    let mut s = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    s.push_str("<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n");
    for u in urls {
        s.push_str(&format!("<url><loc>{}</loc></url>\n", xml_escape(u)));
    }
    s.push_str("</urlset>");
    s
}

/// robots.txt (§16.3): one `User-agent:`/`Disallow:` group per `(agent, rule)`,
/// optional trailing `Sitemap:` line.
///
/// With no rules the emitted file is still valid — a crawler reads an empty
/// `robots.txt` as "allow everything" — but an explicit `User-agent: *`
/// group is clearer and is what every deployment actually wants. Emit that
/// default when the caller passes no rules.
pub fn robots_txt(rules: &[(&str, &str)], sitemap: Option<&str>) -> String {
    let mut s = String::new();
    if rules.is_empty() {
        s.push_str("User-agent: *\nDisallow:\n");
    } else {
        for (agent, rule) in rules {
            s.push_str(&format!("User-agent: {agent}\nDisallow: {rule}\n"));
        }
    }
    if let Some(sm) = sitemap {
        s.push_str(&format!("Sitemap: {sm}\n"));
    }
    s
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

// ---------------- OG images (§16.3) ----------------

/// OG image template: neutral element tree rendered through taffy/parley/resvg
/// behind the OgBackend trait (D19); cached by input hash. Default backend returns
/// a diagnostic explaining the feature-gated rasterizer (docs/deviations.md).
pub trait OgBackend: Send + Sync {
    fn render(&self, tree: &serde_json::Value, width: u32, height: u32) -> Result<Vec<u8>>;
}

pub fn og_cache_key(tree: &serde_json::Value, width: u32, height: u32) -> String {
    let mut h = Hasher::new();
    h.update(serde_json::to_vec(tree).unwrap_or_default().as_slice());
    h.update(&width.to_le_bytes());
    h.update(&height.to_le_bytes());
    h.finalize().to_hex().to_string()
}

// ---------------- Content (§16.5) ----------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContentEntry {
    pub path: PathBuf,
    pub slug: String,
    pub frontmatter: BTreeMap<String, serde_json::Value>,
    pub body_markdown: String,
}

/// Markdown → HTML (CommonMark + tables via a minimal renderer; comrak slots in).
pub fn markdown_to_html(md: &str) -> String {
    content::markdown_to_html(md)
}

/// Typed content collections (§16.5): Standard Schema frontmatter validation happens
/// in TS; here: parse frontmatter, slug, reference checking between entries.
pub fn parse_content_entry(path: &Path, source: &str) -> Result<ContentEntry> {
    content::parse_content_entry(path, source)
}

/// Syntax highlighting at build time (§16.5): minimal token highlighter for common
/// languages (tree-sitter/syntect slot in behind the same signature).
pub fn highlight(code: &str, lang: &str) -> String {
    content::highlight(code, lang)
}

#[cfg(test)]
mod tests;
