//! The config-side refusal walkthrough (V2-R02's config half + V2-F09's
//! `search` key, spec §V2-R02 / Appendix V2.L): load a typed config, map it
//! into the adapters crate's `ConfigFeatureReport` through
//! `OxideConfig::feature_report`, and watch `check_config` refuse what a
//! target cannot honor — the exact pipeline `oxide build` runs before any
//! deploy-artifact work.
//!
//! This crate had no consumer example before; this one is the copy-paste
//! starting point for tooling that wants the §18 verdicts without the CLI.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-config --example feature_report
//! ```

use oxide_fw_adapters::{ConfigFeatureReport, Target, check_config};
use oxide_fw_config::{CollectionDef, OxideConfig, ProjectFacts};

fn main() {
    // The typed config the §14 pipeline produces. Here: a project that
    // opted into the built-in search (F09) and glyph-usage font subsetting
    // (C08) — the two config-KEY surfaces the refusal consults. The other
    // surfaces (live routes, `*.server.rs` presence, non-static routes)
    // are project facts only the driver can know; the driver fills them
    // through `ProjectFacts`.
    let cfg: OxideConfig = serde_json::from_str(
        r#"{
            "adapter": "cloudflare",
            "search": { "collections": ["posts", "docs"], "fields": { "title": 3, "body": 1 } },
            "fonts": { "subset": ["used"] }
        }"#,
    )
    .expect("a valid typed config");

    // The driver's facts, carried verbatim — the config layer is never the
    // judge of them (the recorded V2-R02 ruling). Here: a live route opted
    // in, no `*.server.rs` files, one non-static route.
    let facts = ProjectFacts {
        live_routes: true,
        rust_fns: false,
        non_static_routes: true,
    };

    // The mapping + the refusal: pure functions over the two inputs.
    let report = cfg.feature_report(facts);
    println!("§18 config-key refusal — the build's verdict per target\n");

    for t in Target::ALL {
        let findings = check_config(&report, *t);
        if findings.is_empty() {
            println!("{:>16}  ships", t.as_str());
            continue;
        }
        for d in &findings {
            let kind = match d.severity {
                oxide_fw_diagnostics::Severity::Error => "REFUSED",
                _ => "degrades",
            };
            println!(
                "{:>16}  {:<8} [{}] {} (fix: {})",
                t.as_str(),
                kind,
                d.code,
                d.message,
                d.fix.as_deref().unwrap_or("(none)")
            );
        }
    }

    // The key subtlety the mapping owns: the §16.5 collection indexes are
    // static JSON — the registry's own remedy for unsupported targets — so
    // they must NOT set the "search configured" surface. Only the
    // `search.collections` key does.
    let mut indexed_only = OxideConfig::default();
    indexed_only.content.collections.push(CollectionDef {
        name: "posts".into(),
        dir: "content/posts".into(),
        indexed_fields: Vec::new(),
    });
    let report = indexed_only.feature_report(ProjectFacts::default());
    assert!(
        !report.search_configured(),
        "content collections index to static JSON and never refuse"
    );
    println!(
        "\ncontent collections WITHOUT the `search` key → search_configured() = false (the static-JSON remedy never refuses)"
    );
}
