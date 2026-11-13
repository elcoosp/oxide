//! Criterion bench for the §V2-C07 SVG optimizer — the per-import cost the
//! build pays around an SVG document, and the rule-enforcement overhead
//! every optimizer output carries.
//!
//! The shapes: the identity passthrough (the knob-off baseline — the cost
//! of the seam itself), the real `OxvgSvg` optimize on an icon-sized and a
//! banner-sized document (behind `svg-oxvg`), the referenced-id scan (the
//! rule pre-pass every output pays) and the CAS key (the blake3 hash + the
//! id/version prefix). The preservation `assert_preserved` walk is part of
//! the optimize shapes' measured cost.

use criterion::{Criterion, criterion_group, criterion_main};
use oxide_fw_assets::svg::{
    IdentitySvg, SvgOptimizer, assert_preserved, optimize_for_build, referenced_svg_ids,
    svg_cas_key,
};

/// An icon-sized document (the `<use>`-referencing fixture the WP names).
const ICON: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
  <!-- generated icon shell -->
  <defs><linearGradient id="grad"><stop stop-color="red"/><stop stop-color="blue"/></linearGradient></defs>
  <path id="a" d="M0 0 L24 24 Z" fill="url(#grad)"/>
  <use href="#a"/>
  <rect width="10" height="10" fill="url(#grad)"/>
</svg>"##;

/// A banner-sized document (repeated shapes — the realistic SVG asset the
/// docs site ships).
fn banner() -> String {
    let mut doc = String::from(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1200 400"><style>.lbl { font: 16px sans-serif; }</style>"#,
    );
    for i in 0..120 {
        doc.push_str(&format!(
            r#"<g id="cell{i}"><rect x="{x}" y="20" width="90" height="360" class="lbl"/><circle cx="{cx}" cy="200" r="40"/></g>"#,
            x = i * 10,
            cx = i * 10 + 45,
        ));
    }
    doc.push_str("</svg>");
    doc
}

fn bench_svg_optimizer(c: &mut Criterion) {
    let mut group = c.benchmark_group("svg_optimizer");
    group.throughput(criterion::Throughput::Bytes(ICON.len() as u64));

    // The knob-off baseline: the seam's own cost (byte-identical
    // passthrough).
    group.bench_function("identity_icon", |b| {
        b.iter(|| {
            let out = IdentitySvg.optimize(ICON).expect("identity never fails");
            assert_eq!(out, ICON);
        });
    });

    // The rule pre-pass on its own: the referenced-id scan + the
    // viewBox/id extraction the enforcement pays per optimize call.
    group.bench_function("referenced_ids_icon", |b| {
        b.iter(|| {
            let ids = referenced_svg_ids(ICON);
            assert!(!ids.is_empty());
        });
    });

    // The enforcement walk on a rule-clean output.
    let identity_out = IdentitySvg.optimize(ICON).expect("identity never fails");
    group.bench_function("assert_preserved_icon", |b| {
        b.iter(|| {
            assert_preserved(ICON, &identity_out).expect("identity satisfies the rules");
        });
    });

    // The CAS key: the blake3 digest + the id/version prefix.
    group.bench_function("cas_key_icon", |b| {
        b.iter(|| {
            let key = svg_cas_key(&IdentitySvg, ICON);
            assert!(key.starts_with("svg:identity@"));
        });
    });

    // The build hook with the knob on: the selection + optimize + rules.
    group.bench_function("optimize_for_build_icon", |b| {
        b.iter(|| {
            let out = optimize_for_build(ICON, true);
            let _ = &out;
        });
    });

    // The real OXVG backend on the icon-sized document, when compiled in.
    #[cfg(feature = "svg-oxvg")]
    group.bench_function("oxvg_icon", |b| {
        use oxide_fw_assets::svg::OxvgSvg;
        b.iter(|| {
            // A refusal in a bench is a timing datapoint, not a failure —
            // the correctness pins live in the TDD suite.
            if let Ok(out) = OxvgSvg.optimize(ICON) {
                let _ = &out;
            }
        });
    });

    // The real OXVG backend on the banner-sized document.
    let big = banner();
    #[cfg(feature = "svg-oxvg")]
    group.bench_function("oxvg_banner", |b| {
        use oxide_fw_assets::svg::OxvgSvg;
        b.iter(|| {
            if let Ok(out) = OxvgSvg.optimize(&big) {
                let _ = &out;
            }
        });
    });
    #[cfg(not(feature = "svg-oxvg"))]
    criterion::black_box(&big);

    group.finish();
}

criterion_group!(benches, bench_svg_optimizer);
criterion_main!(benches);
