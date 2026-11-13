//! The glyph-usage font-subsetting walkthrough (§V2-C08): the
//! [`FontSubsetter`] seam, the post-prerender harvest (the `text!`-over-body
//! walk, script/style stripped), the always-needed set, the CAS key and the
//! build-side `used` strategy — the whole thing the font bundler wires
//! against the real [`SkrifaSubsetter`] backend and fakes, so the contract
//! is readable without the bundler.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-assets --example font_subsetter --features subset
//! ```

use std::collections::BTreeSet;

use oxide_fw_assets::font_subset::{
    ALWAYS_NEEDED_CHARS, FontSubsetter, SkrifaSubsetter, font_cas_key, harvest_html_codepoints,
    harvest_text_codepoints, select_subsetter, subset_used_for_build,
};

/// The committed subset fixture (the same bytes the TDD suite drives): a
/// real TrueType font carrying `abcde`, the digits and the punctuation.
const FIXTURE: &[u8] = include_bytes!("../tests/fixtures/subset-fixture.ttf");

/// A page whose visible text and its script internals are disjoint — the
/// harvest keeps the visible chars and never sees the internals'.
const PAGE: &str = r#"<!doctype html>
<html><head>
  <style>.hq { color: red; }</style>
  <script>var ZZqq = 1;</script>
</head>
<body>
  <h1>Héllo ☃</h1>
  <p>A sunny day.</p>
</body></html>"#;

/// A third-party backend shape: returns the font unchanged, but with its
/// own id/version — the D40 key dimensions the CAS key carries. A
/// published `fontcull` drops in exactly like this (the module header
/// records the stand-in deviation).
struct VendorSubsetter;

impl FontSubsetter for VendorSubsetter {
    fn subset(&self, font: &[u8], _chars: &BTreeSet<char>) -> oxide_fw_assets::Result<Vec<u8>> {
        Ok(font.to_vec())
    }
    fn id(&self) -> &'static str {
        "vendor"
    }
    fn version(&self) -> &'static str {
        "1.2.3"
    }
}

fn main() {
    println!("== the backends ==");
    let selected = select_subsetter().expect("the `subset` feature is compiled into this run");
    println!("  select_subsetter() -> id={}", selected.id());
    for (name, backend) in [
        (
            "skrifa",
            Box::new(SkrifaSubsetter) as Box<dyn FontSubsetter>,
        ),
        (
            "vendor",
            Box::new(VendorSubsetter) as Box<dyn FontSubsetter>,
        ),
    ] {
        println!(
            "  {name}: id={} version={}",
            backend.id(),
            backend.version()
        );
    }
    drop(selected);
    println!("\n== the post-prerender harvest (lol_html text! over body) ==");
    let harvested = harvest_html_codepoints([PAGE]);
    let visible: String = "Hélo ☃A suny d.".chars().collect();
    for c in visible.chars() {
        assert!(harvested.contains(&c), "visible `{c}` harvested");
    }
    assert!(!harvested.contains(&'Z'), "script text skipped");
    assert!(!harvested.contains(&'q'), "script text skipped");
    println!(
        "  {} chars harvested, script/style internals excluded",
        harvested.len()
    );

    println!("\n== the content-collection field text (plain sources) ==");
    let fields = harvest_text_codepoints(["Émile Zola — 1865"]);
    assert!(fields.contains(&'É') && fields.contains(&'—'));
    println!(
        "  {} chars from the collection fields (always-needed included)",
        fields.len()
    );

    println!("\n== the always-needed set (§V2-C08 step 2) ==");
    for c in ALWAYS_NEEDED_CHARS.chars().take(10) {
        assert!(harvested.contains(&c));
    }
    println!("  digits, space and common punctuation ride every harvest");

    println!("\n== the real subsetting pass (the spec's cmap pin) ==");
    let requested: BTreeSet<char> = "ab".chars().collect();
    let subset = SkrifaSubsetter
        .subset(FIXTURE, &requested)
        .expect("the fixture subsets");
    println!(
        "  {} bytes -> {} bytes with {{a,b}} requested",
        FIXTURE.len(),
        subset.len()
    );
    // The spec's verification half: read the subset's cmap back with
    // skrifa — `a` and `b` map, `c` does not.
    use skrifa::MetadataProvider as _;
    let font = skrifa::FontRef::new(&subset).expect("the subset parses as a font");
    let map = font.charmap();
    assert!(map.map('a').is_some() && map.map('b').is_some());
    assert!(map.map('c').is_none(), "unharvested glyph dropped");
    println!("  cmap: 'a' and 'b' map, 'c' does not");

    println!("\n== the build hook (subset_used_for_build) ==");
    let hooked = subset_used_for_build(FIXTURE, &requested, true)
        .expect("the fixture subset shrinks the source");
    println!(
        "  knob on : {} bytes (subset, always-needed unioned)",
        hooked.len()
    );
    let off = subset_used_for_build(FIXTURE, &requested, false);
    assert!(off.is_none(), "knob off never touches the font");
    println!("  knob off: None (byte-identical source)");

    println!("\n== the CAS key (D40: content + backend id + version) ==");
    let key_skrifa = font_cas_key(&SkrifaSubsetter, FIXTURE);
    let key_vendor = font_cas_key(&VendorSubsetter, FIXTURE);
    println!("  skrifa : {key_skrifa}");
    println!("  vendor : {key_vendor}");
    assert_ne!(key_skrifa, key_vendor, "the backend is a key dimension");

    println!("\nwalkthrough complete");
}
