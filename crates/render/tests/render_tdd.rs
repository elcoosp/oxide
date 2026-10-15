//! TDD suite for oxide-render (spec §7.1, §10, §17.1).
//!
//! Written BEFORE the implementation; every pub fn of the crate is exercised.

use bytes::Bytes;
use futures::channel::mpsc;
use oxide_diagnostics::Diagnostic;
use oxide_render::*;

fn head(title: Option<&str>, meta: Vec<(&str, &str)>) -> HeadTags {
    HeadTags {
        title: title.map(|t| t.to_string()),
        meta: meta
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        links: Vec::new(),
        scripts: Vec::new(),
        json_ld: Vec::new(),
    }
}

// ---------------------------------------------------------------- HeadTags::merge

#[test]
fn merge_inner_title_wins() {
    let outer = head(Some("Outer"), vec![]);
    let inner = head(Some("Inner"), vec![]);
    assert_eq!(outer.merge(inner).title.as_deref(), Some("Inner"));
}

#[test]
fn merge_outer_title_kept_when_inner_has_none() {
    let outer = head(Some("Outer"), vec![]);
    let inner = head(None, vec![]);
    assert_eq!(outer.merge(inner).title.as_deref(), Some("Outer"));
}

#[test]
fn merge_meta_inner_overrides_same_name_key() {
    let outer = head(None, vec![("name:description", "old")]);
    let inner = head(None, vec![("name:description", "new")]);
    let m = outer.merge(inner);
    assert_eq!(m.meta, vec![("name:description".into(), "new".into())]);
}

#[test]
fn merge_meta_bare_key_normalizes_to_name() {
    // A bare key means `name`; it must still collide with an explicit `name:` entry.
    let outer = head(None, vec![("description", "old")]);
    let inner = head(None, vec![("name:description", "new")]);
    let m = outer.merge(inner);
    assert_eq!(m.meta, vec![("name:description".into(), "new".into())]);
}

#[test]
fn merge_meta_property_key_overrides_property_only() {
    let outer = head(
        None,
        vec![("property:og:title", "A"), ("name:description", "D")],
    );
    let inner = head(None, vec![("property:og:title", "B")]);
    let m = outer.merge(inner);
    assert_eq!(
        m.meta,
        vec![
            ("name:description".into(), "D".into()), // untouched outer entry keeps position
            ("property:og:title".into(), "B".into()), // inner appended after
        ]
    );
}

#[test]
fn merge_meta_appends_new_entries_preserving_order() {
    let outer = head(None, vec![("name:a", "1"), ("name:b", "2")]);
    let inner = head(None, vec![("name:c", "3"), ("name:a", "9")]);
    let m = outer.merge(inner);
    assert_eq!(
        m.meta,
        vec![
            ("name:b".into(), "2".into()), // non-overridden outer stays first
            ("name:c".into(), "3".into()), // inner new
            ("name:a".into(), "9".into()), // inner override
        ]
    );
}

#[test]
fn merge_links_scripts_jsonld_concatenate_outer_first() {
    let mut outer = head(Some("O"), vec![]);
    outer.links.push((
        "o.css".into(),
        [("rel".to_string(), "stylesheet".to_string())].into(),
    ));
    outer.scripts.push(("outer()".into(), Default::default()));
    outer.json_ld.push("{\"o\":1}".into());
    let mut inner = head(Some("I"), vec![]);
    inner.links.push((
        "i.css".into(),
        [("rel".to_string(), "stylesheet".to_string())].into(),
    ));
    inner.scripts.push(("inner()".into(), Default::default()));
    inner.json_ld.push("{\"i\":2}".into());
    let m = outer.merge(inner);
    assert_eq!(
        m.links.iter().map(|(h, _)| h.as_str()).collect::<Vec<_>>(),
        ["o.css", "i.css"]
    );
    assert_eq!(
        m.scripts
            .iter()
            .map(|(b, _)| b.as_str())
            .collect::<Vec<_>>(),
        ["outer()", "inner()"]
    );
    assert_eq!(m.json_ld, ["{\"o\":1}", "{\"i\":2}"]);
}

