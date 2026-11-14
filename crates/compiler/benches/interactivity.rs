//! Criterion bench for the §V2-F02 auto-islands inference — the cost of
//! the per-file scan (the Oxc parse + the AST walk) and of the project
//! fixpoint (the import-graph BFS), at the 8- and 32-file shapes the
//! island planner groups use.

use std::collections::BTreeMap;
use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use oxide_fw_compiler::framework::Framework;
use oxide_fw_compiler::interactivity::IslandManifest;

/// `n` React component sources: every third one holds a `useState` hit,
/// the rest import (unresolved) children and stay pure until the
/// fixpoint has nothing to promote them with.
fn react_files(n: usize) -> BTreeMap<String, (Framework, String)> {
    (0..n)
        .map(|i| {
            let src = if i % 3 == 0 {
                format!(
                    "import {{ useState }} from 'react';\nexport default function C{i}() {{\n  const [n] = useState({i});\n  return <p>{{n}}</p>;\n}}\n"
                )
            } else {
                format!(
                    "import Child from './child{i}';\nexport default function P{i}() {{\n  return <p>static {i}</p>;\n}}\n"
                )
            };
            (format!("src/islands/island{i}.tsx"), (Framework::React, src))
        })
        .collect()
}

/// `n` Svelte components: script-state and template-directive arms mixed
/// with pure markup, so both the parse and the token-scan arms run.
fn svelte_files(n: usize) -> BTreeMap<String, (Framework, String)> {
    (0..n)
        .map(|i| {
            let src = if i % 2 == 0 {
                format!(
                    "<script>\n  let count = $state({i});\n</script>\n<button on:click={{go}}>{{count}}</button>\n"
                )
            } else {
                format!("<p>static {i}</p>\n")
            };
            (format!("src/islands/badge{i}.svelte"), (Framework::Svelte, src))
        })
        .collect()
}

fn bench_interactivity(c: &mut Criterion) {
    c.bench_function("interactivity_scan_react_8", |b| {
        let files = react_files(8);
        b.iter(|| {
            for (src, (fw, _)) in black_box(&files) {
                black_box(oxide_fw_compiler::scan_interactivity(src, *fw));
            }
        });
    });
    c.bench_function("interactivity_scan_svelte_8", |b| {
        let files = svelte_files(8);
        b.iter(|| {
            for (src, (fw, _)) in black_box(&files) {
                black_box(oxide_fw_compiler::scan_interactivity(src, *fw));
            }
        });
    });
    c.bench_function("interactivity_manifest_8", |b| {
        let files = react_files(8);
        b.iter(|| black_box(IslandManifest::build(black_box(&files))));
    });
    c.bench_function("interactivity_manifest_32", |b| {
        let files = react_files(32);
        b.iter(|| {
            let manifest = IslandManifest::build(black_box(&files));
            let json = manifest.to_json();
            let hydrated: Vec<String> = manifest
                .interactive_files()
                .into_iter()
                .map(str::to_string)
                .collect();
            black_box((json, hydrated))
        });
    });
}

criterion_group!(benches, bench_interactivity);
criterion_main!(benches);
