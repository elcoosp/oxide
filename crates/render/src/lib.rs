//! oxide-fw-render — streaming pipeline, html injection, boundary protocol (spec §7.1, §10, D12).
//!
//! The render sequence (§10.2): route matched + L0 checked → loads in isolate →
//! adapter.render() streams the shell → Rust injects head/CSS/modulepreload/CSP nonces/
//! ODF data/hydration bootstrap via html rewriting → deferred boundaries stream as they
//! resolve → eligible responses stored in L0 with recorded tags. Early Hints (103) are
//! sent for known critical assets before loads finish.
//!
//! # Deferred-boundary swapper split (§7.1)
//!
//! Both [`boundary_content`] and [`swapper_script`] derive their JS from the single
//! [`SWAPPER_SRC`] constant:
//!
//! - [`boundary_content`] emits a **self-contained IIFE** with the boundary id baked in.
//!   It must work even if only that one script ran (e.g. mid-stream, before the shell's
//!   shared swapper was ever installed), so it never touches `window.__oxSwap`.
//! - [`swapper_script`] installs the **shared** `window.__oxSwap` function plus an
//!   `ox:defer` event listener; the hydration bootstrap (and the client router's deferred
//!   queue, §8.4) can then replay queued swaps by dispatching `ox:defer` events.

pub mod ppr;

use base64::Engine as _;
use bytes::Bytes;
use futures::StreamExt;
use futures::channel::mpsc;
use lol_html::html_content::ContentType;
use lol_html::{ElementContentHandlers, HtmlRewriter, RewriteStrSettings, Selector};
use oxide_fw_diagnostics::{Diagnostic, OxideError};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub type Result<T> = std::result::Result<T, OxideError>;

pub fn err(code: &'static str, msg: impl Into<String>) -> OxideError {
    OxideError(Box::new(Diagnostic::error(code, msg)))
}

/// Render mode (§10.1). Inference: `static` unless the load reads request-specific
/// values (cookies, headers, undeclared search params, locals) → `dynamic`;
/// explicit `config.render` always wins. The compiler reports the exact read that
/// made a route dynamic (`oxide routes`, dev inspector).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RenderMode {
    Static,
    Isr,
    Dynamic,
    Spa,
    Islands,
}

impl RenderMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            RenderMode::Static => "static",
            RenderMode::Isr => "isr",
            RenderMode::Dynamic => "dynamic",
            RenderMode::Spa => "spa",
            RenderMode::Islands => "islands",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "static" => RenderMode::Static,
            "isr" => RenderMode::Isr,
            "dynamic" => RenderMode::Dynamic,
            "spa" => RenderMode::Spa,
            "islands" => RenderMode::Islands,
            _ => return None,
        })
    }
}

/// What made a route dynamic (§10.1 inference report).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DynamicReason {
    pub read: String,
    pub location: Option<String>,
}

/// Head tags contributed by loads, meta and adapters (neutral model, §16.3).
///
/// Meta entries are `(key, content)` where `key` is either `name:<n>`,
/// `property:<p>`, or a bare `<n>` (which means `name:<n>`). Link/script entries are
/// `(href|body, extra attributes)`; attributes serialize in sorted order so output is
/// deterministic.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HeadTags {
    pub title: Option<String>,
    pub meta: Vec<(String, String)>,
    pub links: Vec<(String, BTreeMap<String, String>)>,
    pub scripts: Vec<(String, BTreeMap<String, String>)>,
    pub json_ld: Vec<String>,
}

impl HeadTags {
    /// Merge across layouts: outer-first, later (inner) wins on conflicts (§16.3).
    ///
    /// Title: inner wins when present. Meta: an inner entry replaces the outer entry
    /// with the same normalized (`name`/`property`) key — the outer entry keeps its
    /// position for keys untouched by inner, new keys append in inner order. Links,
    /// scripts and JSON-LD are concatenated (outer first).
    pub fn merge(self, inner: HeadTags) -> HeadTags {
        let title = inner.title.or(self.title);
        let inner_keys: std::collections::BTreeSet<String> =
            inner.meta.iter().map(|(k, _)| meta_norm_key(k)).collect();
        let mut meta: Vec<(String, String)> = self
            .meta
            .into_iter()
            .filter(|(k, _)| !inner_keys.contains(&meta_norm_key(k)))
            .collect();
        meta.extend(inner.meta);
        let mut links = self.links;
        links.extend(inner.links);
        let mut scripts = self.scripts;
        scripts.extend(inner.scripts);
        let mut json_ld = self.json_ld;
        json_ld.extend(inner.json_ld);
        HeadTags {
            title,
            meta,
            links,
            scripts,
            json_ld,
        }
    }

