//! TDD test suite for oxide-assets (spec §6.6–6.7, §16, D5, D19).
//!
//! Every public function in the crate has at least one test here (CONVENTIONS.md #1).

use super::*;
#[allow(unused_imports)]
use crate::font_metrics::{metric_overrides, read_metrics, FontMetric, TIMES_NEW_ROMAN};
use std::io::Write as _;

// ---------------- parse_asset_query (§6.7) ----------------

#[test]
fn asset_query_parses_image_queries() {
    let q = parse_asset_query("./hero.png?w=640&format=avif").unwrap();
    assert_eq!(q.path, "./hero.png");
    assert_eq!(q.query.get("w").unwrap(), "640");
    assert_eq!(q.query.get("format").unwrap(), "avif");
}

#[test]
fn asset_query_flag_queries_have_empty_values() {
    for spec in [
        "./logo.svg?url",
        "./data.txt?raw",
        "./inline.svg?inline",
        "./mod.ts?worker",
    ] {
        let q = parse_asset_query(spec).unwrap();
        let key = q.path.rsplit('.').next().unwrap();
        let _ = key;
        let flag = q.query.iter().next().unwrap();
        assert_eq!(flag.1, "", "flag query `{spec}` maps to empty value");
    }
    assert_eq!(
        parse_asset_query("./logo.svg?url")
            .unwrap()
            .query
            .get("url")
            .unwrap(),
        ""
    );
}

#[test]
fn asset_query_without_query_is_empty_map() {
    let q = parse_asset_query("./plain.png").unwrap();
    assert_eq!(q.path, "./plain.png");
    assert!(q.query.is_empty());
}

#[test]
fn asset_query_escaped_question_mark_stays_in_path() {
    // Documented: `\?` is a literal `?` inside the path; the first *unescaped* `?`
    // starts the query.
    let q = parse_asset_query("./weird\\?name.png?w=32").unwrap();
    assert_eq!(q.path, "./weird?name.png");
    assert_eq!(q.query.get("w").unwrap(), "32");
}

#[test]
fn asset_query_rejects_empty_and_leading_question_mark() {
    for bad in ["", "?w=640"] {
        let e = parse_asset_query(bad).unwrap_err();
        assert_eq!(e.0.code, "AST-QUERY", "specifier `{bad}` must be AST-QUERY");
    }
}

// ---------------- DefaultCss (§6.6, D5) ----------------

fn css(code: &str, kind: CssKind, minify: bool) -> CssOutput {
    DefaultCss
        .transform(&CssInput {
            code: code.into(),
            file: None,
            kind,
            minify,
            targets: vec![],
        })
        .unwrap()
}

#[test]
fn css_minify_strips_comments_and_collapses_whitespace() {
    let out = css(
        "/* header */\n.a  {\n  color :  red ;  /* inner */ }\n",
        CssKind::Global,
        true,
    );
    assert_eq!(out.code, ".a{color:red}");
}

#[test]
fn css_minify_keeps_at_rules_drops_empty_rules_and_final_semicolons() {
    let out = css(
        "@media (min-width: 100px) { .a { color: red; } }\n.b { }\n.c { color: blue; margin: 0; }",
        CssKind::Global,
        true,
    );
    assert_eq!(
        out.code,
        "@media (min-width:100px){.a{color:red}}.c{color:blue;margin:0}"
    );
}

#[test]
fn css_minify_compresses_child_combinator() {
    let out = css(".a > .b { color: red; }", CssKind::Global, true);
    assert_eq!(out.code, ".a>.b{color:red}");
}

#[test]
fn css_minify_leaves_strings_alone() {
    let out = css(
        ".a::before { content: \"a  b : c ; }\"; }",
        CssKind::Global,
        true,
    );
    assert!(
        out.code.contains("\"a  b : c ; }\""),
        "string content preserved: {}",
        out.code
    );
}

