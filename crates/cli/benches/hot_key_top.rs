//! V2-F42: the boot flag's window resolver bench — the
//! `--cache-hot-key-top` override slot and the §20 parse core the boot
//! resolves ONCE at the `set_cache_plane` wire (the `trace_file_path`
//! posture: a boot-time read, not a per-scrape env poll). The arms cover
//! the resolver's three shapes — the default read (no flag, no env), the
//! env spelling's parse core (falsy, operator, the ledger-bound clamp)
//! and the flag's override read — so the resolve cost a boot pays (and a
//! windowed consumer re-pays per read) is measured on this machine, not
//! assumed.
//!
//! Run with `cargo bench -p oxide-fw-cli --features bench-util --bench
//! hot_key_top` (the bench-util opt-in keeps plain `cargo test` off
//! criterion).

use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;

use oxide_fw_cli::observability::{
    cache_hot_key_top, cache_hot_key_top_from, set_cache_hot_key_top_override,
};

/// The parse core over the spellings a boot actually resolves: the unset
/// spelling (the default answer), the falsy spelling (the default again),
/// the operator's value (the parse rides verbatim) and the clamp (the
/// ledger's own bound).
fn bench_cache_hot_key_top_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("cache_hot_key_top_parse");
    group.bench_function("unset", |b| {
        b.iter(|| black_box(cache_hot_key_top_from(black_box(None))));
    });
    group.bench_function("falsy_off", |b| {
        b.iter(|| black_box(cache_hot_key_top_from(black_box(Some("off")))));
    });
    group.bench_function("operator_50", |b| {
        b.iter(|| black_box(cache_hot_key_top_from(black_box(Some("50")))));
    });
    group.bench_function("clamp_cap", |b| {
        b.iter(|| black_box(cache_hot_key_top_from(black_box(Some("999999")))));
    });
    group.finish();
}

/// The override read: the flag's slot wins ahead of the env read (the
/// `trace_log_enabled` shape). The flag-set arm measures the slot hit;
/// the flag-absent arm measures the miss plus the env read the F41 shape
/// carries.
fn bench_cache_hot_key_top_override(c: &mut Criterion) {
    let mut group = c.benchmark_group("cache_hot_key_top_override");
    set_cache_hot_key_top_override(Some(50));
    group.bench_function("flag_set", |b| {
        b.iter(|| black_box(cache_hot_key_top()));
    });
    set_cache_hot_key_top_override(None);
    group.bench_function("flag_absent", |b| {
        b.iter(|| black_box(cache_hot_key_top()));
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_cache_hot_key_top_parse,
    bench_cache_hot_key_top_override
);
criterion_main!(benches);
