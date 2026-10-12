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
mod css;
mod images;

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

/// Default CSS engine: nesting transform, module scoping (content-hash class names),
/// comment stripping, whitespace minify, autoprefix subset for common properties.
/// Implementation in `css.rs`.
pub struct DefaultCss;

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
    let hex = h.finalize().to_hex().to_string();
    let sig = &hex[..16];
    if url.contains('?') {
        format!("{url}&s={sig}")
    } else {
        format!("{url}?s={sig}")
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
                    css.push_str(&format!("  src: url(/fonts/{name}) format('woff2');\n"));
                }
                css.push_str("  font-display: swap;\n");
                css.push_str("}\n");
                face += 1;
            }
        }
        // Metric-override fallback face (placeholder math, see doc above).
        css.push_str("@font-face {\n");
        css.push_str(&format!("  font-family: '{}-fallback';\n", spec.family));
        css.push_str("  src: local('Times New Roman');\n");
        css.push_str("  size-adjust: 100%;\n");
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
pub fn robots_txt(rules: &[(&str, &str)], sitemap: Option<&str>) -> String {
    let mut s = String::new();
    for (agent, rule) in rules {
        s.push_str(&format!("User-agent: {agent}\nDisallow: {rule}\n"));
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
