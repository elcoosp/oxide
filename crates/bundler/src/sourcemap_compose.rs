//! V2-C06: source-map composition in the plugin chain.
//!
//! The JS and WASM plugin `transform` hooks used to return code only, so a
//! plugin that rewrites code broke the chain from the final output back to
//! the original source. This module is the composition seam: the chain folds
//! each plugin's optional V3 map into the accumulated one, and the bundlers
//! carry the folded map to the emitted `.map` files.
//!
//! Three layers, each independently testable:
//!
//! 1. The pure algorithm — [`compose`] over [`Tok`] lists (spec-verbatim:
//!    for every token of `next`, take its (intermediate line, column) and
//!    find the greatest token in `prev` at or before that position on the
//!    same line; emit a token with the final position and `prev`'s original
//!    source, line and column; drop tokens with no match). No engine, no
//!    I/O, no dependency on the map format.
//! 2. The V3 JSON layer — [`V3Map`], [`parse_v3`], [`toks_of`],
//!    [`v3_from_toks`], [`compose_v3`] and [`compose_v3_json`]. A minimal
//!    serde model of a V3 map (only the fields composition needs) plus a
//!    base64-VLQ decoder/encoder for the `mappings` payload. Still
//!    engine-free: these run against raw JSON strings.
//! 3. The oxc interop — [`oxc_from_v3`] and [`oxc_source_map_from_json`]
//!    build the `oxc_sourcemap` maps the rolldown backend attaches to
//!    `HookTransformOutput`, so Rolldown composes the per-module maps into
//!    the chunk maps it writes.
//!
//! Every refusal here is [`BLD-SMAP`] (see `docs/errors.toml`): a plugin
//! map that does not parse is never allowed to fail a build — the wire-up
//! warn-degrades to "no map" — but it is never silently accepted either.

use serde::{Deserialize, Serialize};

use crate::{Result, err};

/// The registry code for every malformed-plugin-map refusal in this module.
pub const BLD_SMAP: &str = "BLD-SMAP";

// ------------------------------------------------------------------ layer 1

/// One mapping token of a V3 sourcemap: a position in the GENERATED code
/// (`gen_line`/`gen_col`) pointing at position (`src_line`/`src_col`) of the
/// source file indexed by `src` in the map's `sources` array.
///
/// Spec-verbatim shape — five fields, no name id: the composition algorithm
/// preserves positions and source indices only, so a composed map drops the
/// intermediate name table (the original map's names ride `V3Map::names`
/// unchanged).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Tok {
    pub gen_line: u32,
    pub gen_col: u32,
    pub src: u32,
    pub src_line: u32,
    pub src_col: u32,
}

/// Compose two mapping lists: `prev` maps intermediate positions to the
/// original source, `next` maps final positions to the intermediate space.
///
/// For every token of `next`, the greatest token of `prev` at or before
/// `(n.src_line, n.src_col)` on the same line carries its original position
/// through; a token with no predecessor on its line is dropped. `prev` must
/// be sorted by `(gen_line, gen_col)` — the order [`toks_of`] decodes and
/// [`v3_from_toks`] encodes.
#[must_use]
pub fn compose(prev: &[Tok], next: &[Tok]) -> Vec<Tok> {
    let mut out = Vec::with_capacity(next.len());
    for n in next {
        let i = prev.partition_point(|p| (p.gen_line, p.gen_col) <= (n.src_line, n.src_col));
        if i == 0 {
            continue;
        }
        let p = &prev[i - 1];
        if p.gen_line != n.src_line {
            continue;
        }
        out.push(Tok {
            gen_line: n.gen_line,
            gen_col: n.gen_col,
            src: p.src,
            src_line: p.src_line,
            src_col: p.src_col,
        });
    }
    out
}

// ------------------------------------------------------------------ layer 2

