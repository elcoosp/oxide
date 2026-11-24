//! V2-F69 config/CLI half: the boot resolver's compose cost — the
//! `cache.handler = { type: 'sharded', shards: [...] }` array the boot
//! resolves into the F69 composite ONCE per boot (the erased tier the
//! worker then sees). The arms measure the resolution at the shard
//! counts a node actually fronts — the single-plane shape the pre-F69
//! config spoke, four planes, eight — so the compose cost a boot pays
//! is measured on this machine, not assumed.
//!
//! Run with `cargo bench -p oxide-fw-cli --features bench-util --bench
//! sharded_resolver` (the bench-util opt-in keeps plain `cargo test` off
//! criterion).

use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;

use oxide_fw_cli::remote_cache_handler;
use oxide_fw_config::CacheHandlerConfig;

fn http_shard(i: usize) -> CacheHandlerConfig {
    CacheHandlerConfig::Http {
        url: format!("https://plane-{i}.internal:3100"),
    }
}

fn bench_sharded_resolver(c: &mut Criterion) {
    let mut group = c.benchmark_group("sharded_resolver");
    let rt = tokio::runtime::Runtime::new().expect("the bench's tokio runtime");
    group.bench_function("single_http", |b| {
        let cfg = http_shard(0);
        b.iter(|| {
            let h = rt.block_on(remote_cache_handler(black_box(Some(&cfg))));
            black_box(h.is_some());
        });
    });
    for n in [4usize, 8] {
        group.bench_function(format!("sharded_{n}"), |b| {
            let cfg = CacheHandlerConfig::Sharded {
                shards: (0..n).map(http_shard).collect(),
            };
            b.iter(|| {
                let h = rt.block_on(remote_cache_handler(black_box(Some(&cfg))));
                black_box(h.is_some());
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_sharded_resolver);
criterion_main!(benches);