// ---------------------------------------------------------------- HeadTags::to_html

#[test]
fn to_html_title_meta_link() {
    let mut h = head(
        Some("T & <title>"),
        vec![("name:description", "a \"quoted\" <b>")],
    );
    h.links.push((
        "/s.css".into(),
        [("rel".to_string(), "stylesheet".to_string())].into(),
    ));
    let html = h.to_html("n1");
    assert_eq!(
        html,
        "<title>T &amp; &lt;title&gt;</title><meta name=\"description\" content=\"a &quot;quoted&quot; &lt;b&gt;\">\
<link href=\"/s.css\" rel=\"stylesheet\">"
    );
}

#[test]
fn to_html_meta_property_kind() {
    let h = head(
        None,
        vec![("property:og:title", "Hi"), ("description", "D")],
    );
    assert_eq!(
        h.to_html(""),
        "<meta property=\"og:title\" content=\"Hi\"><meta name=\"description\" content=\"D\">"
    );
}

#[test]
fn to_html_scripts_carry_nonce_and_sort_attrs() {
    let mut h = head(None, vec![]);
    h.scripts.push((
        "console.log(1)".into(),
        [("type".to_string(), "module".to_string())].into(),
    ));
    h.scripts
        .push(("var a = '</SCRIPT>'".into(), Default::default()));
    let html = h.to_html("abc123");
    assert!(
        html.contains("<script type=\"module\" nonce=\"abc123\">console.log(1)</script>"),
        "{html}"
    );
    // `</script>` inside a script body must be neutralized (case-insensitive; the
    // replacement is emitted in lowercase).
    assert!(
        html.contains("<script nonce=\"abc123\">var a = '<\\/script>'</script>"),
        "{html}"
    );
    assert_eq!(html.matches("nonce=\"abc123\"").count(), 2);
}

#[test]
fn to_html_json_ld_uses_ld_type_and_escapes_lt() {
    let mut h = head(None, vec![]);
    h.json_ld.push("{\"a\":\"<script>\"}".into());
    let html = h.to_html("n");
    assert_eq!(
        html,
        "<script type=\"application/ld+json\" nonce=\"n\">{\"a\":\"\\u003cscript>\"}</script>"
    );
}

#[test]
fn to_html_is_deterministic() {
    let mut h = head(Some("T"), vec![("name:a", "1"), ("name:b", "2")]);
    h.links.push((
        "/x.css".into(),
        [("rel".to_string(), "stylesheet".to_string())].into(),
    ));
    assert_eq!(h.to_html("n"), h.to_html("n"));
}

// ------------------------------------------------------- boundary protocol (§7.1)

#[test]
fn boundary_fallback_uses_ox_b_attr() {
    assert_eq!(
        boundary_fallback(7, "<p>load…</p>"),
        "<template data-ox-b='7'><p>load…</p></template>"
    );
}

#[test]
fn boundary_content_structure_template_plus_script() {
    let out = boundary_content(7, "<p class='done'>ok</p>", "n0nce");
    assert!(
        out.starts_with("<template data-ox-c='7'><p class='done'>ok</p></template>"),
        "{out}"
    );
    assert!(out.contains("<script nonce='n0nce'>"), "{out}");
    assert!(out.contains("</script>"), "{out}");
}

