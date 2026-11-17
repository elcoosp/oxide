//! Criterion bench for the §V2-F17 `data-ox-src` injection — the cost of
//! the DOM→source stamp the dev lowering pays per served module, at the
//! 8- and 32-component shapes the island planner groups use.

use std::fmt::Write as _;
use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use oxide_fw_compiler::data_src::inject_data_ox_src;

/// A route module with `n` components, each rendering its own element —
/// every walk arm (declarations, bound arrows, returns) fires.
fn module_with(n: usize) -> String {
    let mut src = String::from("import Card from './Card';\n\n");
    for i in 0..n {
        let _ = writeln!(
            src,
            "export function Comp{i}() {{\n  return (\n    <section className=\"c{i}\">\n      <Card label=\"row {i}\" />\n    </section>\n  );\n}}\n"
        );
    }
    let _ = writeln!(src, "export const Root = () => <main><p>done</p></main>;");
    src
}

fn bench_data_src(c: &mut Criterion) {
    c.bench_function("data_src_inject_8", |b| {
        let src = module_with(8);
        b.iter(|| {
            black_box(inject_data_ox_src(
                black_box(src.as_str()),
                "tsx",
                "src/routes/page.tsx",
            ))
        });
    });
    c.bench_function("data_src_inject_32", |b| {
        let src = module_with(32);
        b.iter(|| {
            black_box(inject_data_ox_src(
                black_box(src.as_str()),
                "tsx",
                "src/routes/page.tsx",
            ))
        });
    });
    c.bench_function("data_src_noop_plain_ts", |b| {
        let src = "export const k = 42;\nexport function helper() { return 1; }\n";
        b.iter(|| black_box(inject_data_ox_src(black_box(src), "ts", "src/lib.ts")));
    });
}

criterion_group!(benches, bench_data_src);
criterion_main!(benches);
