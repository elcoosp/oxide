//! Criterion bench for the §18 config-key refusal seam (V2-R02's config
//! half, spec §V2-R02) — the `check_config` verdict the build path runs once
//! per deploy.
//!
//! The shapes: the all-surfaces report over every target (the worst case —
//! four findings allocated per refusal target, zero per Full cell), the
//! empty report (the default project: twelve targets, zero findings), and
//! the single-surface splits (which report field costs what).
//!
//! Run with:
//!
//! ```text
//! cargo bench -p oxide-fw-adapters --features bench-util -- config_refusal
//! ```

use criterion::{Criterion, criterion_group, criterion_main};
use oxide_fw_adapters::{ConfigFeatureReport, Target, check_config};

/// Per-surface bitmask for the all-surfaces and single-surface shapes
/// (bit 0 live, bit 1 rust fns, bit 2 search, bit 3 fonts).
struct MaskedReport(u8);

impl ConfigFeatureReport for MaskedReport {
    fn live_routes_present(&self) -> bool {
        self.0 & 0b0001 != 0
    }
    fn rust_fns_present(&self) -> bool {
        self.0 & 0b0010 != 0
    }
    fn search_configured(&self) -> bool {
        self.0 & 0b0100 != 0
    }
    fn font_subset_used(&self) -> bool {
        self.0 & 0b1000 != 0
    }
    fn non_static_routes_present(&self) -> bool {
        // The fonts rule is the conjunction — the splits that set
        // `font_subset_used` carry the dynamic-routes half too.
        self.0 & 0b1000 != 0
    }
}

const ALL_SURFACES: u8 = 0b1111;

fn bench_config_refusal(c: &mut Criterion) {
    let mut group = c.benchmark_group("config_refusal");

    // The worst case: all surfaces on, every target evaluated. Refusal
    // targets (Static) allocate four findings; Full targets allocate none.
    group.bench_function("all_surfaces_all_targets", |b| {
        b.iter(|| {
            for t in Target::ALL {
                let _ = check_config(&MaskedReport(ALL_SURFACES), *t);
            }
        });
    });

    // The default project: the build path's common case (nothing to refuse,
    // nothing to warn).
    group.bench_function("empty_report_all_targets", |b| {
        b.iter(|| {
            for t in Target::ALL {
                let _ = check_config(&MaskedReport(0), *t);
            }
        });
    });

    // Single-surface splits on the refusal-heavy target: which report field
    // allocates what (live → the AST-LIV-001 error, rust fns → RFN-002,
    // search → the AST-SRC-001 warning on Static's degraded cell, fonts →
    // the target-independent AST-FNT-001).
    for (name, mask) in [
        ("live_only", 0b0001_u8),
        ("rust_fns_only", 0b0010),
        ("search_configured", 0b0100),
        ("fonts_used_dynamic", 0b1000),
    ] {
        group.bench_function(name, |b| {
            b.iter(|| {
                let _ = check_config(&MaskedReport(mask), Target::Static);
            });
        });
    }

    group.finish();
}

criterion_group!(benches, bench_config_refusal);
criterion_main!(benches);