#[test]
fn boundary_content_script_is_self_contained_iife_with_id_baked() {
    let out = boundary_content(42, "x", "n");
    let script = out.split_once("<script").unwrap().1;
    // The swapper must run standalone: an IIFE, not a call into window.__oxSwap.
    assert!(script.contains("(function(id){"), "{script}");
    assert!(script.contains(")('42')"), "{script}");
    assert!(
        !script.contains("window.__oxSwap"),
        "boundary script must not depend on the shared swapper: {script}"
    );
    // DOM mechanics: find content template, swap into placeholder, dispatch ox:swap.
    assert!(script.contains("template[data-ox-c='"), "{script}");
    assert!(script.contains("template[data-ox-b='"), "{script}");
    assert!(script.contains("replaceWith"), "{script}");
    assert!(script.contains("ox:swap"), "{script}");
    assert!(script.contains("CustomEvent"), "{script}");
    assert!(!script.contains("eval"), "no eval (§17.1 CSP)");
}

#[test]
fn boundary_content_script_within_size_budget() {
    // Spec: ~300-byte inline swapper; the whole script must stay ≤ ~400 bytes.
    let out = boundary_content(1, "y", "abcd1234");
    let script = out.split_once("<script").unwrap().1;
    assert!(
        script.len() <= 400,
        "boundary script is {} bytes: {script}",
        script.len()
    );
}

#[test]
fn boundary_content_reuses_shared_swapper_source() {
    // Both emitters must derive from the same SWAPPER_SRC constant.
    let shared = swapper_script("n");
    let boundary = boundary_content(3, "z", "n");
    let body = SWAPPER_SRC;
    assert!(
        shared.contains(body),
        "swapper_script must embed SWAPPER_SRC"
    );
    assert!(
        boundary.contains(body),
        "boundary_content must embed SWAPPER_SRC"
    );
}

#[test]
fn swapper_script_installs_oxswap_and_defer_listener() {
    let s = swapper_script("nn");
    assert!(s.starts_with("<script nonce='nn'>"), "{s}");
    assert!(s.ends_with("</script>"), "{s}");
    assert!(s.contains("window.__oxSwap="), "{s}");
    assert!(s.contains("addEventListener('ox:defer'"), "{s}");
    assert!(s.contains("e.detail.id"), "{s}");
}

// ---------------------------------------------------------------- islands (§10.3)

#[test]
fn island_element_load_strategy() {
    let d = IslandDirective {
        strategy: IslandStrategy::Load,
        component: "Cart".into(),
        chunk: "/chunks/cart-abc.js".into(),
        props_b64: "eyJpZCI6MX0=".into(),
    };
    assert_eq!(
        island_element(&d, "<div>ssr</div>"),
        "<ox-island data-ox-island='Cart' data-ox-chunk='/chunks/cart-abc.js' data-ox-strategy='load' \
         data-ox-props='eyJpZCI6MX0='><div>ssr</div></ox-island>"
    );
}

#[test]
fn island_element_media_strategy_and_visible() {
    let mut d = IslandDirective {
        strategy: IslandStrategy::Visible,
        component: "Gallery".into(),
        chunk: "g.js".into(),
        props_b64: "AA==".into(),
    };
    let v = island_element(&d, "");
    assert!(v.contains("data-ox-strategy='visible'"), "{v}");
    d.strategy = IslandStrategy::Media("max-width: 600px".into());
    assert!(
        island_element(&d, "").contains("data-ox-strategy='media(max-width: 600px)'"),
        "{}",
        island_element(&d, "")
    );
    d.strategy = IslandStrategy::Idle;
    assert!(island_element(&d, "").contains("data-ox-strategy='idle'"));
}

// ------------------------------------------------------- hydration bootstrap (§9.5)

#[test]
fn hydration_bootstrap_exact_shape() {
    let out = hydration_bootstrap("T0RG", "routes/index", "n");
    assert_eq!(
        out,
        "<script type='application/oxide-data' nonce='n'>T0RG</script>\
<script nonce='n'>window.__oxide={route:\"routes/index\",data:document.currentScript.previousElementSibling.textContent}</script>"
    );
}

