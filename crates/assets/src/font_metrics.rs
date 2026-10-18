//! Font metric extraction and CSS override computation (spec §16.2, D19).
//!
//! Reads OpenType/TrueType metrics via `skrifa` (the pure-Rust fontations stack)
//! and computes the CSS `size-adjust` / `ascent-override` / `descent-override` /
//! `line-gap-override` percentages that eliminate font-swap layout shift when a
//! web font fails to load and the browser substitutes the fallback face.
//!
//! The formula matches next/font's published metric-override algorithm:
//! normalize everything to em units, then scale the target's metrics by the ratio
//! of fallback total height to target total height.

use crate::{Result, err};
use skrifa::instance::{LocationRef, Size};
use skrifa::{FontRef, MetadataProvider};

/// The metric-relevant subset of a parsed font, in font design units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FontMetric {
    pub ascent: f32,
    pub descent: f32,
    pub line_gap: f32,
    pub units_per_em: f32,
}

/// Times New Roman fallback metric — the crate's chosen `local(...)` fallback
/// face (see `DefaultFontBackend::bundle`). Values from the canonical
/// OpenType `hhea` table (`ascent=1825, descent=443, lineGap=87, upem=2048`).
pub const TIMES_NEW_ROMAN: FontMetric = FontMetric {
    ascent: 1825.0,
    descent: 443.0,
    line_gap: 87.0,
    units_per_em: 2048.0,
};

/// Read metrics from OpenType/TrueType bytes. Returns `AST-FONT` on any parse
/// failure (WOFF2-compressed input is not decompressed here; callers fall back).
pub fn read_metrics(bytes: &[u8]) -> Result<FontMetric> {
    let font =
        FontRef::new(bytes).map_err(|e| err("AST-FONT", format!("skrifa parse failed: {e}")))?;
    let m = font.metrics(Size::unscaled(), LocationRef::default());
    if m.units_per_em == 0 {
        return Err(err("AST-FONT", "font reports 0 units-per-em"));
    }
    Ok(FontMetric {
        ascent: m.ascent,
        descent: m.descent,
        line_gap: m.leading,
        units_per_em: m.units_per_em as f32,
    })
}

/// The four CSS override values, each expressed as a percentage (e.g. `106.25`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MetricOverrides {
    pub size_adjust: f32,
    pub ascent_override: f32,
    pub descent_override: f32,
    pub line_gap_override: f32,
}

/// Compute the CSS overrides that make the `fallback` face occupy exactly the same
/// vertical box as `target`. See module docs for the algorithm.
pub fn metric_overrides(target: FontMetric, fallback: FontMetric) -> MetricOverrides {
    let target_total = (target.ascent + target.descent + target.line_gap) / target.units_per_em;
    let fallback_total =
        (fallback.ascent + fallback.descent + fallback.line_gap) / fallback.units_per_em;
    let ratio = if target_total == 0.0 {
        1.0
    } else {
        fallback_total / target_total
    };
    MetricOverrides {
        size_adjust: ratio * 100.0,
        ascent_override: (target.ascent / target.units_per_em) * ratio * 100.0,
        descent_override: (target.descent / target.units_per_em) * ratio * 100.0,
        line_gap_override: (target.line_gap / target.units_per_em) * ratio * 100.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A font with the same metric proportions as the fallback yields 100% everywhere.
    #[test]
    fn matching_metrics_yield_no_overrides() {
        let o = metric_overrides(TIMES_NEW_ROMAN, TIMES_NEW_ROMAN);
        assert!((o.size_adjust - 100.0).abs() < 0.01);
        assert!((o.ascent_override - (1825.0 / 2048.0 * 100.0)).abs() < 0.01);
        assert!((o.descent_override - (443.0 / 2048.0 * 100.0)).abs() < 0.01);
        assert!((o.line_gap_override - (87.0 / 2048.0 * 100.0)).abs() < 0.01);
    }

    /// A target font that is half as tall overall gets size-adjust ≈ 200%.
    #[test]
    fn half_height_target_doubles_size_adjust() {
        let tiny = FontMetric {
            ascent: 912.5,
            descent: 221.5,
            line_gap: 43.5,
            units_per_em: 2048.0,
        };
        let o = metric_overrides(tiny, TIMES_NEW_ROMAN);
        assert!(
            (o.size_adjust - 200.0).abs() < 0.05,
            "got {}",
            o.size_adjust
        );
    }

    /// The three individual overrides sum to size-adjust * (total_height/upem).
    #[test]
    fn overrides_are_consistent_with_size_adjust() {
        let target = FontMetric {
            ascent: 2000.0,
            descent: 500.0,
            line_gap: 0.0,
            units_per_em: 1000.0,
        };
        let o = metric_overrides(target, TIMES_NEW_ROMAN);
        let total = (o.ascent_override + o.descent_override + o.line_gap_override) / 100.0;
        let expected = o.size_adjust / 100.0 * ((2000.0 + 500.0 + 0.0) / 1000.0);
        assert!((total - expected).abs() < 0.01, "{total} vs {expected}");
    }
}
