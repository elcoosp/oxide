//! Criterion bench for the V2-E11 docs generators — the reference-page
//! renders `oxide docs generate` performs (and `--check` re-renders on
//! every CI run).
//!
//! The shape: each registered page rendered fresh (the whole page, the
//! docs path's worst case) — the config walk (the schema tree flattened),
//! the clap tree walk, the matrix fold and the registry parse + render.
//!
//! Run with:
//!
//! ```text
//! cargo bench -p oxide-fw-cli --features bench-util -- docs
//! ```

use criterion::{Criterion, criterion_group, criterion_main};
use oxide_fw_cli::docs_gen::{
    CapabilitiesPage, CliPage, ConfigPage, ErrorsPage, ReferenceGenerator,
};

fn bench_docs_generate(c: &mut Criterion) {
    let mut group = c.benchmark_group("docs_generate");

    group.bench_function("capabilities_page", |b| {
        b.iter(|| {
            CapabilitiesPage
                .render()
                .expect("capabilities page renders")
        });
    });
    group.bench_function("cli_page", |b| {
        b.iter(|| CliPage.render().expect("cli page renders"));
    });
    group.bench_function("config_page", |b| {
        b.iter(|| ConfigPage.render().expect("config page renders"));
    });
    group.bench_function("errors_page", |b| {
        b.iter(|| ErrorsPage.render().expect("errors page renders"));
    });

    group.finish();
}

criterion_group!(benches, bench_docs_generate);
criterion_main!(benches);