/// A minimal V3 source map: only the fields composition reads and carries.
///
/// `version` is validated (must be 3) by [`parse_v3`]; `sources`/`names`
/// (and the optional contents/root) ride unchanged from the ORIGINAL map
/// through [`compose_v3`], because the composed map points at the original
/// source files. `mappings` is the base64-VLQ payload, decoded by
/// [`toks_of`] and encoded by [`v3_from_toks`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct V3Map {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_root: Option<String>,
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sources_content: Option<Vec<Option<String>>>,
    #[serde(default)]
    pub names: Vec<String>,
    #[serde(default)]
    pub mappings: String,
}

fn default_version() -> u32 {
    3
}

impl V3Map {
    /// A minimal map from a hand-written `mappings` payload — the shape the
    /// pins and synthetic builders use (one source, no contents).
    #[must_use]
    pub fn from_mappings(mappings: &str, sources: Vec<String>) -> Self {
        Self {
            version: 3,
            file: None,
            source_root: None,
            sources,
            sources_content: None,
            names: Vec::new(),
            mappings: mappings.to_string(),
        }
    }

    /// Serialize to the V3 JSON wire form. The struct holds only strings,
    /// numbers and arrays, so serialization cannot fail in practice — a
    /// failure (none known) falls back to the valid empty V3 document.
    #[must_use]
    pub fn to_json_string(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| {
            serde_json::json!({ "version": 3, "sources": [], "names": [], "mappings": "" })
                .to_string()
        })
    }
}

/// Parse a V3 map from its JSON serialization.
///
/// # Errors
///
/// [`BLD-SMAP`] (`err(BLD_SMAP, ..)`) when the JSON does not parse as a V3
/// map or the `version` field is not 3.
pub fn parse_v3(json: &str) -> Result<V3Map> {
    let map: V3Map = serde_json::from_str(json)
        .map_err(|e| err(BLD_SMAP, format!("plugin map is not valid JSON: {e}")))?;
    if map.version != 3 {
        return Err(err(
            BLD_SMAP,
            format!("plugin map version {} is not V3", map.version),
        ));
    }
    Ok(map)
}

/// Decode `map.mappings` into the token list (sorted by
/// `(gen_line, gen_col)`, the order [`compose`] and [`v3_from_toks`] use).
///
/// Sourceless segments (a single VLQ field: a generated position with no
/// original position) are skipped — they carry nothing the composer can map
/// through.
///
/// # Errors
///
/// [`BLD-SMAP`] when a segment has an invalid field count or VLQ payload,
/// or when a token's source index is out of the `sources` range.
pub fn toks_of(map: &V3Map) -> Result<Vec<Tok>> {
    let mut toks = Vec::new();
    // Delta state: gen_col resets per line (declared in the loop);
    // src/src_line/src_col persist across lines (the V3 wire format's
    // rules).
    let (mut src, mut src_line, mut src_col) = (0u32, 0u32, 0u32);
    let mut line = 0u32;
    for group in map.mappings.split(';') {
        let mut gen_col = 0u32;
        for seg in group.split(',') {
            if seg.is_empty() {
                continue;
            }
            let fields = decode_segment(seg)
                .ok_or_else(|| err(BLD_SMAP, format!("malformed mappings segment `{seg}`")))?;
            let f0 = fields[0];
            let f0 = i32::try_from(f0)
                .map_err(|_| err(BLD_SMAP, "mappings gen_col delta out of range"))?;
            gen_col = gen_col
                .checked_add_signed(f0)
                .ok_or_else(|| err(BLD_SMAP, "mappings gen_col overflow"))?;
            if fields.len() == 1 {
                // Sourceless: a generated position with no original one.
                continue;
            }
            if fields.len() != 4 && fields.len() != 5 {
                return Err(err(
                    BLD_SMAP,
                    format!(
                        "mappings segment `{seg}` has {} fields (want 1, 4 or 5)",
                        fields.len()
                    ),
                ));
            }
            src = add_u32(src, fields[1])?;
            src_line = add_u32(src_line, fields[2])?;
            src_col = add_u32(src_col, fields[3])?;
            // A 5-field segment's name delta is decoded and dropped: the
            // composed map preserves positions and source indices only.
            if src as usize >= map.sources.len() {
                return Err(err(
                    BLD_SMAP,
                    format!(
                        "mappings token at line {line} references source {src} but the map has {} sources",
                        map.sources.len()
                    ),
                ));
            }
            toks.push(Tok {
                gen_line: line,
                gen_col,
                src,
                src_line,
                src_col,
            });
        }
        line += 1;
    }
    Ok(toks)
}

