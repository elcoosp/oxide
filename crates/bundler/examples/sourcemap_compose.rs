//! Consumer example for the V2-C06 source-map composition seam: how a
//! plugin chain's per-plugin V3 maps fold into one map that still points at
//! the original source — the property the emitted `.map` files now carry.
//!
//! Shows the three layers the feature composes:
//!
//! 1. [`oxide_fw_bundler::sourcemap_compose::compose`] — the pure
//!    spec-verbatim algorithm over [`Tok`] lists (greatest `prev` token at
//!    or before the position on the same line; unmatched tokens drop). No
//!    engine, no I/O.
//! 2. [`oxide_fw_bundler::sourcemap_compose::compose_v3_json`] — the wire
//!    form: the `{ code, map }` shape a JS plugin returns, where `map` is a
//!    V3 JSON string. The chain folds it into the accumulated map; a
//!    mapless plugin is position-preserving and keeps the map intact.
//! 3. The bundlers: `RolldownBundler` attaches the folded map to
//!    `HookTransformOutput.map`, and the ChunkBundler stitches the
//!    per-module compiler maps into the emitted chunk `.map` (replacing the
//!    old `{}` stub).
//!
//! Run it:
//!
//! ```text
//! cargo run -p oxide-fw-bundler --example sourcemap_compose
//! ```

use oxide_fw_bundler::sourcemap_compose::{Tok, compose, parse_v3, toks_of, v3_from_toks};

fn main() {
    // 1. The pure algorithm: `prev` is the identity over the module source
    //    (three lines), `next` is what a banner-prepending plugin's map
    //    says — every original line shifted down by one generated line.
    let prev: Vec<Tok> = (0..3)
        .map(|l| Tok {
            gen_line: l,
            gen_col: 0,
            src: 0,
            src_line: l,
            src_col: 0,
        })
        .collect();
    let next: Vec<Tok> = (0..3)
        .map(|l| Tok {
            gen_line: l + 1,
            gen_col: 0,
            src: 0,
            src_line: l,
            src_col: 0,
        })
        .collect();
    let composed = compose(&prev, &next);
    println!("1. compose(identity, banner-shift):");
    for t in &composed {
        println!(
            "   gen {1}:{0} -> src line {2}",
            t.gen_col, t.gen_line, t.src_line
        );
    }
    assert_eq!(
        composed[0].src_line, 0,
        "line 1 of the output is line 1 of the source"
    );

    // 2. The wire form: the same pair as V3 JSON — what a JS plugin's
    //    `map` field carries and what the CLI's `fold_plugin_map` folds.
    let prev_map = v3_from_toks(
        &prev,
        Some("page.js".to_string()),
        vec!["src/routes/index/page.tsx".to_string()],
        Vec::new(),
    );
    let next_map = v3_from_toks(
        &next,
        Some("page.js".to_string()),
        vec!["intermediate.js".to_string()],
        Vec::new(),
    );
    let composed_json = oxide_fw_bundler::sourcemap_compose::compose_v3_json(
        &prev_map.to_json_string(),
        &next_map.to_json_string(),
    )
    .expect("the maps compose");
    let map = parse_v3(&composed_json).expect("the composed map parses");
    let toks = toks_of(&map).expect("the composed map decodes");
    println!("2. compose_v3_json over the wire forms:");
    println!("   sources: {:?}", map.sources);
    println!(
        "   first token: gen {}:{} -> src {}:{}",
        toks[0].gen_line, toks[0].gen_col, toks[0].src_line, toks[0].src_col
    );
    assert_eq!(
        map.sources,
        ["src/routes/index/page.tsx"],
        "the composed map names the ORIGINAL source, never the intermediate"
    );

    // 3. A malformed plugin map is refused, never fatal: the wire-up warns
    //    with BLD-SMAP and keeps the accumulated map (or none).
    let refused = oxide_fw_bundler::sourcemap_compose::compose_v3_json(
        &prev_map.to_json_string(),
        "{not a map",
    );
    println!(
        "3. malformed plugin map -> {} (warn + drop, the code stands)",
        refused.expect_err("refused").0.code
    );

    println!("\nthe emitted chunk .map now carries this composed map — see the");
    println!("sourcemap_compose_tdd pins and the rolldown banner acceptance.");
}
