//! V2-F16 "why dynamic?" engine tests (spec `docs/spec-v0.2-draft.md` §V2-F16,
//! error-code registry Appendix V2.K `OX-DYN`).
//!
//! Each fixture under `fixtures/why/` pins one reason kind end to end:
//! `infer_reasons` must report the kind, the 1-based line and the snippet the
//! diagnostic points at, and `infer_render_mode` must keep answering exactly
//! what it answered before F16 (its contract is unchanged: request-state
//! reads flip the mode, an uncached fetch only feeds the *why* report).

use oxide_fw_compiler::{ReasonKind, infer_reasons, infer_render_mode};
use oxide_fw_diagnostics::Severity;

fn fixture(name: &str) -> String {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/why")
        .join(name);
    std::fs::read_to_string(p).expect("fixture file exists")
}

#[test]
fn cookie_fixture_reports_the_bind_and_the_read() {
    let src = fixture("cookie.server.ts");
    let reasons = infer_reasons(&src, "cookie.server.ts");
    assert_eq!(
        reasons.len(),
        2,
        "params bind + member read, got {reasons:?}"
    );
    assert_eq!(reasons[0].kind, ReasonKind::CookieRead);
    assert_eq!(reasons[0].line, 1, "the destructured bind is on line 1");
    assert!(
        reasons[0].snippet.contains("cookies"),
        "{}",
        reasons[0].snippet
    );
    assert_eq!(reasons[1].kind, ReasonKind::CookieRead);
    assert_eq!(reasons[1].line, 2, "`cookies.get` is on line 2");
    assert!(
        reasons[1].snippet.contains("cookies.get"),
        "snippet names the read: {}",
        reasons[1].snippet
    );
}

#[test]
fn header_fixture_reports_a_header_read() {
    let src = fixture("header.server.ts");
    let reasons = infer_reasons(&src, "header.server.ts");
    assert!(!reasons.is_empty(), "the `request` read must be reported");
    assert!(reasons.iter().all(|r| r.kind == ReasonKind::HeaderRead));
    assert_eq!(reasons[0].line, 1);
}

#[test]
fn search_fixture_reports_a_search_param_read() {
    let src = fixture("search.server.ts");
    let reasons = infer_reasons(&src, "search.server.ts");
    assert!(!reasons.is_empty());
    assert!(
        reasons
            .iter()
            .all(|r| r.kind == ReasonKind::SearchParamRead)
    );
    assert_eq!(reasons[0].line, 1);
}

#[test]
fn fetch_fixture_reports_one_uncached_fetch() {
    let src = fixture("fetch.server.ts");
    let reasons = infer_reasons(&src, "fetch.server.ts");
    assert_eq!(reasons.len(), 1, "one fetch call, got {reasons:?}");
    assert_eq!(reasons[0].kind, ReasonKind::UncachedFetch);
    assert_eq!(reasons[0].line, 2);
    assert!(
        reasons[0].snippet.contains("fetch("),
        "snippet names the call: {}",
        reasons[0].snippet
    );
    // §V2-F16: a fetch is a *why* reason, not a mode trigger — the mode
    // contract of `infer_render_mode` is unchanged.
    assert_eq!(infer_render_mode(&src, "fetch.server.ts").mode, "static");
}

#[test]
fn use_cache_fetch_is_not_a_reason() {
    let src = fixture("cached.server.ts");
    let reasons = infer_reasons(&src, "cached.server.ts");
    assert!(
        reasons.is_empty(),
        "a fetch inside 'use cache' is not a reason: {reasons:?}"
    );
}

#[test]
fn comment_and_string_mentions_are_not_reasons() {
    let src = fixture("mentions.server.ts");
    let reasons = infer_reasons(&src, "mentions.server.ts");
    assert!(reasons.is_empty(), "prose is not a read: {reasons:?}");
    assert_eq!(infer_render_mode(&src, "mentions.server.ts").mode, "static");
}

#[test]
fn multi_reason_route_reports_every_hit_in_preorder() {
    let src = fixture("mixed.server.ts");
    let reasons = infer_reasons(&src, "mixed.server.ts");
    let kinds: Vec<ReasonKind> = reasons.iter().map(|r| r.kind).collect();
    assert_eq!(
        kinds,
        vec![
            ReasonKind::CookieRead,    // params bind `{ cookies, request }`
            ReasonKind::CookieRead,    // cookies.get on line 2
            ReasonKind::HeaderRead,    // request.headers on line 3
            ReasonKind::UncachedFetch  // fetch( on line 6
        ],
        "pre-order: params, then body statements, got {reasons:?}"
    );
    let lines: Vec<u32> = reasons.iter().map(|r| r.line).collect();
    // The fixture carries a two-line comment header; the loader starts on
    // line 3 (bind), then the body reads on 4, 5 and the fetch on 6.
    assert_eq!(lines, vec![3, 4, 5, 6]);
}

