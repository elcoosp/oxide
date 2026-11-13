//! TDD suite for the §V2-C07 SVG optimizer: the [`SvgOptimizer`] seam
//! (`crates/assets/src/svg.rs`) — `IdentitySvg` (the always-available
//! default), `OxvgSvg` (behind the crate's `svg-oxvg` feature), the
//! preservation rules the wrapper enforces on every optimizer output
//! (viewBox kept, `<use>`/`url(#…)`/`aria-*` referenced ids kept, the
//! output parses back as well-formed XML), the content-hash CAS key
//! (`svg:{id}@{version}:{blake3-16hex}`, the D40 shape), the feature-aware
//! selection (`select_optimizer`) and the build-side warn-and-fallback
//! helper (`optimize_for_build`).
//!
//! Written BEFORE the implementation; every pin imports public API only.
//! The optimizer-behavior pins drive the seam through fakes wherever the
//! spec names a rule (so the rules are pinned against ANY optimizer, not
//! just oxvg), and drive the real `OxvgSvg` on the three fixtures the WP
//! names (`<use href="#a">`, inline `<style>`, `viewBox` only) under the
//! `svg-oxvg` feature.

use std::collections::BTreeSet;

use oxide_fw_assets::svg::{
    IdentitySvg, SvgOptimizer, assert_preserved, optimize_for_build, referenced_svg_ids,
    select_optimizer, svg_cas_key,
};
use oxide_fw_diagnostics::OxideError;

/// The WP's first named fixture: an id referenced by `<use href="#a">`.
const FIXTURE_USE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
  <!-- a build comment -->
  <path id="a" d="M0 0 L24 24 Z"/>
  <use href="#a"/>
</svg>"##;

/// The WP's second named fixture: an inline `<style>` the rules must keep
/// working (the class is referenced, so the style survives).
const FIXTURE_STYLE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
  <style>.icon { fill: currentColor; }</style>
  <rect class="icon" width="24" height="24"/>
</svg>"##;

/// The WP's third named fixture: `viewBox` only (no width/height — the
/// attribute the rules must preserve verbatim).
const FIXTURE_VIEWBOX: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48">
  <!-- generated icon shell -->
  <circle cx="24" cy="24" r="20"/>
</svg>"##;

/// All three fixtures in one document (the acceptance's "fixture set").
const FIXTURES: [&str; 3] = [FIXTURE_USE, FIXTURE_STYLE, FIXTURE_VIEWBOX];

/// An optimizer under test's control: what the fake returns and whether
/// it claims a distinct id (so the CAS key divergence pins have a second
/// optimizer to compare against).
struct FakeOptimizer {
    output: String,
    id: &'static str,
}

impl SvgOptimizer for FakeOptimizer {
    fn optimize(&self, _svg: &str) -> oxide_fw_assets::Result<String> {
        Ok(self.output.clone())
    }
    fn id(&self) -> &'static str {
        self.id
    }
    fn version(&self) -> &'static str {
        "9.9.9-fake"
    }
}

/// A fake that drops the `<use>` target id — the rule-violation shape the
/// enforcement must refuse with `AST-SVG-001`.
fn id_dropper() -> FakeOptimizer {
    FakeOptimizer {
        output: r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M0 0 L24 24 Z"/><use href="#a"/></svg>"##.to_string(),
        id: "fake-dropper",
    }
}

/// A fake that rewrites the `viewBox` value — the other refusal shape.
fn viewbox_changer() -> FakeOptimizer {
    FakeOptimizer {
        output: r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1 1"><path id="a" d="M0 0"/><use href="#a"/></svg>"##.to_string(),
        id: "fake-viewbox",
    }
}

/// A fake that emits bytes no XML parser accepts — the well-formedness rule.
fn malformed_emitter() -> FakeOptimizer {
    FakeOptimizer {
        output: "<svg><path</svg>".to_string(),
        id: "fake-broken",
    }
}

fn code_of(e: &OxideError) -> String {
    e.0.code.clone()
}

#[test]
fn identity_passthrough_returns_the_source_unchanged() {
    for src in FIXTURES {
        let out = IdentitySvg.optimize(src).expect("identity never fails");
        assert_eq!(out, src, "identity must not mutate the document");
    }
}

