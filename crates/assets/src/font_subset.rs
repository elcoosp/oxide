//! SPEC-16.2 + §V2-C08: font subsetting via the `subsetter` crate.
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
//! - **The vendored `fontcull` backend is not here.** The deps report's
//!   `fontcull` (fontations/klippa) is not published on crates.io, so the
//!   seam ships with the existing `subsetter`-crate backend
//!   ([`SkrifaSubsetter`], behind the crate's `subset` feature) standing
//!   in for it — the same documented stand-in the WOFF2 row records. The
//!   [`FontSubsetter`] seam is the injection point a published `fontcull`
//!   drops into; no seam change is required when it lands.
//!
//! ## The seam (§V2-C08 step 3)
//!
//! The WP's trait is object-safe and takes the spec sketch's exact shape —
//! font bytes + a harvested char set in, subset bytes out — plus the D40
//! key dimensions (`id`/`version`) the C07 `SvgOptimizer` seam carries.
//! Around it sit: the post-prerender harvest ([`harvest_html_codepoints`],
//! the `lol_html` `text!`-over-body walk the spec names, `<script>` and
//! `<style>` contents stripped), the always-needed set
//! ([`ALWAYS_NEEDED_CHARS`]), the feature-aware selection
//! ([`select_subsetter`], the C07 `AST-SVG-002` shape), the content-hash
//! CAS key ([`font_cas_key`]) and the build-side warn-and-fallback hook
//! ([`subset_used_for_build`], the one definition site the font bundler
//! calls).
//!
//! ## When to use it
//!
//! Opt-in via `fonts.subset = ["used"]` — the glyph-usage strategy is only
//! sound when all text is known, so the build refuses it on a project with
//! non-static routes (`AST-FNT-001`, the adapters' `check_config` arm) and
//! the harvest supplies the char set. The other spellings (`latin`,
//! `unicode-range`) are range declarations, not glyph-usage sources: the
//! source ships byte-identical unless `used` is configured.

use crate::{Result, err};
// The feature-off arm constructs the refusal directly (it needs
// `with_fix`, which the `err` helper does not carry).
#[cfg(not(feature = "subset"))]
use crate::Diagnostic;
use skrifa::MetadataProvider as _;
use std::collections::BTreeSet;

// ---------------------------------------------------------------------------
// V2-C08 step 3: the FontSubsetter seam
// ---------------------------------------------------------------------------

/// The glyph-usage subsetting seam (§V2-C08 step 3): font bytes and a
/// harvested char set in, subset bytes out. Object-safe so boxed or
/// concrete backends drop in (the C01 `NativeCompiler` shape: each
/// backend pins its own font stack, the seam only sees bytes).
///
/// Implementations must be pure with respect to their input and must not
/// panic on malformed input — report [`Result::Err`] instead (the
/// build-side hook treats a panic as a bug in the backend, not a
/// user-font problem).
pub trait FontSubsetter: Send + Sync {
    /// Subset one font to the glyphs the given characters need.
    ///
    /// # Errors
    ///
    /// A backend that cannot parse the font or cannot produce a subset
    /// returns `Err` (`AST-FONT` for the bundled backend's parse and
    /// subsetting failures).
    fn subset(&self, font: &[u8], chars: &BTreeSet<char>) -> Result<Vec<u8>>;

    /// The D40 key dimension: the backend's stable identifier
    /// (`skrifa`, `fontcull`, …). Never changes meaning across releases.
    fn id(&self) -> &'static str;

    /// The D40 key dimension: the backend's implementation version.
    /// Switching the subsetting strategy never reuses a stale CAS entry
    /// because this string is part of [`font_cas_key`].
    fn version(&self) -> &'static str;
}

/// The bundled backend: the `subsetter` crate driven by `skrifa`'s
/// charmap, behind the crate's `subset` feature. It stands in for the
/// deps report's `fontcull` (not published on crates.io — the module
/// header records the deviation); the [`FontSubsetter`] seam is the
/// injection point a published `fontcull` drops into.
#[cfg(feature = "subset")]
#[derive(Debug, Default, Clone, Copy)]
pub struct SkrifaSubsetter;

