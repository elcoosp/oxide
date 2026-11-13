//! TDD suite for the §V2-C08 glyph-usage font subsetting: the
//! [`FontSubsetter`] seam (`crates/assets/src/font_subset.rs`) — the spec
//! sketch's trait (font bytes + a harvested char set in, subset bytes
//! out, object-safe so a vendored backend drops in), the `SkrifaSubsetter`
//! (the existing `subsetter` backend standing in for `fonts-fontcull`,
//! behind the crate's `subset` feature), the always-needed set, the
//! post-prerender `lol_html` harvest (the `text!` handler over body the
//! spec names, with `<script>`/`<style>` contents skipped), the
//! content-hash CAS key (`font:{id}@{version}:{blake3-16hex}`, the D40
//! shape), the feature-aware selection (`select_subsetter`, the
//! `AST-SVG-002` shape) and the build-side warn-and-fallback helper
//! (`subset_used_for_build`).
//!
//! Written BEFORE the implementation; every pin imports public API only.
//! The subsetting pins drive the real backend on the committed
//! `fixtures/subset-fixture.ttf` (the spec's cmap pin: subset to
//! `{"a","b"}` and read the subset's cmap back with `skrifa`), so the
//! seam is proven on a real font, not only on fakes.

use std::collections::BTreeSet;

use oxide_fw_assets::font_subset::{
    FontSubsetter, SkrifaSubsetter, font_cas_key, harvest_html_codepoints, harvest_text_codepoints,
    select_subsetter, subset_used_for_build,
};
use oxide_fw_diagnostics::OxideError;

/// The committed subset fixture (§16.2): a real TrueType font carrying
/// the glyphs for `abcde`, the ASCII digits and the common punctuation —
/// small enough to commit, big enough that a `{"a","b"}` subset still
/// shrinks it.
const FIXTURE: &[u8] = include_bytes!("fixtures/subset-fixture.ttf");

fn chars(s: &str) -> BTreeSet<char> {
    s.chars().collect()
}

fn code_of(e: &OxideError) -> &str {
    &e.0.code
}

// --------------------------------------------------------------- the seam

/// A backend under test's control: returns canned bytes and claims its
/// own id/version — the D40 key dimensions the CAS key carries.
struct FakeSubsetter {
    out: Vec<u8>,
}

impl FontSubsetter for FakeSubsetter {
    fn subset(&self, _font: &[u8], _chars: &BTreeSet<char>) -> oxide_fw_assets::Result<Vec<u8>> {
        Ok(self.out.clone())
    }
    fn id(&self) -> &'static str {
        "fake"
    }
    fn version(&self) -> &'static str {
        "9.9.9"
    }
}

#[test]
fn the_subsetter_trait_is_object_safe_and_drives_fakes() {
    // The spec sketch's shape: `&dyn FontSubsetter` dispatches to any
    // backend — the injection point a vendored `fontcull` build uses.
    let fake = FakeSubsetter {
        out: b"subset".to_vec(),
    };
    let backend: &dyn FontSubsetter = &fake;
    let out = backend.subset(FIXTURE, &chars("ab")).expect("fake runs");
    assert_eq!(out, b"subset");
    assert_eq!(backend.id(), "fake");
    assert_eq!(backend.version(), "9.9.9");
}

#[test]
fn font_cas_key_is_content_plus_backend_dimensions() {
    // D40: the key binds the content hash to the backend that produced
    // the artifact — switching backends never reuses a stale subset.
    let fake = FakeSubsetter { out: vec![] };
    let key = font_cas_key(&fake, FIXTURE);
    assert!(
        key.starts_with("font:fake@9.9.9:"),
        "the key carries the backend id and version (got: {key})"
    );
    let hex = key.rsplit(':').next().expect("a hash segment");
    assert_eq!(hex.len(), 16, "the hash is the truncated blake3 (16 hex)");
    assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));

    // Same backend, different bytes → different key.
    let other = font_cas_key(&fake, b"other bytes");
    assert_ne!(key, other, "the content is a key dimension");

    // Same bytes, different backend → different key.
    struct OtherSubsetter;
    impl FontSubsetter for OtherSubsetter {
        fn subset(
            &self,
            _font: &[u8],
            _chars: &BTreeSet<char>,
        ) -> oxide_fw_assets::Result<Vec<u8>> {
            Ok(vec![])
        }
        fn id(&self) -> &'static str {
            "other"
        }
        fn version(&self) -> &'static str {
            "0.1.0"
        }
    }
    let different_backend = font_cas_key(&OtherSubsetter, FIXTURE);
    assert_ne!(key, different_backend, "the backend is a key dimension");
}