    /// Serialize into `<head>` markup with CSP nonce on injected scripts (§17.1).
    pub fn to_html(&self, nonce: &str) -> String {
        let mut out = String::new();
        if let Some(t) = &self.title {
            out.push_str("<title>");
            out.push_str(&escape_text(t));
            out.push_str("</title>");
        }
        for (key, content) in &self.meta {
            let (kind, ident) = meta_kind_and_ident(key);
            out.push_str(&format!(
                "<meta {kind}=\"{}\" content=\"{}\">",
                escape_attr(ident),
                escape_attr(content)
            ));
        }
        for (href, attrs) in &self.links {
            out.push_str(&format!("<link href=\"{}\"", escape_attr(href)));
            for (k, v) in attrs {
                out.push_str(&format!(" {k}=\"{}\"", escape_attr(v)));
            }
            out.push('>');
        }
        for (body, attrs) in &self.scripts {
            out.push_str("<script");
            for (k, v) in attrs {
                out.push_str(&format!(" {k}=\"{}\"", escape_attr(v)));
            }
            out.push_str(&format!(" nonce=\"{}\"", escape_attr(nonce)));
            out.push('>');
            out.push_str(&guard_script(body));
            out.push_str("</script>");
        }
        for json in &self.json_ld {
            out.push_str(&format!(
                "<script type=\"application/ld+json\" nonce=\"{}\">",
                escape_attr(nonce)
            ));
            out.push_str(&escape_json_script(json));
            out.push_str("</script>");
        }
        out
    }
}

/// Split a meta key into (`kind`, `ident`); bare keys default to `name`.
fn meta_kind_and_ident(key: &str) -> (&str, &str) {
    if let Some(rest) = key.strip_prefix("name:") {
        ("name", rest)
    } else if let Some(rest) = key.strip_prefix("property:") {
        ("property", rest)
    } else {
        ("name", key)
    }
}

fn meta_norm_key(key: &str) -> String {
    let (kind, ident) = meta_kind_and_ident(key);
    format!("{kind}:{ident}")
}

/// Escape an HTML attribute value minimally: `& < > " '`.
/// All injected attributes are single-quoted (SEC-07), so `'` must escape.
pub fn escape_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Escape HTML text content minimally: `& < >`.
/// SEC-07: 500 pages interpolate worker error text — escape it.
pub fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

/// Neutralize `</script` (case-insensitive) inside an inline script body so the HTML
/// parser cannot terminate the script early. Only exact closers are rewritten (the next
/// character must not be alphanumeric), so e.g. `</SCRIPTX` is left alone.
fn guard_script(s: &str) -> String {
    let lower = s.to_ascii_lowercase();
    let pat = "</script";
    let mut out = String::with_capacity(s.len());
    let mut rest = 0usize;
    while let Some(i) = lower[rest..].find(pat) {
        let at = rest + i;
        let after_ok = lower[at + pat.len()..]
            .chars()
            .next()
            .map(|c| !c.is_ascii_alphanumeric())
            .unwrap_or(true);
        if !after_ok {
            out.push_str(&s[rest..at + pat.len()]);
            rest = at + pat.len();
            continue;
        }
        out.push_str(&s[rest..at]);
        out.push_str("<\\/script");
        rest = at + pat.len();
    }
    out.push_str(&s[rest..]);
    out
}

/// Escape `<` as `\u003c` for JSON embedded in a script element (valid only inside JSON
/// strings, which is where `<` can legally appear in JSON).
fn escape_json_script(s: &str) -> String {
    s.replace('<', "\\u003c")
}

