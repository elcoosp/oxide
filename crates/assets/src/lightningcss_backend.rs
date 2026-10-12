//! Real Lightning CSS backend (spec D5: "Lightning CSS; Tailwind v4 through its
//! Rust engine").
//!
//! `LightningCssBackend` implements the same `CssBackend` trait as the in-house
//! [`crate::DefaultCss`] engine. It drives lightningcss' `StyleSheet` parser and
//! `to_css` printer: parse with optional CSS-modules config, serialize with browser
//! targets and minify, and — for CSS modules — surface the local→scoped name map
//! straight from lightningcss' `ToCssResult.exports`.
//!
//! Both backends stay available: `DefaultCss` is deterministic, has zero external
//! deps, and produces the `ox_<hash6>_<local>` scoped-name scheme the crate's own
//! tests target; `LightningCssBackend` uses the engine the spec names (§6.6, D5),
//! with its own hash-driven scoped-name scheme.

use crate::{err, CssBackend, CssInput, CssKind, CssOutput, Result};
use lightningcss::css_modules::CssModuleExport;
use lightningcss::stylesheet::{ParserOptions, PrinterOptions, StyleSheet};
use lightningcss::targets::{Browsers, Targets};
use std::collections::BTreeMap;

/// Real Lightning CSS-backed implementation of the `CssBackend` trait (D5).
pub struct LightningCssBackend;

impl CssBackend for LightningCssBackend {
    fn transform(&self, input: &CssInput) -> Result<CssOutput> {
        // Module mode: enable lightningcss' CSS-modules config so class names are
        // scoped on parse and reflected in `ToCssResult.exports`.
        let mut parser = ParserOptions::default();
        if input.kind == CssKind::Module {
            parser.css_modules = Some(lightningcss::css_modules::Config {
                pattern: lightningcss::css_modules::Pattern::default(),
                ..Default::default()
            });
        }

        let sheet = StyleSheet::parse(&input.code, parser)
            .map_err(|e| err("AST-CSS", format!("lightningcss parse: {e}")))?;

        let targets = browser_targets(&input.targets);
        let opts = PrinterOptions {
            minify: input.minify,
            targets,
            ..Default::default()
        };

        let res = sheet
            .to_css(opts)
            .map_err(|e| err("AST-CSS", format!("lightningcss serialize: {e}")))?;

        // CSS-module exports come straight from the printer result. The local name
        // is the map key; `CssModuleExport::name` is the compiled (scoped) name.
        let exports: BTreeMap<String, String> = res
            .exports
            .unwrap_or_default()
            .into_iter()
            .map(|(local, CssModuleExport { name, .. })| (local, name))
            .collect();

        Ok(CssOutput {
            code: res.code,
            exports,
            diagnostics: Vec::new(),
        })
    }
}

/// Browserslist-style strings (`"chrome 100"`) → `Targets`. Unknown entries are
/// ignored; empty list → `Targets::default()` (broad prefixing).
fn browser_targets(targets: &[String]) -> Targets {
    if targets.is_empty() {
        return Targets::default();
    }
    let mut b = Browsers::default();
    for t in targets {
        let mut parts = t.split_whitespace();
        let Some(name) = parts.next() else { continue };
        let Some(ver) = parts.next().and_then(|v| v.parse::<u32>().ok()) else {
            continue;
        };
        match name {
            "chrome" => b.chrome = Some(ver),
            "firefox" => b.firefox = Some(ver),
            "safari" => b.safari = Some(ver),
            "edge" => b.edge = Some(ver),
            "opera" => b.opera = Some(ver),
            "ie" => b.ie = Some(ver),
            "ios_saf" | "ios" => b.ios_saf = Some(ver),
            "android" => b.android = Some(ver),
            _ => {}
        }
    }
    Targets::from(b)
}
