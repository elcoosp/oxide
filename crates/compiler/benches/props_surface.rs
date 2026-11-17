//! Criterion bench for the §V2-F02 dev warn — the cost of the props
//! surface (the one Oxc parse collecting both the `on*`-props usage sites
//! and the import bindings) and of the pure-classified cross-reference,
//! at the 8- and 32-file shapes the island planner groups use.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use oxide_fw_compiler::framework::Framework;
use oxide_fw_compiler::interactivity::IslandManifest;
use oxide_fw_compiler::props_surface::{pure_on_props_warnings, scan_on_props};

/// One page source wiring `n` event props through `n` imported pure
/// components — every parse arm (usages + imports) fires.
fn page_with(n: usize) -> String {
    let mut src = String::new();
    for i in 0..n {
        let _ = writeln!(src, "import C{i} from './c{i}';");
    }
    src.push_str("export default function Page() {\n  return (\n    <>\n");
    for i in 0..n {
        let _ = writeln!(
            src,
            "      <C{i} onClick={{() => {{}}}} onInput={{() => {{}}}} />"
        );
    }
    src.push_str("    </>\n  );\n}\n");
    src
}

/// An `n`-file project: one page with `n` `on*`-props usages, the `n`
/// components it imports (pure: no hits, importing nothing interactive),
/// and an interactive counter the fixpoint promotes nothing through.
fn files(n: usize) -> BTreeMap<String, (Framework, String)> {
    let mut files = BTreeMap::new();
    for i in 0..n {
        files.insert(
            format!("src/islands/c{i}.tsx"),
            (
                Framework::React,
                "export default function C(props: { children?: unknown }) {\n  return <button>{props.children}</button>;\n}\n".to_string(),
            ),
        );
    }
    files.insert(
        "routes/page.tsx".to_string(),
        (Framework::React, page_with(n)),
    );
    files
}

fn bench_props_surface(c: &mut Criterion) {
    c.bench_function("props_surface_scan_8", |b| {
        let src = page_with(8);
        b.iter(|| black_box(scan_on_props(black_box(&src))));
    });
    c.bench_function("props_surface_scan_32", |b| {
        let src = page_with(32);
        b.iter(|| black_box(scan_on_props(black_box(&src))));
    });
    c.bench_function("props_surface_warn_8", |b| {
        let files = files(8);
        let manifest = IslandManifest::build(&files);
        b.iter(|| {
            let manifest = &manifest;
            black_box(pure_on_props_warnings(black_box(&files), manifest))
        });
    });
    c.bench_function("props_surface_warn_32", |b| {
        let files = files(32);
        let manifest = IslandManifest::build(&files);
        b.iter(|| {
            let manifest = &manifest;
            black_box(pure_on_props_warnings(black_box(&files), manifest))
        });
    });
}

criterion_group!(benches, bench_props_surface);
criterion_main!(benches);