#[test]
fn two_cookie_reads_are_two_reasons_plus_the_bind() {
    let src = fixture("two_cookies.server.ts");
    let reasons = infer_reasons(&src, "two_cookies.server.ts");
    assert_eq!(reasons.len(), 3, "bind + two reads, got {reasons:?}");
    let lines: Vec<u32> = reasons.iter().map(|r| r.line).collect();
    // The fixture carries a one-line comment header; the bind is line 2 and
    // the two `.get` reads are lines 3 and 4.
    assert_eq!(lines, vec![2, 3, 4]);
}

#[test]
fn first_reason_agrees_with_the_mode_trigger() {
    for name in ["cookie.server.ts", "header.server.ts", "search.server.ts"] {
        let src = fixture(name);
        let mode = infer_render_mode(&src, name);
        let reasons = infer_reasons(&src, name);
        assert_eq!(mode.mode, "dynamic", "{name}");
        assert_eq!(
            reasons.first().map(|r| r.kind.trigger()),
            mode.trigger,
            "{name}: the first reason is the trigger `oxide routes` already prints"
        );
    }
}

#[test]
fn unparseable_module_yields_no_reasons() {
    // `infer_render_mode` conservatively answers `dynamic` for a module that
    // does not parse (a build error is reported elsewhere); the why engine
    // has no span to point at, so it reports nothing.
    let src = "export const load = async ( { \n";
    assert!(infer_reasons(src, "page.server.ts").is_empty());
    assert_eq!(infer_render_mode(src, "page.server.ts").mode, "dynamic");
}

#[test]
fn spans_point_at_real_source_bytes() {
    let src = fixture("cookie.server.ts");
    let reasons = infer_reasons(&src, "cookie.server.ts");
    for r in &reasons {
        assert!(r.span.0 < r.span.1, "non-degenerate span: {r:?}");
        assert!(
            src.get(r.span.0..r.span.1).is_some(),
            "span indexes into the source: {r:?}"
        );
    }
}

#[test]
fn reason_diagnostic_carries_ox_dyn_code_frame_and_fix() {
    let src = fixture("cookie.server.ts");
    let reasons = infer_reasons(&src, "cookie.server.ts");
    let member_read = &reasons[1];
    let d = member_read.diagnostic("src/routes/dashboard/page.server.ts", &src);
    assert_eq!(d.code, "OX-DYN", "Appendix V2.K registry");
    assert_eq!(d.severity, Severity::Info);
    let loc = d.location.as_ref().expect("source-mapped location");
    assert_eq!(
        loc.file.as_deref(),
        Some("src/routes/dashboard/page.server.ts")
    );
    assert_eq!(loc.line, 2);
    let frame = d
        .code_frame
        .as_ref()
        .expect("a code frame points at the read");
    assert!(
        frame.contains("cookies.get"),
        "frame shows the line: {frame}"
    );
    assert!(frame.contains('^'), "frame underlines the column: {frame}");
    let fix = d
        .fix
        .as_ref()
        .expect("every OX-DYN carries the miette help text");
    assert!(!fix.is_empty());
    assert!(!d.causes.is_empty(), "the why-chain names the reason kind");
    let rendered = d.to_string();
    assert!(rendered.contains("[OX-DYN]"), "{rendered}");
    assert!(rendered.contains("page.server.ts:2"), "{rendered}");
}

#[test]
fn reason_rows_serialize_for_the_side_car() {
    let src = fixture("fetch.server.ts");
    let reasons = infer_reasons(&src, "fetch.server.ts");
    let row = reasons[0].row();
    let json = serde_json::to_string(&row).expect("rows serialize");
    assert!(json.contains("\"kind\":\"uncachedFetch\""), "{json}");
    assert!(json.contains("\"line\":2"), "{json}");
    assert!(json.contains("\"snippet\":"), "{json}");
    assert!(json.contains("\"span\":["), "{json}");
}

#[test]
fn reason_kind_trigger_names_match_the_scanner_vocabulary() {
    assert_eq!(ReasonKind::CookieRead.trigger(), "cookies");
    assert_eq!(ReasonKind::HeaderRead.trigger(), "request.headers");
    assert_eq!(ReasonKind::LocalsRead.trigger(), "locals");
    assert_eq!(ReasonKind::SearchParamRead.trigger(), "searchParams");
    assert_eq!(ReasonKind::UncachedFetch.trigger(), "fetch");
}