#[test]
fn css_modules_scope_class_names_deterministically() {
    let file = "src/app.css";
    let scoped = |name: &str| {
        let hex = blake3::hash(format!("{file}{name}").as_bytes())
            .to_hex()
            .to_string();
        format!("ox_{}_{name}", &hex[..6])
    };
    let out = DefaultCss
        .transform(&CssInput {
            code: ".foo { color: red; } .foo.bar { color: blue; }".into(),
            file: Some(file.into()),
            kind: CssKind::Module,
            minify: true,
            targets: vec![],
        })
        .unwrap();
    let foo = scoped("foo");
    let bar = scoped("bar");
    assert_eq!(out.exports.get("foo").unwrap(), &foo);
    assert_eq!(out.exports.get("bar").unwrap(), &bar);
    assert_eq!(
        out.code,
        format!(".{foo}{{color:red}}.{foo}.{bar}{{color:blue}}")
    );
    assert!(!out.code.contains(".foo"), "local class name must not leak");
}

#[test]
fn css_modules_scope_keyframes_and_animation_references() {
    let file = "src/spin.css";
    let hex = blake3::hash(format!("{file}spin").as_bytes())
        .to_hex()
        .to_string();
    let scoped = format!("ox_{}_spin", &hex[..6]);
    let out = DefaultCss
        .transform(&CssInput {
            code: "@keyframes spin { from { opacity: 0 } to { opacity: 1 } } .a { animation: spin 1s linear; }"
                .into(),
            file: Some(file.into()),
            kind: CssKind::Module,
            minify: true,
            targets: vec![],
        })
        .unwrap();
    assert!(
        out.code.contains(&format!("@keyframes {scoped}")),
        "{}",
        out.code
    );
    assert!(
        out.code.contains(&format!("animation:{scoped} 1s linear")),
        "{}",
        out.code
    );
}

#[test]
fn css_module_leaves_urls_untouched() {
    let out = DefaultCss
        .transform(&CssInput {
            code: ".a { background: url(./img.png); }".into(),
            file: Some("x.css".into()),
            kind: CssKind::Module,
            minify: true,
            targets: vec![],
        })
        .unwrap();
    assert!(out.code.contains("url(./img.png)"), "{}", out.code);
}

#[test]
fn css_global_kind_is_untouched_with_empty_exports() {
    let out = css(".foo { color: red; }", CssKind::Global, false);
    assert!(out.code.contains(".foo"));
    assert!(out.exports.is_empty());
}

#[test]
fn css_nesting_flattens_single_and_nested_levels() {
    let out = css(
        ".a { color: red; &.b { color: blue; } & .c { color: green; } }",
        CssKind::Global,
        true,
    );
    assert_eq!(out.code, ".a{color:red}.a.b{color:blue}.a .c{color:green}");
}

#[test]
fn css_nesting_is_recursive() {
    let out = css(".a { &.b { &.c { color: red; } } }", CssKind::Global, true);
    assert_eq!(out.code, ".a.b.c{color:red}");
}

#[test]
fn css_nesting_inside_media_stays_in_media_block() {
    let out = css(
        ".a { color: red; @media (min-width: 100px) { color: blue; } }",
        CssKind::Global,
        true,
    );
    assert_eq!(
        out.code,
        ".a{color:red}@media (min-width:100px){.a{color:blue}}"
    );
}

#[test]
fn css_autoprefixes_known_properties_always() {
    let out = css(
        ".a { transform: translateX(1px); user-select: none; appearance: none; backdrop-filter: blur(2px); }",
        CssKind::Global,
        true,
    );
    for want in [
        "-webkit-transform:translateX(1px)",
        "-ms-transform:translateX(1px)",
        "transform:translateX(1px)",
        "-webkit-user-select:none",
        "-moz-user-select:none",
        "user-select:none",
        "-webkit-appearance:none",
        "appearance:none",
        "-webkit-backdrop-filter:blur(2px)",
        "backdrop-filter:blur(2px)",
    ] {
        assert!(out.code.contains(want), "missing `{want}` in {}", out.code);
    }
}

#[test]
fn css_targets_are_accepted_but_not_yet_influential() {
    // Documented: browserslist-style targets parse/are stored but the simplified
    // prefixer always emits the same prefix set (lightningcss swap pending, D5).
    let out = DefaultCss
        .transform(&CssInput {
            code: ".a { transform: scale(2); }".into(),
            file: None,
            kind: CssKind::Global,
            minify: true,
            targets: vec!["chrome 100".into(), "safari 15".into()],
        })
        .unwrap();
    assert!(out.code.contains("-webkit-transform:scale(2)"));
}

