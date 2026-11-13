//! Criterion bench for the §V2-C08 glyph-usage font subsetting — the
//! per-build cost the font pipeline pays around the seam, and the real
//! subsetting pass the `used` strategy runs per font file.
//!
//! The shapes: the HTML harvest on a docs-page-sized document (the
//! `lol_html` walk the build pays per prerendered page when `fonts.subset`
//! carries `used`), the CAS key (the blake3 hash + the id/version prefix),
//! and the real [`SkrifaSubsetter`] pass on the committed subset fixture
//! with a latin-ish request set (the per-font CPU work). The knob-off path
//! is a field read — no bench shape for it.

use std::collections::BTreeSet;

use criterion::{Criterion, criterion_group, criterion_main};
use oxide_fw_assets::font_subset::{
    FontSubsetter, SkrifaSubsetter, font_cas_key, harvest_html_codepoints,
};

/// The committed subset fixture (the same bytes the TDD suite drives).
const FIXTURE: &[u8] = include_bytes!("../tests/fixtures/subset-fixture.ttf");

/// A docs-page-sized document: prose paragraphs over a small nav shell —
/// the realistic prerendered page the harvest walks.
fn docs_page() -> String {
    let mut page = String::from(
        "<!doctype html><html><head><style>.n{color:red}</style></head><body><nav>Docs / Guide</nav><main>",
    );
    for i in 0..120 {
        page.push_str(&format!(
            "<h2>Section {i}</h2><p>The renderer walks the layout chain and \
             collects every text node it renders, so the shipped font carries \
             exactly the glyphs the build proved it needs — no more.</p>"
        ));
    }
    page.push_str("<script>var boot=1;</script></main></body></html>");
    page
}

fn bench_font_subsetter(c: &mut Criterion) {
    let mut group = c.benchmark_group("font_subsetter");
    let page = docs_page();

    // The harvest walk: what every prerendered page pays when `used` is
    // configured (throughput in page bytes).
    group.throughput(criterion::Throughput::Bytes(page.len() as u64));
    group.bench_function("harvest_html_docs_page", |b| {
        b.iter(|| {
            let set = harvest_html_codepoints([page.as_str()]);
            assert!(!set.is_empty());
        });
    });

    // The CAS key: the blake3 digest + the id/version prefix.
    group.bench_function("cas_key_fixture", |b| {
        b.iter(|| {
            let key = font_cas_key(&SkrifaSubsetter, FIXTURE);
            assert!(key.starts_with("font:skrifa@"));
        });
    });

    // The real subsetting pass on the fixture: the per-font CPU work the
    // `used` strategy runs once per font file (cmap walk + subsetter).
    let chars: BTreeSet<char> =
        "abcdefghijklmnopqrstuvwxyz ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789.,!?:;()"
            .chars()
            .collect();
    group.throughput(criterion::Throughput::Bytes(FIXTURE.len() as u64));
    group.bench_function("skrifa_subset_latin_set", |b| {
        b.iter(|| {
            let out = SkrifaSubsetter
                .subset(FIXTURE, &chars)
                .expect("the fixture subsets");
            assert!(!out.is_empty());
        });
    });

    group.finish();
}

criterion_group!(benches, bench_font_subsetter);
criterion_main!(benches);
