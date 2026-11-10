//! The config-key refusal walkthrough (V2-R02's config half, spec §V2-R02):
//! print the §18 verdicts `oxide build` produces per deploy target, watch
//! the refusal consult the matrix (never a second opinion), and watch a
//! Degraded cell warn instead of refuse.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-adapters --example config_refusal
//! ```

use oxide_fw_adapters::{ConfigFeatureReport, FeatureSupport, Target, check_config, support};
use oxide_fw_diagnostics::Severity;

/// A project with every surface the seam consults enabled: a live route
/// (F05), `*.server.rs` functions (F08), `fonts.subset: ['used']` (C08)
/// and a dynamic route.
struct EverythingProject;

impl ConfigFeatureReport for EverythingProject {
    fn live_routes_present(&self) -> bool {
        true
    }
    fn rust_fns_present(&self) -> bool {
        true
    }
    fn font_subset_used(&self) -> bool {
        true
    }
    fn non_static_routes_present(&self) -> bool {
        true
    }
}

/// A project that configured nothing — the inert-defaults contract.
struct DefaultProject;

impl ConfigFeatureReport for DefaultProject {
    fn live_routes_present(&self) -> bool {
        false
    }
    fn rust_fns_present(&self) -> bool {
        false
    }
    fn font_subset_used(&self) -> bool {
        false
    }
    fn non_static_routes_present(&self) -> bool {
        false
    }
}

fn main() {
    println!("§18 config-key refusal (V2-R02 config half) — the build's verdict per target\n");

    // 1. The default project deploys everywhere without a single finding
    //    (the refusal inherits the inert-defaults contract).
    let default_findings: usize = Target::ALL
        .iter()
        .map(|t| check_config(&DefaultProject, *t).len())
        .sum();
    println!(
        "default project × 12 targets → {default_findings} findings (nothing enabled, nothing refused)\n"
    );

    // 2. The everything project: errors refuse, warnings degrade.
    println!("{:>16}  verdict", "target");
    for t in Target::ALL {
        let findings = check_config(&EverythingProject, *t);
        let errors = findings
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| d.code.clone())
            .collect::<Vec<_>>()
            .join(" ");
        let warnings = findings
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .map(|d| d.code.clone())
            .collect::<Vec<_>>()
            .join(" ");
        let verdict = if errors.is_empty() {
            "ships"
        } else {
            "REFUSED"
        };
        println!(
            "{:>16}  {verdict:<8} errors: [{errors}]  warnings: [{warnings}]",
            t.as_str()
        );
    }

    // 3. The seam consults the matrix — for every finding, the cell it
    //    cites has exactly the severity the matrix states.
    let mut agreed = 0usize;
    for t in Target::ALL {
        for d in check_config(&EverythingProject, *t) {
            let cell = match d.code.as_str() {
                "AST-LIV-001" => support(*t, oxide_fw_adapters::Capability::Live),
                "RFN-002" => support(*t, oxide_fw_adapters::Capability::RustFns),
                "AST-FNT-001" => continue, // target-independent (C08)
                other => panic!("unexpected code {other}"),
            };
            assert!(
                (d.severity == Severity::Error && cell == FeatureSupport::Unsupported)
                    || (d.severity == Severity::Warning && cell == FeatureSupport::Degraded),
                "severity agrees with the matrix cell"
            );
            agreed += 1;
        }
    }
    println!(
        "\n{agreed} findings, every severity equal to its matrix cell (the seam is a matrix consumer, never a second opinion)"
    );

    // 4. The registry fix rides along (V2.K): print one refusal the way the
    //    build prints it.
    let refusal = check_config(&EverythingProject, Target::Static)
        .into_iter()
        .find(|d| d.severity == Severity::Error)
        .expect("the static target refuses the everything project");
    println!(
        "\nexample refusal on `static`:\n  [{}] {}\n    fix: {}",
        refusal.code,
        refusal.message,
        refusal.fix.as_deref().unwrap_or("(none)")
    );
}
