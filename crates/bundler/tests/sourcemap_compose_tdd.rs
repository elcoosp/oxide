//! TDD suite for V2-C06 (source-map composition in the plugin chain).
//!
//! The pins:
//! - the spec-verbatim pure `compose` over `Tok` lists (identity, a column
//!   prefix, a prepended banner line, the unmatched-token drop, three-stage
//!   associativity);
//! - the V3 JSON layer (`parse_v3` / `toks_of` / `v3_from_toks`) including
//!   the 1-field-segment skip and the `BLD-SMAP` refusals;
//! - `compose_v3`'s array semantics (the composed map points at the ORIGINAL
//!   sources, so it carries `prev`'s sources/names);
//! - the oxc interop (`oxc_from_v3`), and
//! - the two wire-ups: the ChunkBundler emits a REAL stitched V3 map (never
//!   the old `{}` stub) and the rolldown path with a banner plugin keeps the
//!   original line-1 mapping (the spec's acceptance).
//!
//! Every pin here runs without a JS engine: `compose` itself is pure, the
//! plugin maps in the wire tests are Rust closures, and oxc_sourcemap is a
//! plain Rust library.

use oxide_fw_bundler::sourcemap_compose::{
    Tok, V3Map, compose, compose_v3, compose_v3_json, oxc_from_v3, parse_v3, toks_of, v3_from_toks,
};
use oxide_fw_bundler::{BundleEnv, BundleOptions, Bundler, ChunkBundler, ChunkKind, TransformHook};
use std::collections::BTreeMap;
use std::path::Path;

// ------------------------------------------------------------------ helpers

fn tok(gen_line: u32, gen_col: u32, src: u32, src_line: u32, src_col: u32) -> Tok {
    Tok {
        gen_line,
        gen_col,
        src,
        src_line,
        src_col,
    }
}

/// An identity map over `lines` lines (one token per line, col 0) into
/// `sources`.
fn identity(lines: u32, sources: &[&str]) -> V3Map {
    let toks: Vec<Tok> = (0..lines).map(|l| tok(l, 0, 0, l, 0)).collect();
    v3_from_toks(
        &toks,
        Some("out.js".to_string()),
        sources.iter().map(|s| s.to_string()).collect(),
        Vec::new(),
    )
}