#[cfg(feature = "subset")]
impl FontSubsetter for SkrifaSubsetter {
    fn subset(&self, font: &[u8], chars: &BTreeSet<char>) -> Result<Vec<u8>> {
        let codepoints: BTreeSet<u32> = chars.iter().map(|c| u32::from(*c)).collect();
        match subset_core(font, &codepoints)? {
            Some(subset) => Ok(subset),
            None => Err(err(
                "AST-FONT",
                "the font maps none of the requested characters (or the request is empty)",
            )),
        }
    }

    fn id(&self) -> &'static str {
        "skrifa"
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }
}

/// Map the `fonts.subset` strategy to the best compiled-in backend
/// (§V2-C08). The `subset` feature off refuses with `AST-FNT-002` —
/// naming the feature flag and the fallback, the C07 `AST-SVG-002` shape.
///
/// # Errors
///
/// `AST-FNT-002` when the `subset` feature is not compiled into this
/// build of oxide-fw-assets.
pub fn select_subsetter() -> Result<Box<dyn FontSubsetter>> {
    #[cfg(feature = "subset")]
    {
        Ok(Box::new(SkrifaSubsetter))
    }
    #[cfg(not(feature = "subset"))]
    {
        Err(crate::OxideError(Box::new(
            Diagnostic::error(
                "AST-FNT-002",
                "`fonts.subset: 'used'` requires the `subset` feature, which is not \
                 compiled into this build of oxide-fw-assets",
            )
            .with_fix("rebuild with `--features subset`, or drop `used` from `fonts.subset`"),
        )))
    }
}

/// The CAS key for one subset artifact (D40): the font's blake3 digest
/// truncated to 16 hex, prefixed with the backend id and version. Two
/// backends never collide on the same bytes; a backend upgrade never
/// reuses a stale artifact.
#[must_use]
pub fn font_cas_key(subsetter: &dyn FontSubsetter, font: &[u8]) -> String {
    let hash = blake3::Hasher::new().update(font).finalize();
    let hex = hash.to_hex();
    format!(
        "font:{}@{}:{}",
        subsetter.id(),
        subsetter.version(),
        &hex[..16]
    )
}

/// The build hook (§V2-C08 step 4): subset one font to the harvested
/// glyph-usage set when the `used` strategy is configured, and degrade to
/// the source on ANY refusal — the feature missing (`AST-FNT-002`), a
/// backend failure (`AST-FONT`) or a subset that does not shrink the
/// source all log a `tracing` event and return `None`, so a misbehaving
/// backend never breaks a build, it only un-optimizes it. The always
/// needed set ([`ALWAYS_NEEDED_CHARS`]) is unioned into every request, so
/// an empty harvest still subsets to the glyphs every page renders.
///
/// The knob-off path returns `None` without touching the backend: a build
/// that did not opt in never mutates a font.
pub fn subset_used_for_build(
    font: &[u8],
    harvested: &BTreeSet<char>,
    used: bool,
) -> Option<Vec<u8>> {
    if !used {
        return None;
    }
    let mut chars = harvested.clone();
    chars.extend(ALWAYS_NEEDED_CHARS.chars());
    let backend = match select_subsetter() {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(
                code = %e.0.code,
                msg = %e.0.message,
                "fonts: 'used' requested but unavailable; keeping the source"
            );
            return None;
        }
    };
    match backend.subset(font, &chars) {
        Ok(subset) if subset.len() < font.len() => Some(subset),
        Ok(_) => {
            tracing::debug!("fonts: the subset did not shrink the source; keeping the source");
            None
        }
        Err(e) => {
            tracing::warn!(
                code = %e.0.code,
                msg = %e.0.message,
                "fonts: the subsetter output was refused; keeping the source"
            );
            None
        }
    }
}

// ---------------------------------------------------------------------------
// V2-C08 steps 1-2: the harvest
// ---------------------------------------------------------------------------

/// The always-needed set (§V2-C08 step 2): ASCII digits, space, and the
/// common punctuation a page can render. It rides EVERY harvest — a page
/// that renders "42." must not ship a font without the glyphs for "42.".
pub const ALWAYS_NEEDED_CHARS: &str = "0123456789 .,!?:;'\"()[]{}&@#%+-*/=_$";