#[test]
fn hydration_bootstrap_sanitizes_route_id_by_percent_encoding() {
    let out = hydration_bootstrap("AA", "routes/blog/[slug]", "n");
    assert!(out.contains("route:\"routes/blog/%5Bslug%5D\""), "{out}");
    // Only [a-z0-9-/] survive verbatim.
    let out2 = hydration_bootstrap("AA", "routes/UsEr-1/x", "n");
    assert!(
        out2.contains("route:\"routes/%55s%45r-1/x\"") || !out2.contains("UsEr"),
        "{out2}"
    );
    assert!(
        !out2.contains("route:\"routes/UsEr"),
        "uppercase must be encoded: {out2}"
    );
}

// ---------------------------------------------------------------- Server-Timing (§20)

#[test]
fn server_timing_format_order_and_precision() {
    let t = RequestTimings {
        middleware_ms: 0.0,
        load_ms: 12.3456,
        render_ms: 1.0,
        inject_ms: 0.125,
        cache_ms: 3.0,
    };
    assert_eq!(
        t.server_timing(),
        "match;dur=0.00, cache;dur=3.00, load;dur=12.35, render;dur=1.00, inject;dur=0.12"
    );
    assert_eq!(
        RequestTimings::default()
            .server_timing()
            .matches(";dur=")
            .count(),
        5
    );
}

// ---------------------------------------------------------------- Injector

fn injector() -> Injector {
    let mut head = head(Some("Shell"), vec![("name:description", "d")]);
    head.links.push((
        "/app.css".into(),
        [("rel".to_string(), "stylesheet".to_string())].into(),
    ));
    let mut inj = Injector::new(head, "n0nce".into(), "routes/index".into());
    inj.preload.push("/chunks/main.js".into());
    inj.css_links.push("/theme.css".into());
    inj.odf_payload_b64 = Some("T0RGMQ==".into());
    inj
}

const DOC: &str = "<!doctype html><html><head><meta charset=\"utf-8\"><title>x</title></head><body><h1>hi</h1></body></html>";

#[test]
fn injector_injects_head_links_and_payload() {
    let out = injector().inject_into_html(DOC).unwrap();
    // head tags + preloads + css appended to existing head content, before </head>
    let head_end = out.find("</head>").unwrap();
    let injected = &out[out.find("<meta charset").unwrap()..head_end];
    assert!(injected.contains("<title>Shell</title>"), "{out}");
    assert!(
        injected.contains("<meta name=\"description\" content=\"d\">"),
        "{out}"
    );
    assert!(
        injected.contains("<link rel=\"modulepreload\" href=\"/chunks/main.js\">"),
        "{out}"
    );
    assert!(
        injected.contains("<link rel=\"stylesheet\" href=\"/theme.css\">"),
        "{out}"
    );
    // links come after the head tags
    let title_at = injected.find("<title>").unwrap();
    let preload_at = injected.find("modulepreload").unwrap();
    assert!(title_at < preload_at);
    // payload before </body>
    let body_end = out.find("</body>").unwrap();
    let tail = &out[..body_end];
    assert!(
        tail.contains("<script type='application/oxide-data' nonce='n0nce'>T0RGMQ==</script>"),
        "{out}"
    );
    assert!(
        tail.contains("window.__oxide={route:\"routes/index\""),
        "{out}"
    );
    // rest of the document byte-preserved
    assert!(out.starts_with("<!doctype html><html><head><meta charset=\"utf-8\">"));
    assert!(out.ends_with("</body></html>"));
}

#[test]
fn injector_handles_missing_head_by_inserting_one() {
    let mut inj = injector();
    inj.odf_payload_b64 = None;
    let doc = "<html><body><p>x</p></body></html>";
    let out = inj.inject_into_html(doc).unwrap();
    assert!(out.contains("<head><title>Shell</title>"), "{out}");
    assert!(out.contains("</head><body><p>x</p></body></html>"), "{out}");
    assert!(
        !out.contains("application/oxide-data"),
        "no payload configured → no body inject"
    );
}

