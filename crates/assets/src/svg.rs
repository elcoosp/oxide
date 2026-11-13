//! The SVG optimizer seam (spec v0.2, §V2-C07): SVG documents that enter
//! the build stop being "shipped as authored" — they are optimized by a
//! swappable backend, with the rules that keep an optimization honest
//! enforced on EVERY backend's output, not just the bundled one.
//!
//! # The pieces the spec names
//!
//! - **The seam** ([`SvgOptimizer`]): string in, string out — no DOM type
//!   crosses the boundary (the C01 `NativeCompiler` shape). Each backend
//!   pins its own parser; the wrapper only sees documents. `id()` and
//!   `version()` are the D40 key dimensions: switching optimizers never
//!   reuses a stale artifact.
//! - **`IdentitySvg`** (the default): the byte-identical passthrough, so
//!   a build with `svg.optimize: false` (and every feature-off build)
//!   never mutates a document.
//! - **`OxvgSvg`** (behind the crate's `svg-oxvg` feature): the OXVG
//!   optimiser (`oxvg_optimiser`, its `Jobs::safe()` correctness preset —
//!   the preset that is *less likely to visually change* a document,
//!   which is the only honest default for a build pipeline).
//! - **The rules** ([`assert_preserved`]): after ANY optimizer runs, the
//!   output (a) keeps the source's `viewBox` value verbatim, (b) keeps
//!   every id the source referenced through `<use>`/`xlink:href`,
//!   `url(#…)` and `aria-labelledby`/`aria-describedby`, and (c) parses
//!   back as well-formed XML. A violation is refused with `AST-SVG-001`
//!   and the caller falls back to the source — the rules are the seam's
//!   contract, testable against any [`SvgOptimizer`] impl (see the pins
//!   in `tests/svg_optimizer_tdd.rs`, which drive fakes on purpose).
//! - **The CAS key** ([`svg_cas_key`]): the content hash plus the
//!   optimizer id and version (`svg:{id}@{version}:{blake3-16hex}`), so
//!   a stored result is only ever reused for the exact bytes and backend
//!   that produced it.
//! - **The selection** ([`select_optimizer`]): the config knob
//!   (`svg.optimize`) maps to the best compiled-in backend; `true`
//!   without the `svg-oxvg` feature refuses with `AST-SVG-002` (the
//!   C01 `CMP-NAT-001` shape — name the missing flag and the fallback).
//! - **The build hook** ([`optimize_for_build`]): the one definition site
//!   the import pipeline calls — knob off passes through, every refusal
//!   logs a `tracing` event with the diagnostic code and keeps the source,
//!   so an optimizer that misbehaves degrades the build to
//!   "unoptimized" instead of breaking it.
//!
//! The `?url`/`?raw`/`?inline`/SVG-as-component import hooks the WP lists
//! ride the CLI import pipeline (the `.svg?react` site consumes
//! [`optimize_for_build`] today; the remaining import variants are the
//! recorded §6.7 tail — see `docs/deviations.md`).

use std::collections::BTreeSet;

use blake3::Hasher;

use crate::{OxideError, err};
// The feature-off arm constructs the refusal directly (it needs
// `with_fix`, which the `err` helper does not carry).
#[cfg(not(feature = "svg-oxvg"))]
use crate::Diagnostic;

/// The SVG optimization seam (§V2-C07): a document in, an optimized
/// document out. Object-safe so boxed or concrete backends drop in.
///
/// Implementations must be pure with respect to their input and must not
/// panic on malformed input — report [`Result::Err`] instead (the
/// build-side hook treats a panic as a bug in the backend, not a
/// user-document problem).
pub trait SvgOptimizer: Send + Sync {
    /// Optimize one SVG document.
    ///
    /// # Errors
    ///
    /// A backend that cannot produce a rule-clean output returns `Err`
    /// (`AST-SVG-003` for its own failures; `AST-SVG-001` when the
    /// wrapper's preservation rules refuse the output).
    fn optimize(&self, svg: &str) -> Result<String, OxideError>;

