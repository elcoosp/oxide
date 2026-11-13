//! Criterion bench for the §V2-C01 native-compiler seam — the per-file
//! selection cost the precompile integration adds ahead of every compile.
//!
//! The shapes: `node` (the early return — the backend the switch can
//! short-circuit to), `auto` over a registry of 8 padding backends with
//! no match (the pool-fallback walk), `auto` with the match LAST (the
//! worst-case scan, the shape C02-C04's registries grow into) and with
//! the match FIRST (the hot path once Solid's backend is compiled in).
//! All picks run against warmed health caches, so the bench measures the
//! SELECTION, never a real compiler's CPU time.

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_compiler::native::{
    Backend, CompileOutput, CompileRequest, NativeCompiler, NativeRegistry,
};
use oxide_fw_diagnostics::OxideError;

const PAD_IDS: [&str; 8] = [
    "pad-0", "pad-1", "pad-2", "pad-3", "pad-4", "pad-5", "pad-6", "pad-7",
];

/// A backend that matches `solid` + `tsx` and answers instantly; the
/// health fixture compiles to a non-empty string so `auto` accepts it.
struct FakeSolid {
    id: &'static str,
}

impl NativeCompiler for FakeSolid {
    fn framework(&self) -> &'static str {
        "solid"
    }
    fn id(&self) -> &'static str {
        self.id
    }
    fn version(&self) -> String {
        "0.0.0".to_string()
    }
    fn handles(&self, ext: &str) -> bool {
        ext == "tsx"
    }
    fn compile(&self, req: &CompileRequest<'_>) -> Result<CompileOutput, OxideError> {
        Ok(CompileOutput {
            js: req.source.to_string(),
            css: None,
            warnings: Vec::new(),
        })
    }
    fn health_fixture(&self) -> (&'static str, &'static str) {
        ("fixture.tsx", "export default () => null;")
    }
}

/// A backend for a framework the bench never picks (registry padding —
/// the scan cost when the answer is "not here, use the Node pool").
struct Padding {
    framework: &'static str,
    id: &'static str,
}

impl NativeCompiler for Padding {
    fn framework(&self) -> &'static str {
        self.framework
    }
    fn id(&self) -> &'static str {
        self.id
    }
    fn version(&self) -> String {
        "0.0.0".to_string()
    }
    fn handles(&self, _ext: &str) -> bool {
        false
    }
    fn compile(&self, _req: &CompileRequest<'_>) -> Result<CompileOutput, OxideError> {
        Ok(CompileOutput::default())
    }
    fn health_fixture(&self) -> (&'static str, &'static str) {
        ("fixture.tsx", "export default () => null;")
    }
}

/// The padding backends: two each for the frameworks the scan walks past.
fn padding() -> Vec<Padding> {
    let frameworks = [
        "react", "vue", "svelte", "react", "vue", "svelte", "react", "vue",
    ];
    frameworks
        .iter()
        .zip(PAD_IDS)
        .map(|(fw, id)| Padding { framework: fw, id })
        .collect()
}

fn bench_native(c: &mut Criterion) {
    let mut group = c.benchmark_group("native");
    group.throughput(criterion::Throughput::Elements(1));

    // The `node` switch: the early return the dispatch closure pays when
    // the config pins the pool backend.
    let empty = NativeRegistry::empty();
    group.bench_function("pick_node", |b| {
        b.iter(|| {
            black_box(
                empty
                    .pick("solid", "tsx", Backend::Node)
                    .expect("node never errors"),
            )
        });
    });

    // `auto` miss: the scan walks all 8 padding backends (none handle the
    // ext in a matching framework) and answers None — the pool fallback.
    let mut miss = NativeRegistry::empty();
    for p in padding() {
        miss.register(Box::new(p));
    }
    let _ = miss.report(); // warm the health cache: pick, not the probe
    group.bench_function(BenchmarkId::new("pick_auto_miss", 8), |b| {
        b.iter(|| {
            black_box(
                miss.pick("solid", "tsx", Backend::Auto)
                    .expect("auto never errors"),
            )
        });
    });

    // `auto` hit, match LAST: the worst-case scan (9 entries) plus the
    // cached-health read.
    let mut last = NativeRegistry::empty();
    for p in padding() {
        last.register(Box::new(p));
    }
    last.register(Box::new(FakeSolid {
        id: "solid-native-last",
    }));
    let _ = last.report();
    group.bench_function(BenchmarkId::new("pick_auto_hit_scan9", 9), |b| {
        b.iter(|| {
            black_box(
                last.pick("solid", "tsx", Backend::Auto)
                    .expect("auto never errors"),
            )
        });
    });

    // `auto` hit, match FIRST: the hot path every Solid file takes in a
    // C02-enabled build (1-entry scan + cached health).
    let mut first = NativeRegistry::empty();
    first.register(Box::new(FakeSolid {
        id: "solid-native-first",
    }));
    for p in padding() {
        first.register(Box::new(p));
    }
    let _ = first.report();
    group.bench_function(BenchmarkId::new("pick_auto_hit_scan1", 1), |b| {
        b.iter(|| {
            black_box(
                first
                    .pick("solid", "tsx", Backend::Auto)
                    .expect("auto never errors"),
            )
        });
    });

    // The D40 CAS tool-version spelling: the string the precompile cache
    // folds into every key.
    let picked = first
        .pick("solid", "tsx", Backend::Auto)
        .expect("auto never errors")
        .expect("the fake is healthy");
    group.bench_function("cas_tool_version", |b| {
        b.iter(|| black_box(oxide_fw_compiler::native::cas_tool_version(picked)));
    });

    group.finish();
}

criterion_group!(benches, bench_native);
criterion_main!(benches);
