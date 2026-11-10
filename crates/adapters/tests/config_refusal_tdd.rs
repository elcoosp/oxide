//! TDD suite for the §18 config-key refusal seam (`check_config`,
//! V2-R02's config half, spec §V2-R02).
//!
//! Written BEFORE the implementation; every pin imports public API only.
//! The fixture reports stand in for the config layer's feature report the
//! same way the manifest fixtures stand in for `check_requirements`.

use oxide_fw_adapters::{
    ConfigFeatureReport, FeatureSupport, Target, capability_matrix, check_config, degradation,
    support,
};
use oxide_fw_diagnostics::Severity;

// ------------------------------------------------------------------ fixtures

/// The C08 font surface: the `'used'` spelling and the route-shape half
/// (the refusal is their conjunction, so the fixture groups them).
#[derive(Debug, Clone, Copy, Default)]
struct FontSurface {
    used: bool,
    dynamic_routes: bool,
}

/// A stand-in feature report: one field per trait method, so each pin
/// states exactly which surface it exercises.
#[derive(Debug, Clone, Copy, Default)]
struct Report {
    live_routes: bool,
    rust_fns: bool,
    search: bool,
    fonts: FontSurface,
}

impl Report {
    fn none() -> Self {
        Self::default()
    }
    fn all() -> Self {
        Self {
            live_routes: true,
            rust_fns: true,
            search: true,
            fonts: FontSurface {
                used: true,
                dynamic_routes: true,
            },
        }
    }
}

impl ConfigFeatureReport for Report {
    fn live_routes_present(&self) -> bool {
        self.live_routes
    }
    fn rust_fns_present(&self) -> bool {
        self.rust_fns
    }
    fn search_configured(&self) -> bool {
        self.search
    }
    fn font_subset_used(&self) -> bool {
        self.fonts.used
    }
    fn non_static_routes_present(&self) -> bool {
        self.fonts.dynamic_routes
    }
}

fn codes(findings: &[oxide_fw_diagnostics::Diagnostic]) -> Vec<&str> {
    findings.iter().map(|d| d.code.as_str()).collect()
}

// ------------------------------------------------------------ live (AST-LIV-001)

#[test]
fn refuses_live_on_static() {
    // The spec's own red-first pin name: `live: true` routes on the static
    // target are an error with the registry code + fix (V2.K).
    let findings = check_config(
        &Report {
            live_routes: true,
            ..Report::none()
        },
        Target::Static,
    );
    assert_eq!(codes(&findings), vec!["AST-LIV-001"], "codes: {findings:?}");
    let d = &findings[0];
    assert_eq!(d.severity, Severity::Error);
    assert!(
        d.message.contains("static"),
        "message names the target: {}",
        d.message
    );
    assert_eq!(
        d.fix.as_deref(),
        Some("Use `live.transport: 'poll'` or change target"),
        "the registry fix text (V2.K)"
    );
}

#[test]
fn allows_live_on_full_cells() {
    // Live is Full on the native and NodeLike classes (§18 rows): no
    // findings at all — the refusal never fires on a Full cell.
    for t in [
        Target::Binary,
        Target::Docker,
        Target::Node,
        Target::Deno,
        Target::Bun,
    ] {
        let findings = check_config(
            &Report {
                live_routes: true,
                ..Report::none()
            },
            t,
        );
        assert!(
            findings.is_empty(),
            "live on {} must not be refused: {findings:?}",
            t.as_str()
        );
    }
}

#[test]
fn warns_live_on_degraded_cells_with_the_documented_string() {
    // Serverless and Edge are Degraded (the landed F05 ruling): a warning
    // carrying the documented degradation string, never a refusal.
    for t in [
        Target::Lambda,
        Target::Vercel,
        Target::VercelFunctions,
        Target::Netlify,
        Target::Cloudflare,
        Target::CloudflarePages,
    ] {
        let findings = check_config(
            &Report {
                live_routes: true,
                ..Report::none()
            },
            t,
        );
        assert_eq!(codes(&findings), vec!["AST-LIV-001"], "on {}", t.as_str());
        let d = &findings[0];
        assert_eq!(d.severity, Severity::Warning, "degraded on {}", t.as_str());
        let reason = degradation(t, oxide_fw_adapters::Capability::Live)
            .expect("every Degraded Live cell documents its reason");
        assert!(
            d.message.contains(reason),
            "warning carries the degradation string on {}: {}",
            t.as_str(),
            d.message
        );
    }
}

// ------------------------------------------------------ rust fns (RFN-002)