    /// The D40 key dimension: the backend's stable identifier
    /// (`identity`, `oxvg`, …). Never changes meaning across releases.
    fn id(&self) -> &'static str;

    /// The D40 key dimension: the backend's implementation version.
    /// Switching `svg.optimize` never reuses a stale CAS entry because
    /// this string is part of [`svg_cas_key`].
    fn version(&self) -> &'static str;
}

/// The default backend: the byte-identical passthrough. `svg.optimize:
/// false` (and every build without the `svg-oxvg` feature) ships the
/// document exactly as authored.
#[derive(Debug, Default, Clone, Copy)]
pub struct IdentitySvg;

impl SvgOptimizer for IdentitySvg {
    fn optimize(&self, svg: &str) -> Result<String, OxideError> {
        Ok(svg.to_string())
    }
    fn id(&self) -> &'static str {
        "identity"
    }
    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }
}

/// The OXVG backend (`oxvg_optimiser` 0.0.9), behind the crate's
/// `svg-oxvg` feature. Runs the `Jobs::safe()` correctness preset — the
/// preset documented as "less likely to visually change the document" —
/// and then enforces the preservation rules on its own output
/// ([`assert_preserved`]), so the bundled optimizer is held to the same
/// contract a third-party [`SvgOptimizer`] would be.
#[cfg(feature = "svg-oxvg")]
#[derive(Debug, Default, Clone, Copy)]
pub struct OxvgSvg;

#[cfg(feature = "svg-oxvg")]
impl SvgOptimizer for OxvgSvg {
    fn optimize(&self, svg: &str) -> Result<String, OxideError> {
        use oxvg_ast::parse::roxmltree::parse;
        use oxvg_ast::serialize::Node as _;
        use oxvg_ast::visitor::Info;
        use oxvg_optimiser::Jobs;

        // The parse and the jobs run share the document's arena, so every
        // failure inside the closure is stringified there (the job error
        // borrows the input) and mapped to the AST-SVG-003 shape outside.
        let out = parse(
            svg,
            |dom, allocator| -> std::result::Result<String, String> {
                let jobs = Jobs::safe();
                jobs.run(dom, &Info::new(allocator))
                    .map_err(|e| format!("the optimiser failed: {e}"))?;
                dom.serialize()
                    .map_err(|e| format!("the serialiser failed: {e}"))
            },
        )
        .map_err(|e| {
            err(
                "AST-SVG-003",
                format!("the SVG optimizer could not parse the document: {e}"),
            )
        })?
        .map_err(|e| err("AST-SVG-003", format!("the SVG optimizer failed: {e}")))?;

        // The bundled optimizer is subject to the same rules as any
        // injected backend — a violation is refused, never shipped.
        assert_preserved(svg, &out)?;
        Ok(out)
    }

    fn id(&self) -> &'static str {
        "oxvg"
    }

    fn version(&self) -> &'static str {
        "0.0.9"
    }
}

