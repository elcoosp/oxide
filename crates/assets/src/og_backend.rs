//! OG image rasterization (spec §16.3, D19).
//!
//! `SvgOgBackend` accepts a neutral OG element tree as JSON — a compact, absolute-
//! positioned layout spec for OG cards — serializes it to SVG, then rasterizes
//! through `usvg` + `resvg` (sRGB output via `tiny-skia`). This is the real
//! renderer the spec calls for; no headless browser is involved.
//!
//! # Element tree shape
//!
//! ```json
//! {
//!   "background": "#0a0a0a",
//!   "children": [
//!     { "type": "rect", "x": 60, "y": 80, "width": 1080, "height": 4, "fill": "#ffffff" },
//!     { "type": "text", "x": 60, "y": 180, "text": "Hello, world",
//!       "font_size": 96, "font_family": "Inter", "font_weight": 700, "fill": "#ffffff" },
//!     { "type": "image", "x": 900, "y": 60, "width": 240, "height": 240,
//!       "href": "data:image/png;base64,..." }
//!   ]
//! }
//! ```
//!
//! Unknown element types are skipped silently (the pattern is additive). Colors
//! are CSS color strings; SVG's own parser resolves named colors, `#rgb`, `#rrggbb`,
//! `rgb(...)`, and `rgba(...)`.

use crate::{err, OgBackend, Result};

/// Real resvg-backed OG image renderer (D19).
pub struct SvgOgBackend;

impl OgBackend for SvgOgBackend {
    fn render(&self, tree: &serde_json::Value, width: u32, height: u32) -> Result<Vec<u8>> {
        let svg = build_svg(tree, width, height)?;

        let opt = usvg::Options::default();
        let usvg_tree = usvg::Tree::from_str(&svg, &opt)
            .map_err(|e| err("AST-OG", format!("usvg parse failed: {e}")))?;

        let mut pixmap = tiny_skia::Pixmap::new(width, height)
            .ok_or_else(|| err("AST-OG", format!("cannot allocate {width}x{height} pixmap")))?;
        let size = usvg_tree.size();
        // Scale the SVG's own coordinate space to fill the requested pixmap.
        let sx = width as f32 / size.width().max(1.0);
        let sy = height as f32 / size.height().max(1.0);
        let transform = tiny_skia::Transform::from_scale(sx, sy);
        resvg::render(&usvg_tree, transform, &mut pixmap.as_mut());

        pixmap
            .encode_png()
            .map_err(|e| err("AST-OG", format!("png encode failed: {e}")))
    }
}

/// Build an SVG document from the OG element tree.
fn build_svg(tree: &serde_json::Value, width: u32, height: u32) -> Result<String> {
    let bg = tree
        .get("background")
        .and_then(|v| v.as_str())
        .unwrap_or("#0a0a0a");

    let mut svg = String::with_capacity(1024);
    svg.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" \
         xmlns:xlink=\"http://www.w3.org/1999/xlink\" \
         width=\"{width}\" height=\"{height}\" \
         viewBox=\"0 0 {width} {height}\">"
    ));
    svg.push_str(&format!(
        "<rect x=\"0\" y=\"0\" width=\"{width}\" height=\"{height}\" fill=\"{}\"/>",
        escape_attr(bg)
    ));

    if let Some(children) = tree.get("children").and_then(|v| v.as_array()) {
        for child in children {
            append_element(&mut svg, child);
        }
    }

    svg.push_str("</svg>");
    Ok(svg)
}