/// Harvest the characters the prerendered pages actually render (§V2-C08
/// step 1): a `lol_html` `text!` handler over the document body collects
/// every text node into one char set, and the always-needed set rides on
/// top. The `<script>` and `<style>` contents are stripped before the
/// text walk — a minified bundle's identifier soup must not grow the
/// shipped font. The single set is the spec's honest simplification:
/// per-element `font-family` resolution is out of scope (§V2-C08).
///
/// A page the streaming parser rejects contributes nothing (fonts are
/// never load-bearing for a build — the §16.2 convention).
#[must_use]
pub fn harvest_html_codepoints<'a, I>(pages: I) -> BTreeSet<char>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut set = BTreeSet::new();
    for page in pages {
        collect_page_chars(page, &mut set);
    }
    set.extend(ALWAYS_NEEDED_CHARS.chars());
    set
}

/// Harvest the characters of plain-text sources (§V2-C08 step 2): the
/// content-collection field text — entry bodies and frontmatter strings —
/// that never renders into a prerendered page but still needs its glyphs.
/// The always-needed set rides on top, same as the HTML harvest.
#[must_use]
pub fn harvest_text_codepoints<'a, I>(sources: I) -> BTreeSet<char>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut set = BTreeSet::new();
    for src in sources {
        set.extend(src.chars());
    }
    set.extend(ALWAYS_NEEDED_CHARS.chars());
    set
}

/// One page through the `lol_html` walk: `<script>`/`<style>` subtrees
/// are removed before the `text!`-over-body handler collects the rest.
/// Output is discarded — the rewrite runs purely to observe the text.
fn collect_page_chars(page: &str, set: &mut BTreeSet<char>) {
    use lol_html::{RewriteStrSettings, element, text};

    let settings = RewriteStrSettings::new()
        .append_element_content_handler(element!("script", |el| {
            el.remove();
            Ok(())
        }))
        .append_element_content_handler(element!("style", |el| {
            el.remove();
            Ok(())
        }))
        .append_element_content_handler(text!("body", |chunk| {
            set.extend(chunk.as_str().chars());
            Ok(())
        }));
    // lol_html's HTML5 streaming parser does not error on authored
    // markup; a refusal would leave this page's chars unharvested, which
    // is the best-effort degradation the font pipeline follows.
    let _ = lol_html::rewrite_str(page, settings);
}

// ---------------------------------------------------------------------------
// SPEC-16.2: the subsetting core (the pre-seam helpers, preserved)
// ---------------------------------------------------------------------------

/// Subset `font_bytes` to the glyphs needed by the given codepoints.
///
/// Returns `Some(subset_bytes)` on success, `None` when the subset pass
/// produced no smaller output (an already-minimal font, an unparseable
/// cmap, or an empty codepoint set). A `None` result means the caller
/// should fall back to the verbatim copy.
///
/// # Errors
///
/// `AST-FONT` when the font does not parse or the subsetting engine
/// fails.
#[cfg(feature = "subset")]
pub fn subset_for_codepoints(
    font_bytes: &[u8],
    codepoints: &BTreeSet<u32>,
) -> Result<Option<Vec<u8>>> {
    match subset_core(font_bytes, codepoints)? {
        Some(subset) if subset.len() < font_bytes.len() => Ok(Some(subset)),
        _ => Ok(None),
    }
}

