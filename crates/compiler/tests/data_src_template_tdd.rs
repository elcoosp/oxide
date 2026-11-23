//! TDD pins for the §V2-F17 `data-ox-src` injection — the Vue/Svelte
//! template half of the DOM→source mapping (the JSX half landed in
//! `data_src_tdd`; this closes the recorded remaining half named in
//! `docs/deviations.md`: "the compiler's `data-ox-src` attribute
//! injection ... the Vue/Svelte compile hooks"). In dev, the compiler
//! stamps the first element of each component's template with
//! `data-ox-src="path:line"` so the overlay can attribute a hovered DOM
//! node to its defining component (and to the `/__oxide/cost?src=`
//! record the server half serves).
//!
//! Written BEFORE the implementation; every pin imports public API only
//! (red-first: E0432 over `oxide_fw_compiler::data_src`).
//!
//! The contracts pinned here (the spec V2-F17 line: "Vue and Svelte
//! through their compile hooks ... `data-ox-src='path:line'` to the
//! first element of each component"):
//!
//! - A Vue SFC (`.vue`) stamps the first element inside the
//!   `<template>` block — the component's root rendered element. The
//!   `<script>` blocks are never touched.
//! - A Svelte (`.svelte`) file stamps the first element of its markup
//!   region (after the `</script>` close) — the component's root
//!   rendered element.
//! - The stamp value is `path:line` with line 1 (the file IS the
//!   component — the same `src` spelling the cost endpoint's `?src=`
//!   contract takes, and the bundler's per-file `source_costs` key).
//! - The edit is span surgery on the original text: only the attribute
//!   bytes are inserted, every other byte is preserved.
//! - Idempotent: a template that already carries `data-ox-src` on the
//!   chosen element is returned unchanged.
//! - Honest no-vouch direction: a file without a template/markup
//!   element is returned unchanged — a skipped stamp costs the overlay
//!   nothing, a wrong one misleads it.
//! - The public entry `inject_data_ox_src` dispatches `vue`/`svelte`
//!   langs to the template stamp (one entry point, two stamp paths).

use oxide_fw_compiler::data_src::{DATA_OX_SRC_ATTR, inject_data_ox_src};

fn stamp(path: &str, line: u32) -> String {
    format!("{DATA_OX_SRC_ATTR}=\"{path}:{line}\"")
}

// ---------------- the stamp rule: first element of the template ----------------

#[test]
fn vue_sfc_stamps_first_template_element() {
    let src = "<script setup lang=\"ts\">\nimport { ref } from 'vue'\nconst count = ref(0)\n</script>\n\n<template>\n  <section>\n    <h2>title</h2>\n    <button @click=\"count++\">{{ count }}</button>\n  </section>\n</template>\n";
    let out = inject_data_ox_src(src, "vue", "src/components/counter.vue")
        .unwrap()
        .unwrap();
    assert!(
        out.contains(&stamp("src/components/counter.vue", 1)),
        "the first template element carries the stamp: {out}"
    );
    assert!(
        out.contains("<section") && out.contains("<h2>title</h2>"),
        "the source survives the surgery: {out}"
    );
    // Only the FIRST element: the inner `<h2>` and `<button>` carry no stamp.
    assert!(
        !out.contains("<h2 data-ox-src") && !out.contains("<button data-ox-src"),
        "only the root template element is stamped: {out}"
    );
    // The script block is untouched.
    assert!(
        !src.contains(DATA_OX_SRC_ATTR),
        "the source carried no stamp before"
    );
}

#[test]
fn svelte_file_stamps_first_markup_element() {
    let src = "<script>\n  let count = $state(0)\n</script>\n\n<section>\n  <h2>title</h2>\n  <button onclick={() => (count += 1)}>{count}</button>\n</section>\n";
    let out = inject_data_ox_src(src, "svelte", "src/components/counter.svelte")
        .unwrap()
        .unwrap();
    assert!(
        out.contains(&stamp("src/components/counter.svelte", 1)),
        "the first markup element carries the stamp: {out}"
    );
    assert!(
        out.contains("<section") && out.contains("<h2>title</h2>"),
        "the source survives the surgery: {out}"
    );
    // Only the FIRST element.
    assert!(
        !out.contains("<h2 data-ox-src") && !out.contains("<button data-ox-src"),
        "only the root markup element is stamped: {out}"
    );
}