/// Extract the ids the document references — `<use href="#shape">`,
/// `<use xlink:href="#shape">`, the paint-server spelling
/// `fill="url(#grad)"`, and the accessibility spellings
/// `aria-labelledby="a b"` / `aria-describedby`. These are exactly the
/// ids the preservation rule refuses to lose.
///
/// The scan is attribute-focused on purpose: `href`/`aria-*` are matched
/// as attribute names (whitespace tolerated around `=`), and `url(#…)`
/// is matched inside attribute values, so prose in a `<desc>` that
/// happens to contain `#word` is never mistaken for a reference.
#[must_use]
pub fn referenced_svg_ids(svg: &str) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();

    // `href="#id"` (which also matches `xlink:href="#id"`) and the
    // accessibility spellings, matched case-insensitively as attribute
    // names; the values are sliced from the ORIGINAL string so idents
    // keep their case.
    for name in ["href", "aria-labelledby", "aria-describedby"] {
        for value in attr_values(svg, name, true) {
            if name == "href" {
                if let Some(id) = value.strip_prefix('#') {
                    ids.insert(id.to_string());
                }
            } else {
                ids.extend(value.split_whitespace().map(str::to_string));
            }
        }
    }

    // `url(#id)` — the paint-server spelling (`fill`, `clip-path`,
    // `filter`, `mask`, …), matched case-insensitively.
    let lower = svg.to_ascii_lowercase();
    let mut from = 0;
    while let Some(found) = lower[from..].find("url(#") {
        let start = from + found + "url(#".len();
        let end = lower[start..]
            .find(')')
            .map(|i| start + i)
            .unwrap_or(lower.len());
        let id = svg[start..end].trim();
        if !id.is_empty() {
            ids.insert(id.to_string());
        }
        from = start;
    }

    ids
}

/// Iterate the values of every `name="…"` attribute in the document.
/// `ci` selects case-insensitive name matching (`href`/`aria-*` against
/// authored spellings); the value is always sliced from the original
/// string so idents keep their case. A name match is boundary-checked so
/// `id=` never matches `aria-describedby=` and `viewBox=` never matches
/// `data-viewBox=`.
fn attr_values<'a>(svg: &'a str, name: &str, ci: bool) -> Vec<&'a str> {
    let haystack: std::borrow::Cow<'_, str> = if ci {
        std::borrow::Cow::Owned(svg.to_ascii_lowercase())
    } else {
        std::borrow::Cow::Borrowed(svg)
    };
    let needle = format!("{name}=");
    let bytes = haystack.as_bytes();
    let mut values = Vec::new();
    let mut from = 0;
    while let Some(found) = haystack[from..].find(&needle) {
        let at = from + found;
        let boundary_ok =
            at == 0 || !(bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'-');
        if boundary_ok && let Some(value) = quoted_at(svg, at + needle.len()) {
            values.push(value);
        }
        from = at + needle.len();
    }
    values
}

/// The quoted attribute value at `start` (just past the `=`), single- or
/// double-quoted. `svg` is the original document (byte offsets from a
/// lowercase copy are identical — `to_ascii_lowercase` is 1:1 on bytes).
fn quoted_at(svg: &str, start: usize) -> Option<&str> {
    let rest = svg[start..].trim_start();
    let lead = start + (svg[start..].len() - rest.len());
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let body = lead + 1;
    let end = svg[body..].find(quote).map(|i| body + i)?;
    Some(&svg[body..end])
}

/// The preservation rules the wrapper enforces on EVERY optimizer output
/// (§V2-C07): the source's `viewBox` value survives verbatim, every
/// referenced id ([`referenced_svg_ids`]) survives, and the output parses
/// back as well-formed XML.
///
/// # Errors
///
/// A violation is `AST-SVG-001` with the rule named in the message — the
/// caller keeps the source and logs the refusal.
pub fn assert_preserved(source: &str, optimized: &str) -> Result<(), OxideError> {
    // (c) well-formedness: the output must parse back as XML.
    if let Err(e) = roxmltree::Document::parse(optimized) {
        return Err(err(
            "AST-SVG-001",
            format!("the optimized output is not well-formed XML (rule: well-formed): {e}"),
        ));
    }

    // (a) the viewBox value survives verbatim.
    if let Some(want) = attribute_value(source, "viewBox") {
        let got = attribute_value(optimized, "viewBox").map(str::trim);
        if got != Some(want.trim()) {
            return Err(err(
                "AST-SVG-001",
                format!(
                    "the optimized output lost the source's viewBox (rule: viewBox): want {:?}, got {got:?}",
                    want.trim()
                ),
            ));
        }
    }

    // (b) every referenced id survives.
    let kept = id_set(optimized);
    for id in referenced_svg_ids(source) {
        if !kept.contains(&id) {
            return Err(err(
                "AST-SVG-001",
                format!(
                    "the optimized output dropped the referenced id `{id}` (rule: referenced id)"
                ),
            ));
        }
    }

    Ok(())
}

