//! §22: `insta` snapshot suite for oxide-render's public output surface.
//!
//! The spec calls for "insta snapshots for HTML and manifests". These lock the
//! exact byte shape of every emitted fragment so a refactor that changes
//! hydration markup, nonce placement, or boundary framing fails loudly instead
//! of silently breaking the client protocol.
//!
//! Each snapshot is a `.snap` file beside this test. Update with
//! `INSTA_UPDATE=always cargo test -p oxide-render --test snapshots_render`.

use oxide_fw_diagnostics::Diagnostic;
use oxide_fw_render::*;

fn sample_head() -> HeadTags {
    let mut link_attrs = std::collections::BTreeMap::new();
    link_attrs.insert("rel".to_string(), "stylesheet".to_string());
    let mut script_attrs = std::collections::BTreeMap::new();
    script_attrs.insert("defer".to_string(), "".to_string());
    HeadTags {
        title: Some("Hello & <World>".to_string()),
        meta: vec![
            (
                "name:description".to_string(),
                "A \"quoted\" desc".to_string(),
            ),
            ("property:og:title".to_string(), "OG".to_string()),
            ("charset".to_string(), "utf-8".to_string()),
        ],
        links: vec![("/style.css".to_string(), link_attrs)],
        scripts: vec![("console.log('hi')".to_string(), script_attrs)],
        json_ld: vec![serde_json::json!({"@type": "Article", "name": "x"}).to_string()],
    }
}

// ---------------------------------------------------------------- HeadTags::to_html

#[test]
fn snapshot_head_to_html() {
    insta::assert_snapshot!("head_to_html", sample_head().to_html("NONCE123"));
}

#[test]
fn snapshot_head_to_html_empty() {
    let empty = HeadTags::default();
    insta::assert_snapshot!("head_to_html_empty", empty.to_html("n0nce"));
}

// ---------------------------------------------------------------- escaping

#[test]
fn snapshot_escape_attr() {
    insta::assert_snapshot!("escape_attr", escape_attr("a&b<c>d\"e'f"));
}

#[test]
fn snapshot_escape_text() {
    insta::assert_snapshot!("escape_text", escape_text("a&b<c>d\"e'f"));
}

// ---------------------------------------------------------------- boundary protocol (§7.1)

#[test]
fn snapshot_boundary_fallback() {
    insta::assert_snapshot!(
        "boundary_fallback",
        boundary_fallback(7, "<p class='loading'>Loading…</p>")
    );
}

#[test]
fn snapshot_boundary_content() {
    insta::assert_snapshot!(
        "boundary_content",
        boundary_content(42, "<p class='done'>ok</p>", "abc123")
    );
}

#[test]
fn snapshot_swapper_script() {
    insta::assert_snapshot!("swapper_script", swapper_script("nonce-xyz"));
}

// ---------------------------------------------------------------- islands (§10.3)

#[test]
fn snapshot_island_element() {
    let d = IslandDirective {
        component: "Counter".to_string(),
        chunk: "/islands/counter.js".to_string(),
        strategy: IslandStrategy::Load,
        props_b64: "T0RGMQAK".to_string(),
    };
    insta::assert_snapshot!("island_element", island_element(&d, "<span>3</span>"));
}

// ---------------------------------------------------------------- script placeholder (§16.6)

#[test]
fn snapshot_script_placeholder() {
    let mut attrs = std::collections::BTreeMap::new();
    attrs.insert("data-analytics".to_string(), "x".to_string());
    insta::assert_snapshot!(
        "script_placeholder",
        script_placeholder("idle", Some("/a.js"), None, false, &attrs, "NONCE1")
            .expect("script placeholder")
    );
}

// ---------------------------------------------------------------- hydration bootstrap (§9.5)

#[test]
fn snapshot_hydration_bootstrap() {
    insta::assert_snapshot!(
        "hydration_bootstrap",
        hydration_bootstrap("T0RGMQAK", "routes/index/page", "NONCE9")
    );
}

// ---------------------------------------------------------------- error recovery (§10.2 step 5)

#[test]
fn snapshot_stream_error_recovery() {
    let diag = Diagnostic::error("OX-500", "the data module exploded <bad>");
    insta::assert_snapshot!(
        "stream_error_recovery",
        stream_error_recovery("boundary-7", &diag, "NONCE3")
    );
}

// ---------------------------------------------------------------- Injector (§10.2 step 4)

#[test]
fn snapshot_inject_into_html() {
    let mut inj = Injector::new(
        HeadTags::default(),
        "NONCE4".to_string(),
        "routes/index/page".to_string(),
    );
    inj.odf_payload_b64 = Some("T0RGMQAK".to_string());
    inj.css_links = vec!["/app.css".to_string()];
    inj.preload = vec!["/chunk.js".to_string()];
    let html =
        "<!doctype html><html><head><title>x</title></head><body><main>hi</main></body></html>";
    insta::assert_snapshot!(
        "inject_into_html",
        inj.inject_into_html(html).expect("inject")
    );
}