#[test]
fn referenced_svg_ids_finds_href_url_and_aria_sources() {
    let doc = r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink">
  <defs>
    <linearGradient id="grad"><stop stop-color="red"/></linearGradient>
    <path id="shape" d="M0 0"/>
    <text id="label" d="label"/>
    <text id="desc" d="desc"/>
  </defs>
  <use href="#shape"/>
  <use xlink:href="#shape" fill='url(#grad)'/>
  <rect fill="url(#grad)"/>
  <svg aria-labelledby="label desc"><title>ok</title></svg>
</svg>"##;
    let ids = referenced_svg_ids(doc);
    let want: BTreeSet<String> =
        BTreeSet::from(["shape", "grad", "label", "desc"].map(String::from));
    assert_eq!(
        ids, want,
        "href/xlink:href/url(#)/aria-labelledby all resolve"
    );
}

#[test]
fn referenced_svg_ids_handles_single_and_double_quotes() {
    let doc = r##"<svg><use href='#single'/><use href="#double"/></svg>"##;
    let ids = referenced_svg_ids(doc);
    let want: BTreeSet<String> = BTreeSet::from(["single", "double"].map(String::from));
    assert_eq!(ids, want);
}

#[test]
fn a_rule_violation_is_refused_with_ast_svg_001() {
    let fake = id_dropper();
    let out = fake.optimize(FIXTURE_USE).expect("fake succeeds");
    let err =
        assert_preserved(FIXTURE_USE, &out).expect_err("a dropped <use> target must be refused");
    assert_eq!(
        code_of(&err),
        "AST-SVG-001",
        "the rule refusal is AST-SVG-001"
    );
    assert!(
        err.0.message.contains("referenced id"),
        "the message names the rule (got: {})",
        err.0.message
    );
}

#[test]
fn a_viewbox_rewrite_is_refused_with_ast_svg_001() {
    let fake = viewbox_changer();
    let out = fake.optimize(FIXTURE_VIEWBOX).expect("fake succeeds");
    let err =
        assert_preserved(FIXTURE_VIEWBOX, &out).expect_err("a rewritten viewBox must be refused");
    assert_eq!(code_of(&err), "AST-SVG-001");
    assert!(
        err.0.message.contains("viewBox"),
        "the message names viewBox (got: {})",
        err.0.message
    );
}

#[test]
fn an_unparseable_output_is_refused_as_not_well_formed() {
    let fake = malformed_emitter();
    let out = fake.optimize(FIXTURE_USE).expect("fake succeeds");
    let err = assert_preserved(FIXTURE_USE, &out).expect_err("malformed output must be refused");
    assert_eq!(
        code_of(&err),
        "AST-SVG-001",
        "the well-formedness rule is AST-SVG-001"
    );
    assert!(
        err.0.message.contains("well-formed"),
        "the message names the rule (got: {})",
        err.0.message
    );
}

#[test]
fn a_preserved_output_passes_the_rules() {
    // The identity shape (a comment stripped, everything else kept) is
    // the minimal rewrite the rules accept.
    let optimized = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path id="a" d="M0 0 L24 24 Z"/><use href="#a"/></svg>"##;
    assert_preserved(FIXTURE_USE, optimized).expect("the rules hold");
}