#[test]
fn css_invalid_input_reports_ast_css_with_line() {
    for (bad, line) in [("{ color: red;", 1u32), (".a { color: red;\n}}", 2u32)] {
        let e = DefaultCss
            .transform(&CssInput {
                code: bad.into(),
                file: Some("bad.css".into()),
                kind: CssKind::Global,
                minify: false,
                targets: vec![],
            })
            .unwrap_err();
        assert_eq!(e.0.code, "AST-CSS");
        let loc = e.0.location.expect("line info is cheap, include it");
        assert!(loc.line >= line, "expected line >= {line}");
    }
}

// ---------------- scan_tailwind_candidates (§6.6) ----------------

#[test]
fn tailwind_scan_collects_class_tokens_sorted_and_deduped() {
    let html =
        r#"<div class="flex items-center p-4  p-4"><span class="hidden md:flex"></span></div>"#;
    let tsx = r#"export const A = () => <div className="grid gap-2 md:flex">hi</div>;"#;
    let astro = r#"<div class:list="'w-full hidden'"></div>"#;
    let vue = r#"<div :class="'shrink-0'"></div>"#;
    let cands = scan_tailwind_candidates(&[html, tsx, astro, vue]);
    assert_eq!(
        cands,
        vec![
            "flex",
            "gap-2",
            "grid",
            "hidden",
            "items-center",
            "md:flex",
            "p-4",
            "shrink-0",
            "w-full"
        ]
    );
}

#[test]
fn tailwind_scan_ignores_non_attribute_text() {
    // Only attribute values count — utility-looking words in code/text are not candidates.
    let src = "function flex() { return 'items-center'; }";
    assert!(scan_tailwind_candidates(&[src]).is_empty());
}

// ---------------- Images (§16.1, D19) ----------------

/// A deterministic 100x50 gradient PNG built in-memory with the image crate.
fn gradient_png(w: u32, h: u32) -> Vec<u8> {
    let img = image::DynamicImage::ImageRgba8(image::ImageBuffer::from_fn(w, h, |x, y| {
        image::Rgba([
            (x * 255 / w.max(1)) as u8,
            (y * 255 / h.max(1)) as u8,
            128,
            255,
        ])
    }));
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
    buf.into_inner()
}

fn req(
    source: Vec<u8>,
    width: Option<u32>,
    format: ImageFormat,
    ph: Option<PlaceholderKind>,
) -> ImageRequest {
    ImageRequest {
        source,
        source_format: ImageFormat::Png,
        width,
        height: None,
        format,
        quality: 80,
        placeholder: ph,
    }
}

#[test]
fn image_resize_preserves_aspect_ratio_and_png_roundtrips() {
    let out = RustImageBackend
        .process(req(gradient_png(100, 50), Some(40), ImageFormat::Png, None))
        .unwrap();
    assert_eq!((out.width, out.height), (40, 20));
    let decoded = image::load_from_memory(&out.bytes).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (40, 20));
    assert_eq!(out.format, ImageFormat::Png);
}

#[test]
fn image_jpeg_encode_produces_bytes_and_clamps_quality() {
    for q in [0u8, 80, 250] {
        let mut r = req(gradient_png(32, 16), None, ImageFormat::Jpeg, None);
        r.quality = q;
        let out = RustImageBackend.process(r).unwrap();
        assert_eq!(out.format, ImageFormat::Jpeg);
        assert_eq!(&out.bytes[..2], &[0xFF, 0xD8], "JPEG SOI marker");
        assert!(!out.bytes.is_empty());
    }
}

#[test]
fn image_webp_succeeds_or_falls_back_to_png_with_ast_webp_diagnostic() {
    let out = RustImageBackend
        .process(req(gradient_png(32, 16), None, ImageFormat::Webp, None))
        .unwrap();
    match out.format {
        ImageFormat::Webp => assert!(out.diagnostics.is_empty()),
        ImageFormat::Png => {
            assert!(
                out.diagnostics.iter().any(|d| d.code == "AST-WEBP"),
                "fallback must be diagnosed"
            );
        }
        other => panic!("unexpected format {other:?}"),
    }
}

