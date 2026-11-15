//! V2-C02 — the native Solid JSX compiler (`SolidNative`, behind
//! `native-solid`), written per the spec's C02 build shape and the C02
//! re-open record (`docs/decisions/C02-solid-native.md`).
//!
//! # The oracle
//!
//! `tooling:solid-compile` in `crates/jshost/worker/oxide-worker.mjs`:
//! the JSX compiles to `babel-preset-solid`-shaped runtime calls with
//! `moduleName: "@solidjs/web"` (the Solid 2.0 runtime location),
//! `generate: 'dom' | 'ssr'`, `hydratable: true`, the source filename;
//! TypeScript is stripped; `solid-js/web` user imports are rewritten to
//! `@solidjs/web`. The backend mirrors the contract in-repo over the
//! workspace oxc stack (the upstream artifact — `@solidjs/compiler`
//! 2.0.0-rc.21 on npm — carries no sourceable Rust crate; see the
//! decision doc).
//!
//! # What the pins hold
//!
//! The emitted DOM shape (templates + marker inserts + delegated events +
//! `createComponent` getter props), the emitted SSR shape (`ssr` template
//! literals + `escape` + `ssrAttribute` + `ssrHydrationKey`), the TS
//! strip, the import rewrite, the refusal contract (`CMP-SOL-001` naming
//! the unsupported construct and the Node-pool fix), the seam metadata
//! (framework/id/version/handles/health fixture) and byte determinism.
//!
//! Run: `cargo test -p oxide-fw-compiler --features native-solid --test
//! solid_native_spike`
#![cfg(feature = "native-solid")]

use oxide_fw_compiler::native::solid::SolidNative;
use oxide_fw_compiler::native::{CompileRequest, Generate, NativeCompiler, cas_tool_version};
use std::path::Path;

/// The spec's spike fixture: a signal, an event handler, a `<Show>`.
const COUNTER: &str = include_str!("fixtures/native/solid/Counter.tsx");
/// The smoke-component shape (static multi-child + delegated event).
const HMR_TARGET: &str = include_str!("fixtures/native/solid/HmrTarget.tsx");
/// The smoke-route shape (typed props + member-expr hole + component child).
const PAGE: &str = include_str!("fixtures/native/solid/Page.tsx");

fn compile_dom(
    source: &str,
    filename: &str,
) -> Result<oxide_fw_compiler::native::CompileOutput, oxide_fw_diagnostics::OxideError> {
    SolidNative.compile(&CompileRequest {
        path: Path::new(filename),
        source,
        generate: Generate::Dom,
        dev: false,
    })
}

fn compile_ssr(
    source: &str,
    filename: &str,
) -> Result<oxide_fw_compiler::native::CompileOutput, oxide_fw_diagnostics::OxideError> {
    SolidNative.compile(&CompileRequest {
        path: Path::new(filename),
        source,
        generate: Generate::Ssr,
        dev: false,
    })
}

// ---------------------------------------------------------------------------
// The DOM shape
// ---------------------------------------------------------------------------