fn write(root: &Path, rel: &str, content: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

fn chunk_opts(root: &Path, env: BundleEnv, entries: &[(&str, &str)]) -> BundleOptions {
    BundleOptions {
        env,
        entries: entries
            .iter()
            .map(|(r, p)| (r.to_string(), root.join(p)))
            .collect(),
        out_dir: root.join("dist"),
        minify: false,
        sourcemap: true,
        externals: Vec::new(),
        define: BTreeMap::new(),
        tsconfig_paths: BTreeMap::new(),
        build_secret: "test-secret".to_string(),
        precompiled: BTreeMap::new(),
        jsx_import_source: None,
        transform_hook: None,
        resolve_hook: None,
        load_hook: None,
        render_chunk_hook: None,
        generate_bundle_hook: None,
        transform_html_hook: None,
        css_transform: None,
        inline_dynamic_imports: false,
        resolve_conditions: None,
        virtual_modules: Default::default(),
    }
}

// ------------------------------------------------- 1. the pure compose pins

#[test]
fn compose_identity_map_composes_to_itself() {
    let prev = [tok(0, 0, 0, 0, 0), tok(1, 0, 0, 1, 0), tok(2, 0, 0, 2, 0)];
    let next = prev;
    assert_eq!(compose(&prev, &next), prev);
}

#[test]
fn compose_column_prefix_composes_correctly() {
    // A plugin prepends a 10-column prefix (no new line). `prev` is the
    // identity over line 0; `next` shifts every token 10 columns right. The
    // spec's algorithm maps each shifted token through the greatest `prev`
    // token at or before its (intermediate) position on the same line, so
    // the composed map undoes the shift exactly: each output position
    // points back at the source position it came from.
    let prev = [tok(0, 0, 0, 0, 0), tok(0, 5, 0, 0, 5), tok(0, 10, 0, 0, 10)];
    let next = [
        tok(0, 10, 0, 0, 0),
        tok(0, 15, 0, 0, 5),
        tok(0, 20, 0, 0, 10),
    ];
    assert_eq!(
        compose(&prev, &next),
        vec![
            tok(0, 10, 0, 0, 0),
            tok(0, 15, 0, 0, 5),
            tok(0, 20, 0, 0, 10)
        ]
    );
}

#[test]
fn compose_prepended_banner_keeps_line_one_pointing_at_the_source() {
    // The spec's acceptance shape: a plugin prepends a banner line; the
    // composed map's first original-code token still points at (0-indexed)
    // line 0 of the source file.
    let prev = identity(2, &["src/a.ts"]);
    let next = [tok(1, 0, 0, 0, 0), tok(2, 0, 0, 1, 0)];
    let prev_toks = toks_of(&prev).expect("identity map decodes");
    let composed = compose(&prev_toks, &next);
    assert_eq!(composed, vec![tok(1, 0, 0, 0, 0), tok(2, 0, 0, 1, 0)]);
    assert_eq!(
        composed[0].src_line, 0,
        "line 1 of the output is line 1 of the source"
    );
}

#[test]
fn compose_drops_tokens_with_no_predecessor_on_their_line() {
    let prev = [tok(0, 0, 0, 0, 0)];
    // (0,10) has a predecessor; (5,0) points at prev line 5 which has no
    // token at all — dropped, per the spec.
    let next = [tok(0, 10, 0, 0, 0), tok(5, 0, 0, 5, 0)];
    assert_eq!(compose(&prev, &next), vec![tok(0, 10, 0, 0, 0)]);
}

#[test]
fn compose_three_stage_equals_two_stage_then_one() {
    // source --m1--> a --m2--> b --m3--> final. Composing pairwise is
    // associative for well-formed consecutive maps:
    // compose(compose(m1, m2), m3) == compose(m1, compose(m2, m3)).
    let m1 = [
        // a -> source: identity over 3 lines
        tok(0, 0, 0, 0, 0),
        tok(1, 0, 0, 1, 0),
        tok(2, 0, 0, 2, 0),
    ];
    let m2 = [
        // b -> a: banner shift
        tok(1, 0, 0, 0, 0),
        tok(2, 0, 0, 1, 0),
        tok(3, 0, 0, 2, 0),
    ];
    let m3 = [
        // final -> b: two-column prefix on the same line
        tok(0, 2, 0, 1, 0),
        tok(1, 2, 0, 2, 0),
        tok(2, 2, 0, 3, 0),
    ];
    let left = compose(&compose(&m1, &m2), &m3);
    let right = compose(&m1, &compose(&m2, &m3));
    assert_eq!(left, right, "map composition is associative");
}

// ------------------------------------------------- 2. the V3 JSON layer

#[test]
fn v3_round_trip_preserves_tokens() {
    let toks = vec![
        tok(0, 0, 0, 0, 0),
        tok(0, 9, 0, 0, 4),
        tok(1, 0, 0, 1, 0),
        tok(3, 7, 0, 2, 12),
    ];
    let map = v3_from_toks(
        &toks,
        Some("chunk.js".to_string()),
        vec!["../src/a.ts".to_string()],
        vec!["Card".to_string()],
    );
    let back = toks_of(&map).expect("round trip");
    assert_eq!(back, toks);
}

#[test]
fn v3_parse_skips_sourceless_segments() {
    // A 1-field segment (generated position only, no source) carries no
    // mapping the composer can use — skipped, not an error. VLQ: 0 -> "A".
    // Line 0: a sourceless segment then a 4-field one; line 1: one 4-field.
    let map = V3Map::from_mappings("A,AAAA;AACA", vec!["a.ts".to_string()]);
    let toks = toks_of(&map).expect("sourceless segments skipped");
    assert_eq!(toks, vec![tok(0, 0, 0, 0, 0), tok(1, 0, 0, 1, 0)]);
}

#[test]
fn parse_v3_refuses_malformed_json_with_bld_smap() {
    let e = parse_v3("definitely not json").expect_err("malformed json");
    assert_eq!(e.0.code, "BLD-SMAP", "the registry code names the refusal");
}

#[test]
fn parse_v3_refuses_a_non_v3_version() {
    let e = parse_v3("{\"version\":2,\"sources\":[],\"names\":[],\"mappings\":\"\"}")
        .expect_err("version 2 is not V3");
    assert_eq!(e.0.code, "BLD-SMAP");
}

#[test]
fn toks_of_refuses_a_source_index_out_of_range() {
    // src=1 (the second segment's delta is `C` = +1) but the map has one
    // source — a malformed map the composer must not silently index out of
    // bounds on.
    let map = V3Map::from_mappings("AAAA;ACAA", vec!["a.ts".to_string()]);
    let e = toks_of(&map).expect_err("src index out of range");
    assert_eq!(e.0.code, "BLD-SMAP");
}

#[test]
fn compose_v3_json_refuses_a_malformed_next_map() {
    let prev = identity(2, &["src/a.ts"]);
    let prev_json = serde_json::to_string(&prev).unwrap();
    let e =
        compose_v3_json(&prev_json, "{not a map").expect_err("a malformed plugin map is refused");
    assert_eq!(e.0.code, "BLD-SMAP");
}

// --------------------------------- 3. compose_v3 array + acceptance pins

#[test]
fn compose_v3_carries_the_original_sources_and_names() {
    // The composed map points at the ORIGINAL source files, so it carries
    // prev's sources (and names), never next's intermediate ones.
    let mut prev = identity(2, &["../src/a.ts"]);
    prev.names.push("Card".to_string());
    let next = v3_from_toks(
        &[tok(1, 0, 0, 0, 0)],
        Some("b.js".to_string()),
        vec!["intermediate.js".to_string()],
        Vec::new(),
    );
    let composed = compose_v3(&prev, &next).expect("compose");
    assert_eq!(composed.sources, ["../src/a.ts"]);
    assert_eq!(composed.names, ["Card"]);
}

#[test]
fn compose_v3_json_banner_acceptance_points_back_at_line_one() {
    // The spec's acceptance, at the wire level: `prev` is the identity map
    // of the module source, `next` is the map a banner-prepending plugin
    // returns (its output shifts every original line down by one). The
    // composed JSON's first original-code token must point at line 1
    // (src_line 0, 0-indexed) of the source file.
    let prev = identity(3, &["src/routes/index/page.tsx"]);
    let prev_json = serde_json::to_string(&prev).unwrap();
    let next = v3_from_toks(
        &[tok(1, 0, 0, 0, 0), tok(2, 0, 0, 1, 0), tok(3, 0, 0, 2, 0)],
        Some("page.js".to_string()),
        vec!["intermediate.js".to_string()],
        Vec::new(),
    );
    let next_json = serde_json::to_string(&next).unwrap();
    let composed = compose_v3_json(&prev_json, &next_json).expect("compose");
    let map = parse_v3(&composed).expect("composed map parses");
    let toks = toks_of(&map).expect("tokens decode");
    assert!(!toks.is_empty());
    assert_eq!(
        toks[0],
        tok(1, 0, 0, 0, 0),
        "the first original-code token points at line 1 of the source"
    );
    assert_eq!(
        map.sources,
        ["src/routes/index/page.tsx"],
        "the composed map names the original source"
    );
}

// --------------------------------------------- 4. the oxc_sourcemap interop

#[test]
fn oxc_from_v3_round_trips_through_the_oxc_map() {
    let toks = vec![tok(0, 0, 0, 0, 0), tok(1, 4, 0, 1, 2)];
    let map = v3_from_toks(
        &toks,
        Some("chunk.js".to_string()),
        vec!["../src/a.ts".to_string()],
        Vec::new(),
    );
    let owned = oxc_from_v3(&map).expect("build an OwnedSourceMap");
    let back = parse_v3(&owned.to_json_string()).expect("oxc serializes valid V3");
    assert_eq!(toks_of(&back).expect("decode"), toks);
}

// ----------------------------------------------- 5. the ChunkBundler wire-up

#[test]
fn chunk_bundler_writes_a_real_stitched_v3_map_not_a_stub() {
    // The pre-C06 behavior wrote a literal `{}` next to every chunk — not a
    // valid V3 map at all. With the composition seam the chunk map is real:
    // the per-module compiler maps stitched at the module line offsets, so
    // the first token points into the original TS source.
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "src/routes/index/page.tsx",
        "import { Card } from \"../../components/Card.tsx\";\nexport const meta = { title: \"Home\" };\nexport default function Page() {\n  return Card(\"home\");\n}\n",
    );
    write(
        dir.path(),
        "src/components/Card.tsx",
        "export function Card(label: string) {\n  return { tag: \"div\", label };\n}\n",
    );

    let out = ChunkBundler::new(dir.path())
        .bundle(&chunk_opts(
            dir.path(),
            BundleEnv::Client,
            &[("index", "src/routes/index/page.tsx")],
        ))
        .expect("bundle with sourcemaps");

    let entry = out
        .chunks
        .iter()
        .find(|c| c.kind == ChunkKind::Entry)
        .expect("entry chunk");
    let map_path = dir.path().join("dist").join(format!("{}.map", entry.file));
    let map_json = std::fs::read_to_string(&map_path).expect("the chunk map is written");
    let map = parse_v3(&map_json).expect("a VALID V3 map, not the `{}` stub");
    let toks = toks_of(&map).expect("tokens decode");
    assert!(
        !toks.is_empty(),
        "lowered TS modules contribute real mappings"
    );
    assert!(
        map.sources.iter().any(|s| s.contains("Card.tsx")),
        "the stitched map names the original module sources: {:?}",
        map.sources
    );
}