/// The deferred-boundary protocol (§7.1):
/// - placeholder: `<template data-ox-b='ID'>` + fallback content
/// - resolution: `<template data-ox-c='ID'>…</template>` + ~300-byte inline swapper,
///   with CSP nonce. `suspense: 'native'` adapters use their own mechanism.
pub const BOUNDARY_FALLBACK_ATTR: &str = "data-ox-b";
pub const BOUNDARY_CONTENT_ATTR: &str = "data-ox-c";

pub fn boundary_fallback(id: u32, fallback_html: &str) -> String {
    format!("<template {BOUNDARY_FALLBACK_ATTR}='{id}'>{fallback_html}</template>")
}

/// The single source of truth for the swapper logic (§7.1): a JS function expression
/// parameterized by the boundary id. ~350 bytes, CSP-safe (no eval, DOM APIs only):
/// finds `template[data-ox-c='<id>']`, moves its content over the matching
/// `template[data-ox-b='<id>']` placeholder via `replaceWith`, then dispatches the
/// `ox:swap` CustomEvent (`detail: {id, value}`) that triggers adapter hydration.
///
/// `detail.value` is the resolved boundary's content (the `innerHTML` of the
/// content template — the HTML the server streamed). Before this carried a
/// value, `attachDeferGlue`'s `ox:swap` handler resolved every deferred slot to
/// `undefined` because the event only had `{id}`. Consumers that wait on a
/// `DeferredClient` now receive the server-streamed content, not an opaque
/// undefined — the data-level value still needs the richer protocol (§9.2
/// `defer()` returns a placeholder; the server streams the *rendered* boundary,
/// not the raw data value), but at least the swap event now carries what it
/// swapped in.
/// The §7.1 swapper.
///
/// The selectors are attribute-only (`[data-ox-c='<id>']`, not
/// `template[data-ox-c='<id>']`) so the same script drives both boundary
/// hosts:
///   - the IR renderer emits `<template data-ox-*>` (a template is the
///     natural inert container and needs no wrapper element);
///   - a `suspense: 'core'` framework emits its own host element —
///     `<ox-defer data-ox-*>` — because a Vue/Svelte `<template>` renders
///     with the framework's hydration markers inside it, which `innerHTML`
///     would then move into the live DOM.
///
/// `querySelector` matches either. A deployment that emits one host shape
/// is unaffected by the selector being wider.
pub const SWAPPER_SRC: &str = r#"function(id){var t=document.querySelector("[data-ox-c='"+id+"']"),p=document.querySelector("[data-ox-b='"+id+"']");if(t&&p){var d=document.createElement("div");d.innerHTML=t.innerHTML;p.replaceWith.apply(p,d.childNodes);document.dispatchEvent(new CustomEvent("ox:swap",{detail:{id:id,value:t.innerHTML}}))}}"#;

/// Streamed boundary resolution (§7.1): the content template plus a self-contained
/// swapper IIFE with `id` baked in. The script does not depend on `window.__oxSwap`
/// (see the module docs for the split between this and [`swapper_script`]).
pub fn boundary_content(id: u32, html: &str, nonce: &str) -> String {
    boundary_content_with_id(&id.to_string(), html, nonce)
}

fn boundary_content_with_id(id: &str, html: &str, nonce: &str) -> String {
    format!(
        "<template {BOUNDARY_CONTENT_ATTR}='{id}'>{html}</template><script nonce='{n}'>({src})('{id}')</script>",
        n = escape_attr(nonce),
        src = SWAPPER_SRC,
    )
}

/// The shared swapper bootstrap (§7.1): installs `window.__oxSwap` (from the same
/// [`SWAPPER_SRC`] used by [`boundary_content`]) plus an `ox:defer` listener used by the
/// hydration bootstrap to replay deferred boundaries queued before hydration.
pub fn swapper_script(nonce: &str) -> String {
    format!(
        "<script nonce='{n}'>window.__oxSwap={src};window.addEventListener('ox:defer',function(e){{window.__oxSwap(e.detail.id)}});</script>",
        n = escape_attr(nonce),
        src = SWAPPER_SRC,
    )
}

