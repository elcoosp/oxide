//! TDD suite for the config side of the §18 config-key refusal seam
//! (V2-R02's config half): the `OxideConfig` → `ConfigFeatureReport`
//! mapping and the `ProjectFacts` the driver supplies.
//!
//! Written BEFORE the implementation; every pin imports public API only.

use oxide_fw_adapters::{ConfigFeatureReport, Target, check_config};
use oxide_fw_config::{OxideConfig, ProjectFacts};

#[test]
fn default_config_and_default_facts_report_no_features() {
    // The inert-defaults contract (the same one `live`/`stream`/`brownout`
    // set): a default config with default facts exposes NO feature surface,
    // so the refusal never fires on a project that configured nothing.
    let cfg = OxideConfig::default();
    let report = cfg.feature_report(ProjectFacts::default());
    assert!(!report.live_routes_present());
    assert!(!report.rust_fns_present());
    assert!(!report.font_subset_used());
    assert!(!report.non_static_routes_present());
    for t in Target::ALL {
        assert!(check_config(&report, *t).is_empty(), "on {}", t.as_str());
    }
}

#[test]
fn font_subset_used_maps_the_used_entry() {
    // C08: `fonts.subset` is an array of subset names; the glyph-usage
    // spelling is the `"used"` ENTRY (`subset: ['used']`).
    let mut cfg = OxideConfig::default();
    cfg.fonts.subset = vec!["latin".into(), "used".into()];
    assert!(
        cfg.feature_report(ProjectFacts::default())
            .font_subset_used()
    );

    // The default subset list carries no `used` entry.
    let cfg = OxideConfig::default();
    assert!(
        !cfg.feature_report(ProjectFacts::default())
            .font_subset_used()
    );
}

#[test]
fn font_subset_parses_from_camel_case_json() {
    // The TS key round trip: `fonts.subset` arrives inside the `fonts`
    // object of a config literal.
    let cfg: OxideConfig =
        serde_json::from_str(r#"{"fonts": {"providers": [], "subset": ["used"], "families": []}}"#)
            .unwrap();
    assert!(
        cfg.feature_report(ProjectFacts::default())
            .font_subset_used()
    );
}

#[test]
fn project_facts_pass_through_untouched() {
    // live/rust-fns/route facts are project state, not config keys — the
    // report carries them verbatim so the driver (the CLI build, which
    // scanned the route tree and walked the sources) is the single judge.
    let cfg = OxideConfig::default();
    let report = cfg.feature_report(ProjectFacts {
        live_routes: true,
        rust_fns: true,
        non_static_routes: true,
    });
    assert!(report.live_routes_present());
    assert!(report.rust_fns_present());
    assert!(report.non_static_routes_present());
}

#[test]
fn end_to_end_fonts_refusal_through_the_config_mapping() {
    // The full seam: a config with `fonts.subset: ['used']` and a
    // non-static route refuses on EVERY target (the C08 rule is
    // target-independent), with the registry code.
    let mut cfg = OxideConfig::default();
    cfg.fonts.subset = vec!["used".into()];
    let report = cfg.feature_report(ProjectFacts {
        non_static_routes: true,
        ..ProjectFacts::default()
    });
    for t in Target::ALL {
        let findings = check_config(&report, *t);
        assert_eq!(findings.len(), 1, "on {}", t.as_str());
        assert_eq!(findings[0].code, "AST-FNT-001");
    }
}

#[test]
fn end_to_end_live_refusal_through_the_config_mapping() {
    // The F05 deferral closes: live route facts + the static target produce
    // the `AST-LIV-001` refusal through the config mapping.
    let cfg = OxideConfig::default();
    let report = cfg.feature_report(ProjectFacts {
        live_routes: true,
        ..ProjectFacts::default()
    });
    let findings = check_config(&report, Target::Static);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].code, "AST-LIV-001");
    assert_eq!(findings[0].severity, oxide_fw_diagnostics::Severity::Error);
}