#[test]
fn injector_on_fragment_prepends_head_and_appends_payload() {
    let out = injector().inject_into_html("<p>fragment</p>").unwrap();
    assert!(out.contains("<head><title>Shell</title>"), "{out}");
    // no <body> element → payload appended at the very end (documented fallback)
    assert!(out.ends_with("</script>"), "{out}");
    assert!(
        out.contains("window.__oxide={route:\"routes/index\""),
        "{out}"
    );
}

#[test]
fn inject_stream_matches_batch_byte_for_byte() {
    let doc = format!("{DOC}{}", "");
    let batch = injector().inject_into_html(&doc).unwrap();
    let mut stream = injector().streaming();
    let bytes = doc.as_bytes();
    let (a, b, c) = (&bytes[..10], &bytes[10..40], &bytes[40..]);
    let mut acc = Vec::new();
    acc.extend_from_slice(&stream.push(a).unwrap());
    acc.extend_from_slice(&stream.push(b).unwrap());
    acc.extend_from_slice(&stream.push(c).unwrap());
    acc.extend_from_slice(&stream.finish().unwrap());
    assert_eq!(
        String::from_utf8(acc).unwrap(),
        batch,
        "streamed injection must equal batch injection"
    );
}

#[test]
fn inject_stream_two_pushes_also_equal() {
    let doc = "<html><head><title>t</title></head><body><main>m</main></body></html>";
    let batch = injector().inject_into_html(doc).unwrap();
    let mut s = injector().streaming();
    let mut acc = s.push(doc.as_bytes()).unwrap();
    acc.extend_from_slice(&s.finish().unwrap());
    assert_eq!(String::from_utf8(acc).unwrap(), batch);
}

#[test]
fn inject_stream_without_body_element_appends_payload_at_end() {
    let mut inj = injector();
    inj.head = head(None, vec![]);
    inj.preload.clear();
    inj.css_links.clear();
    let batch = inj.inject_into_html("<p>no body tag</p>").unwrap();
    let mut s = inj.streaming();
    let mut acc = s.push(b"<p>no body tag</p>").unwrap();
    acc.extend_from_slice(&s.finish().unwrap());
    assert_eq!(String::from_utf8(acc).unwrap(), batch);
}

// ------------------------------------------------------- error recovery (§10.2 step 5)

#[test]
fn stream_error_recovery_renders_escaped_error_boundary() {
    let d = Diagnostic::error("RND-42", "load blew <up> & \"burned\"");
    let out = stream_error_recovery("77", &d, "n9");
    assert!(out.contains("<template data-ox-c='77'>"), "{out}");
    assert!(out.contains("class='ox-error'"), "{out}");
    assert!(out.contains("RND-42"), "{out}");
    assert!(out.contains("&lt;up&gt; &amp; &quot;burned&quot;"), "{out}");
    assert!(out.contains("<script nonce='n9'>"), "{out}");
    assert!(out.contains(")('77')"), "{out}");
}

#[test]
fn stream_error_recovery_sanitizes_hostile_boundary_id() {
    let d = Diagnostic::error("X-1", "m");
    let out = stream_error_recovery("a'b`c", &d, "n");
    assert!(!out.contains("a'b"), "{out}");
    assert!(out.contains("a%27b%60c"), "{out}");
}

// ---------------------------------------------------------------- RenderStream

#[tokio::test]
async fn render_stream_collects_to_string() {
    let (tx, rx) = mpsc::channel::<Result<Bytes>>(4);
    let stream = RenderStream {
        status: 200,
        headers: vec![],
        rx,
        tags: vec!["t".into()],
    };
    let mut tx = tx;
    tx.try_send(Ok(Bytes::from_static(b"hel"))).unwrap();
    tx.try_send(Ok(Bytes::from_static(b"lo"))).unwrap();
    drop(tx);
    assert_eq!(stream.collect_to_string().await.unwrap(), "hello");
}