#[test]
fn image_avif_encoding_produces_real_avif_bytes() {
    let out = RustImageBackend
        .process(req(gradient_png(32, 16), None, ImageFormat::Avif, None))
        .unwrap();
    assert_eq!(out.format, ImageFormat::Avif);
    assert!(!out.bytes.is_empty());
    // AVIF is an ISO-BMFF format: bytes 4..8 are the "ftyp" box type.
    assert_eq!(
        &out.bytes[4..8],
        b"ftyp",
        "AVIF output must be an ISO-BMFF container"
    );
    assert_eq!((out.width, out.height), (32, 16));
}

#[test]
fn image_lqip_placeholder_is_a_base64_jpeg_data_uri() {
    for kind in [PlaceholderKind::Lqip, PlaceholderKind::Blur] {
        let out = RustImageBackend
            .process(req(
                gradient_png(64, 32),
                None,
                ImageFormat::Png,
                Some(kind),
            ))
            .unwrap();
        let ph = out.placeholder.expect("placeholder requested");
        assert!(ph.starts_with("data:image/jpeg;base64,"), "{ph}");
    }
}

#[test]
fn image_cache_key_is_stable_and_param_sensitive() {
    let a = RustImageBackend
        .process(req(gradient_png(32, 16), None, ImageFormat::Png, None))
        .unwrap();
    let b = RustImageBackend
        .process(req(gradient_png(32, 16), None, ImageFormat::Png, None))
        .unwrap();
    assert_eq!(a.cache_key, b.cache_key);
    assert_eq!(a.cache_key.len(), 16);
    let c = RustImageBackend
        .process(req(gradient_png(32, 16), Some(16), ImageFormat::Png, None))
        .unwrap();
    assert_ne!(a.cache_key, c.cache_key, "width must change the key");
    let d = RustImageBackend
        .process(req(gradient_png(48, 16), None, ImageFormat::Png, None))
        .unwrap();
    assert_ne!(a.cache_key, d.cache_key, "source bytes must change the key");
}

#[test]
fn image_decode_failure_is_an_ast_img_error() {
    let e = RustImageBackend
        .process(req(
            vec![0xDE, 0xAD, 0xBE, 0xEF],
            None,
            ImageFormat::Png,
            None,
        ))
        .unwrap_err();
    assert_eq!(e.0.code, "AST-IMG");
}

// ---------------- srcset + signed URLs (§16.1) ----------------

#[test]
fn srcset_lists_widths_ascending() {
    let s = srcset(&[1024, 640], "/_oxide/image/hero.png");
    assert_eq!(
        s,
        "/_oxide/image/hero.png?w=640 640w, /_oxide/image/hero.png?w=1024 1024w"
    );
    assert_eq!(srcset(&[], "/x.png"), "");
}

#[test]
fn sign_image_url_appends_s_param() {
    let with_q = sign_image_url("/i.png?w=640", "secret");
    let without_q = sign_image_url("/i.png", "secret");
    assert!(with_q.starts_with("/i.png?w=640&s="));
    assert!(without_q.starts_with("/i.png?s="));
    let sig = &with_q["/i.png?w=640&s=".len()..];
    assert_eq!(sig.len(), 16);
    assert!(sig.chars().all(|c| c.is_ascii_hexdigit()));
    // deterministic, secret-sensitive
    assert_eq!(with_q, sign_image_url("/i.png?w=640", "secret"));
    assert_ne!(with_q, sign_image_url("/i.png?w=640", "other"));
}

// ---------------- Fonts (§16.2) ----------------

fn font_spec(dir: &std::path::Path) -> FontSpec {
    let p = dir.join("inter.woff2");
    std::fs::File::create(&p)
        .unwrap()
        .write_all(b"wOF2-fake-bytes")
        .unwrap();
    FontSpec {
        family: "Inter".into(),
        provider: "local".into(),
        weights: vec![400, 700],
        styles: vec!["normal".into()],
        subset: vec![],
        local_paths: vec![p],
    }
}