#[test]
fn chunk_bundler_minified_map_is_still_a_valid_v3_document() {
    // Minify shifts positions the stitched map cannot describe, so the
    // tokens are withheld — but the file stays a VALID empty V3 map (never
    // the `{}` stub browsers choke on).
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "src/routes/index/page.tsx",
        "export default function Page() {\n  return 1;\n}\n",
    );

    let mut o = chunk_opts(
        dir.path(),
        BundleEnv::Client,
        &[("index", "src/routes/index/page.tsx")],
    );
    o.minify = true;
    let out = ChunkBundler::new(dir.path())
        .bundle(&o)
        .expect("bundle minified");

    let entry = out
        .chunks
        .iter()
        .find(|c| c.kind == ChunkKind::Entry)
        .expect("entry chunk");
    let map_json =
        std::fs::read_to_string(dir.path().join("dist").join(format!("{}.map", entry.file)))
            .expect("the chunk map is written");
    let map = parse_v3(&map_json).expect("a valid V3 document");
    assert!(
        toks_of(&map).expect("decode").is_empty(),
        "minified output carries no stitched tokens"
    );
}

// ----------------------------------------------- 6. the TransformHook result

#[test]
fn transform_hook_code_only_helper_wraps_the_output() {
    // `new_code` is the code-only convenience constructor: the WASM hook and
    // the dev paths produce no map.
    let hook = TransformHook::new_code(|_id: &str, source: &str| Ok(format!("banner\n{source}")));
    let out = hook.call("a.ts", "let x = 1;\n").expect("call");
    assert_eq!(out.code, "banner\nlet x = 1;\n");
    assert!(out.map.is_none(), "a code-only hook carries no map");
}