// ------------------------------------------------------------- the harvest

/// A page whose visible text and its script/style internals are disjoint:
/// the harvest keeps the visible chars and never sees the internals'
/// (`K`/`Z`/`q` appear only inside `<style>`/`<script>` — deliberately
/// chars the always-needed punctuation set does not carry).
const PAGE: &str = r#"<!doctype html>
<html><head>
  <style>.Kq { color: red; }</style>
  <script>var ZZqq = 1;</script>
</head>
<body>
  <h1>Héllo ☃</h1>
  <p>A sunny day.</p>
</body></html>"#;

#[test]
fn harvest_html_codepoints_extracts_body_text_and_skips_script_and_style() {
    let cps = harvest_html_codepoints([PAGE]);
    // The visible text survives.
    for c in "Hélo ☃A suny d.".chars() {
        assert!(cps.contains(&c), "visible `{c}` harvested");
    }
    // The `<script>`/`<style>` internals never leak into the set — a
    // minified bundle's identifier soup must not grow the shipped font.
    assert!(!cps.contains(&'Z'), "script text skipped");
    assert!(!cps.contains(&'K'), "style text skipped");
    assert!(!cps.contains(&'q'), "script and style text skipped");
}

#[test]
fn harvest_html_codepoints_includes_the_always_needed_set() {
    // §V2-C08 step 2: the always-needed set (ASCII digits, space, common
    // punctuation) rides EVERY harvest — a page that renders "42." must
    // not ship a font without the glyphs for "42.".
    let cps = harvest_html_codepoints(["<body>plain</body>"]);
    for c in "0123456789 .,!?:;()&".chars() {
        assert!(cps.contains(&c), "always-needed `{c}` present");
    }
}

#[test]
fn harvest_html_codepoints_unions_pages_deterministically() {
    let a = harvest_html_codepoints(["<body>alpha</body>"]);
    let b = harvest_html_codepoints(["<body>beta</body>"]);
    let both = harvest_html_codepoints(["<body>alpha</body>", "<body>beta</body>"]);
    for c in a.iter().chain(b.iter()) {
        assert!(both.contains(c), "the union keeps `{c}`");
    }
    // BTreeSet: two runs over the same pages are equal.
    assert_eq!(
        both,
        harvest_html_codepoints(["<body>beta</body>", "<body>alpha</body>"]),
        "page order does not change the set"
    );
}

#[test]
fn harvest_text_codepoints_harvests_plain_sources() {
    // The content-collection field text (§V2-C08 step 2): plain authored
    // text (markdown body, frontmatter strings) — no HTML parsing.
    let cps = harvest_text_codepoints(["Émile Zola — 1865"]);
    for c in "Émile Zol 1865—".chars() {
        assert!(cps.contains(&c), "`{c}` harvested");
    }
    assert!(cps.contains(&' '), "space is in the always-needed set");
}

// ------------------------------------------------------------ the backend

#[cfg(feature = "subset")]
#[test]
fn skrifa_subsetter_maps_only_the_requested_chars() {
    // The spec's named pin: subset a fixture font with `{"a","b"}` and
    // check via skrifa that the cmap contains those two and not `c`.
    let subset = SkrifaSubsetter
        .subset(FIXTURE, &chars("ab"))
        .expect("the fixture subsets");
    assert!(
        subset.len() < FIXTURE.len(),
        "subset ({} B) must be smaller than the fixture ({} B)",
        subset.len(),
        FIXTURE.len()
    );
    assert!(maps(&subset, 'a'), "'a' must map in the subset cmap");
    assert!(maps(&subset, 'b'), "'b' must map in the subset cmap");
    assert!(!maps(&subset, 'c'), "'c' must NOT map in the subset cmap");
    // The mapping is not merely present — it points at the same glyph
    // the original font used (a wrong-gid cmap would render the wrong
    // shape for a mapped char).
    use skrifa::MetadataProvider as _;
    let original = skrifa::FontRef::new(FIXTURE).expect("fixture parses");
    let subset_font = skrifa::FontRef::new(&subset).expect("subset parses");
    for c in ['a', 'b'] {
        assert_eq!(
            subset_font.charmap().map(c),
            original.charmap().map(c),
            "'{c}' maps to the same glyph id as the original"
        );
    }
}

