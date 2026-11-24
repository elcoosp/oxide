//! Criterion bench for the §V2-E08 frame-mapping seam (the B09 pattern:
//! plain `cargo test` never pays the criterion build — run behind
//! `--features bench-util`).
//!
//! The wiring-relevant question is the per-error cost of the mapped overlay
//! payload: the mapping runs once per frame when an error surfaces (the
//! rare, user-visible-latency path), never on the happy path. The groups:
//!
//! - `map_frame/{single,stack8}`: `map_diagnostic_frames` over 1 and 8
//!   generated frames through a 2,048-token module map (the stack a pooled
//!   render produces).
//! - `map_frame/no_maps`: the same diagnostic through `NoMaps` — the
//!   pass-through floor every no-map caller keeps.
//! - `payload/{with_maps,plain}`: the mapped §13.4 payload build vs the
//!   unmapped `overlay_payload` — the serialize cost the two entry points
//!   share.
//!
//! Run with:
//!
//! ```text
//! cargo bench -p oxide-fw-dev --features bench-util --bench frames
//! ```

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxc_sourcemap::{OwnedSourceMap, SourceMap, Token};
use oxide_fw_dev::frames::{ModuleMaps, NoMaps, map_diagnostic_frames, overlay_payload_with_maps};
use oxide_fw_dev::overlay_payload;
use oxide_fw_diagnostics::{Diagnostic, StackFrame};

/// The generated module path the bench's frames name.
const GEN_MODULE: &str = "dist/.oxide/pool/comp.tsx";
const SRC_MODULE: &str = "src/comp.tsx";

/// A 2,048-token module map: one token per generated line, each mapping
/// column 0 to source line `g`, column 4 — the shape a real module map
/// carries at a tenth of the density.
fn module_map(tokens: u32) -> OwnedSourceMap {
    let toks: Vec<Token> = (0..tokens)
        .map(|g| Token::new(g, 0, g, 4, Some(0), None))
        .collect();
    let map = SourceMap::new(
        None,
        Vec::new(),
        None,
        vec![SRC_MODULE.into()],
        vec![None],
        toks.into_boxed_slice(),
        None,
    );
    OwnedSourceMap::new(map)
}

fn generated_frame(i: u32) -> StackFrame {
    StackFrame {
        file: GEN_MODULE.to_string(),
        // Spread the probes over the map so each frame walks a different
        // line's bucket.
        line: (i * 211) % 2_000 + 1,
        column: 12,
        function: Some(format!("fn_{i}")),
        mapped: false,
    }
}

fn diag_with_frames(n: u32) -> Diagnostic {
    Diagnostic::error("RT-POOL", "boom in the pooled render")
        .with_frames((0..n).map(generated_frame).collect())
}

fn bench_frames(c: &mut Criterion) {
    let maps = ModuleMaps::new().with_module(GEN_MODULE, module_map(2_048));

    let mut group = c.benchmark_group("map_frame");
    for n in [1_u32, 8] {
        let d = diag_with_frames(n);
        group.bench_with_input(BenchmarkId::new("stack", n), &d, |b, d| {
            b.iter(|| map_diagnostic_frames(d, &maps));
        });
    }
    let d = diag_with_frames(8);
    group.bench_function("no_maps", |b| {
        b.iter(|| map_diagnostic_frames(&d, &NoMaps));
    });
    group.finish();

    let mut group = c.benchmark_group("payload");
    let d = diag_with_frames(8);
    group.bench_function("with_maps", |b| {
        b.iter(|| overlay_payload_with_maps(&d, &maps));
    });
    group.bench_function("plain", |b| {
        b.iter(|| overlay_payload(&d));
    });
    group.finish();
}

criterion_group!(benches, bench_frames);
criterion_main!(benches);