#[test]
fn dom_counter_emits_the_solid_runtime_shape() {
    let out = compile_dom(COUNTER, "src/components/Counter.tsx").expect("dom compile");
    let js = &out.js;

    // The runtime import names `@solidjs/web` (the Solid 2.0 runtime the
    // worker's `moduleName` spells), aliased, sorted. The codegen prints
    // string literals normalized to double quotes, so the user's own
    // `solid-js` import is AST-preserved (specifier + bindings verbatim).
    assert!(
        js.contains(
            "import { createComponent as _createComponent, delegateEvents as _delegateEvents, \
             insert as _insert, template as _template } from \"@solidjs/web\";"
        ),
        "runtime import shape:\n{js}"
    );
    // The user's own `solid-js` import is preserved (only `solid-js/web`
    // is rewritten — the worker's rewrite rule).
    assert!(
        js.contains("import { createSignal, Show } from \"solid-js\";"),
        "user import preserved:\n{js}"
    );
    // Templates hoist, deduped per distinct HTML, first-use order (the
    // `<Show>` children compile before the parent interns its own shape);
    // the dynamic text hole rides a `<!>` marker next to the static text.
    assert!(
        js.contains("const _tmpl$ = _template(`<p>over zero</p>`);"),
        "show-children template:\n{js}"
    );
    assert!(
        js.contains(
            "const _tmpl$2 = _template(`<section class=\"counter\">\
             <button type=\"button\">Clicked <!> times</button><!></section>`);"
        ),
        "section template:\n{js}"
    );
    // The dynamic element binds named walk variables from the root
    // (the section is the second template — the Show children interned
    // first).
    assert!(js.contains("const _el$ = _tmpl$2();"), "root bind:\n{js}");
    assert!(
        js.contains("const _el$2 = _el$.firstChild;"),
        "button bind (child index 0):\n{js}"
    );
    // The delegated event handler rides the `$$`-prefixed property and the
    // module tail registers the delegated set.
    assert!(
        js.contains("_el$2.$$click = inc;"),
        "delegated handler:\n{js}"
    );
    assert!(
        js.contains("_delegateEvents([\"click\"]);"),
        "delegation registration:\n{js}"
    );
    // The mixed-text hole inserts before its marker (children: text,
    // comment, text — the marker is `firstChild.nextSibling`).
    assert!(
        js.contains("_insert(_el$2, () => count(), _el$2.firstChild.nextSibling);"),
        "marker insert:\n{js}"
    );
    // `<Show>` is a plain component: `createComponent` with getter props.
    assert!(
        js.contains("_insert(_el$, _createComponent(Show, {"),
        "show component call:\n{js}"
    );
    assert!(js.contains("get when() {"), "when getter:\n{js}");
    assert!(
        js.contains("return count() > 0;"),
        "when getter body:\n{js}"
    );
    assert!(js.contains("get children() {"), "children getter:\n{js}");
    assert!(
        js.contains("return _tmpl$();"),
        "children getter body (static child):\n{js}"
    );
    // The Show position is the section's second child: the marker walk.
    assert!(
        js.contains("_el$.firstChild.nextSibling);"),
        "show marker path:\n{js}"
    );
    // The IIFE returns the root node.
    assert!(js.contains("return _el$;"), "iife return:\n{js}");
}

#[test]
fn dom_counter_strips_typescript() {
    let out = compile_dom(COUNTER, "src/components/Counter.tsx").expect("dom compile");
    let js = &out.js;
    assert!(!js.contains(": number"), "TS annotation stripped:\n{js}");
    assert!(!js.contains("props: {"), "TS props type stripped:\n{js}");
    // The runtime value expression survives the strip.
    assert!(
        js.contains("createSignal(props.start)"),
        "value expr kept:\n{js}"
    );
}

#[test]
fn dom_hmr_target_shape_matches_the_smoke_component() {
    let out = compile_dom(HMR_TARGET, "src/components/HmrTarget.tsx").expect("dom compile");
    let js = &out.js;
    // The static multi-child tree serializes into one template; the empty
    // string attribute stays quoted-empty; the button's sole dynamic text
    // child rides no marker (the insert appends to the element).
    assert!(
        js.contains(
            "const _tmpl$ = _template(`<section><h2 data-hmr-target=\"\">HMR-TARGET-ORIGINAL</h2>\
             <button data-hmr-counter=\"\" type=\"button\"></button></section>`);"
        ),
        "template:\n{js}"
    );
    // The button (child index 1) binds via firstChild.nextSibling.
    assert!(
        js.contains("const _el$2 = _el$.firstChild.nextSibling;"),
        "button bind:\n{js}"
    );
    // The inline arrow handler is the delegated value; the sole dynamic
    // text child needs no marker.
    assert!(
        js.contains("_el$2.$$click = () => setCount(count() + 1);"),
        "handler:\n{js}"
    );
    assert!(
        js.contains("_insert(_el$2, () => count());"),
        "insert:\n{js}"
    );
    assert!(
        js.contains("_delegateEvents([\"click\"]);"),
        "delegation:\n{js}"
    );
}

