//! The §V2-F17 `data-ox-src` template-stamp walkthrough: the DOM→source
//! mapping the dev transform stamps onto the first element of a Vue SFC's
//! `<template>` block and a Svelte file's markup region (the JSX half's
//! twin — the JSX `data_src` example walks the React/Solid shapes). A
//! Vue SFC and a Svelte component are injected, the stamped source is
//! printed, and the idempotency + no-vouch directions are shown — the
//! value the overlay reads to attribute a hovered DOM node to `path:1`
//! and to fetch that component's `/__oxide/cost` record without
//! re-spelling the path.
//!
//! ```text
//! cargo run -p oxide-fw-compiler --example data_src_template
//! ```

use oxide_fw_compiler::data_src::DATA_OX_SRC_ATTR;
use oxide_fw_compiler::inject_data_ox_src;

fn main() {
    // A Vue SFC: the `<script setup>` block is untouched; the first
    // element of the `<template>` block (the component's root rendered
    // element) carries the stamp.
    let vue = "<script setup lang=\"ts\">\nimport { ref } from 'vue'\nconst count = ref(0)\n</script>\n\n<template>\n  <section>\n    <h2>counter</h2>\n    <button @click=\"count++\">{{ count }}</button>\n  </section>\n</template>\n";
    println!("== the Vue SFC (unstamped) ==");
    println!("{vue}");

    let vue_stamped = inject_data_ox_src(vue, "vue", "src/components/counter.vue")
        .unwrap()
        .expect("the SFC has a template element to stamp");
    println!("== the stamped Vue SFC (dev serve) ==");
    println!("{vue_stamped}");

    // The idempotency contract: re-injecting the stamped source is a
    // no-op (the dev pipeline re-serves modules).
    let re = inject_data_ox_src(&vue_stamped, "vue", "src/components/counter.vue").unwrap();
    println!(
        "re-injection: {}",
        match re {
            None => "unchanged (the element is already attributed)".to_string(),
            Some(_) => "UNEXPECTED second stamp".to_string(),
        }
    );

    // A Svelte component: the `<script>` block is untouched; the first
    // element of the markup region (after the `</script>` close) carries
    // the stamp.
    let svelte = "<script>\n  let count = $state(0)\n</script>\n\n<section>\n  <h2>counter</h2>\n  <button onclick={() => (count += 1)}>{count}</button>\n</section>\n";
    println!("\n== the Svelte component (unstamped) ==");
    println!("{svelte}");

    let svelte_stamped = inject_data_ox_src(svelte, "svelte", "src/components/counter.svelte")
        .unwrap()
        .expect("the Svelte file has a markup element to stamp");
    println!("== the stamped Svelte component (dev serve) ==");
    println!("{svelte_stamped}");

    // The no-vouch direction: a Vue SFC without a `<template>` block is a
    // no-op (a render-less SFC has no element to attribute).
    let renderless = "<script setup lang=\"ts\">\nexport const x = 1\n</script>\n";
    let none = inject_data_ox_src(renderless, "vue", "src/util.vue").unwrap();
    println!(
        "\nthe render-less SFC carries no stamp (no template element) — {none}",
        none = if none.is_none() {
            "no-op"
        } else {
            "UNEXPECTED stamp"
        }
    );

    // The stamp value is `path:1` — the same `src` spelling the cost
    // endpoint's `?src=` contract takes, and the bundler's per-file
    // `source_costs` key (the file IS the component for the template
    // frameworks).
    println!(
        "\nthe stamp attribute `{DATA_OX_SRC_ATTR}` carries `path:1` — the overlay fetches `/__oxide/cost?src=<path>` to read the component's cost record"
    );
}