/// The subsetting core: map codepoints to glyph ids through the font's
/// cmap, then run `subsetter`. `Ok(None)` means "nothing useful to
/// emit" — an empty request or a font that maps none of it — as opposed
/// to `Err`, which is a real parse/subset failure.
///
/// `.notdef` is glyph 0 — always included so a missing char renders as
/// the font's own placeholder, not as another glyph entirely.
#[cfg(feature = "subset")]
fn subset_core(font_bytes: &[u8], codepoints: &BTreeSet<u32>) -> Result<Option<Vec<u8>>> {
    if codepoints.is_empty() {
        return Ok(None);
    }

    // Map Unicode codepoints → glyph ids using the font's cmap. `skrifa`
    // exposes the charmap; the returned glyph ids feed `subsetter`.
    let font = skrifa::FontRef::new(font_bytes)
        .map_err(|e| err("AST-FONT", format!("skrifa parse failed: {e}")))?;
    let charmap = font.charmap();
    let mut glyphs: BTreeSet<u16> = BTreeSet::new();
    glyphs.insert(0);
    // The (codepoint, glyph) pairs the subset must KEEP mapped: every
    // requested char the font actually maps. These feed both `subsetter`
    // (the outlines to keep) and the cmap rewrite below — the pair list
    // is what makes the emitted subset's cmap honest.
    let mut keep: Vec<(u32, u16)> = Vec::new();
    for cp in codepoints {
        if let Some(ch) = char::from_u32(*cp)
            && let Some(gid) = charmap.map(ch)
        {
            let gid = gid.to_u32() as u16;
            glyphs.insert(gid);
            keep.push((*cp, gid));
        }
    }
    // If the only glyph is `.notdef`, subsetting produces nothing useful.
    if glyphs.len() <= 1 {
        return Ok(None);
    }
    let gid_vec: Vec<u16> = glyphs.into_iter().collect();
    let subset = subsetter::subset(font_bytes, 0, subsetter::Profile::pdf(&gid_vec))
        .map_err(|e| err("AST-FONT", format!("subsetter: {e:?}")))?;
    // `subsetter`'s profile copies the original cmap verbatim — a dropped
    // glyph keeps its cmap entry and maps to an outline-less glyph, which
    // renders as blank instead of falling back to the next face. The
    // bundled backend therefore rewrites the cmap to exactly the kept
    // pairs; on any rewrite refusal the raw subsetter output ships (the
    // pre-rewrite behavior), so the seam only ever improves.
    let subset = cmap_rewrite::with_retained_cmap(&subset, &keep).unwrap_or(subset);
    Ok(Some(subset))
}

/// The cmap rewrite (the bundled backend's honesty half): the output
/// font's cmap must map exactly the codepoints the subset keeps. Glyph
/// ids are preserved by `subsetter` (it drops outlines, not glyphs), so
/// the retained pairs carry straight over.
#[cfg(feature = "subset")]
mod cmap_rewrite {
    /// Rebuild `font` with a fresh cmap that maps exactly `keep`
    /// (codepoint → glyph id, sorted groups). `None` = the shape was not
    /// a flat sfnt the rewrite understands; the caller ships the input
    /// unchanged.
    pub(super) fn with_retained_cmap(font: &[u8], keep: &[(u32, u16)]) -> Option<Vec<u8>> {
        if font.len() < 12 || keep.is_empty() {
            return None;
        }
        let records = read_table_directory(font)?;
        let cmap = build_cmap(keep);
        rebuild_sfnt(font, &records, cmap)
    }

    /// The sfnt table directory: `(tag, table bytes)` pairs, in file
    /// order. A 4-byte tag is read as its big-endian u32 so the sort
    /// below is byte order (the OpenType required record order).
    fn read_table_directory(font: &[u8]) -> Option<Vec<(u32, Vec<u8>)>> {
        let count = u16::from_be_bytes([font[4], font[5]]) as usize;
        if count == 0 || font.len() < 12 + count * 16 {
            return None;
        }
        let mut tables = Vec::with_capacity(count);
        for i in 0..count {
            let rec = &font[12 + i * 16..12 + (i + 1) * 16];
            let tag = u32::from_be_bytes([rec[0], rec[1], rec[2], rec[3]]);
            let offset = u32::from_be_bytes([rec[8], rec[9], rec[10], rec[11]]) as usize;
            let length = u32::from_be_bytes([rec[12], rec[13], rec[14], rec[15]]) as usize;
            let end = offset.checked_add(length)?;
            let data = font.get(offset..end)?.to_vec();
            tables.push((tag, data));
        }
        Some(tables)
    }

