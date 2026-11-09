//! Consumer example for the V2-F03 `interactionReplay` config surface:
//! how the typed config flag reaches the emitted hydration bootstrap
//! (`window.__oxide`), the last leg of the F03 deferral contract
//! (`docs/deviations.md`).
//!
//! Shows the two integration points a pipeline (or an adapter) uses:
//!
//! 1. [`oxide_fw_config::OxideConfig::interaction_replay`] — the typed
//!    flag, default `true` (flag absent = enabled); a non-boolean value
//!    fails the load with the DSP-114 diagnostic.
//! 2. [`oxide_fw_render::Injector::with_interaction_replay`] /
//!    [`oxide_fw_render::hydration_bootstrap_with_replay`] — the
//!    opt-out advertisement. `false` emits `interactionReplay:false`
//!    between the optional `live` field and the `data` reference; `true`
//!    keeps the pre-F03 bytes unchanged (additive substrate).
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-render --example interaction_replay
//! ```

use oxide_fw_config::OxideConfig;
use oxide_fw_render::{HeadTags, Injector};

const B64: &str = "eyJkIjoxfQ==";
const NONCE: &str = "n0nce";
const BUILD: &str = "deadbeef00000001";

fn main() {
    // The typed flag: absent = enabled (the §V2-F03 default contract).
    let default_cfg: OxideConfig =
        serde_json::from_str(r#"{ "framework": { "adapter": "react" } }"#).expect("default");
    let opted_out: OxideConfig = serde_json::from_str(
        r#"{ "framework": { "adapter": "react" }, "interactionReplay": false }"#,
    )
    .expect("opted out");
    println!(
        "default interactionReplay  : {}",
        default_cfg.interaction_replay
    );
    println!(
        "opted-out interactionReplay: {}",
        opted_out.interaction_replay
    );

    // The direct emitter: `false` advertises the opt-out between the
    // optional `live` field and the `data` reference; `true` is
    // byte-identical to the pre-F03 shapes.
    println!();
    println!(
        "{}",
        oxide_fw_render::hydration_bootstrap_with_replay(
            B64,
            "routes/index",
            NONCE,
            BUILD,
            false,
            false,
        )
    );

    // The Injector path (what the render pipeline uses): the default
    // never emits the field; `.with_interaction_replay(false)` does.
    let page = "<!doctype html><html><head></head><body><h1>hi</h1></body></html>";
    let default_html = Injector::new(
        HeadTags::default(),
        NONCE.to_string(),
        "routes/index".into(),
    )
    .with_build_id(BUILD)
    .inject_into_html(page)
    .expect("inject");
    let opted_html = Injector::new(
        HeadTags::default(),
        NONCE.to_string(),
        "routes/index".into(),
    )
    .with_build_id(BUILD)
    .with_interaction_replay(false)
    .inject_into_html(page)
    .expect("inject");
    println!();
    println!(
        "default bootstrap carries the field: {}",
        default_html.contains("interactionReplay")
    );
    println!(
        "opted-out bootstrap carries the field: {}",
        opted_html.contains("interactionReplay:false")
    );
}