#[test]
fn dom_page_shape_matches_the_smoke_route() {
    let out = compile_dom(PAGE, "src/routes/index/page.tsx").expect("dom compile");
    let js = &out.js;
    // No events here: the runtime import carries no delegateEvents and no
    // registration statement lands.
    assert!(
        js.contains(
            "import { createComponent as _createComponent, insert as _insert, \
             template as _template } from \"@solidjs/web\";"
        ),
        "runtime import (no delegateEvents):\n{js}"
    );
    assert!(!js.contains("delegateEvents("), "no registration:\n{js}");
    // The import of the component module is preserved verbatim.
    assert!(
        js.contains("import HmrTarget from \"./HmrTarget.tsx\";"),
        "component import:\n{js}"
    );
    // The member-expression hole is the p's only dynamic child (no
    // marker); the component child rides the marker.
    assert!(
        js.contains("const _tmpl$ = _template(`<main><h1>Home</h1><p></p><!></main>`);"),
        "main template:\n{js}"
    );
    assert!(
        js.contains("_insert(_el$2, () => props.data.message);"),
        "member-expr insert:\n{js}"
    );
    assert!(
        js.contains("_createComponent(HmrTarget, {})"),
        "component child:\n{js}"
    );
    assert!(!js.contains("message: string"), "TS stripped:\n{js}");
}

#[test]
fn dom_fragment_compiles_to_an_array() {
    let src = "import { createSignal } from 'solid-js';\nexport const A = () => {\n  const [n] = createSignal(1);\n  return (\n    <>\n      <p>a {n()}</p>\n      <b>b</b>\n    </>\n  );\n};\n";
    let out = compile_dom(src, "src/A.tsx").expect("dom fragment");
    let js = &out.js;
    // A fragment compiles to the array of its compiled children (the
    // Solid runtime accepts arrays as children) — no `Fragment`
    // component wrapper.
    assert!(!js.contains("Fragment"), "no fragment wrapper:\n{js}");
    assert!(
        js.contains("const _tmpl$ = _template(`<p>a <!></p>`);"),
        "fragment child template:\n{js}"
    );
    assert!(
        js.contains("return [(() => {"),
        "array starts with the hole child's IIFE:\n{js}"
    );
    assert!(
        js.contains("_insert(_el$, () => n(), _el$.firstChild.nextSibling);"),
        "the hole child inserts before its marker (static text precedes it):\n{js}"
    );
    assert!(
        js.contains(", _tmpl$2()];"),
        "the static child rides the array verbatim:\n{js}"
    );
}

#[test]
fn dom_component_children_cover_static_dynamic_and_component_items() {
    let src = "import { createSignal } from 'solid-js';\nconst Comp = (props) => props.children;\nexport const B = () => {\n  const [x] = createSignal(1);\n  return (\n    <Comp>\n      <p>a</p>\n      {x()}\n      <em>c</em>\n    </Comp>\n  );\n};\n";
    let out = compile_dom(src, "src/B.tsx").expect("dom children");
    let js = &out.js;
    // Multiple children ride the array form; dynamic items are accessors,
    // static elements compile inline.
    assert!(js.contains("get children() {"), "children getter:\n{js}");
    assert!(
        js.contains("_tmpl$(),") && js.contains("() => x(),") && js.contains("_tmpl$2()"),
        "children array (static element, accessor, static element):\n{js}"
    );
}