#[test]
fn svg_cas_key_is_stable_content_sensitive_and_optimizer_scoped() {
    let a = svg_cas_key(&IdentitySvg, FIXTURE_USE);
    let b = svg_cas_key(&IdentitySvg, FIXTURE_USE);
    assert_eq!(a, b, "the same input and optimizer key identically");

    let other = svg_cas_key(&IdentitySvg, FIXTURE_STYLE);
    assert_ne!(a, other, "different content keys differently");

    let fake = FakeOptimizer {
        output: String::new(),
        id: "fake",
    };
    let fake_key = svg_cas_key(&fake, FIXTURE_USE);
    assert_ne!(a, fake_key, "the optimizer id is a key dimension (D40)");

    // The documented shape: `svg:{id}@{version}:{blake3-16hex}`.
    let (head, hex) = fake_key
        .rsplit_once(':')
        .expect("the key carries a final hash segment");
    assert_eq!(head, "svg:fake@9.9.9-fake");
    assert_eq!(hex.len(), 16, "the hash is the truncated blake3 (16 hex)");
    assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn select_optimizer_honors_the_config_flag_and_the_feature() {
    let enabled = select_optimizer(true);
    let disabled = select_optimizer(false);
    if cfg!(feature = "svg-oxvg") {
        let opt = enabled.expect("the feature is compiled in");
        assert_eq!(opt.id(), "oxvg", "svg.optimize: true selects OxvgSvg");
        let off = disabled.expect("identity is always available");
        assert_eq!(
            off.id(),
            "identity",
            "svg.optimize: false selects IdentitySvg"
        );
    } else {
        // `expect_err` would require `Box<dyn SvgOptimizer>: Debug`; the
        // destructure pins the refusal without that bound.
        let err = match enabled {
            Err(e) => e,
            Ok(_) => panic!("optimize without the feature must refuse (AST-SVG-002)"),
        };
        assert_eq!(code_of(&err), "AST-SVG-002");
        assert!(
            err.0.message.contains("svg-oxvg"),
            "the refusal names the feature flag (got: {})",
            err.0.message
        );
        let off = disabled.expect("identity is always available");
        assert_eq!(off.id(), "identity");
    }
}

#[test]
fn optimize_for_build_falls_back_to_the_source_on_any_refusal() {
    // With the knob off the source must pass through byte-identical —
    // the build never mutates documents the user did not opt in for.
    assert_eq!(optimize_for_build(FIXTURE_USE, false), FIXTURE_USE);

    // With the knob on, the happy path is the optimized bytes; every
    // refusal (feature missing, rule violation, optimizer error) keeps
    // the source and logs — the build stays unbroken either way.
    let out = optimize_for_build(FIXTURE_USE, true);
    if cfg!(feature = "svg-oxvg") {
        assert!(
            out.len() <= FIXTURE_USE.len(),
            "the optimized bytes are kept"
        );
        assert!(
            out.contains("viewBox"),
            "the output still satisfies the rules"
        );
    } else {
        assert_eq!(
            out, FIXTURE_USE,
            "no feature: the source is kept with a warn"
        );
    }
}

#[cfg(feature = "svg-oxvg")]
mod oxvg {
    //! The real-optimizer pins: the three fixtures the WP names, the
    //! acceptance shrink and the malformed-source refusal.

    use super::{FIXTURE_USE, FIXTURE_VIEWBOX, FIXTURES, code_of};
    use oxide_fw_assets::svg::{OxvgSvg, SvgOptimizer, assert_preserved};

    #[test]
    fn oxvg_preserves_the_named_rules_on_all_three_fixtures() {
        for src in FIXTURES {
            let out = OxvgSvg
                .optimize(src)
                .unwrap_or_else(|e| panic!("oxvg optimizes the fixture: {e}"));
            assert_preserved(src, &out)
                .unwrap_or_else(|e| panic!("the rules hold on the real output: {e}"));
            assert!(
                out.len() < src.len(),
                "the real optimizer shrinks the document ({} -> {})",
                src.len(),
                out.len()
            );
        }
    }

    #[test]
    fn the_use_target_survives_the_real_optimizer() {
        let out = OxvgSvg
            .optimize(FIXTURE_USE)
            .expect("oxvg optimizes the use fixture");
        assert!(
            out.contains("id=\"a\""),
            "the referenced id survives verbatim"
        );
        assert!(out.contains("href=\"#a\""), "the <use> reference survives");
    }

    #[test]
    fn the_viewbox_value_survives_the_real_optimizer() {
        let out = OxvgSvg
            .optimize(FIXTURE_VIEWBOX)
            .expect("oxvg optimizes the viewBox fixture");
        assert!(
            out.contains("viewBox=\"0 0 48 48\""),
            "the viewBox attribute survives with its value (got: {out})"
        );
    }

    #[test]
    fn the_fixture_set_shrinks_at_least_15_percent() {
        let total_in: usize = FIXTURES.iter().map(|s| s.len()).sum();
        let total_out: usize = FIXTURES
            .iter()
            .map(|s| OxvgSvg.optimize(s).expect("oxvg runs").len())
            .sum();
        assert!(
            total_out * 100 <= total_in * 85,
            "the acceptance: at least 15% total shrink ({} -> {} bytes)",
            total_in,
            total_out
        );
    }

    #[test]
    fn a_malformed_source_is_ast_svg_003() {
        let err = OxvgSvg
            .optimize("this is not xml at all")
            .expect_err("malformed source refuses");
        assert_eq!(code_of(&err), "AST-SVG-003");
        assert!(
            err.0.message.contains("parse"),
            "the message names the parse failure (got: {})",
            err.0.message
        );
    }

    #[test]
    fn the_oxvg_optimizer_is_pinned_to_a_version() {
        assert_eq!(OxvgSvg.id(), "oxvg");
        assert_eq!(
            OxvgSvg.version(),
            "0.0.9",
            "the CAS key pins the oxvg version"
        );
    }
}
