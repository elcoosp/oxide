//! Integration tests for the `oxide-fw-dispatch` unified request locals
//! (spec v0.2, V2-A05 step 1 — the V8-free `RequestLocals` substrate).
//!
//! §V2-A05: one `Locals` value reaches loaders, layout loaders, actions,
//! `route.ts` and render. The pipeline owns the reserved keys
//! `session` / `locale` / `experiments`; a middleware that sets one gets a
//! dev warning (`DSP-101`) and the reserved value wins — these tests pin
//! that contract so the `stage_prelude` wiring inherits exactly these
//! semantics.

use oxide_fw_dispatch::locals::{RESERVED_KEYS, RequestLocals};

#[test]
fn to_value_merges_user_and_reserved() {
    let mut locals = RequestLocals::default();
    locals
        .user
        .insert("section".into(), serde_json::json!("blog"));
    locals.session = Some(serde_json::json!({ "user": "elmo" }));
    locals.locale = "fr".into();
    locals
        .experiments
        .insert("hero".into(), serde_json::json!("b"));

    assert_eq!(
        locals.to_value(),
        serde_json::json!({
            "section": "blog",
            "session": { "user": "elmo" },
            "locale": "fr",
            "experiments": { "hero": "b" },
        })
    );
}

#[test]
fn reserved_keys_win_over_user() {
    // A middleware (or loader) that smuggles a reserved key into the user
    // map must not win: the pipeline-owned value takes precedence.
    let mut locals = RequestLocals::default();
    locals
        .user
        .insert("session".into(), serde_json::json!({ "forged": true }));
    locals.user.insert("locale".into(), serde_json::json!("xx"));
    locals
        .user
        .insert("experiments".into(), serde_json::json!({ "forged": 1 }));
    locals.session = Some(serde_json::json!({ "user": "elmo" }));
    locals.locale = "fr".into();
    locals
        .experiments
        .insert("hero".into(), serde_json::json!("b"));

    let value = locals.to_value();
    assert_eq!(value["session"], serde_json::json!({ "user": "elmo" }));
    assert_eq!(value["locale"], "fr");
    assert_eq!(value["experiments"], serde_json::json!({ "hero": "b" }));
}

#[test]
fn empty_reserved_keys_are_omitted() {
    // No session, empty locale, no experiments: the composed value carries
    // only the user keys — an empty object stays an empty object.
    let locals = RequestLocals::default();
    assert_eq!(locals.to_value(), serde_json::json!({}));
}

#[test]
fn empty_user_composes_reserved_only() {
    // An empty user map plus a resolved locale: the composed value carries
    // only the reserved key — the shape `stage_prelude` hands to a stage
    // that no middleware touched.
    let locals = RequestLocals {
        locale: "fr".into(),
        ..RequestLocals::default()
    };
    let value = locals.to_value();
    assert_eq!(value, serde_json::json!({ "locale": "fr" }));
}

#[test]
fn apply_middleware_locals_merges_later_wins() {
    let mut locals = RequestLocals::default();
    locals
        .user
        .insert("section".into(), serde_json::json!("root"));
    let collisions = locals.apply_middleware_locals(serde_json::json!({
        "section": "blog",
        "extra": 1,
    }));
    assert!(collisions.is_empty(), "no reserved keys touched");
    assert_eq!(
        locals.user.get("section"),
        Some(&serde_json::json!("blog")),
        "later (inner) middleware wins"
    );
    assert_eq!(locals.user.get("extra"), Some(&serde_json::json!(1)));
}

#[test]
fn apply_middleware_locals_reports_reserved_collisions_dsp101() {
    // DSP-101: a middleware that sets a reserved key is reported, and the
    // reserved value it staged must NOT land in the user map (the
    // pipeline-owned slot decides at to_value time).
    let mut locals = RequestLocals::default();
    let collisions = locals.apply_middleware_locals(serde_json::json!({
        "session": { "forged": true },
        "locale": "xx",
        "experiments": { "forged": 1 },
        "ok": true,
    }));
    assert_eq!(
        collisions,
        vec![
            "experiments".to_string(),
            "locale".to_string(),
            "session".to_string()
        ],
        "collided reserved keys, sorted, exactly as DSP-101 needs"
    );
    assert!(!locals.user.contains_key("session"));
    assert!(!locals.user.contains_key("locale"));
    assert!(!locals.user.contains_key("experiments"));
    assert_eq!(locals.user.get("ok"), Some(&serde_json::json!(true)));
}

#[test]
fn apply_middleware_locals_ignores_non_object() {
    // A middleware returning `locals: 42` (or null) contributes nothing
    // and reports nothing — the pipeline never panics on shape.
    let mut locals = RequestLocals::default();
    let null_result = locals.apply_middleware_locals(serde_json::Value::Null);
    assert_eq!(null_result, Vec::<String>::new());
    let number_result = locals.apply_middleware_locals(serde_json::json!(42));
    assert_eq!(number_result, Vec::<String>::new());
    assert!(locals.user.is_empty());
}

#[test]
fn reserved_keys_are_the_documented_three() {
    // The reserve list is part of the documented contract: `session`,
    // `locale`, `experiments`. Adding a fourth is a deliberate, doc-visible
    // change — this pin keeps it honest.
    assert_eq!(RESERVED_KEYS, ["experiments", "locale", "session"]);
}

#[test]
fn session_only_composes_when_present() {
    let mut locals = RequestLocals::default();
    locals.user.insert("a".into(), serde_json::json!(1));
    locals.locale = "fr".into();
    // session stays None: no `session` key in the composed value.
    assert_eq!(
        locals.to_value(),
        serde_json::json!({ "a": 1, "locale": "fr" })
    );
}