#[test]
fn refuses_rust_fns_on_every_non_native_target() {
    // RFN-002: `*.server.rs` present but the target cannot load native
    // functions — the message names the adapter (spec F08), the fix is the
    // registry text.
    for t in [
        Target::Node,
        Target::Static,
        Target::Cloudflare,
        Target::CloudflarePages,
        Target::Lambda,
        Target::Vercel,
        Target::VercelFunctions,
        Target::Netlify,
        Target::Deno,
        Target::Bun,
    ] {
        let findings = check_config(
            &Report {
                rust_fns: true,
                ..Report::none()
            },
            t,
        );
        assert_eq!(codes(&findings), vec!["RFN-002"], "on {}", t.as_str());
        let d = &findings[0];
        assert_eq!(d.severity, Severity::Error);
        assert!(
            d.message.contains(t.as_str()),
            "RFN-002 names the adapter `{}`: {}",
            t.as_str(),
            d.message
        );
        assert_eq!(
            d.fix.as_deref(),
            Some("Use Binary or Docker, or remove the Rust functions")
        );
    }
}

#[test]
fn allows_rust_fns_on_native_targets() {
    for t in [Target::Binary, Target::Docker] {
        let findings = check_config(
            &Report {
                rust_fns: true,
                ..Report::none()
            },
            t,
        );
        assert!(
            findings.is_empty(),
            "rust fns on {} must be allowed: {findings:?}",
            t.as_str()
        );
    }
}

// ------------------------------------------------------ search (AST-SRC-001)

#[test]
fn refuses_search_on_edge() {
    // F09's `search` key on the Edge class is the registry's AST-SRC-001
    // row: an error with the registry fix text (V2.K). The Edge class is
    // the ONLY Unsupported cell in the Search row (V2-F09: "Unsupported on
    // edge until a WASM build exists").
    for t in [Target::Cloudflare, Target::CloudflarePages] {
        let findings = check_config(
            &Report {
                search: true,
                ..Report::none()
            },
            t,
        );
        assert_eq!(codes(&findings), vec!["AST-SRC-001"], "on {}", t.as_str());
        let d = &findings[0];
        assert_eq!(d.severity, Severity::Error, "on {}", t.as_str());
        assert!(
            d.message.contains(t.as_str()),
            "AST-SRC-001 names the target `{}`: {}",
            t.as_str(),
            d.message
        );
        assert_eq!(
            d.fix.as_deref(),
            Some("Use static JSON search or a Binary or Docker target")
        );
    }
}

#[test]
fn warns_degraded_search_on_node() {
    // The spec's own red-first pin name (V2-R02): search on Node is
    // Degraded (static JSON) — a warning carrying the documented
    // degradation string, never a refusal. The same shape holds for every
    // other Degraded cell in the Search row (NodeLike, Static, Serverless).
    for t in [
        Target::Node,
        Target::Deno,
        Target::Bun,
        Target::Static,
        Target::Lambda,
        Target::Vercel,
        Target::VercelFunctions,
        Target::Netlify,
    ] {
        let findings = check_config(
            &Report {
                search: true,
                ..Report::none()
            },
            t,
        );
        assert_eq!(codes(&findings), vec!["AST-SRC-001"], "on {}", t.as_str());
        let d = &findings[0];
        assert_eq!(d.severity, Severity::Warning, "degraded on {}", t.as_str());
        let reason = degradation(t, oxide_fw_adapters::Capability::Search)
            .expect("every Degraded Search cell documents its reason");
        assert!(
            d.message.contains(reason),
            "warning carries the degradation string on {}: {}",
            t.as_str(),
            d.message
        );
    }
}

#[test]
fn allows_search_on_native_targets() {
    // The Search row is Full on the native classes: the configured key
    // produces no finding at all — the refusal never fires on a Full cell.
    for t in [Target::Binary, Target::Docker] {
        let findings = check_config(
            &Report {
                search: true,
                ..Report::none()
            },
            t,
        );
        assert!(
            findings.is_empty(),
            "search on {} must not be refused: {findings:?}",
            t.as_str()
        );
    }
}

// ------------------------------------------------------- fonts (AST-FNT-001)

#[test]
fn refuses_fonts_used_with_non_static_routes_on_every_target() {
    // C08: glyph-usage subsetting is only sound when all text is known, so
    // the rule is target-INDEPENDENT — it fires wherever a non-static route
    // exists, on every row of the matrix.
    for t in Target::ALL {
        let findings = check_config(
            &Report {
                fonts: FontSurface {
                    used: true,
                    dynamic_routes: true,
                },
                ..Report::none()
            },
            *t,
        );
        assert_eq!(codes(&findings), vec!["AST-FNT-001"], "on {}", t.as_str());
        let d = &findings[0];
        assert_eq!(d.severity, Severity::Error);
        assert_eq!(
            d.fix.as_deref(),
            Some("Use `latin` or `unicode-range`, or make all routes static")
        );
    }
}

