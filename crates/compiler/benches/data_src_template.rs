//! Criterion bench for the §V2-F17 `data-ox-src` template stamp — the cost
//! of the Vue SFC `<template>` and Svelte markup DOM→source stamp the dev
//! serve path pays per served module, at the 8- and 32-component shapes
//! (a route module listing N child components) and the no-op plain-ts
//! floor (the dispatch returns `None` for an owned lang without scanning).
//!
//! The stamp is a pure-Rust scan over the template syntax (the JSX AST
//! visitor cannot see Vue/Svelte template syntax), so the bench measures
//! the byte-scan + span-surgery cost, not a parse.

use std::fmt::Write as _;
use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use oxide_fw_compiler::data_src::inject_data_ox_src;

/// A Vue SFC with `n` child component uses inside its `<template>` — the
/// realistic shape of a route module that lists a table of rows. Only the
/// FIRST element (the root `<section>`) is stamped, but the scan walks past
/// every `<Row>` use to find it.
fn vue_sfc_with(n: usize) -> String {
    let mut src = String::from(
        "<script setup lang=\"ts\">\nimport Row from './Row.vue'\n</script>\n\n<template>\n  <section>\n",
    );
    for i in 0..n {
        let _ = writeln!(src, "    <Row label=\"row {i}\" />");
    }
    src.push_str("  </section>\n</template>\n");
    src
}

/// A Svelte file with `n` child component uses in its markup region — the
/// realistic shape of a route module that lists a table of rows.
fn svelte_with(n: usize) -> String {
    let mut src =
        String::from("<script>\n  import Row from './Row.svelte'\n</script>\n\n<section>\n");
    for i in 0..n {
        let _ = writeln!(src, "  <Row label=\"row {i}\" />");
    }
    src.push_str("</section>\n");
    src
}

fn bench_data_src_template(c: &mut Criterion) {
    c.bench_function("data_src_template_vue_8", |b| {
        let src = vue_sfc_with(8);
        b.iter(|| {
            black_box(inject_data_ox_src(
                black_box(src.as_str()),
                "vue",
                "src/routes/page.vue",
            ))
        });
    });
    c.bench_function("data_src_template_vue_32", |b| {
        let src = vue_sfc_with(32);
        b.iter(|| {
            black_box(inject_data_ox_src(
                black_box(src.as_str()),
                "vue",
                "src/routes/page.vue",
            ))
        });
    });
    c.bench_function("data_src_template_svelte_32", |b| {
        let src = svelte_with(32);
        b.iter(|| {
            black_box(inject_data_ox_src(
                black_box(src.as_str()),
                "svelte",
                "src/routes/page.svelte",
            ))
        });
    });
    c.bench_function("data_src_template_noop_plain_ts", |b| {
        let src = "export const k = 42;\nexport function helper() { return 1; }\n";
        b.iter(|| black_box(inject_data_ox_src(black_box(src), "ts", "src/lib.ts")));
    });
}

criterion_group!(benches, bench_data_src_template);
criterion_main!(benches);