#[test]
fn dom_single_expression_child_is_a_direct_getter() {
    let src = "import { createSignal } from 'solid-js';\nconst Comp = (props) => props.children;\nexport const C = () => {\n  const [x] = createSignal(1);\n  return <Comp>{x()}</Comp>;\n};\n";
    let out = compile_dom(src, "src/C.tsx").expect("dom single child");
    let js = &out.js;
    assert!(
        js.contains("get children() {"),
        "single-child getter:\n{js}"
    );
    assert!(
        js.contains("return x();"),
        "the expression rides the getter verbatim:\n{js}"
    );
}

#[test]
fn dom_dynamic_attribute_uses_the_runtime_helper() {
    let src = "import { createSignal } from 'solid-js';\nexport const D = () => {\n  const [id] = createSignal('x');\n  return <div id={id()} title=\"static\">t</div>;\n};\n";
    let out = compile_dom(src, "src/D.tsx").expect("dom dynamic attr");
    let js = &out.js;
    // The static attribute rides the template; the dynamic one is set at
    // runtime through the runtime helper with an accessor.
    assert!(
        js.contains("const _tmpl$ = _template(`<div title=\"static\">t</div>`);"),
        "static attrs in template:\n{js}"
    );
    assert!(
        js.contains("_setAttribute(_el$, \"id\", () => id());"),
        "dynamic attribute:\n{js}"
    );
    assert!(
        js.contains("setAttribute as _setAttribute"),
        "helper imported:\n{js}"
    );
}

#[test]
fn jsx_source_without_typescript_compiles() {
    let src = "export const E = () => <b>bold</b>;\n";
    let out = compile_dom(src, "src/E.jsx").expect("jsx compile");
    assert!(
        out.js.contains("_template(`<b>bold</b>`)"),
        "js:\n{}",
        out.js
    );
}

#[test]
fn user_solid_js_web_import_is_rewritten_to_the_2_0_runtime() {
    // The worker's rewrite rule: `solid-js/web` (the Solid 1.x runtime
    // location) → `@solidjs/web`, so Rolldown resolves the import in a
    // Solid 2.0 workspace.
    let src = "import { template } from 'solid-js/web';\nexport const F = () => <b>bold</b>;\n";
    let out = compile_dom(src, "src/F.tsx").expect("compile");
    assert!(
        out.js
            .contains("import { template } from \"@solidjs/web\";"),
        "user import rewritten (the codegen normalizes quotes):\n{}",
        out.js
    );
}

// ---------------------------------------------------------------------------
// The SSR shape
// ---------------------------------------------------------------------------

#[test]
fn ssr_counter_emits_the_ssr_template_shape() {
    let out = compile_ssr(COUNTER, "src/components/Counter.tsx").expect("ssr compile");
    let js = &out.js;
    // The SSR helpers, aliased and sorted.
    assert!(
        js.contains(
            "import { createComponent as _createComponent, escape as _escape, ssr as _ssr, \
             ssrHydrationKey as _ssrHydrationKey } from \"@solidjs/web\";"
        ),
        "ssr runtime import (no ssrAttribute: Counter has no dynamic attrs):\n{js}"
    );
    // Static attrs in the template; hydration keys after each tag name
    // (the worker compiles with `hydratable: true`).
    assert!(
        js.contains("<section${_ssrHydrationKey()} class=\"counter\">"),
        "section opener:\n{js}"
    );
    assert!(
        js.contains("<button${_ssrHydrationKey()} type=\"button\">"),
        "button opener:\n{js}"
    );
    // Text holes ride `escape`.
    assert!(
        js.contains("Clicked ${_escape(count())} times"),
        "text hole:\n{js}"
    );
    // `<Show>` interpolates as a component call inside the template
    // literal; its children compile as an SSR string.
    assert!(
        js.contains("${_createComponent(Show, {"),
        "component interpolation:\n{js}"
    );
    assert!(
        js.contains("return _ssr(`<p${_ssrHydrationKey()}>over zero</p>`);"),
        "ssr children getter body (the child element compiles to an ssr string):\n{js}"
    );
    // No client-only helpers on the SSR path.
    assert!(!js.contains("delegateEvents"), "no delegation:\n{js}");
    assert!(!js.contains("$$click"), "no delegated props:\n{js}");
    assert!(!js.contains("_template("), "no DOM templates:\n{js}");
    assert!(!js.contains("_insert("), "no DOM inserts:\n{js}");
    // The user import is preserved.
    assert!(
        js.contains("import { createSignal, Show } from \"solid-js\";"),
        "user import:\n{js}"
    );
}

