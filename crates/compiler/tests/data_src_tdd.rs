//! TDD pins for the §V2-F17 `data-ox-src` injection — the DOM→source
//! mapping half of the component-cost overlay
//! (`oxide_fw_compiler::data_src`): in dev, the compiler stamps the first
//! element each component renders with `data-ox-src="path:line"` so the
//! overlay can attribute a hovered DOM node to its defining component
//! (and to the `/__oxide/cost?src=` record the server half serves).
//!
//! Written BEFORE the implementation; every pin imports public API only
//! (red-first: E0432 over `oxide_fw_compiler::data_src`).
//!
//! The contracts pinned here:
//!
//! - Every top-level component (the React-Refresh rule: function and
//!   class declarations, `export`-wrapped variants, and arrow/function/
//!   class expressions bound to a variable) gets its first **returned**
//!   JSX element stamped; inner elements, fragments (the walk is
//!   transparent through them — the fragment itself is not an element)
//!   and non-returned JSX are never stamped.
//! - The stamp value is `path:line` with the 1-based line of the
//!   component's declaration — the same `src` spelling the cost
//!   endpoint's `?src=` contract takes, so the overlay's fetch needs no
//!   re-spelling.
//! - The edit is span surgery on the original text: only the attribute
//!   bytes differ, every other byte (and therefore every line count, so
//!   the dev sourcemap stays valid) is preserved.
//! - Idempotent: a source that already carries `data-ox-src` on the
//!   chosen element is returned unchanged (the dev pipeline re-serves
//!   modules; a second injection must not stack a second attribute).
//! - Honest no-vouch direction: a source that does not parse, a non-JSX
//!   language, or a module with no component is returned unchanged — a
//!   skipped stamp costs the overlay nothing, a wrong one misleads it.
//! - The dev lowering composition: `lower_to_js_with_refresh` runs the
//!   stamp BEFORE the JSX lowering, so the attribute rides the element
//!   into the runtime's props call, and AFTER it the React-Refresh
//!   preamble still lands — both dev passes compose.

use oxide_fw_compiler::data_src::{DATA_OX_SRC_ATTR, inject_data_ox_src};
use oxide_fw_compiler::{lower_to_js, lower_to_js_with_refresh};

fn stamp(path: &str, line: u32) -> String {
    format!("{DATA_OX_SRC_ATTR}=\"{path}:{line}\"")
}

// ---------------- the stamp rule: first returned element per component ----------------

#[test]
fn function_component_first_element_is_stamped() {
    let src = "export default function Page() {\n  return <main><h1>hi</h1></main>;\n}\n";
    let out = inject_data_ox_src(src, "tsx", "src/routes/page.tsx")
        .unwrap()
        .unwrap();
    assert!(
        out.contains(&stamp("src/routes/page.tsx", 1)),
        "the declaration line stamps the first element: {out}"
    );
    assert!(
        out.contains("<main") && out.contains("<h1>hi</h1>"),
        "the source survives the surgery: {out}"
    );
    // Only the FIRST element: the `<h1>` inside must not carry a stamp.
    let h1_index = out.find("<h1").unwrap();
    assert!(
        !out[..h1_index].ends_with(&stamp("src/routes/page.tsx", 1)),
        "the inner element is not stamped: {out}"
    );
    assert_eq!(out.matches(DATA_OX_SRC_ATTR).count(), 1, "{out}");
}

#[test]
fn arrow_const_component_implicit_return_is_stamped() {
    let src =
        "import Card from './Card';\n\nconst Row = () => (\n  <article><Card /></article>\n);\n";
    let out = inject_data_ox_src(src, "tsx", "src/islands/row.tsx")
        .unwrap()
        .unwrap();
    assert!(
        out.contains(&stamp("src/islands/row.tsx", 3)),
        "the arrow's declaration line stamps the implicit return: {out}"
    );
    assert_eq!(out.matches(DATA_OX_SRC_ATTR).count(), 1, "{out}");
}

#[test]
fn fragment_is_transparent_to_the_first_element() {
    let src = "function Page() {\n  return (\n    <>\n      <section id=\"a\" />\n      <p>after</p>\n    </>\n  );\n}\n";
    let out = inject_data_ox_src(src, "tsx", "src/a.tsx")
        .unwrap()
        .unwrap();
    assert!(
        out.contains("<section id=\"a\"") && out.contains(&stamp("src/a.tsx", 1)),
        "the fragment's first element child carries the stamp: {out}"
    );
    assert!(
        !out.contains("<p data-ox-src"),
        "the second element is not stamped: {out}"
    );
}

#[test]
fn conditional_return_stamps_the_first_rendered_element() {
    let src = "function Page() {\n  if (loading) {\n    return null;\n  }\n  return <div>ready</div>;\n}\n";
    let out = inject_data_ox_src(src, "tsx", "src/a.tsx")
        .unwrap()
        .unwrap();
    assert!(
        out.contains("<div data-ox-src=\"src/a.tsx:1\""),
        "the first JSX-bearing return wins over the earlier null one: {out}"
    );
}

#[test]
fn self_closing_element_is_stamped() {
    let src = "const Icon = () => <span name=\"x\"/>;\n";
    let out = inject_data_ox_src(src, "tsx", "src/icon.tsx")
        .unwrap()
        .unwrap();
    assert!(
        out.contains("<span name=\"x\" data-ox-src=\"src/icon.tsx:1\"/>"),
        "the self-closing tag keeps its slash: {out}"
    );
}

