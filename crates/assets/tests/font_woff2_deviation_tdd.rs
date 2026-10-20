//! 16-B: the WOFF2 encoder is a documented deviation.
//!
//! No published crate provides a WOFF2 encoder (`woff2` / `woff2-sys` are not
//! on crates.io; `subsetter` emits TrueType). Hand-writing one means the
//! WOFF2 glyf/loca transform plus Brotli.
//!
//! These tests lock the *current* behaviour so a future encoder swap is a
//! deliberate, visible change. When an encoder lands, they fail and the
//! author updates the extension and the CSS hint together.

/// The subset path names its output `.ttf` (subsetter emits sfnt).
#[test]
fn subset_output_extension_is_ttf() {
    let src = include_str!("../src/lib.rs");
    assert!(
        src.contains(".subset.ttf"),
        "the subset output is renamed to .ttf; a switch to .woff2 without an \
         encoder would emit an undecodable file",
    );
}

/// The CSS `format()` hint is derived from the extension, and `ttf` maps to
/// `truetype` — never `woff2`.
#[test]
fn css_format_hint_matches_the_ttf_container() {
    let src = include_str!("../src/lib.rs");
    assert!(
        src.contains(r#"Some("ttf") => "truetype""#),
        "a .ttf file must carry format('truetype'); format('woff2') on a .ttf \
         makes the browser skip the face",
    );
}

/// The subset module documents that its output is TrueType.
#[test]
fn subset_module_documents_truetype_output() {
    let src = include_str!("../src/font_subset.rs");
    assert!(
        src.contains("truetype"),
        "the subset module must document the TrueType output deviation",
    );
}