#[cfg(feature = "subset")]
#[test]
fn skrifa_subsetter_errors_cleanly_on_an_unparseable_font() {
    let e = SkrifaSubsetter
        .subset(b"not a font", &chars("a"))
        .expect_err("a non-font must be refused");
    assert_eq!(code_of(&e), "AST-FONT");
}

/// Read the subset's charmap back with skrifa (the spec's verification
/// half of the pin). `skrifa` is an implementation dependency of the
/// backend; the pin re-derives the mapping from the emitted bytes.
#[cfg(feature = "subset")]
fn maps(font_bytes: &[u8], c: char) -> bool {
    use skrifa::MetadataProvider as _;
    let font = skrifa::FontRef::new(font_bytes).expect("subset output parses as a font");
    font.charmap().map(c).is_some()
}

// ----------------------------------------------------------- the selection

#[test]
fn select_subsetter_honors_the_feature() {
    let selected = select_subsetter();
    if cfg!(feature = "subset") {
        let backend = selected.expect("the feature is compiled in");
        assert_eq!(
            backend.id(),
            "skrifa",
            "the existing backend is the default"
        );
    } else {
        // `expect_err` would require `Box<dyn FontSubsetter>: Debug`; the
        // destructure pins the refusal without that bound.
        let err = match selected {
            Err(e) => e,
            Ok(_) => panic!("subsetting without the feature must refuse (AST-FNT-002)"),
        };
        assert_eq!(code_of(&err), "AST-FNT-002");
        assert!(
            err.0.message.contains("subset"),
            "the refusal names the feature flag (got: {})",
            err.0.message
        );
    }
}

// ----------------------------------------------------------- the build hook

#[test]
fn subset_used_for_build_passes_the_source_through_when_the_knob_is_off() {
    // Knob off: the build never mutates a font the user did not opt in
    // for — the byte-identical contract the bundler relies on.
    assert!(subset_used_for_build(FIXTURE, &chars("ab"), false).is_none());
}

#[cfg(feature = "subset")]
#[test]
fn subset_used_for_build_subsets_when_the_knob_is_on() {
    let Some(subset) = subset_used_for_build(FIXTURE, &chars("ab"), true) else {
        panic!("the fixture subset must be smaller than the source");
    };
    assert!(
        subset.len() < FIXTURE.len(),
        "subset ({} B) < source ({} B)",
        subset.len(),
        FIXTURE.len()
    );
    // The emitted bytes still parse, and only the requested glyphs ride.
    assert!(maps(&subset, 'a') && maps(&subset, 'b'));
    assert!(!maps(&subset, 'c'));
}

#[cfg(feature = "subset")]
#[test]
fn subset_used_for_build_falls_back_to_the_source_on_a_backend_failure() {
    // A font the backend cannot parse is a warn-and-fallback: the font
    // pipeline is never load-bearing for a build (the §16.2 convention),
    // so the source ships verbatim instead of failing the build.
    assert!(subset_used_for_build(b"not a font", &chars("a"), true).is_none());
}

#[cfg(not(feature = "subset"))]
#[test]
fn subset_used_for_build_falls_back_without_the_feature() {
    // Feature-off: the refusal (AST-FNT-002) is the honest surface — the
    // build hook degrades to the source and logs, it never pretends.
    assert!(subset_used_for_build(FIXTURE, &chars("ab"), true).is_none());
}

// ------------------------------------------------- the FontSpec wire shape