/// Encode `toks` into a [`V3Map`] carrying `file`/`sources`/`names`.
///
/// The tokens are sorted by `(gen_line, gen_col)` first (the wire format's
/// requirement); every token's `src` must index `sources`.
///
/// # Errors
///
/// [`BLD-SMAP`] when a token's source index is out of the `sources` range.
pub fn v3_from_toks(
    toks: &[Tok],
    file: Option<String>,
    sources: Vec<String>,
    names: Vec<String>,
) -> V3Map {
    let mut sorted: Vec<Tok> = toks.to_vec();
    sorted.sort();
    let mut mappings = String::new();
    let (mut gen_col, mut src, mut src_line, mut src_col) = (0u32, 0u32, 0u32, 0u32);
    let mut line = 0u32;
    for t in &sorted {
        while line < t.gen_line {
            mappings.push(';');
            gen_col = 0;
            line += 1;
        }
        if !mappings.is_empty() && !mappings.ends_with(';') {
            mappings.push(',');
        }
        encode_vlq(&mut mappings, t.gen_col as i64 - gen_col as i64);
        encode_vlq(&mut mappings, t.src as i64 - src as i64);
        encode_vlq(&mut mappings, t.src_line as i64 - src_line as i64);
        encode_vlq(&mut mappings, t.src_col as i64 - src_col as i64);
        gen_col = t.gen_col;
        src = t.src;
        src_line = t.src_line;
        src_col = t.src_col;
    }
    if mappings.is_empty() {
        // A map over a line-only prefix still needs its separators: emit the
        // empty document (`""`), the valid V3 spelling of "no mappings".
        return V3Map {
            version: 3,
            file,
            source_root: None,
            sources,
            sources_content: None,
            names,
            mappings: String::new(),
        };
    }
    V3Map {
        version: 3,
        file,
        source_root: None,
        sources,
        sources_content: None,
        names,
        mappings,
    }
}

/// Compose two parsed maps: the result maps `next`'s generated space to
/// `prev`'s original sources, carrying `prev`'s `sources`/`names`/contents
/// (and `next`'s `file`, the final output the map describes).
///
/// # Errors
///
/// [`BLD-SMAP`] when either map's `mappings` payload is malformed.
pub fn compose_v3(prev: &V3Map, next: &V3Map) -> Result<V3Map> {
    let prev_toks = toks_of(prev)?;
    let next_toks = toks_of(next)?;
    let composed = compose(&prev_toks, &next_toks);
    let mut out = v3_from_toks(
        &composed,
        next.file.clone().or_else(|| prev.file.clone()),
        prev.sources.clone(),
        prev.names.clone(),
    );
    out.source_root = prev.source_root.clone();
    out.sources_content = prev.sources_content.clone();
    Ok(out)
}

/// Compose two V3 maps given as JSON strings — the wire form a JS plugin's
/// `map` field arrives in.
///
/// # Errors
///
/// [`BLD-SMAP`] when either JSON does not parse as a V3 map or a `mappings`
/// payload is malformed.
pub fn compose_v3_json(prev_json: &str, next_json: &str) -> Result<String> {
    let prev = parse_v3(prev_json)?;
    let next = parse_v3(next_json)?;
    let composed = compose_v3(&prev, &next)?;
    serde_json::to_string(&composed)
        .map_err(|e| err(BLD_SMAP, format!("composed map serialization failed: {e}")))
}

// ------------------------------------------------------------------ layer 3

