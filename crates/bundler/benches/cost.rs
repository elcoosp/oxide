//! Criterion bench for the §V2-F17 build-side cost engine — the
//! per-module fold (bytes + the real gzip stream + the chunk lookup)
//! over the 8- and 32-module shapes the smoke apps' route graphs use.
//! It must stay noise next to the bundling itself.

use std::hint::black_box;
use std::path::{Path, PathBuf};

use criterion::{Criterion, criterion_group, criterion_main};
use oxide_fw_bundler::cost::source_costs;
use oxide_fw_bundler::{BundleOutput, Chunk, ChunkKind, Module};

/// A fake module whose source repeats `lines` banner lines (the gzip
/// stream does real work on every one).
fn module(resolved: &str, lines: usize) -> Module {
    let mut source = String::new();
    for i in 0..lines {
        source.push_str(&format!("export const line{i} = 'oxide oxide oxide';\n"));
    }
    Module {
        specifier: resolved.rsplit('/').next().unwrap_or(resolved).to_string(),
        resolved: PathBuf::from(resolved),
        source,
        hash: format!("h{resolved}"),
        imports: Vec::new(),
        is_asset: false,
        server_only: false,
    }
}

/// An `n`-module output with every second module claimed by a chunk.
fn output(n: usize) -> BundleOutput {
    let modules: Vec<Module> = (0..n)
        .map(|i| module(&format!("/proj/src/routes/comp{i}.tsx"), 16))
        .collect();
    let chunks: Vec<Chunk> = (0..n)
        .step_by(2)
        .map(|i| Chunk {
            name: format!("entry{i}"),
            file: format!("entry.comp{i}-abc{i:03}.js"),
            hash: format!("h{i}"),
            modules: vec![PathBuf::from(format!("/proj/src/routes/comp{i}.tsx"))],
            imports: Vec::new(),
            css: Vec::new(),
            kind: ChunkKind::Entry,
        })
        .collect();
    BundleOutput {
        modules,
        chunks,
        client_manifest: None,
        server_manifest: None,
        diagnostics: Vec::new(),
    }
}

fn bench_cost(c: &mut Criterion) {
    let root = Path::new("/proj");
    c.bench_function("cost_8", |b| {
        let out = output(8);
        b.iter(|| black_box(source_costs(black_box(&out), root)));
    });
    c.bench_function("cost_32", |b| {
        let out = output(32);
        b.iter(|| black_box(source_costs(black_box(&out), root)));
    });
}

criterion_group!(benches, bench_cost);
criterion_main!(benches);