#[test]
fn font_bundle_emits_faces_preload_and_size_adjust_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let spec = font_spec(dir.path());
    let b = DefaultFontBackend.bundle(&spec).unwrap();

    assert_eq!(b.files.len(), 1);
    assert_eq!(b.files[0].0, "inter.woff2");
    assert_eq!(b.files[0].1, b"wOF2-fake-bytes");

    assert_eq!(
        b.css.matches("@font-face").count(),
        3,
        "two weights + one fallback face"
    );
    assert!(b.css.contains("font-family: 'Inter';"));
    assert!(b
        .css
        .contains("src: url(/fonts/inter.woff2) format('woff2');"));
    assert!(b.css.contains("font-display: swap;"));
    assert!(b.css.contains("font-weight: 400;"));
    assert!(b.css.contains("font-weight: 700;"));
    assert!(b.css.contains("font-family: 'Inter-fallback';"));
    // The fake `wOF2-fake-bytes` file is not a real OpenType font, so skrifa
    // rejects it and the fallback face stays neutral.
    assert!(b.css.contains("size-adjust: 100%;"));

    assert_eq!(b.preload, vec!["/fonts/inter.woff2".to_string()]);
}

#[test]
fn font_bundle_missing_file_is_an_error() {
    let spec = FontSpec {
        family: "X".into(),
        provider: "local".into(),
        weights: vec![],
        styles: vec![],
        subset: vec![],
        local_paths: vec!["/definitely/not/here.woff2".into()],
    };
    let e = DefaultFontBackend.bundle(&spec).unwrap_err();
    assert_eq!(e.0.code, "AST-FONT");
}

// ---------------- Metadata / SEO (§16.3) ----------------

#[test]
fn metadata_to_head_covers_title_description_canonical_robots() {
    let m = Metadata {
        title: Some("Home".into()),
        title_template: Some("%s · Acme".into()),
        description: Some("desc".into()),
        canonical: Some("https://a.dev/".into()),
        robots: Some("index,follow".into()),
        ..Default::default()
    };
    let h = m.to_head();
    assert_eq!(
        h.title.as_deref(),
        Some("Home · Acme"),
        "template applied when title set"
    );
    assert_eq!(h.title.as_deref(), Some("Home · Acme"));
    let meta = |key: &str| {
        h.meta
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.clone())
    };
    assert_eq!(meta("property:og:title").as_deref(), Some("Home · Acme"));
    assert_eq!(meta("name:description").as_deref(), Some("desc"));
    assert_eq!(meta("property:og:description").as_deref(), Some("desc"));
    assert_eq!(meta("name:robots").as_deref(), Some("index,follow"));
    let (rel, attrs) = &h.links[0];
    assert_eq!(rel, "canonical");
    assert_eq!(attrs.get("href").unwrap(), "https://a.dev/");
}