#[test]
fn vue_sfc_self_closing_root_stamps_before_slash() {
    // A self-closing root element: the attribute lands before the `/>`
    // (the JSX half's `stamp_edit` rule — the attribute glues to the slash,
    // the two stamp paths produce byte-consistent shapes).
    let src = "<template>\n  <Counter label=\"x\" />\n</template>\n";
    let out = inject_data_ox_src(src, "vue", "src/c.vue")
        .unwrap()
        .unwrap();
    assert!(
        out.contains("<Counter label=\"x\"  data-ox-src=\"src/c.vue:1\"/>"),
        "the self-closing root carries the stamp before the slash: {out}"
    );
}

#[test]
fn svelte_root_with_attributes_stamps_after_existing_attrs() {
    let src = "<script>\n  export let data\n</script>\n\n<main class=\"page\" id=\"root\">\n  <p>{data}</p>\n</main>\n";
    let out = inject_data_ox_src(src, "svelte", "src/page.svelte")
        .unwrap()
        .unwrap();
    assert!(
        out.contains("<main class=\"page\" id=\"root\" data-ox-src=\"src/page.svelte:1\">"),
        "the stamp joins the existing attributes: {out}"
    );
}

// ---------------- idempotency ----------------

#[test]
fn vue_template_already_stamped_is_unchanged() {
    let src = "<template>\n  <section data-ox-src=\"src/c.vue:1\">\n    <p>hi</p>\n  </section>\n</template>\n";
    let re = inject_data_ox_src(src, "vue", "src/c.vue").unwrap();
    assert!(
        re.is_none(),
        "a template that already carries the stamp is a no-op"
    );
}

#[test]
fn svelte_markup_already_stamped_is_unchanged() {
    let src = "<main data-ox-src=\"src/p.svelte:1\">\n  <p>hi</p>\n</main>\n";
    let re = inject_data_ox_src(src, "svelte", "src/p.svelte").unwrap();
    assert!(
        re.is_none(),
        "a markup that already carries the stamp is a no-op"
    );
}

// ---------------- the no-vouch direction ----------------

#[test]
fn vue_sfc_without_template_is_unchanged() {
    // A render-less SFC (script only): no element to stamp.
    let src = "<script setup lang=\"ts\">\nexport const x = 1\n</script>\n";
    assert_eq!(
        inject_data_ox_src(src, "vue", "src/c.vue").unwrap(),
        None,
        "a Vue SFC without a template element is a no-op"
    );
}

#[test]
fn svelte_script_only_is_unchanged() {
    // A Svelte file with only a script block: no markup element.
    let src = "<script>\n  export const x = 1\n</script>\n";
    assert_eq!(
        inject_data_ox_src(src, "svelte", "src/c.svelte").unwrap(),
        None,
        "a Svelte file without a markup element is a no-op"
    );
}

#[test]
fn non_template_lang_is_unchanged_by_template_path() {
    // The dispatch: a lang the template stamp does not own stays a no-op
    // (the JSX half owns tsx/jsx; the template half owns vue/svelte).
    assert_eq!(
        inject_data_ox_src("<main></main>", "html", "src/a.html").unwrap(),
        None,
        "an unknown lang is a no-op"
    );
}

// ---------------- span surgery preserves bytes ----------------

#[test]
fn vue_stamp_is_span_surgery() {
    // Only the attribute bytes are inserted: every other byte survives,
    // so line counts (and the dev sourcemap) stay valid.
    let src = "<template>\n  <main>\n    <p>hi</p>\n  </main>\n</template>\n";
    let out = inject_data_ox_src(src, "vue", "src/p.vue")
        .unwrap()
        .unwrap();
    let stripped = out.replace(" data-ox-src=\"src/p.vue:1\"", "");
    assert_eq!(
        stripped, src,
        "removing the inserted bytes recovers the original source"
    );
}

#[test]
fn svelte_stamp_is_span_surgery() {
    let src = "<script>\n  let x = 1\n</script>\n\n<main>\n  <p>hi</p>\n</main>\n";
    let out = inject_data_ox_src(src, "svelte", "src/p.svelte")
        .unwrap()
        .unwrap();
    let stripped = out.replace(" data-ox-src=\"src/p.svelte:1\"", "");
    assert_eq!(
        stripped, src,
        "removing the inserted bytes recovers the original source"
    );
}