fn append_element(svg: &mut String, el: &serde_json::Value) {
    let kind = el.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let x = num(el, "x", 0.0);
    let y = num(el, "y", 0.0);
    match kind {
        "rect" => {
            let w = num(el, "width", 0.0);
            let h = num(el, "height", 0.0);
            let fill = el.get("fill").and_then(|v| v.as_str()).unwrap_or("#ffffff");
            let radius = num(el, "radius", 0.0);
            if radius > 0.0 {
                svg.push_str(&format!(
                    "<rect x=\"{x}\" y=\"{y}\" width=\"{w}\" height=\"{h}\" \
                     rx=\"{radius}\" ry=\"{radius}\" fill=\"{}\"/>",
                    escape_attr(fill)
                ));
            } else {
                svg.push_str(&format!(
                    "<rect x=\"{x}\" y=\"{y}\" width=\"{w}\" height=\"{h}\" fill=\"{}\"/>",
                    escape_attr(fill)
                ));
            }
        }
        "text" => {
            let text = el.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let size = num(el, "font_size", 24.0);
            let family = el
                .get("font_family")
                .and_then(|v| v.as_str())
                .unwrap_or("sans-serif");
            let weight = el
                .get("font_weight")
                .and_then(|v| v.as_u64())
                .unwrap_or(400);
            let fill = el.get("fill").and_then(|v| v.as_str()).unwrap_or("#ffffff");
            let anchor = el
                .get("text_anchor")
                .and_then(|v| v.as_str())
                .unwrap_or("start");
            svg.push_str(&format!(
                "<text x=\"{x}\" y=\"{y}\" font-size=\"{size}\" \
                 font-family=\"{}\" font-weight=\"{weight}\" fill=\"{}\" \
                 text-anchor=\"{anchor}\">{}</text>",
                escape_attr(family),
                escape_attr(fill),
                escape_text(text)
            ));
        }
        "image" => {
            let w = num(el, "width", 0.0);
            let h = num(el, "height", 0.0);
            let href = el.get("href").and_then(|v| v.as_str()).unwrap_or("");
            if !href.is_empty() {
                svg.push_str(&format!(
                    "<image x=\"{x}\" y=\"{y}\" width=\"{w}\" height=\"{h}\" \
                     xlink:href=\"{}\" preserveAspectRatio=\"xMidYMid slice\"/>",
                    escape_attr(href)
                ));
            }
        }
        _ => {} // additive: unknown types are ignored
    }
}

fn num(el: &serde_json::Value, key: &str, default: f64) -> f64 {
    el.get(key).and_then(|v| v.as_f64()).unwrap_or(default)
}

fn escape_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn escape_text(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn builds_svg_with_background_rect_and_text() {
        let tree = json!({
            "background": "#101010",
            "children": [
                {"type": "rect", "x": 40, "y": 60, "width": 800, "height": 4, "fill": "#ff0000"},
                {"type": "text", "x": 40, "y": 200, "text": "Hello & <world>",
                 "font_size": 72, "fill": "#ffffff"}
            ]
        });
        let svg = build_svg(&tree, 1200, 630).unwrap();
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("width=\"1200\""));
        assert!(svg.contains("height=\"630\""));
        assert!(svg.contains("fill=\"#101010\""));
        assert!(svg.contains("<rect x=\"40\" y=\"60\" width=\"800\" height=\"4\" fill=\"#ff0000\"/>"));
        assert!(svg.contains("font-size=\"72\""));
        // XML escaping in text
        assert!(svg.contains("Hello &amp; &lt;world&gt;"));
        assert!(svg.ends_with("</svg>"));
    }

    #[test]
    fn unknown_element_types_are_silently_skipped() {
        let tree = json!({
            "children": [
                {"type": "unknown-widget"},
                {"type": "rect", "width": 10, "height": 10}
            ]
        });
        let svg = build_svg(&tree, 100, 100).unwrap();
        assert!(svg.contains("<rect x=\"0\" y=\"0\" width=\"10\" height=\"10\""));
    }

    #[test]
    fn image_element_with_href_produces_image_tag() {
        let tree = json!({
            "children": [
                {"type": "image", "x": 10, "y": 20, "width": 30, "height": 40,
                 "href": "data:image/png;base64,AAAA"}
            ]
        });
        let svg = build_svg(&tree, 100, 100).unwrap();
        assert!(svg.contains("<image"));
        assert!(svg.contains("xlink:href=\"data:image/png;base64,AAAA\""));
    }
}