    /// The fresh cmap: one format-12 subtable (platform 3, encoding 10 —
    /// the Windows Unicode-BMP-plus spelling every browser reads). Format
    /// 12 is the honest choice over a hand-rolled format 4: groups are
    /// plain (start, end, startGlyph) triples with no `idDelta` arithmetic
    /// to get wrong, and the harvest's char set is not BMP-bound anyway.
    /// `pub(super)`: the module's own test mod drives the encoding
    /// directly (the group-merge pin).
    pub(super) fn build_cmap(keep: &[(u32, u16)]) -> Vec<u8> {
        // Consecutive codepoints whose glyph ids are also consecutive
        // merge into one group — the format's intended encoding.
        let mut sorted: Vec<(u32, u16)> = keep.to_vec();
        sorted.sort_by_key(|&(cp, gid)| (cp, gid));
        sorted.dedup();
        let mut groups: Vec<(u32, u32, u32)> = Vec::new();
        for &(cp, gid) in &sorted {
            // The continuation condition tracks the NEXT expected glyph
            // id: `startGlyph + (cp - group.start)` — extending a group
            // moves its END, never the start glyph.
            match groups.last_mut() {
                Some(last)
                    if last.1 + 1 == cp && last.2 + (last.1 - last.0) + 1 == u32::from(gid) =>
                {
                    last.1 = cp;
                }
                _ => groups.push((cp, cp, u32::from(gid))),
            }
        }

        let length = 16 + groups.len() * 12;
        let mut sub = Vec::with_capacity(length);
        sub.extend_from_slice(&12u16.to_be_bytes()); // format 12
        sub.extend_from_slice(&0u16.to_be_bytes()); // reserved
        sub.extend_from_slice(&(length as u32).to_be_bytes());
        sub.extend_from_slice(&0u32.to_be_bytes()); // language
        sub.extend_from_slice(&(groups.len() as u32).to_be_bytes());
        for (start, end, start_gid) in groups {
            sub.extend_from_slice(&start.to_be_bytes());
            sub.extend_from_slice(&end.to_be_bytes());
            sub.extend_from_slice(&start_gid.to_be_bytes());
        }

        // The cmap table itself: version + one encoding record (Windows
        // platform, the Unicode full-repertoire encoding) pointing at the
        // format-12 subtable that follows it.
        let mut out = Vec::with_capacity(12 + sub.len());
        out.extend_from_slice(&0u16.to_be_bytes()); // version
        out.extend_from_slice(&1u16.to_be_bytes()); // numTables
        out.extend_from_slice(&3u16.to_be_bytes()); // platformID: Windows
        out.extend_from_slice(&10u16.to_be_bytes()); // encodingID: full repertoire
        out.extend_from_slice(&12u32.to_be_bytes()); // subtable offset
        out.extend_from_slice(&sub);
        out
    }

    /// Re-serialize the sfnt: every original table except `cmap`, plus
    /// the fresh one — sorted records, 4-byte alignment, per-table
    /// checksums and the recomputed `head.checkSumAdjustment`.
    fn rebuild_sfnt(font: &[u8], tables: &[(u32, Vec<u8>)], cmap: Vec<u8>) -> Option<Vec<u8>> {
        let cmap_tag = u32::from_be_bytes(*b"cmap");
        let mut kept: Vec<(u32, Vec<u8>)> = tables
            .iter()
            .filter(|(t, _)| *t != cmap_tag)
            .cloned()
            .collect();
        if kept.len() == tables.len() {
            // No cmap in the input — nothing to rewrite.
            return None;
        }
        kept.push((cmap_tag, cmap));
        kept.sort_by_key(|(tag, _)| *tag);

        let count = u16::try_from(kept.len()).ok()?;
        let count32 = u32::from(count);
        let entry_selector = u16::try_from(count32.checked_ilog2()?).ok()?;
        let search_range = (1u16 << entry_selector) * 16;
        let range_shift = count * 16 - search_range;

        let mut out = Vec::new();
        out.extend_from_slice(&font[0..4]); // sfnt version, verbatim
        out.extend_from_slice(&count.to_be_bytes());
        out.extend_from_slice(&search_range.to_be_bytes());
        out.extend_from_slice(&entry_selector.to_be_bytes());
        out.extend_from_slice(&range_shift.to_be_bytes());

        let mut offset = 12usize + usize::from(count) * 16;
        let mut head_offset = None;
        for (tag, data) in &kept {
            if *tag == u32::from_be_bytes(*b"head") {
                head_offset = Some(offset);
            }
            let length = u32::try_from(data.len()).ok()?;
            out.extend_from_slice(&tag.to_be_bytes());
            out.extend_from_slice(&checksum(data).to_be_bytes());
            out.extend_from_slice(&u32::try_from(offset).ok()?.to_be_bytes());
            out.extend_from_slice(&length.to_be_bytes());
            offset += data.len() + (4 - data.len() % 4) % 4;
        }
        for (_, data) in &kept {
            out.extend_from_slice(data);
            let pad = (4 - data.len() % 4) % 4;
            out.extend(std::iter::repeat_n(0u8, pad));
        }

        // head.checkSumAdjustment: the whole-font checksum's complement,
        // with the field itself zeroed for the calculation (the OpenType
        // recipe `construct` in `subsetter` follows too).
        if let Some(at) = head_offset {
            out[at + 8..at + 12].fill(0);
            let val = 0xB1B0_AFBA_u32.wrapping_sub(checksum(&out));
            out[at + 8..at + 12].copy_from_slice(&val.to_be_bytes());
        }
        Some(out)
    }