#[test]
fn class_component_render_is_stamped() {
    let src = "class Widget {\n  render() {\n    return <b>bold</b>;\n  }\n}\n";
    let out = inject_data_ox_src(src, "tsx", "src/widget.tsx")
        .unwrap()
        .unwrap();
    assert!(
        out.contains(&stamp("src/widget.tsx", 1)),
        "the class declaration line stamps the render's element: {out}"
    );
}

#[test]
fn lowercase_helper_functions_are_not_components() {
    let src = "function makeRow(item: string) {\n  return <li>{item}</li>;\n}\n";
    let out = inject_data_ox_src(src, "tsx", "src/rows.tsx").unwrap();
    assert_eq!(
        out, None,
        "an uppercase-first name is the component rule; `makeRow` is a helper"
    );
}

#[test]
fn multiple_components_each_get_one_stamp() {
    let src = "import { useState } from 'react';\n\nexport function Header() {\n  return <header>x</header>;\n}\n\nexport const Footer = () => <footer>y</footer>;\n";
    let out = inject_data_ox_src(src, "tsx", "src/layout.tsx")
        .unwrap()
        .unwrap();
    assert!(
        out.contains(&stamp("src/layout.tsx", 3)),
        "the function declaration stamps: {out}"
    );
    assert!(
        out.contains(&stamp("src/layout.tsx", 7)),
        "the const-bound arrow stamps: {out}"
    );
    assert_eq!(out.matches(DATA_OX_SRC_ATTR).count(), 2, "{out}");
}

// ---------------- the no-vouch direction ----------------

#[test]
fn unparseable_source_is_returned_unchanged() {
    let src = "function Broken( {\n  return <div></div\n}\n";
    let out = inject_data_ox_src(src, "tsx", "src/broken.tsx").unwrap();
    assert_eq!(out, None, "a parse failure stamps nothing");
}

#[test]
fn non_jsx_language_is_returned_unchanged() {
    let src = "export const n = 42;\n";
    let out = inject_data_ox_src(src, "ts", "src/plain.ts").unwrap();
    assert_eq!(out, None, "a non-JSX language stamps nothing");
}

#[test]
fn module_without_components_is_returned_unchanged() {
    let src = "export const k = <T,>(x: T) => x;\nexport function helper() { return 1; }\n";
    let out = inject_data_ox_src(src, "tsx", "src/lib.tsx").unwrap();
    assert_eq!(out, None, "no component, no stamp");
}

// ---------------- span surgery keeps every other byte ----------------

#[test]
fn surgery_preserves_every_byte_outside_the_insertions() {
    let src = "export function Page() {\n  return <main className=\"m\">hi</main>;\n}\n";
    let out = inject_data_ox_src(src, "tsx", "src/page.tsx")
        .unwrap()
        .unwrap();
    let stripped = out.replace(&format!(" {DATA_OX_SRC_ATTR}=\"src/page.tsx:1\""), "");
    assert_eq!(stripped, src, "only the attribute bytes differ: {out:?}");
    assert_eq!(
        src.matches('\n').count(),
        out.matches('\n').count(),
        "line count preserved"
    );
}

// ---------------- idempotency ----------------

#[test]
fn existing_stamp_is_not_duplicated() {
    let src = "function Page() {\n  return <div data-ox-src=\"hand-written:1\">x</div>;\n}\n";
    let out = inject_data_ox_src(src, "tsx", "src/page.tsx").unwrap();
    assert_eq!(
        out, None,
        "the element already carries the attribute; a second stamp would be a lie"
    );
}

#[test]
fn injection_is_idempotent_end_to_end() {
    let src = "export function Page() {\n  return <main>hi</main>;\n}\n";
    let once = inject_data_ox_src(src, "tsx", "src/page.tsx")
        .unwrap()
        .unwrap();
    let twice = inject_data_ox_src(&once, "tsx", "src/page.tsx").unwrap();
    assert_eq!(twice, None, "the stamped element is already attributed");
    assert_eq!(once.matches(DATA_OX_SRC_ATTR).count(), 1);
}

// ---------------- the dev lowering composition ----------------

#[test]
fn dev_lowering_rides_the_stamp_into_the_lowered_props() {
    let src = "export function Page() {\n  return <main>hi</main>;\n}\n";
    let (out, _) = lower_to_js_with_refresh(src, "tsx", "src/page.tsx", None).unwrap();
    assert!(
        out.contains("\"data-ox-src\"")
            || out.contains("'data-ox-src'")
            || out.contains("data-ox-src"),
        "the stamp survives the JSX lowering as a prop: {out}"
    );
    assert!(
        out.contains("$RefreshReg$(Page"),
        "the refresh preamble still lands after the stamp: {out}"
    );
}

#[test]
fn production_lowering_does_not_stamp() {
    let src = "export function Page() {\n  return <main>hi</main>;\n}\n";
    let (out, _) = lower_to_js(src, "tsx", "src/page.tsx", None).unwrap();
    assert!(
        !out.contains("data-ox-src"),
        "plain lowering (the production entry) carries no stamp: {out}"
    );
}