#[test]
fn allows_fonts_used_when_every_route_is_static() {
    for t in Target::ALL {
        let findings = check_config(
            &Report {
                fonts: FontSurface {
                    used: true,
                    dynamic_routes: false,
                },
                ..Report::none()
            },
            *t,
        );
        assert_eq!(
            codes(&findings),
            Vec::<&str>::new(),
            "`subset: 'used'` with all-static routes on {} must pass",
            t.as_str()
        );
    }
}

#[test]
fn fonts_rule_needs_both_halves() {
    // `subset: 'used'` alone (all static) passes; dynamic routes alone
    // (subset latin) pass. The refusal is the CONJUNCTION.
    for t in Target::ALL {
        assert_eq!(
            codes(&check_config(
                &Report {
                    fonts: FontSurface {
                        used: true,
                        dynamic_routes: false,
                    },
                    ..Report::none()
                },
                *t
            )),
            Vec::<&str>::new()
        );
        assert_eq!(
            codes(&check_config(
                &Report {
                    fonts: FontSurface {
                        used: false,
                        dynamic_routes: true,
                    },
                    ..Report::none()
                },
                *t
            )),
            Vec::<&str>::new()
        );
    }
}

// ------------------------------------------------------------------- shape

#[test]
fn every_target_with_an_empty_report_has_no_findings() {
    // The default project (no live routes, no rust fns, latin subset, all
    // static) deploys to every target without a single finding.
    for t in Target::ALL {
        let findings = check_config(&Report::none(), *t);
        assert!(
            findings.is_empty(),
            "empty report on {}: {findings:?}",
            t.as_str()
        );
    }
}

#[test]
fn finding_order_is_deterministic() {
    // All surfaces firing on Static: live, rust fns and search in
    // capability declaration order (Live < RustFns < Search), then the
    // target-independent fonts rule last. The order is part of the seam's
    // contract (the build prints them in this order).
    let findings = check_config(&Report::all(), Target::Static);
    assert_eq!(
        codes(&findings),
        vec!["AST-LIV-001", "RFN-002", "AST-SRC-001", "AST-FNT-001"],
        "order: {findings:?}"
    );
}

#[test]
fn severity_split_on_a_degraded_target() {
    // Lambda with everything enabled: live degrades (warning), rust fns
    // are unsupported (error), fonts is target-independent (error).
    let findings = check_config(&Report::all(), Target::Lambda);
    let errors: Vec<&str> = findings
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.code.as_str())
        .collect();
    let warnings: Vec<&str> = findings
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .map(|d| d.code.as_str())
        .collect();
    assert_eq!(errors, vec!["RFN-002", "AST-FNT-001"]);
    assert_eq!(warnings, vec!["AST-LIV-001", "AST-SRC-001"]);
}

#[test]
fn the_seam_consults_the_matrix_not_a_private_table() {
    // The refusal's verdict for each capability cell equals what
    // `support` returns for it — the seam is a consumer of the matrix,
    // never a second opinion (the R02 shape: one source of truth).
    for t in Target::ALL {
        let report = Report {
            live_routes: true,
            rust_fns: true,
            search: true,
            ..Report::none()
        };
        for d in check_config(&report, *t) {
            let expected = match d.code.as_str() {
                "AST-LIV-001" => support(*t, oxide_fw_adapters::Capability::Live),
                "RFN-002" => support(*t, oxide_fw_adapters::Capability::RustFns),
                "AST-SRC-001" => support(*t, oxide_fw_adapters::Capability::Search),
                other => panic!("unexpected code {other}"),
            };
            match d.severity {
                Severity::Error => assert_eq!(expected, FeatureSupport::Unsupported),
                Severity::Warning => assert_eq!(expected, FeatureSupport::Degraded),
                Severity::Info => panic!("the refusal never emits info"),
            }
        }
    }
}

#[test]
fn the_matrix_row_agrees_with_the_refusal_wiring() {
    // Sanity linking the two halves: the static target's matrix says Live
    // is Unsupported — the exact cell the refusal turns into AST-LIV-001.
    let row = capability_matrix(Target::Static);
    assert_eq!(
        row.get(&oxide_fw_adapters::Capability::Live),
        Some(&FeatureSupport::Unsupported)
    );
}