/// Islands markup (§10.3): `client:load | client:idle | client:visible | client:media(...)`
/// renders inside `<ox-island>` with ODF-encoded props + chunk reference; the core's
/// 1.5 KB loader schedules hydration; adapter's `hydrateIsland` performs it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IslandDirective {
    pub strategy: IslandStrategy,
    pub component: String,
    pub chunk: String,
    pub props_b64: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum IslandStrategy {
    Load,
    Idle,
    Visible,
    Media(String),
}

pub fn island_element(directive: &IslandDirective, ssr_html: &str) -> String {
    let strategy = match &directive.strategy {
        IslandStrategy::Load => "load".to_string(),
        IslandStrategy::Idle => "idle".to_string(),
        IslandStrategy::Visible => "visible".to_string(),
        IslandStrategy::Media(q) => format!("media({q})"),
    };
    format!(
        "<ox-island data-ox-island='{c}' data-ox-chunk='{k}' data-ox-strategy='{s}' data-ox-props='{p}'>{ssr}</ox-island>",
        c = escape_attr(&directive.component),
        k = escape_attr(&directive.chunk),
        s = escape_attr(&strategy),
        p = escape_attr(&directive.props_b64),
        ssr = ssr_html,
    )
}

/// `<Script>` placeholder emission (spec §16.6).
///
/// The server renders an inert `<script data-ox-script …>` tag that carries
/// the loading intent. The client scheduler in
/// `packages/core/src/client/scripts.ts` scans `script[data-ox-script]` and
/// injects the *real* `<script>` element per strategy:
///
/// - `defer` (default), `idle`, `worker`, `afterInteractive`, `lazy`.
///
/// The placeholder is inert and CSP-safe:
/// - It has **no** `src` attribute (a `src` script's URL goes in `data-ox-src`);
///   appending `<script src>` here would defeat the deferred strategy.
/// - It has **no** text content (an inline body is base64-encoded into
///   `data-ox-inline`).
/// - It carries the page's CSP `nonce`, so a `strict-dynamic` policy admits it.
///   The empty body means nothing runs; the client injects the real code.
///
/// Exactly one of `src` / `content` must be present. A `<Script>` with
/// neither has nothing to load; a `<Script>` with both is ambiguous. Both are
/// rejected with `RND-SCRIPT` — fail loudly rather than silently drop.
///
/// Attribute order is deterministic so golden tests and diff-based CI are
/// stable: marker, strategy, src|inline, module flag, user attrs (sorted by
/// `BTreeMap`), nonce.
pub fn script_placeholder(
    strategy: &str,
    src: Option<&str>,
    content: Option<&str>,
    module: bool,
    attrs: &BTreeMap<String, String>,
    nonce: &str,
) -> Result<String> {
    match (src, content) {
        (None, None) => {
            return Err(err(
                "RND-SCRIPT",
                "<Script> requires either a `src` or inline `content`",
            ));
        }
        (Some(_), Some(_)) => {
            return Err(err(
                "RND-SCRIPT",
                "<Script> cannot have both `src` and inline `content`",
            ));
        }
        _ => {}
    }
    let mut out = String::from("<script data-ox-script data-ox-script-placeholder");
    out.push_str(&format!(" data-ox-strategy=\"{}\"", escape_attr(strategy)));
    if let Some(s) = src {
        out.push_str(&format!(" data-ox-src=\"{}\"", escape_attr(s)));
    } else if let Some(c) = content {
        let b64 = base64::engine::general_purpose::STANDARD.encode(c.as_bytes());
        out.push_str(&format!(" data-ox-inline=\"{}\"", b64));
    }
    if module {
        out.push_str(" data-ox-module");
    }
    for (k, v) in attrs {
        out.push_str(&format!(" {k}=\"{}\"", escape_attr(v)));
    }
    out.push_str(&format!(" nonce=\"{}\"", escape_attr(nonce)));
    out.push_str("></script>");
    Ok(out)
}

