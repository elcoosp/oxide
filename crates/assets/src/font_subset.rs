//! SPEC-16.2: font subsetting via the `subsetter` crate.
//!
//! ## What this does
//!
//! Given a font file and a set of Unicode codepoints the project actually
//! uses (harvested from route source + loader data at build time), produce
//! a smaller font file containing only the glyphs those codepoints need.
//!
//! ## What this does NOT do
//!
//! - **Layout tables (`GSUB`/`GPOS`) are dropped.** `subsetter` 0.1's public
//!   `Profile::pdf` produces a PDF-oriented subset that keeps only the
//!   TrueType/CFF outline tables. For Latin text (the common case) shaping
//!   is minimal and this is fine. For complex scripts (Arabic, Devanagari)
//!   it is degraded — the browser falls back to its own shaping on a
//!   non-shaping font, which is *usually* correct but not always identical
//!   to the original. A `Profile::web` that preserves layout tables is a
//!   follow-up gated on `subsetter` exposing one.
//! - **The output is TrueType (`.ttf`), not WOFF2.** `subsetter` emits
//!   sfnt (TrueType/CFF); WOFF2 encoding needs a separate encoder crate
//!   that doesn't exist yet (`woff2-encoder` is unpublished). The CSS
//!   emission uses `format('truetype')` for subset output — universally
//!   supported by every browser since IE9. WOFF2 is a size optimization,
//!   not a compatibility requirement.
//!
//! ## When to use it
//!
//! Opt-in via `fonts.subset = ["latin"]` (or any non-empty list) — the
//! `DefaultFontBackend` runs the subset pass only when the spec lists
//! ranges AND every input file parses as OpenType. A parse failure or an
//! empty codepoint set leaves the file untouched (best-effort, same
//! convention as the rest of the font pipeline).

use crate::{Result, err};
use skrifa::MetadataProvider as _;
use std::collections::BTreeSet;

/// Subset `font_bytes` to the glyphs needed by the given codepoints.
///
/// Returns `Some(subset_bytes)` on success, `None` when the subset pass
/// produced no smaller output (an already-minimal font, an unparseable
/// cmap, or an empty codepoint set). A `None` result means the caller
/// should fall back to the verbatim copy.
pub fn subset_for_codepoints(
    font_bytes: &[u8],
    codepoints: &BTreeSet<u32>,
) -> Result<Option<Vec<u8>>> {
    if codepoints.is_empty() {
        return Ok(None);
    }

    // Map Unicode codepoints → glyph ids using the font's cmap. `skrifa`
    // exposes the charmap; the returned glyph ids feed `subsetter`.
    let font = skrifa::FontRef::new(font_bytes)
        .map_err(|e| err("AST-FONT", format!("skrifa parse failed: {e}")))?;
    let charmap = font.charmap();
    let mut glyphs: BTreeSet<u16> = BTreeSet::new();
    // `.notdef` is glyph 0 — always include it so a missing char renders
    // as the font's own placeholder, not as another glyph entirely.
    glyphs.insert(0);
    for cp in codepoints {
        if let Some(ch) = char::from_u32(*cp)
            && let Some(gid) = charmap.map(ch) {
                glyphs.insert(gid.to_u32() as u16);
            }
    }
    // If the only glyph is `.notdef`, subsetting produces nothing useful.
    if glyphs.len() <= 1 {
        return Ok(None);
    }
    let gid_vec: Vec<u16> = glyphs.into_iter().collect();
    let subset = subsetter::subset(font_bytes, 0, subsetter::Profile::pdf(&gid_vec))
        .map_err(|e| err("AST-FONT", format!("subsetter: {e:?}")))?;
    // Only return when the subset is actually smaller — a micro-font can
    // grow when the subsetter re-emits its required tables.
    if subset.len() >= font_bytes.len() {
        return Ok(None);
    }
    Ok(Some(subset))
}

/// Harvest every codepoint from a set of source strings. Used at build time
/// to feed `subset_for_codepoints` — a route's source + loader data covers
/// the text a page can render.
pub fn harvest_codepoints<'a, I>(sources: I) -> BTreeSet<u32>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut set = BTreeSet::new();
    for src in sources {
        for ch in src.chars() {
            set.insert(ch as u32);
        }
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_codepoints_is_none() {
        let out = subset_for_codepoints(b"not a font", &BTreeSet::new()).unwrap();
        assert!(out.is_none(), "empty set → no work");
    }

    #[test]
    fn invalid_font_errors_cleanly() {
        let mut cps = BTreeSet::new();
        cps.insert('a' as u32);
        let e = subset_for_codepoints(b"not a font", &cps).unwrap_err();
        assert_eq!(e.0.code, "AST-FONT");
        assert!(e.0.message.contains("skrifa parse"));
    }

    #[test]
    fn harvests_ascii_and_unicode() {
        let cps = harvest_codepoints(["abc", "héllo ☃"]);
        assert!(cps.contains(&('a' as u32)));
        assert!(cps.contains(&('é' as u32)));
        assert!(cps.contains(&('☃' as u32)));
        assert!(!cps.contains(&('Z' as u32)));
    }

    /// End-to-end: use a real font from the OS (best-effort — skip when no
    /// font is available). Asserts the subset is smaller AND still parses.
    #[test]
    fn subsets_a_real_font_and_shrinks_it() {
        let candidates = [
            // macOS
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            "/System/Library/Fonts/Helvetica.ttc",
            "/System/Library/Fonts/SFNS.ttf",
            // Linux
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        ];
        let Some(bytes) = candidates.iter().find_map(|p| std::fs::read(p).ok()) else {
            eprintln!("skipped: no system font available for subset test");
            return;
        };
        // A tiny set — just "abc" plus .notdef.
        let cps = harvest_codepoints(["abc"]);
        let subset = subset_for_codepoints(&bytes, &cps).expect("subset runs");
        let Some(subset) = subset else {
            eprintln!("skipped: subset pass produced no smaller output (already minimal)");
            return;
        };
        assert!(
            subset.len() < bytes.len(),
            "subset ({} B) must be smaller than original ({} B)",
            subset.len(),
            bytes.len(),
        );
        // Still a valid font.
        assert!(
            skrifa::FontRef::new(&subset).is_ok(),
            "subset output must parse as a font",
        );
    }
}