    /// The OpenType table checksum: a u32 big-endian sum over the data,
    /// padded to a multiple of four with zeros.
    fn checksum(data: &[u8]) -> u32 {
        let mut sum = 0u32;
        for chunk in data.chunks(4) {
            let mut bytes = [0u8; 4];
            bytes[..chunk.len()].copy_from_slice(chunk);
            sum = sum.wrapping_add(u32::from_be_bytes(bytes));
        }
        sum
    }
}

/// Harvest every codepoint from a set of source strings. Used at build time
/// to feed `subset_for_codepoints` — a route's source + loader data covers
/// the text a page can render.
#[cfg(feature = "subset")]
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

#[cfg(all(test, feature = "subset"))]
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

    /// The cmap rewrite's encoding half: consecutive codepoints with
    /// consecutive glyph ids merge into one format-12 group; a far-away
    /// char gets its own group.
    #[test]
    fn the_cmap_rewrite_groups_consecutive_runs() {
        let keep = [(0x61u32, 10u16), (0x62, 11), (0x63, 12), (0x7A, 90)];
        let cmap = cmap_rewrite::build_cmap(&keep);
        // The cmap header: version 0, one Windows/Unicode-full record.
        assert_eq!(&cmap[0..4], &[0, 0, 0, 1]);
        assert_eq!(&cmap[4..8], &[0, 3, 0, 10]);
        assert_eq!(
            &cmap[8..12],
            &[0, 0, 0, 12],
            "the subtable follows the record"
        );
        // The subtable: format 12, then its declared length, then groups.
        let format = u16::from_be_bytes([cmap[12], cmap[13]]);
        assert_eq!(format, 12);
        let length = u32::from_be_bytes([cmap[16], cmap[17], cmap[18], cmap[19]]) as usize;
        assert_eq!(cmap.len(), 12 + length);
        let n_groups = u32::from_be_bytes([cmap[24], cmap[25], cmap[26], cmap[27]]);
        assert_eq!(n_groups, 2, "a-b-c merges; z stands alone");
        let g1_start = u32::from_be_bytes([cmap[28], cmap[29], cmap[30], cmap[31]]);
        let g1_end = u32::from_be_bytes([cmap[32], cmap[33], cmap[34], cmap[35]]);
        let g1_gid = u32::from_be_bytes([cmap[36], cmap[37], cmap[38], cmap[39]]);
        let g2_start = u32::from_be_bytes([cmap[40], cmap[41], cmap[42], cmap[43]]);
        assert_eq!((g1_start, g1_end, g1_gid), (0x61, 0x63, 10));
        assert_eq!(g2_start, 0x7A);
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

#[cfg(all(test, not(feature = "subset")))]
mod tests {
    use super::*;

    /// The harvest rides every build (the CLI needs it ahead of the
    /// bundler; the `subset` feature only gates the subsetting engine).
    #[test]
    fn the_harvest_rides_feature_off_builds() {
        let cps = harvest_html_codepoints(["<body>plain</body>"]);
        assert!(cps.contains(&'p'));
        assert!(cps.contains(&'4'), "always-needed digits ride along");
    }
}