/// Hydration bootstrap: `<script type='application/oxide-data' nonce>` payload +
/// router bootstrap wiring (§9.5 HTML embedding + §8.4 client router).
///
/// `route_id` is embedded in a JS string literal; characters outside `[a-z0-9-/]` are
/// percent-encoded (byte-wise, uppercase hex) so the output is always safe and
/// deterministic. Percent-encoding (rather than an error) keeps this total — a hostile
/// route id degrades to an encoded form instead of failing the render.
pub fn hydration_bootstrap(odf_b64: &str, route_id: &str, nonce: &str) -> String {
    let n = escape_attr(nonce);
    format!(
        "<script type='application/oxide-data' nonce='{n}'>{b64}</script>\
<script nonce='{n}'>window.__oxide={{route:\"{route}\",data:document.currentScript.previousElementSibling.textContent}}</script>",
        b64 = odf_b64,
        route = sanitize_route_id(route_id),
    )
}

/// Percent-encode every byte outside the route-id alphabet `[a-z0-9-/]`.
fn sanitize_route_id(route_id: &str) -> String {
    let mut out = String::with_capacity(route_id.len());
    for b in route_id.as_bytes() {
        match b {
            b'a'..=b'z' | b'0'..=b'9' | b'-' | b'/' => out.push(*b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Percent-encode every byte outside the boundary-id alphabet `[A-Za-z0-9_-]`.
fn sanitize_boundary_id(id: &str) -> String {
    let mut out = String::with_capacity(id.len());
    for b in id.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' => out.push(*b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Streaming render output.
pub struct RenderStream {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    /// Byte chunks; None = end of stream.
    pub rx: mpsc::Receiver<Result<Bytes>>,
    /// Tags to record in L0 on completion (§10.2 step 6).
    pub tags: Vec<String>,
}

impl RenderStream {
    /// Drain the stream into one string (test/inspection helper).
    pub async fn collect_to_string(self) -> Result<String> {
        let mut rx = self.rx;
        let mut buf: Vec<u8> = Vec::new();
        while let Some(item) = rx.next().await {
            buf.extend_from_slice(&item?);
        }
        String::from_utf8(buf)
            .map_err(|e| err("RND-UTF8", format!("render stream was not UTF-8: {e}")))
    }
}

/// Per-request timing waterfall (§13.4 inspector, §20 Server-Timing).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RequestTimings {
    pub middleware_ms: f64,
    pub load_ms: f64,
    pub render_ms: f64,
    pub inject_ms: f64,
    pub cache_ms: f64,
}

impl RequestTimings {
    /// `Server-Timing` header value (§12.1 step 8): match, cache, load, render, inject.
    pub fn server_timing(&self) -> String {
        format!(
            "match;dur={:.2}, cache;dur={:.2}, load;dur={:.2}, render;dur={:.2}, inject;dur={:.2}",
            self.middleware_ms, self.cache_ms, self.load_ms, self.render_ms, self.inject_ms
        )
    }
}

/// Injection pass: given a streamed HTML shell chunk-stream, inject head tags, CSS +
/// modulepreload links (critical order, §10.2 step 4), CSP nonces, ODF data and the
/// hydration bootstrap — using lol_html streaming rewrites (D: lol_html behind render crate).
pub struct Injector {
    pub head: HeadTags,
    pub nonce: String,
    pub preload: Vec<String>,
    pub css_links: Vec<String>,
    pub odf_payload_b64: Option<String>,
    pub route_id: String,
    /// Extra HTML appended to the body injection fragment, AFTER the
    /// hydration bootstrap. The CLI uses it for the resolved-deferred
    /// `<script type="application/oxide-deferred">` on prerendered pages
    /// (§7.1 / §10.2 step 5: prerendered pages must resolve their defer
    /// slots exactly like live pages do). The fragment is trusted (built
    /// by the pipeline, `<`-escaped JSON inside).
    pub body_extra: String,
}

impl Injector {
    pub fn new(head: HeadTags, nonce: String, route_id: String) -> Self {
        Self {
            head,
            nonce,
            preload: Vec::new(),
            css_links: Vec::new(),
            odf_payload_b64: None,
            route_id,
            body_extra: String::new(),
        }
    }

    /// The head and body injection fragments (shared by the batch and streaming paths).
    fn injection_fragments(&self) -> (String, Option<String>) {
        let mut head = self.head.to_html(&self.nonce);
        for url in &self.preload {
            head.push_str(&format!(
                "<link rel=\"modulepreload\" href=\"{}\">",
                escape_attr(url)
            ));
        }
        for url in &self.css_links {
            head.push_str(&format!(
                "<link rel=\"stylesheet\" href=\"{}\">",
                escape_attr(url)
            ));
        }
        // The body fragment is the hydration bootstrap (when a payload is
        // set) plus any pipeline-supplied extra. Emitted when EITHER is
        // present so a payload-less injector can still append `body_extra`.
        let body = if self.odf_payload_b64.is_some() || !self.body_extra.is_empty() {
            let mut body = String::new();
            if let Some(b64) = self.odf_payload_b64.as_ref() {
                body.push_str(&hydration_bootstrap(b64, &self.route_id, &self.nonce));
            }
            body.push_str(&self.body_extra);
            Some(body)
        } else {
            None
        };
        (head, body)
    }

    /// Inject into a complete HTML document (tests use this; the streaming variant
    /// drives lol_html per chunk).
    ///
    /// Head tags + preload/stylesheet links are appended to the existing `<head>`
    /// content (before `</head>`); the hydration bootstrap + ODF payload script are
    /// appended right before `</body>`. Documents without `<head>` get one inserted
    /// (after `<html>` when present, else at the start); documents without a `<body>`
    /// element get the payload appended at the very end of the document.
    pub fn inject_into_html(&self, html: &str) -> Result<String> {
        let (head_inject, body_inject) = self.injection_fragments();
        let html = ensure_head(html);
        let body_seen = Arc::new(AtomicBool::new(false));
        let out = lol_html::rewrite_str(&html, {
            let mut __oxide_settings = RewriteStrSettings::new();
            for __h in element_handlers(head_inject, body_inject.clone(), Arc::clone(&body_seen)) {
                __oxide_settings = __oxide_settings.append_element_content_handler(__h);
            }
            __oxide_settings
        })
        .map_err(|e| err("RND-INJECT", format!("html injection failed: {e}")))?;
        let mut out = out;
        if !body_seen.load(Ordering::Acquire)
            && let Some(b) = body_inject.as_ref() {
                out.push_str(b);
            }
        Ok(out)
    }

    /// Streaming variant: returns a transform closure over chunks.
    ///
    /// Byte-equal to [`Injector::inject_into_html`] over the same concatenated input.
    /// The `<head>` insertion (documents lacking one) is decided once the streamed
    /// prologue reveals a `<head>`/`<body>` open tag (or [`HEAD_DECISION_LIMIT`] bytes
    /// have arrived), so shells must start their head within that window.
    pub fn streaming(self) -> InjectStream {
        let (head_inject, body_inject) = self.injection_fragments();
        let body_seen = Arc::new(AtomicBool::new(false));
        let settings = {
            let mut __oxide_settings = lol_html::Settings::new();
            for __h in element_handlers(head_inject, body_inject.clone(), Arc::clone(&body_seen)) {
                __oxide_settings = __oxide_settings.append_element_content_handler(__h);
            }
            __oxide_settings
        };
        let sink: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let out = Arc::clone(&sink);
        let sink_fn: Box<dyn FnMut(&[u8])> =
            Box::new(move |c: &[u8]| out.lock().unwrap().extend_from_slice(c));
        let rewriter = HtmlRewriter::new(settings, sink_fn);
        InjectStream {
            rewriter: Some(rewriter),
            sink,
            body_seen,
            body_inject,
            head_checked: false,
            pending: Vec::new(),
        }
    }
}

/// lol_html 1.2 element handlers shared by the batch and streaming paths.
fn element_handlers(
    head_inject: String,
    body_inject: Option<String>,
    body_seen: Arc<AtomicBool>,
) -> Vec<(
    std::borrow::Cow<'static, Selector>,
    ElementContentHandlers<'static>,
)> {
    vec![
        (
            std::borrow::Cow::Owned("head".parse().unwrap()),
            ElementContentHandlers::default().element(
                move |el: &mut lol_html::html_content::Element| {
                    // `append` inserts right before `</head>`, i.e. after the existing
                    // head content (§10.2 step 4: head tags, then links).
                    el.append(&head_inject, ContentType::Html);
                    Ok(())
                },
            ),
        ),
        (
            std::borrow::Cow::Owned("body".parse().unwrap()),
            ElementContentHandlers::default().element(
                move |el: &mut lol_html::html_content::Element| {
                    body_seen.store(true, Ordering::Release);
                    if let Some(b) = body_inject.as_ref() {
                        // `append` inserts right before `</body>` (§10.2 step 4).
                        el.append(b, ContentType::Html);
                    }
                    Ok(())
                },
            ),
        ),
    ]
}

pub struct InjectStream {
    rewriter: Option<HtmlRewriter<'static, Box<dyn FnMut(&[u8])>>>,
    sink: Arc<Mutex<Vec<u8>>>,
    body_seen: Arc<AtomicBool>,
    body_inject: Option<String>,
    head_checked: bool,
    pending: Vec<u8>,
}

/// Upper bound on how long the streaming path buffers the prologue before deciding
/// whether a `<head>` insertion is needed (bytes).
const HEAD_DECISION_LIMIT: usize = 8192;

impl InjectStream {
    /// Transform one streamed chunk; returns the output bytes emitted so far.
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<u8>> {
        let rewriter = self
            .rewriter
            .as_mut()
            .ok_or_else(|| err("RND-INJECT", "stream already finished"))?;
        if !self.head_checked {
            // The `<head>` insertion (documents lacking one) is decided from the
            // prologue: as soon as a head/body open tag (or the byte limit) is seen,
            // matching the batch `ensure_head` decision for any real shell.
            self.pending.extend_from_slice(chunk);
            let hay = String::from_utf8_lossy(&self.pending).into_owned();
            if find_tag_open(&hay, "head").is_none()
                && find_tag_open(&hay, "body").is_none()
                && self.pending.len() <= HEAD_DECISION_LIMIT
            {
                return Ok(Vec::new()); // keep buffering the prologue
            }
            self.head_checked = true;
            let decided: Cow<[u8]> = match std::str::from_utf8(&self.pending) {
                Ok(s) => Cow::Owned(ensure_head(s).into_bytes()),
                Err(_) => Cow::Borrowed(&self.pending),
            };
            rewriter
                .write(&decided)
                .map_err(|e| err("RND-INJECT", format!("streaming injection failed: {e}")))?;
            self.pending.clear();
            return Ok(std::mem::take(&mut *self.sink.lock().unwrap()));
        }
        rewriter
            .write(chunk)
            .map_err(|e| err("RND-INJECT", format!("streaming injection failed: {e}")))?;
        Ok(std::mem::take(&mut *self.sink.lock().unwrap()))
    }

    /// Flush the tail and finish the rewrite.
    pub fn finish(mut self) -> Result<Vec<u8>> {
        let rewriter = self
            .rewriter
            .as_mut()
            .ok_or_else(|| err("RND-INJECT", "stream already finished"))?;
        if !self.head_checked && !self.pending.is_empty() {
            // Never decided: treat the remaining input as a whole document.
            self.head_checked = true;
            let decided: Cow<[u8]> = match std::str::from_utf8(&self.pending) {
                Ok(s) => Cow::Owned(ensure_head(s).into_bytes()),
                Err(_) => Cow::Borrowed(&self.pending),
            };
            rewriter
                .write(&decided)
                .map_err(|e| err("RND-INJECT", format!("streaming injection failed: {e}")))?;
            self.pending.clear();
        }
        let rewriter = self.rewriter.take().unwrap();
        rewriter.end().map_err(|e| {
            err(
                "RND-INJECT",
                format!("streaming injection failed on finish: {e}"),
            )
        })?;
        let mut out = std::mem::take(&mut *self.sink.lock().unwrap());
        if !self.body_seen.load(Ordering::Acquire)
            && let Some(b) = &self.body_inject {
                out.extend_from_slice(b.as_bytes());
            }
        Ok(out)
    }
}

/// Case-insensitive search for an `<tag` open (next char is whitespace, `>` or `/`),
/// so `<header>` does not count as `<head`.
fn find_tag_open(hay: &str, tag: &str) -> Option<usize> {
    let lower = hay.to_ascii_lowercase();
    let pat = format!("<{tag}");
    let mut from = 0usize;
    while let Some(i) = lower[from..].find(&pat) {
        let at = from + i;
        let ok = lower[at + pat.len()..]
            .chars()
            .next()
            .map(|c| c.is_whitespace() || c == '>' || c == '/')
            .unwrap_or(false);
        if ok {
            return Some(at);
        }
        from = at + 1;
    }
    None
}

/// Guarantee an open `<head>` tag exists before the rewrite: insert `<head></head>`
/// right after the `<html …>` open tag when present, else at the very start.
fn ensure_head(html: &str) -> String {
    if find_tag_open(html, "head").is_some() {
        return html.to_string();
    }
    if let Some(i) = find_tag_open(html, "html")
        && let Some(gt) = html[i..].find('>') {
            let at = i + gt + 1;
            return format!("{}<head></head>{}", &html[..at], &html[at..]);
        }
    format!("<head></head>{html}")
}

/// Render error mapping: if a stream errors after the shell is committed, the error
/// boundary content is streamed and the client is told to recover (§10.2 step 5).
///
/// Reuses the [`boundary_content`] swapper with an `ox-error` class on the boundary
/// markup; the diagnostic code and message are HTML-escaped, and a hostile boundary id
/// is percent-encoded so it can never break out of the attribute/JS string contexts.
pub fn stream_error_recovery(boundary_id: &str, diagnostic: &Diagnostic, nonce: &str) -> String {
    let id = sanitize_boundary_id(boundary_id);
    let code = escape_attr(&diagnostic.code);
    let msg = escape_attr(&diagnostic.message);
    let html = format!("<div class='ox-error' data-ox-error='{code}'>{code}: {msg}</div>");
    boundary_content_with_id(&id, &html, nonce)
}

pub use ppr::{Shell, assemble, shell_cache_key, split_shell};
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_attr_and_text_minimal() {
        assert_eq!(escape_attr("a&b<c>d\"e"), "a&amp;b&lt;c&gt;d&quot;e");
        assert_eq!(escape_text("a&b<c>\"d"), "a&amp;b&lt;c&gt;\"d");
    }

    #[test]
    fn escape_attr_covers_single_quote_and_script() {
        // SEC-07: all injected attributes are single-quoted; `<script>`
        // in error text must render inert.
        assert_eq!(escape_attr("a'b"), "a&#39;b");
        assert_eq!(
            escape_text("<script>alert(1)</script>"),
            "&lt;script&gt;alert(1)&lt;/script&gt;"
        );
    }

    #[test]
    fn guard_script_neutralizes_only_script_closers() {
        assert_eq!(guard_script("a</script>b"), "a<\\/script>b");
        assert_eq!(guard_script("</SCRIPT> x"), "<\\/script> x");
        assert_eq!(guard_script("</SCRIPTX ok"), "</SCRIPTX ok");
        assert_eq!(guard_script("plain"), "plain");
    }

    #[test]
    fn ensure_head_inserts_after_html_or_at_start() {
        assert_eq!(
            ensure_head("<html a=\"1\"><body>"),
            "<html a=\"1\"><head></head><body>"
        );
        assert_eq!(ensure_head("<body>"), "<head></head><body>");
        assert_eq!(ensure_head("<head></head><body>"), "<head></head><body>");
        // <header> is not <head>
        assert_eq!(
            ensure_head("<header></header>"),
            "<head></head><header></header>"
        );
    }

    #[test]
    fn find_tag_open_rejects_lookalikes() {
        assert_eq!(find_tag_open("<header><body>", "head"), None);
        assert_eq!(find_tag_open("<HEAD a=1>", "head"), Some(0));
        assert_eq!(find_tag_open("x<html>", "html"), Some(1));
    }

    #[test]
    fn sanitize_route_and_boundary_ids() {
        assert_eq!(
            sanitize_route_id("routes/blog/[slug]"),
            "routes/blog/%5Bslug%5D"
        );
        assert_eq!(sanitize_route_id("routes/ok-1/x"), "routes/ok-1/x");
        assert_eq!(sanitize_boundary_id("a'b`c"), "a%27b%60c");
        assert_eq!(sanitize_boundary_id("A-z_9"), "A-z_9");
    }
}
