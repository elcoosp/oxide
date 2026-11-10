//! Criterion bench for the §18 config-key refusal seam (V2-R02's config
//! half, spec §V2-R02) — the `check_config` verdict the build path runs once
//! per deploy.
//!
//! The shapes: the all-surfaces report over every target (the worst case —
//! three findings allocated per refusal target, zero per Full cell), the
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

/// Per-field parameterizable report for the single-surface splits.
struct Spoof(bool, bool, bool);

impl ConfigFeatureReport for Spoof {
    fn live_routes_present(&self) -> bool {
        self.0
    }
    fn rust_fns_present(&self) -> bool {
        self.1
    }
    fn font_subset_used(&self) -> bool {
        self.2
    }
    fn non_static_routes_present(&self) -> bool {
        // The fonts rule is the conjunction — the splits that set
        // `font_subset_used` carry the dynamic-routes half too.
        self.2
    }
}

fn bench_config_refusal(c: &mut Criterion) {
    let mut group = c.benchmark_group("config_refusal");

    // The worst case: all surfaces on, every target evaluated. Refusal
    // targets (Static) allocate three findings; Full targets allocate none.
    group.bench_function("all_surfaces_all_targets", |b| {
        b.iter(|| {
            for t in Target::ALL {
                let _ = check_config(&Spoof(true, true, true), *t);
            }
        });
    });

    // The default project: the build path's common case (nothing to refuse,
    // nothing to warn).
    group.bench_function("empty_report_all_targets", |b| {
        b.iter(|| {
            for t in Target::ALL {
                let _ = check_config(&Spoof(false, false, false), *t);
            }
        });
    });

    // Single-surface splits on the refusal-heavy target: which report field
    // allocates what (live → the AST-LIV-001 error, rust fns → RFN-002,
    // fonts → the target-independent AST-FNT-001).
    for (name, live, rust_fns, fonts_used) in [
        ("live_only", true, false, false),
        ("rust_fns_only", false, true, false),
        ("fonts_used_dynamic", false, false, true),
    ] {
        group.bench_function(name, |b| {
            b.iter(|| {
                let _ = check_config(&Spoof(live, rust_fns, fonts_used), Target::Static);
            });
        });
    }

    group.finish();
}

criterion_group!(benches, bench_config_refusal);
criterion_main!(benches);