#[test]
fn ssr_page_interpolates_components_and_escapes_holes() {
    let out = compile_ssr(PAGE, "src/routes/index/page.tsx").expect("ssr compile");
    let js = &out.js;
    assert!(
        js.contains("_ssr(`<main${_ssrHydrationKey()}><h1${_ssrHydrationKey()}>Home</h1><p${_ssrHydrationKey()}>${_escape(props.data.message)}</p>${_createComponent(HmrTarget, {})}</main>`)"),
        "full ssr shape:\n{js}"
    );
}

#[test]
fn ssr_dynamic_attribute_uses_ssr_attribute() {
    let src = "import { createSignal } from 'solid-js';\nexport const G = () => {\n  const [id] = createSignal('x');\n  return <div id={id()}>t</div>;\n};\n";
    let out = compile_ssr(src, "src/G.tsx").expect("ssr dynamic attr");
    let js = &out.js;
    assert!(
        js.contains("${_ssrAttribute(\"id\", _escape(id()), false)}>"),
        "ssr attribute hole:\n{js}"
    );
}

#[test]
fn ssr_drops_client_only_props_with_warnings() {
    let src = "import { createSignal } from 'solid-js';\nexport const H = () => {\n  const [n] = createSignal(0);\n  return <button onClick={() => n()}>b</button>;\n};\n";
    let out = compile_ssr(src, "src/H.tsx").expect("ssr compile");
    // Events are client-only: dropped on the SSR output, surfaced as
    // non-fatal warnings (the CompileOutput contract), never silent.
    assert!(!out.js.contains("$$click"), "no handler:\n{}", out.js);
    assert!(
        out.warnings.iter().any(|w| w.contains("onClick")),
        "the dropped handler is warned: {:?}",
        out.warnings
    );
}

// ---------------------------------------------------------------------------
// The refusal contract
// ---------------------------------------------------------------------------

fn assert_refused(src: &str, ssr: bool, needle: &str) {
    let res = if ssr {
        compile_ssr(src, "src/refuse.tsx")
    } else {
        compile_dom(src, "src/refuse.tsx")
    };
    let err = res.expect_err("the construct must refuse loudly");
    assert_eq!(err.0.code, "CMP-SOL-001", "code: {}", err.0.message);
    assert!(
        err.0.message.contains(needle),
        "the refusal names the construct ({needle}): {}",
        err.0.message
    );
    let fix = err.0.fix.clone().unwrap_or_default();
    assert!(
        fix.contains("compilerBackend"),
        "the fix names the Node-pool escape: {fix}"
    );
}

#[test]
fn spreads_refuse() {
    assert_refused(
        "export const A = (props) => <div {...props} />;",
        false,
        "spread",
    );
    assert_refused(
        "export const A = (props) => <div {...props} />;",
        true,
        "spread",
    );
    assert_refused(
        "const Comp = (props) => props.children;\nexport const B = (props) => <Comp {...props} />;",
        false,
        "spread",
    );
}

#[test]
fn refs_refuse() {
    assert_refused(
        "export const A = () => { let el; return <div ref={el} />; };",
        false,
        "ref",
    );
}

#[test]
fn capture_directives_refuse() {
    assert_refused(
        "const h = () => {};\nexport const A = () => <div oncapture:click={h} />;",
        false,
        "oncapture:",
    );
}