/// Build an owned `oxc_sourcemap` map from a parsed V3 map — the form the
/// rolldown backend attaches to `HookTransformOutput`.
///
/// # Errors
///
/// [`BLD-SMAP`] when a token's source index is out of the `sources` range.
pub fn oxc_from_v3(map: &V3Map) -> Result<oxc_sourcemap::OwnedSourceMap> {
    let toks = toks_of(map)?;
    let tokens: Vec<oxc_sourcemap::Token> = toks
        .iter()
        .map(|t| {
            oxc_sourcemap::Token::new(
                t.gen_line,
                t.gen_col,
                t.src_line,
                t.src_col,
                Some(t.src),
                None,
            )
        })
        .collect();
    let sm = oxc_sourcemap::SourceMap::new(
        map.file.clone().map(std::borrow::Cow::Owned),
        map.names
            .iter()
            .map(|n| std::borrow::Cow::Owned(n.clone()))
            .collect(),
        map.source_root.clone().map(std::borrow::Cow::Owned),
        map.sources
            .iter()
            .map(|s| std::borrow::Cow::Owned(s.clone()))
            .collect(),
        map.sources_content
            .clone()
            .map(|contents| {
                contents
                    .into_iter()
                    .map(|c| c.map(std::borrow::Cow::Owned))
                    .collect()
            })
            .unwrap_or_default(),
        tokens.into_boxed_slice(),
        None,
    );
    Ok(oxc_sourcemap::OwnedSourceMap::new(sm))
}

/// Parse a V3 map from JSON and build the `oxc_sourcemap` form — the one
/// call site the rolldown backend's transform hook needs.
///
/// # Errors
///
/// [`BLD-SMAP`] when the JSON or the `mappings` payload is malformed.
pub fn oxc_source_map_from_json(json: &str) -> Result<oxc_sourcemap::SourceMap<'static>> {
    let map = parse_v3(json)?;
    let owned = oxc_from_v3(&map)?;
    Ok(owned.into_inner())
}

// ------------------------------------------------------------- VLQ plumbing

/// Decode one mappings segment into its VLQ field values.
fn decode_segment(seg: &str) -> Option<Vec<i64>> {
    let mut fields = Vec::new();
    let mut rest = seg;
    while !rest.is_empty() {
        let (value, consumed) = decode_vlq(rest)?;
        fields.push(value);
        rest = &rest[consumed..];
    }
    Some(fields)
}

/// Decode one base64-VLQ value starting at `s`; returns the value and the
/// number of chars consumed.
fn decode_vlq(s: &str) -> Option<(i64, usize)> {
    let mut result: u64 = 0;
    let mut shift = 0u32;
    for (i, ch) in s.char_indices() {
        let d = u64::try_from(VLQ_DIGITS.find(ch)?).ok()?;
        result |= (d & 31) << shift;
        if d & 32 == 0 {
            // Sign in the LSB of the first group.
            let magnitude = i64::try_from(result >> 1).ok()?;
            let value = if result & 1 == 1 {
                -magnitude
            } else {
                magnitude
            };
            return Some((value, i + ch.len_utf8()));
        }
        shift += 5;
        if shift > 55 {
            return None; // absurdly long run — refuse, never wrap
        }
    }
    None // ran out of chars with the continuation bit still set
}

/// Append one base64-VLQ value to `out`.
fn encode_vlq(out: &mut String, value: i64) {
    let magnitude = value.unsigned_abs();
    let mut vlq = if value < 0 {
        (magnitude << 1) | 1
    } else {
        magnitude << 1
    };
    loop {
        let digit = (vlq & 31) as usize;
        vlq >>= 5;
        let mut c = char::from(VLQ_DIGITS.as_bytes()[digit]);
        if vlq > 0 {
            c = char::from(VLQ_DIGITS.as_bytes()[digit | 32]);
        }
        out.push(c);
        if vlq == 0 {
            break;
        }
    }
}

/// The base64 VLQ alphabet, in wire order.
const VLQ_DIGITS: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Apply one decoded delta to an absolute `u32` field, refusing a malformed
/// (negative-result or overflowing) map instead of wrapping.
fn add_u32(abs: u32, delta: i64) -> Result<u32> {
    let next = abs as i64 + delta;
    u32::try_from(next).map_err(|_| err(BLD_SMAP, "mappings field out of range"))
}
