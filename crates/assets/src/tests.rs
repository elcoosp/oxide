//! TDD suite for oxide-assets (spec §16, D5, D19). CSS is tested in
//! `tests/lightningcss_tdd.rs`; this module covers asset queries, images,
//! fonts, metadata, sitemap/robots, content, and highlighting.

use super::*;

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
        let flag = q.query.iter().next().unwrap();
        assert_eq!(flag.1, "", "flag query `{spec}` maps to empty value");
    }
}

#[test]
fn asset_query_without_query_is_empty_map() {
    let q = parse_asset_query("./plain.png").unwrap();
    assert_eq!(q.path, "./plain.png");
    assert!(q.query.is_empty());
}

#[test]
fn asset_query_escaped_question_mark_stays_in_path() {
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
    let src = "function flex() { return 'items-center'; }";
    assert!(scan_tailwind_candidates(&[src]).is_empty());
}

// ---------------- Images (§16.1, D19) ----------------

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
}

#[test]
fn image_jpeg_encode_produces_bytes_and_clamps_quality() {
    for q in [0u8, 80, 250] {
        let mut r = req(gradient_png(32, 16), None, ImageFormat::Jpeg, None);
        r.quality = q;
        let out = RustImageBackend.process(r).unwrap();
        assert_eq!(out.format, ImageFormat::Jpeg);
        assert_eq!(&out.bytes[..2], &[0xFF, 0xD8]);
    }
}

#[test]
fn image_webp_succeeds_or_falls_back_to_png_with_ast_webp_diagnostic() {
    let out = RustImageBackend
        .process(req(gradient_png(32, 16), None, ImageFormat::Webp, None))
        .unwrap();
    match out.format {
        ImageFormat::Webp => assert!(out.diagnostics.is_empty()),
        ImageFormat::Png => assert!(out.diagnostics.iter().any(|d| d.code == "AST-WEBP")),
        other => panic!("unexpected format {other:?}"),
    }
}

#[test]
fn image_avif_encoding_produces_real_avif_bytes() {
    let out = RustImageBackend
        .process(req(gradient_png(32, 16), None, ImageFormat::Avif, None))
        .unwrap();
    assert_eq!(out.format, ImageFormat::Avif);
    assert_eq!(&out.bytes[4..8], b"ftyp");
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
    assert_ne!(a.cache_key, c.cache_key);
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
    // SEC-08: the full 256-bit digest (64 hex), not the old 64-bit prefix.
    assert_eq!(sig.len(), 64);
}

// ---------------- Fonts (§16.2) ----------------

fn font_spec(dir: &std::path::Path) -> FontSpec {
    let p = dir.join("inter.woff2");
    std::fs::write(&p, b"wOF2-fake-bytes").unwrap();
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
    assert_eq!(b.css.matches("@font-face").count(), 3);
    assert!(b.css.contains("font-family: 'Inter';"));
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
    assert_eq!(
        DefaultFontBackend.bundle(&spec).unwrap_err().0.code,
        "AST-FONT"
    );
}

// ---------------- Font metric math (§16.2) ----------------