/// The `id="…"` values the document declares, for the survival check.
fn id_set(svg: &str) -> BTreeSet<String> {
    attr_values(svg, "id", false)
        .into_iter()
        .map(|v| v.trim().to_string())
        .collect()
}

/// The value of the first `name="…"` attribute in the document, matched
/// case-sensitively (XML attribute names are case-sensitive; `viewBox`
/// is spelled exactly).
fn attribute_value<'a>(svg: &'a str, name: &str) -> Option<&'a str> {
    attr_values(svg, name, false).into_iter().next()
}

/// The CAS key for one optimized artifact (D40): the document's blake3
/// digest truncated to 16 hex, prefixed with the optimizer id and
/// version. Two optimizers never collide on the same bytes; a backend
/// upgrade never reuses a stale artifact.
#[must_use]
pub fn svg_cas_key(optimizer: &dyn SvgOptimizer, svg: &str) -> String {
    let hash = Hasher::new().update(svg.as_bytes()).finalize();
    let hex = hash.to_hex();
    format!(
        "svg:{}@{}:{}",
        optimizer.id(),
        optimizer.version(),
        &hex[..16]
    )
}

/// Map the `svg.optimize` config knob to the best compiled-in backend
/// (§V2-C07). `false` (the default) always selects [`IdentitySvg`];
/// `true` selects [`OxvgSvg`] when the `svg-oxvg` feature is compiled in
/// and refuses with `AST-SVG-002` otherwise — naming the feature flag and
/// the fallback, the C01 `CMP-NAT-001` shape.
///
/// # Errors
///
/// `AST-SVG-002` when `enabled` is `true` but the `svg-oxvg` feature is
/// not compiled into this build.
pub fn select_optimizer(enabled: bool) -> Result<Box<dyn SvgOptimizer>, OxideError> {
    if !enabled {
        return Ok(Box::new(IdentitySvg));
    }
    #[cfg(feature = "svg-oxvg")]
    {
        Ok(Box::new(OxvgSvg))
    }
    #[cfg(not(feature = "svg-oxvg"))]
    {
        Err(OxideError(Box::new(
            Diagnostic::error(
                "AST-SVG-002",
                "`svg.optimize: true` requires the `svg-oxvg` feature, which is not \
                 compiled into this build of oxide-fw-assets",
            )
            .with_fix("rebuild with `--features svg-oxvg`, or set `svg.optimize: false`"),
        )))
    }
}

/// The one definition site the import pipeline calls (§V2-C07 hook):
/// optimize one SVG document for the build when the knob is on, and
/// degrade to the source on ANY refusal — the feature missing
/// (`AST-SVG-002`), a rule violation (`AST-SVG-001`) or a backend failure
/// (`AST-SVG-003`) all log a `tracing` event with the diagnostic code and
/// return the authored bytes, so a misbehaving optimizer never breaks a
/// build, it only un-optimizes it.
///
/// The knob-off path is byte-identical passthrough: a build that did not
/// opt in never mutates a document.
#[must_use]
pub fn optimize_for_build(svg: &str, enabled: bool) -> String {
    if !enabled {
        return svg.to_string();
    }
    let optimizer = match select_optimizer(true) {
        Ok(o) => o,
        Err(e) => {
            tracing::warn!(code = %e.0.code, msg = %e.0.message, "svg: optimize requested but unavailable; keeping the source");
            return svg.to_string();
        }
    };
    match optimizer.optimize(svg) {
        Ok(out) => out,
        Err(e) => {
            tracing::warn!(code = %e.0.code, msg = %e.0.message, "svg: the optimizer output was refused; keeping the source");
            svg.to_string()
        }
    }
}