#[test]
fn metadata_to_head_emits_og_property_and_twitter_name_tags() {
    let m = Metadata {
        open_graph: BTreeMap::from([("type".to_string(), "website".to_string())]),
        twitter: BTreeMap::from([("card".to_string(), "summary".to_string())]),
        json_ld: vec![r#"{"@type":"WebSite"}"#.into()],
        ..Default::default()
    };
    let h = m.to_head();
    let has = |k: &str, v: &str| h.meta.iter().any(|(a, b)| a == k && b == v);
    assert!(has("property:og:type", "website"), "og: uses property");
    assert!(has("name:twitter:card", "summary"), "twitter: uses name");
    assert_eq!(h.json_ld, vec![r#"{"@type":"WebSite"}"#]);
}

#[test]
fn metadata_title_without_template_passes_through() {
    let m = Metadata {
        title: Some("Raw".into()),
        ..Default::default()
    };
    assert_eq!(m.to_head().title.as_deref(), Some("Raw"));
}

// ---------------- sitemap / robots (§16.3) ----------------

#[test]
fn sitemap_xml_lists_escaped_urls() {
    let s = sitemap_xml(&["https://a.dev/", "https://a.dev/x?y=1&z=2"]);
    assert!(s.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
    assert!(s.contains("<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">"));
    assert!(s.contains("<url><loc>https://a.dev/</loc></url>"));
    assert!(s.contains("<url><loc>https://a.dev/x?y=1&amp;z=2</loc></url>"));
    assert!(s.ends_with("</urlset>"));
}

#[test]
fn robots_txt_renders_groups_and_optional_sitemap() {
    let r = robots_txt(
        &[("*", "/private"), ("Googlebot", "/no-google")],
        Some("https://a.dev/sitemap.xml"),
    );
    assert_eq!(
        r,
        "User-agent: *\nDisallow: /private\nUser-agent: Googlebot\nDisallow: /no-google\nSitemap: https://a.dev/sitemap.xml\n"
    );
    assert_eq!(robots_txt(&[], None), "");
}

// ---------------- Content (§16.5) ----------------

#[test]
fn markdown_headings_bold_italic_code_link_img() {
    assert_eq!(markdown_to_html("# Hello"), "<h1>Hello</h1>\n");
    assert_eq!(markdown_to_html("###### Tiny"), "<h6>Tiny</h6>\n");
    let rich =
        markdown_to_html("This is **bold**, *it*, `code`, [x](https://a.b) and ![alt](/i.png)");
    assert!(rich.contains("<strong>bold</strong>"));
    assert!(rich.contains("<em>it</em>"));
    assert!(rich.contains("<code>code</code>"));
    assert!(rich.contains("<a href=\"https://a.b\">x</a>"));
    assert!(rich.contains("<img alt=\"alt\" src=\"/i.png\">"));
}

#[test]
fn markdown_lists_with_one_nesting_level() {
    let md = "- a\n- b\n  - b1\n  - b2\n- c\n";
    assert_eq!(
        markdown_to_html(md),
        "<ul><li>a</li><li>b<ul><li>b1</li><li>b2</li></ul></li><li>c</li></ul>\n"
    );
}

#[test]
fn markdown_code_fence_goes_through_highlight() {
    let md = "```rust\nfn main() {}\n```\n";
    let out = markdown_to_html(md);
    assert!(out.starts_with("<pre><code class=\"language-rust\">"));
    assert!(out.contains("<span class=\"tok-kw\">fn</span>"));
    assert!(out.ends_with("</code></pre>\n"));
}

#[test]
fn markdown_blockquote_hr_and_paragraphs() {
    let out = markdown_to_html("> quoted text\n");
    assert_eq!(out, "<blockquote><p>quoted text</p></blockquote>\n");
    assert_eq!(markdown_to_html("---\n"), "<hr>\n");
    let two = markdown_to_html("first\n\nsecond\n");
    assert_eq!(two, "<p>first</p>\n<p>second</p>\n");
}

#[test]
fn markdown_escapes_html_in_text() {
    let out = markdown_to_html("<script>alert(1)</script>");
    assert!(out.contains("&lt;script&gt;"), "{out}");
    assert!(!out.contains("<script>"));
}

#[test]
fn content_entry_parses_frontmatter_types_slug_and_path() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("posts").join("hello.md");
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(
        &p,
        "---\ntitle: Hi\ncount: 3\ndraft: true\n---\n\n# Hi\nbody\n",
    )
    .unwrap();
    let e = parse_content_entry(
        &p,
        "---\ntitle: Hi\ncount: 3\ndraft: true\n---\n\n# Hi\nbody\n",
    )
    .unwrap();
    assert_eq!(e.slug, "hello");
    assert_eq!(e.path, p);
    assert_eq!(
        e.frontmatter.get("title").unwrap(),
        &serde_json::json!("Hi")
    );
    assert_eq!(e.frontmatter.get("count").unwrap(), &serde_json::json!(3));
    assert_eq!(
        e.frontmatter.get("draft").unwrap(),
        &serde_json::json!(true)
    );
    assert_eq!(e.body_markdown, "\n# Hi\nbody\n");
}

#[test]
fn content_entry_without_frontmatter_treats_all_as_body() {
    let p = std::path::Path::new("notes/plain.md");
    let e = parse_content_entry(p, "just text\n").unwrap();
    assert!(e.frontmatter.is_empty());
    assert_eq!(e.body_markdown, "just text\n");
    assert_eq!(e.slug, "plain");
}

#[test]
fn content_entry_unclosed_frontmatter_is_ast_front() {
    let p = std::path::Path::new("broken.md");
    let e = parse_content_entry(p, "---\ntitle: no end\n").unwrap_err();
    assert_eq!(e.0.code, "AST-FRONT");
}

// ---------------- highlight (§16.5) ----------------

#[test]
fn highlight_rust_keywords_strings_comments_numbers() {
    let out = highlight("fn main() { let x = 42; /* c */ }", "rust");
    assert!(out.contains("<span class=\"tok-kw\">fn</span>"), "{out}");
    assert!(out.contains("<span class=\"tok-kw\">let</span>"));
    assert!(out.contains("<span class=\"tok-num\">42</span>"));
    assert!(out.contains("<span class=\"tok-com\">/* c */</span>"));
}

#[test]
fn highlight_js_and_ts_share_keyword_set() {
    let out = highlight("const a = \"s\"; // note", "ts");
    assert!(out.contains("<span class=\"tok-kw\">const</span>"), "{out}");
    assert!(out.contains("<span class=\"tok-str\">"), "{out}");
    assert!(
        out.contains("<span class=\"tok-com\">// note</span>"),
        "{out}"
    );
    let js = highlight("function f() { return 1; }", "js");
    assert!(js.contains("<span class=\"tok-kw\">function</span>"));
    assert!(js.contains("<span class=\"tok-kw\">return</span>"));
}

#[test]
fn highlight_python_hash_comments() {
    let out = highlight("# comment\nx = 1", "py");
    assert!(
        out.contains("<span class=\"tok-com\"># comment</span>"),
        "{out}"
    );
    assert!(out.contains("<span class=\"tok-num\">1</span>"));
}

#[test]
fn highlight_escapes_html_before_wrapping() {
    let out = highlight("let h = \"<b>\";", "js");
    assert!(out.contains("&lt;b&gt;"), "{out}");
    assert!(!out.contains("<b>"));
}


// ---------------- font metrics (§16.2, D19) ----------------

#[test]
fn font_metric_overrides_use_real_metrics_when_a_font_parses() {
    let candidates = [
        "/System/Library/Fonts/Supplemental/Arial.ttf",
        "/System/Library/Fonts/Geneva.ttf",
        "/Library/Fonts/Arial.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
    ];
    let Some(path) = candidates.iter().map(std::path::Path::new).find(|p| p.exists()) else {
        eprintln!("skipping: no system TTF available");
        return;
    };

    let bytes = std::fs::read(path).unwrap();
    let m = read_metrics(&bytes).expect("system font parses via skrifa");
    assert!(m.units_per_em > 0.0);
    assert!(m.ascent > 0.0);

    let o = metric_overrides(m, TIMES_NEW_ROMAN);
    assert!(o.size_adjust.is_finite() && o.size_adjust > 10.0 && o.size_adjust < 500.0);

    // End-to-end: the bundle's fallback face carries real overrides now.
    let spec = FontSpec {
        family: "RealFont".into(),
        provider: "local".into(),
        weights: vec![400],
        styles: vec!["normal".into()],
        subset: vec![],
        local_paths: vec![path.to_path_buf()],
    };
    let b = DefaultFontBackend.bundle(&spec).unwrap();
    assert!(b.css.contains("ascent-override:"));
    assert!(b.css.contains("descent-override:"));
    assert!(b.css.contains("line-gap-override:"));
}

#[test]
fn font_metric_math_matches_next_font_formula() {
    // Synthetic: target twice as tall overall as fallback → size-adjust 50%.
    let tall = FontMetric {
        ascent: 3650.0,
        descent: 886.0,
        line_gap: 174.0,
        units_per_em: 2048.0,
    };
    let o = metric_overrides(tall, TIMES_NEW_ROMAN);
    assert!((o.size_adjust - 50.0).abs() < 0.05, "got {}", o.size_adjust);
}