#[test]
fn font_metric_overrides_use_real_metrics_when_a_font_parses() {
    let candidates = [
        "/System/Library/Fonts/Supplemental/Arial.ttf",
        "/System/Library/Fonts/Geneva.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    ];
    let Some(path) = candidates
        .iter()
        .map(std::path::Path::new)
        .find(|p| p.exists())
    else {
        eprintln!("skipping: no system TTF available");
        return;
    };
    let bytes = std::fs::read(path).unwrap();
    let m = read_metrics(&bytes).expect("system font parses");
    assert!(m.units_per_em > 0.0);
    let o = metric_overrides(m, TIMES_NEW_ROMAN);
    assert!(o.size_adjust.is_finite() && o.size_adjust > 10.0);
}

#[test]
fn font_metric_math_matches_next_font_formula() {
    let tall = FontMetric {
        ascent: 3650.0,
        descent: 886.0,
        line_gap: 174.0,
        units_per_em: 2048.0,
    };
    let o = metric_overrides(tall, TIMES_NEW_ROMAN);
    assert!((o.size_adjust - 50.0).abs() < 0.05, "got {}", o.size_adjust);
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
    assert_eq!(h.title.as_deref(), Some("Home · Acme"));
    assert!(h.meta.iter().any(|(k, _)| k == "property:og:title"));
    let (rel, _) = &h.links[0];
    assert_eq!(rel, "canonical");
}

#[test]
fn metadata_to_head_emits_og_property_and_twitter_name_tags() {
    let m = Metadata {
        open_graph: BTreeMap::from([("type".to_string(), "website".to_string())]),
        twitter: BTreeMap::from([("card".to_string(), "summary".to_string())]),
        ..Default::default()
    };
    let h = m.to_head();
    assert!(
        h.meta
            .iter()
            .any(|(k, v)| k == "property:og:type" && v == "website")
    );
    assert!(
        h.meta
            .iter()
            .any(|(k, v)| k == "name:twitter:card" && v == "summary")
    );
}

// ---------------- sitemap / robots (§16.3) ----------------

#[test]
fn sitemap_xml_lists_escaped_urls() {
    let s = sitemap_xml(&["https://a.dev/", "https://a.dev/x?y=1&z=2"]);
    assert!(s.contains("<url><loc>https://a.dev/x?y=1&amp;z=2</loc></url>"));
}

#[test]
fn robots_txt_renders_groups_and_optional_sitemap() {
    let r = robots_txt(&[("*", "/private")], Some("https://a.dev/sitemap.xml"));
    assert!(r.contains("User-agent: *\nDisallow: /private"));
    assert!(r.contains("Sitemap: https://a.dev/sitemap.xml"));
}

// ---------------- Content (§16.5) ----------------

#[test]
fn markdown_headings_bold_italic_code_link_img() {
    assert_eq!(markdown_to_html("# Hello"), "<h1>Hello</h1>\n");
    let rich = markdown_to_html("**bold**, *it*, `code`, [x](https://a.b)");
    assert!(rich.contains("<strong>bold</strong>"));
    assert!(rich.contains("<em>it</em>"));
    assert!(rich.contains("<code>code</code>"));
    assert!(rich.contains("<a href=\"https://a.b\">x</a>"));
}

#[test]
fn markdown_lists_with_one_nesting_level() {
    let md = "- a\n- b\n  - b1\n- c\n";
    let out = markdown_to_html(md);
    assert!(out.contains("<ul>"));
    assert!(out.contains("<li>a</li>"));
}

#[test]
fn markdown_escapes_html_in_text() {
    let out = markdown_to_html("<script>alert(1)</script>");
    assert!(!out.contains("<script>"), "{out}");
}

#[test]
fn markdown_blockquote_hr_and_paragraphs() {
    let out = markdown_to_html("> quoted text\n");
    assert!(out.contains("<blockquote>"));
    let two = markdown_to_html("first\n\nsecond\n");
    assert!(two.contains("<p>first</p>"));
    assert!(two.contains("<p>second</p>"));
}

#[test]
fn content_entry_parses_frontmatter_types_slug_and_path() {
    let p = std::path::Path::new("posts/hello.md");
    let e = parse_content_entry(
        p,
        "---\ntitle: Hi\ncount: 3\ndraft: true\n---\n\n# Hi\nbody\n",
    )
    .unwrap();
    assert_eq!(e.slug, "hello");
    assert_eq!(
        e.frontmatter.get("title").unwrap(),
        &serde_json::json!("Hi")
    );
    assert_eq!(e.frontmatter.get("count").unwrap(), &serde_json::json!(3));
    assert_eq!(
        e.frontmatter.get("draft").unwrap(),
        &serde_json::json!(true)
    );
}

#[test]
fn content_entry_without_frontmatter_treats_all_as_body() {
    let p = std::path::Path::new("notes/plain.md");
    let e = parse_content_entry(p, "just text\n").unwrap();
    assert!(e.frontmatter.is_empty());
    assert_eq!(e.body_markdown, "just text\n");
}

#[test]
fn content_entry_unclosed_frontmatter_is_ast_front() {
    let e =
        parse_content_entry(std::path::Path::new("broken.md"), "---\ntitle: no end\n").unwrap_err();
    assert_eq!(e.0.code, "AST-FRONT");
}

// ---------------- highlight (§16.5, syntect) ----------------

#[test]
fn highlight_rust_emits_spans() {
    let out = highlight("fn main() {}", "rust");
    assert!(out.contains("<span"), "{out}");
}

#[test]
fn highlight_js_keywords() {
    let out = highlight("const a = 1;", "js");
    assert!(out.contains("const"));
}

#[test]
fn highlight_unknown_language_is_plain() {
    let out = highlight("some text", "not-a-language");
    assert!(out.contains("some text"));
}

#[test]
fn highlight_escapes_html_before_wrapping() {
    let out = highlight("let h = \"<b>\";", "js");
    assert!(out.contains("&lt;b&gt;") || !out.contains("<b>"), "{out}");
}

// ---------------- §16.5 MDX bridging ----------------

#[test]
fn content_format_from_path_infers_by_extension() {
    use std::path::Path;
    assert_eq!(
        ContentFormat::from_path(Path::new("a.mdx")),
        ContentFormat::Mdx
    );
    assert_eq!(
        ContentFormat::from_path(Path::new("a.md")),
        ContentFormat::Markdown
    );
    assert_eq!(
        ContentFormat::from_path(Path::new("a.txt")),
        ContentFormat::Markdown
    );
}

#[test]
fn parse_content_entry_with_format_is_format_agnostic_at_this_stage() {
    use std::path::Path;
    let src = "---\ntitle: Hi\n---\nbody\n";
    let a =
        parse_content_entry_with_format(Path::new("a.md"), src, ContentFormat::Markdown).unwrap();
    let b = parse_content_entry_with_format(Path::new("a.mdx"), src, ContentFormat::Mdx).unwrap();
    assert_eq!(a.frontmatter, b.frontmatter);
    assert_eq!(a.body_markdown, b.body_markdown);
    assert_eq!(a.slug, "a");
    assert_eq!(b.slug, "a");
}

#[test]
fn parse_mdx_source_targets_mdx_js() {
    let path = std::path::Path::new("posts/hello.mdx");
    let src = "---\ntitle: Hello\n---\n\n# Hi <X />\n";
    let (entry, req) = parse_mdx_source(path, src).unwrap();
    assert_eq!(
        entry.frontmatter.get("title").unwrap(),
        &serde_json::json!("Hello")
    );
    assert_eq!(req.module, "@mdx-js/mdx");
    assert_eq!(req.export, "compile");
    assert!(req.source.contains("<X />"));
}

#[test]
fn mdx_compile_request_round_trips_through_serde() {
    let r = MdxCompileRequest::mdx_js("# Hi", "a.mdx");
    let v = serde_json::to_value(&r).unwrap();
    let back: MdxCompileRequest = serde_json::from_value(v).unwrap();
    assert_eq!(back.module, r.module);
    assert_eq!(back.export, r.export);
    assert_eq!(back.source, r.source);
}

// ---------------------------------------------------------------------------
// §16.1 image endpoint helpers: sign / verify / parse.
// ---------------------------------------------------------------------------

#[test]
fn image_request_parses_all_fields() {
    let req = crate::parse_image_request(
        "url=%2Fhero.png&w=640&h=360&format=webp&q=80&placeholder=blur&s=abcd1234",
    )
    .expect("parse");
    assert_eq!(req.url, "/hero.png");
    assert_eq!(req.width, Some(640));
    assert_eq!(req.height, Some(360));
    assert_eq!(req.format, Some(crate::ImageFormat::Webp));
    assert_eq!(req.quality, Some(80));
    assert_eq!(req.placeholder, Some(crate::PlaceholderKind::Blur));
    assert_eq!(req.signature.as_deref(), Some("abcd1234"));
}

#[test]
fn image_request_requires_url() {
    let e = crate::parse_image_request("w=640").unwrap_err();
    assert!(e.0.message.contains("url"), "{}", e.0.message);
}

#[test]
fn image_request_rejects_bad_format_and_quality() {
    let e = crate::parse_image_request("url=/a.png&format=jxl").unwrap_err();
    assert!(e.0.message.contains("unknown `format`"), "{}", e.0.message);
    let e = crate::parse_image_request("url=/a.png&q=200").unwrap_err();
    assert!(e.0.message.contains("out of range"), "{}", e.0.message);
}

#[test]
fn image_request_ignores_unknown_params() {
    let req = crate::parse_image_request("url=/a.png&w=100&something=else").expect("parse");
    assert_eq!(req.url, "/a.png");
    assert_eq!(req.width, Some(100));
    assert!(req.height.is_none());
}

#[test]
fn image_request_percent_decodes_the_url() {
    let req = crate::parse_image_request("url=%2Fimages%2Fcaf%C3%A9.png").expect("parse");
    assert_eq!(req.url, "/images/café.png");
}

#[test]
fn image_request_plus_is_literal_not_space() {
    // The `url=` parameter carries a URL, so `+` must not become a space
    // (that would corrupt a legitimate plus in a path).
    let req = crate::parse_image_request("url=/a+b.png").expect("parse");
    assert_eq!(req.url, "/a+b.png");
}

#[test]
fn image_signature_round_trip() {
    let secret = "test-secret";
    let signed = crate::sign_image_url("/hero.png?w=640", secret);
    // Extract the `s=` value the signer produced.
    let sig = signed
        .split('&')
        .find_map(|p| p.strip_prefix("s="))
        .expect("s= present");
    assert!(crate::verify_image_signature(
        "/hero.png?w=640",
        secret,
        sig
    ));
    assert!(!crate::verify_image_signature(
        "/hero.png?w=641",
        secret,
        sig
    ));
    assert!(!crate::verify_image_signature(
        "/hero.png?w=640",
        "other",
        sig
    ));
}

#[test]
fn image_signature_appends_with_question_or_ampersand() {
    let secret = "s";
    assert!(crate::sign_image_url("/a.png", secret).starts_with("/a.png?s="));
    assert!(crate::sign_image_url("/a.png?w=1", secret).starts_with("/a.png?w=1&s="));
}

// ----------------------------------------------------------- §16.2 fonts

#[test]
fn font_format_hint_matches_the_file_container() {
    // A `.ttf` face must carry `format('truetype')`, not `format('woff2')` —
    // a mismatched hint makes the browser skip the face.
    let dir = tempfile::tempdir().unwrap();
    let ttf = dir.path().join("Face.ttf");
    // A minimal valid TTF: the backend copies bytes verbatim and only parses
    // for metrics (a parse failure falls back to a neutral face, not an error).
    std::fs::write(&ttf, b"\x00\x01\x00\x00not-a-real-font").unwrap();

    let spec = FontSpec {
        family: "Face".into(),
        provider: "local".into(),
        weights: vec![400],
        styles: vec!["normal".into()],
        subset: vec![],
        local_paths: vec![ttf],
    };
    let bundle = DefaultFontBackend.bundle(&spec).unwrap();
    assert!(
        bundle.css.contains("format('truetype')"),
        "ttf face must hint truetype:\n{}",
        bundle.css
    );
    assert!(
        !bundle.css.contains("url(/fonts/Face.ttf) format('woff2')"),
        "must not mislabel a ttf as woff2"
    );
}

#[test]
fn font_fallback_face_is_emitted_with_a_metric_override() {
    let dir = tempfile::tempdir().unwrap();
    let ttf = dir.path().join("Face.ttf");
    std::fs::write(&ttf, b"not-a-real-font").unwrap();
    let spec = FontSpec {
        family: "Face".into(),
        provider: "local".into(),
        weights: vec![],
        styles: vec![],
        subset: vec![],
        local_paths: vec![ttf],
    };
    let bundle = DefaultFontBackend.bundle(&spec).unwrap();
    // A fallback face always exists; with unparseable input it keeps the
    // neutral 100% size-adjust.
    assert!(bundle.css.contains("font-family: 'Face-fallback'"));
    assert!(bundle.css.contains("size-adjust:"));
}
