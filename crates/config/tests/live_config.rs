//! Integration tests for the V2-F05 `live` config surface (§14 typed
//! config, §V2-F05 step 8): the top-level `live` object parses through the
//! typed [`OxideConfig`] with the spec's defaults, invalid knobs fail the
//! load with the DSP-111 diagnostic, and the JSON schema
//! (`config_schema()`) declares the section so editors can complete it.
//!
//! Written FIRST (red): failed on unresolved-symbol errors before
//! `LiveConfig`/`LiveTransport` and the `live` validation existed.

use oxide_fw_config::{LiveTransport, OxideConfig, config_schema, load_with_env};
use std::collections::BTreeMap;

const EMPTY_ENV: BTreeMap<String, String> = BTreeMap::new();

/// Write `oxide.config.json` into a temp project dir and load it.
fn load_config(
    body: &str,
) -> Result<oxide_fw_config::LoadedConfig, oxide_fw_diagnostics::OxideError> {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("oxide.config.json");
    std::fs::write(&path, body).expect("write config");
    let dir = tmp.path().to_path_buf();
    load_with_env(&dir, "dev", BTreeMap::new(), &EMPTY_ENV)
}

fn config_json(live: &str) -> String {
    format!(
        r#"{{
            "framework": {{ "adapter": "react" }},
            "live": {live}
        }}"#
    )
}

#[test]
fn live_is_a_known_top_level_key_with_the_spec_defaults() {
    let loaded = load_config(&config_json("{}")).expect("an empty live section loads");
    let live = &loaded.config.live;
    assert_eq!(live.max_subscribers, 5_000, "spec step 8 default");
    assert_eq!(live.debounce_ms, 50, "spec step 8 default");
    assert_eq!(live.keepalive_seconds, 20, "spec step 8 default");
    assert_eq!(live.max_reruns_per_second, 20, "spec step 8 default");
    assert_eq!(live.per_ip_max, 20, "spec security bullet default");
    assert_eq!(
        live.transport,
        LiveTransport::Sse,
        "sse is the default transport"
    );
}

#[test]
fn live_absent_means_the_defaults_still_parse() {
    let loaded = load_config(r#"{ "framework": { "adapter": "react" } }"#).expect("loads");
    assert_eq!(loaded.config.live.max_subscribers, 5_000);
    assert_eq!(loaded.config.live.transport, LiveTransport::Sse);
}

#[test]
fn live_knobs_parse_camel_case_and_the_poll_transport() {
    let loaded = load_config(&config_json(
        r#"{ "maxSubscribers": 1000, "debounceMs": 100, "keepaliveSeconds": 10,
             "maxRerunsPerSecond": 5, "perIpMax": 4, "transport": "poll" }"#,
    ))
    .expect("a full live section loads");
    let live = &loaded.config.live;
    assert_eq!(live.max_subscribers, 1_000);
    assert_eq!(live.debounce_ms, 100);
    assert_eq!(live.keepalive_seconds, 10);
    assert_eq!(live.max_reruns_per_second, 5);
    assert_eq!(live.per_ip_max, 4);
    assert_eq!(live.transport, LiveTransport::Poll);
}

#[test]
fn unknown_live_keys_are_rejected() {
    let err = load_config(&config_json(r#"{ "maxSubscribaz": 1 }"#))
        .expect_err("deny_unknown_fields guards the section");
    assert!(
        err.0.message.contains("maxSubscribaz"),
        "the diagnostic names the offending key: {}",
        err.0.message
    );
}

#[test]
fn zero_keepalive_fails_with_dsp_111() {
    let err = load_config(&config_json(r#"{ "keepaliveSeconds": 0 }"#))
        .expect_err("a 0s keepalive would busy-loop every connection");
    assert_eq!(err.0.code, "DSP-111", "the §V2-F05 load-validation code");
}

#[test]
fn zero_max_subscribers_fails_with_dsp_111() {
    let err = load_config(&config_json(r#"{ "maxSubscribers": 0 }"#))
        .expect_err("the hub must hold at least one subscriber");
    assert_eq!(err.0.code, "DSP-111");
}

#[test]
fn per_ip_max_above_max_subscribers_fails_with_dsp_111() {
    let err = load_config(&config_json(r#"{ "maxSubscribers": 10, "perIpMax": 11 }"#))
        .expect_err("the per-IP cap cannot exceed the global cap");
    assert_eq!(err.0.code, "DSP-111");
}

#[test]
fn unknown_transport_fails_the_schema_check() {
    assert!(
        load_config(&config_json(r#"{ "transport": "webhooks" }"#)).is_err(),
        "transport is an sse|poll enum"
    );
}

#[test]
fn schema_declares_the_live_section() {
    let schema = config_schema();
    let text = schema.to_string();
    assert!(
        text.contains("\"live\""),
        "top-level `live` property present"
    );
    assert!(
        text.contains("maxSubscribers") && text.contains("keepaliveSeconds"),
        "the camelCase knobs are in the schema"
    );
    assert!(
        text.contains("\"sse\"") && text.contains("\"poll\""),
        "the transport enum is in the schema"
    );
}

#[test]
fn typed_config_round_trips_the_live_section() {
    let json = config_json(r#"{ "maxSubscribers": 7 }"#);
    let parsed: OxideConfig = serde_json::from_str(&json).expect("typed parse");
    assert_eq!(parsed.live.max_subscribers, 7);
    let out = serde_json::to_string(&parsed).expect("serialize");
    assert!(
        out.contains("\"maxSubscribers\":7"),
        "camelCase on the wire"
    );
}