#[cfg(feature = "subset")]
mod bundle_wire {
    //! The `DefaultFontBackend` wire-up (§V2-C08 step 4): the `used`
    //! strategy subsets through the seam with the harvested set, and the
    //! static-list spellings (`latin`, `unicode-range`) ship the source
    //! verbatim — the marker-string harvest the static list used before
    //! this seam is gone.

    use std::collections::BTreeSet;

    use oxide_fw_assets::{DefaultFontBackend, FontBackend as _, FontSpec};

    use super::{FIXTURE, chars};

    fn used_spec(dir: &std::path::Path, chars: BTreeSet<char>) -> FontSpec {
        let p = dir.join("fixture.ttf");
        std::fs::write(&p, FIXTURE).unwrap();
        FontSpec {
            family: "Fixture".into(),
            provider: "local".into(),
            weights: vec![400],
            styles: vec!["normal".into()],
            subset: vec!["used".into()],
            local_paths: vec![p],
            used_chars: chars,
        }
    }

    #[test]
    fn used_strategy_subsets_through_the_seam() {
        let dir = tempfile::tempdir().unwrap();
        let spec = used_spec(dir.path(), chars("ab"));
        let bundle = DefaultFontBackend.bundle(&spec).unwrap();
        assert_eq!(bundle.files.len(), 1);
        let (name, bytes) = &bundle.files[0];
        assert!(
            name.ends_with(".subset.ttf"),
            "the subset output keeps the .ttf container name (got: {name})"
        );
        assert!(bytes.len() < FIXTURE.len(), "the subset shrinks the source");
        // The emitted file carries exactly the requested glyphs — and
        // the same glyph ids the original used (a wrong-gid cmap would
        // render the wrong shape for a mapped char).
        use skrifa::MetadataProvider as _;
        let subset_font = skrifa::FontRef::new(bytes).expect("subset parses");
        let original = skrifa::FontRef::new(FIXTURE).expect("fixture parses");
        let map = subset_font.charmap();
        assert!(map.map('a').is_some() && map.map('b').is_some());
        assert!(map.map('c').is_none(), "unharvested glyph dropped");
        for c in ['a', 'b'] {
            assert_eq!(
                map.map(c),
                original.charmap().map(c),
                "'{c}' keeps its glyph id"
            );
        }
    }

    #[test]
    fn used_strategy_with_an_empty_harvest_still_ships_the_always_needed_set() {
        // A static project whose pages carry no text (or a harvest that
        // found nothing) still subsets — to the always-needed set, so a
        // page that renders "42." never ships a font without "42.".
        let dir = tempfile::tempdir().unwrap();
        let spec = used_spec(dir.path(), BTreeSet::new());
        let bundle = DefaultFontBackend.bundle(&spec).unwrap();
        let (_, bytes) = &bundle.files[0];
        use skrifa::MetadataProvider as _;
        let font = skrifa::FontRef::new(bytes).expect("subset parses");
        let map = font.charmap();
        assert!(map.map('4').is_some(), "digits ride the always-needed set");
        assert!(map.map('c').is_none(), "unharvested glyph dropped");
    }

    #[test]
    fn static_list_spellings_ship_the_source_verbatim() {
        // `latin` / `unicode-range` are browser-side or range-declaration
        // strategies — they are not glyph-usage sources. The old
        // marker-string harvest (which subsetted to the glyphs of the
        // literal string `"latin"`) is retired: the source ships
        // byte-identical unless the project opts into `used`.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("fixture.ttf");
        std::fs::write(&p, FIXTURE).unwrap();
        let spec = FontSpec {
            family: "Fixture".into(),
            provider: "local".into(),
            weights: vec![400],
            styles: vec!["normal".into()],
            subset: vec!["latin".into(), "unicode-range".into()],
            local_paths: vec![p],
            used_chars: BTreeSet::new(),
        };
        let bundle = DefaultFontBackend.bundle(&spec).unwrap();
        let (name, bytes) = &bundle.files[0];
        assert_eq!(bytes, FIXTURE, "no `used` opt-in — verbatim copy");
        assert!(
            !name.contains(".subset."),
            "the name keeps the original stem"
        );
    }
}