#[test]
fn use_directives_refuse() {
    assert_refused("export const A = () => <div use:tooltip />;", false, "use:");
}

#[test]
fn object_style_and_classlist_refuse() {
    // Object literals are syntactically visible, so the backend can refuse
    // them loudly instead of coercing into attribute strings; dynamic
    // STRING style values are supported and never hit this pin.
    assert_refused(
        "export const A = () => <div style={{ color: 'red' }} />;",
        false,
        "style",
    );
    assert_refused(
        "export const B = () => <div classList={{ big: true }} />;",
        false,
        "classList",
    );
}

#[test]
fn ssr_fragments_refuse() {
    assert_refused(
        "export const A = () => (\n  <>\n    <p>a</p>\n    <b>b</b>\n  </>\n);",
        true,
        "fragment",
    );
}

#[test]
fn jsx_spread_children_refuse() {
    assert_refused(
        "const Comp = (props) => props.children;\nexport const A = (props) => <Comp>{...props.items}</Comp>;",
        false,
        "spread",
    );
}

// ---------------------------------------------------------------------------
// The seam metadata
// ---------------------------------------------------------------------------

#[test]
fn backend_metadata_is_the_seam_contract() {
    assert_eq!(SolidNative.framework(), "solid");
    assert_eq!(SolidNative.id(), "solid-native");
    assert_eq!(SolidNative.version(), "1.0.0");
    assert!(SolidNative.handles("tsx"));
    assert!(SolidNative.handles("jsx"));
    assert!(!SolidNative.handles("vue"));
    assert!(!SolidNative.handles("svelte"));
    // The D40 CAS key spelling folds the version.
    assert_eq!(cas_tool_version(&SolidNative), "native:solid-native@1.0.0");
}

#[test]
fn health_fixture_compiles_on_both_targets() {
    let (name, src) = SolidNative.health_fixture();
    let dom = compile_dom(src, name).expect("dom health");
    assert!(!dom.js.is_empty(), "dom health output non-empty");
    let ssr = compile_ssr(src, name).expect("ssr health");
    assert!(!ssr.js.is_empty(), "ssr health output non-empty");
}

// ---------------------------------------------------------------------------
// Determinism (the CAS contract: identical inputs → identical outputs)
// ---------------------------------------------------------------------------

#[test]
fn compile_is_byte_deterministic() {
    let a = compile_dom(COUNTER, "src/components/Counter.tsx").expect("dom a");
    let b = compile_dom(COUNTER, "src/components/Counter.tsx").expect("dom b");
    assert_eq!(a.js, b.js);
    let sa = compile_ssr(COUNTER, "src/components/Counter.tsx").expect("ssr a");
    let sb = compile_ssr(COUNTER, "src/components/Counter.tsx").expect("ssr b");
    assert_eq!(sa.js, sb.js);
}

#[test]
fn distinct_templates_dedupe_by_html() {
    // Identical ROOT element shapes share one hoisted template variable
    // across the module (nested dynamic subtrees inline into their
    // parent's template — they never intern separately).
    let src = "import { createSignal } from 'solid-js';\nexport const A = () => {\n  const [n] = createSignal(1);\n  return <li>x {n()}</li>;\n};\nexport const B = () => {\n  const [m] = createSignal(2);\n  return <li>x {m()}</li>;\n};\n";
    let out = compile_dom(src, "src/dedupe.tsx").expect("compile");
    assert_eq!(
        out.js.matches("_template(`<li>x <!></li>`)").count(),
        1,
        "the two functions share one hoisted constant:\n{}",
        out.js
    );
    // Each shape still walks its own instance per call (the walk-variable
    // counter is module-scoped: the second root is `_el$2`).
    assert_eq!(
        out.js.matches("_tmpl$();").count(),
        2,
        "two walk roots:\n{}",
        out.js
    );
    assert_eq!(
        out.js.matches("_insert(").count(),
        2,
        "two inserts:\n{}",
        out.js
    );
}