#[tokio::test]
async fn render_stream_propagates_errors() {
    let (tx, rx) = mpsc::channel::<Result<Bytes>>(4);
    let stream = RenderStream {
        status: 500,
        headers: vec![],
        rx,
        tags: vec![],
    };
    let mut tx = tx;
    tx.try_send(Err(err("RND-BOOM", "nope"))).unwrap();
    drop(tx);
    let e = stream.collect_to_string().await.unwrap_err();
    assert_eq!(e.0.code, "RND-BOOM");
}

// ---------------------------------------------------------------- misc contract fns

#[test]
fn render_mode_roundtrip() {
    for s in ["static", "isr", "dynamic", "spa", "islands"] {
        assert_eq!(RenderMode::parse(s).unwrap().as_str(), s);
    }
    assert!(RenderMode::parse("nope").is_none());
}

// ---------------------------------------------------------------- script_placeholder (§16.6)

#[test]
fn script_placeholder_src_carries_strategy_and_nonce() {
    let attrs = std::collections::BTreeMap::new();
    let out = script_placeholder("defer", Some("/a.js"), None, false, &attrs, "n0nce").unwrap();
    assert!(out.starts_with("<script data-ox-script"), "{out}");
    assert!(out.contains("data-ox-strategy=\"defer\""), "{out}");
    assert!(out.contains("data-ox-src=\"/a.js\""), "{out}");
    assert!(out.contains("nonce=\"n0nce\""), "{out}");
    assert!(out.ends_with("></script>"), "{out}");
    // The placeholder must not carry a real `src` attribute or body — those
    // belong on the client-injected element.
    assert!(!out.contains(" src=\"/a.js\""), "{out}");
    assert!(!out.contains(" data-ox-inline="), "{out}");
}

#[test]
fn script_placeholder_inline_is_base64_and_marks_module() {
    let attrs = std::collections::BTreeMap::new();
    let out = script_placeholder("idle", None, Some("console.log(1)"), true, &attrs, "n").unwrap();
    assert!(out.contains("data-ox-strategy=\"idle\""), "{out}");
    assert!(out.contains("data-ox-module"), "{out}");
    assert!(out.contains("data-ox-inline="), "{out}");
    // Decode and confirm round-trip.
    let b64 = out.split("data-ox-inline=\"").nth(1).unwrap();
    let b64 = b64.split('"').next().unwrap();
    use base64::Engine as _;
    let raw = base64::engine::general_purpose::STANDARD.decode(b64).unwrap();
    assert_eq!(String::from_utf8(raw).unwrap(), "console.log(1)");
    // No `data-ox-src` for an inline script.
    assert!(!out.contains("data-ox-src"), "{out}");
}

#[test]
fn script_placeholder_rejects_empty_and_ambiguous() {
    let attrs = std::collections::BTreeMap::new();
    let e = script_placeholder("defer", None, None, false, &attrs, "n").unwrap_err();
    assert_eq!(e.0.code, "RND-SCRIPT");
    let e = script_placeholder("defer", Some("/a.js"), Some("x"), false, &attrs, "n").unwrap_err();
    assert_eq!(e.0.code, "RND-SCRIPT");
}

#[test]
fn script_placeholder_escapes_hostile_src_and_attrs() {
    let mut attrs = std::collections::BTreeMap::new();
    attrs.insert("data-x".to_string(), "\"><script>alert(1)</script>".to_string());
    let out = script_placeholder(
        "defer",
        Some("\"><script>alert(2)</script>"),
        None,
        false,
        &attrs,
        "n&\"",
    )
    .unwrap();
    // No literal `<`/`>`/`"` may leak from any interpolated value.
    assert!(!out.contains("><script>"), "{out}");
    assert!(out.contains("&lt;script&gt;"), "{out}");
    assert!(out.contains("&quot;"), "{out}");
    assert!(out.contains("nonce=\"n&amp;&quot;\""), "{out}");
}
