//! V2-C02 — the native Solid compiler (`SolidNative`, behind `native-solid`),
//! now backed by Solid 2.0's official Oxc-based Rust compiler core (the
//! `solidjs-compiler` crate, git-pinned at the upstream `next` revision — the
//! C02 dependency arm; see `docs/decisions/C02-solid-native.md`).
//!
//! # The oracle
//!
//! `tooling:solid-compile` in `crates/jshost/worker/oxide-worker.mjs`:
//! `moduleName: "@solidjs/web"` (the Solid 2.0 runtime location),
//! `generate: 'dom' | 'ssr'`, `hydratable: true`, the source filename, a
//! TypeScript strip and the `solid-js/web` → `@solidjs/web` import rewrite.
//! The upstream core IS the compiler the worker's `solid-jsx-oxc` binding
//! wraps, so the native backend and the pool's native path emit the same
//! shapes by construction; the adapter mirrors the worker's surrounding
//! passes (the strip, the rewrite).
//!
//! # What the pins hold
//!
//! The emitted DOM shape (`_$template` + `_$getNextElement`/`_$getNextMarker`
//! walks + `_$scope` effects + `_$insert`, delegated `$$` events +
//! `_$delegateEvents`/`_$runHydrationEvents`), the emitted SSR shape (array
//! `_$ssr` templates + `_$escape` + `_$ssrHydrationKey` + `_$ssrAttribute`),
//! the TS strip, the import rewrite, the refusal contract (`CMP-SOL-001`
//! carrying the upstream diagnostic, the fix naming the Node-pool escape),
//! the dev source-map/source-name flags, the seam metadata
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

fn compile_opts(
    source: &str,
    filename: &str,
    generate: Generate,
    dev: bool,
) -> Result<oxide_fw_compiler::native::CompileOutput, oxide_fw_diagnostics::OxideError> {
    SolidNative.compile(&CompileRequest {
        path: Path::new(filename),
        source,
        generate,
        dev,
    })
}

fn compile_dom(
    source: &str,
    filename: &str,
) -> Result<oxide_fw_compiler::native::CompileOutput, oxide_fw_diagnostics::OxideError> {
    compile_opts(source, filename, Generate::Dom, false)
}

fn compile_ssr(
    source: &str,
    filename: &str,
) -> Result<oxide_fw_compiler::native::CompileOutput, oxide_fw_diagnostics::OxideError> {
    compile_opts(source, filename, Generate::Ssr, false)
}

// ---------------------------------------------------------------------------
// The DOM shape
// ---------------------------------------------------------------------------

#[test]
fn dom_counter_emits_the_solid_runtime_shape() {
    let out = compile_dom(COUNTER, "src/components/Counter.tsx").expect("dom compile");
    let js = &out.js;

    // The runtime helpers ride `@solidjs/web` (the Solid 2.0 runtime the
    // worker's `moduleName` spells), one import per helper with the `_$`
    // alias the upstream core emits.
    assert!(
        js.contains("import { template as _$template } from \"@solidjs/web\";"),
        "template import:\n{js}"
    );
    assert!(
        js.contains("import { insert as _$insert } from \"@solidjs/web\";"),
        "insert import:\n{js}"
    );
    assert!(
        js.contains("import { scope as _$scope } from \"@solidjs/web\";"),
        "scope import:\n{js}"
    );
    // The user's own `solid-js` import is preserved (specifier + bindings
    // verbatim, quotes normalized by the strip's print).
    assert!(
        js.contains("import { createSignal, Show } from \"solid-js\";"),
        "user import preserved:\n{js}"
    );
    // Templates hoist with the `/* @__PURE__ */` marker the upstream core
    // emits; the dynamic text hole rides the `<!$><!/>` marker pair.
    assert!(
        js.contains(
            "var _tmpl$ = /* @__PURE__ */ _$template(`<section class=counter>\
             <button type=button>Clicked <!$><!/> times</button><!$><!/>`);"
        ),
        "section template:\n{js}"
    );
    assert!(
        js.contains("var _tmpl$2 = /* @__PURE__ */ _$template(`<p>over zero`);"),
        "show-children template:\n{js}"
    );
    // The hydration walk binds named variables from the root.
    assert!(
        js.contains("var _el$ = _$getNextElement(_tmpl$);"),
        "root bind:\n{js}"
    );
    assert!(
        js.contains("var [_el$5, _el$6] = _$getNextMarker(_el$4.nextSibling);"),
        "marker bind:\n{js}"
    );
    // The delegated event handler rides the `$$`-prefixed property and the
    // module tail registers the delegated set (2.0 adds the
    // `runHydrationEvents` tail the worker's hydratable mode carries).
    assert!(
        js.contains("_el$2._$$click = inc;"),
        "delegated handler:\n{js}"
    );
    assert!(
        js.contains("_$delegateEvents([\"click\"]);"),
        "delegation registration:\n{js}"
    );
    assert!(
        js.contains("_$runHydrationEvents();"),
        "hydration events tail:\n{js}"
    );
    // The dynamic text inserts with its markers, wrapped in the 2.0
    // `scope` effect wrapper.
    assert!(
        js.contains("_$insert(_el$2, _$scope(() => {"),
        "scoped insert:\n{js}"
    );
    // `<Show>` lowers to the memo + ternary the 2.0 core emits in
    // production (`lowerShow`), no `createComponent(Show` call.
    assert!(js.contains("var _c$ = _$memo(() => {"), "memo guard:\n{js}");
    assert!(js.contains("return count() > 0;"), "when body:\n{js}");
    assert!(
        js.contains("_c$() ? _$getNextElement(_tmpl$2) : void 0;"),
        "lowered ternary:\n{js}"
    );
    assert!(
        !js.contains("createComponent(Show"),
        "show is lowered, not a component:\n{js}"
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
    // string attributes drop the `=""` (the upstream `omitQuotes`
    // default); the button's sole dynamic text child rides no marker.
    assert!(
        js.contains(
            "var _tmpl$ = /* @__PURE__ */ _$template(`<section><h2 data-hmr-target>\
             HMR-TARGET-ORIGINAL</h2><button data-hmr-counter type=button>`);"
        ),
        "template:\n{js}"
    );
    // The button (child index 1) binds via firstChild.nextSibling.
    assert!(
        js.contains("var _el$3 = _el$2.nextSibling;"),
        "button bind:\n{js}"
    );
    // The inline arrow handler is the delegated value.
    assert!(
        js.contains("_el$3._$$click = () => setCount(count() + 1);"),
        "handler:\n{js}"
    );
    assert!(
        js.contains("_$insert(_el$3, _$scope(() => {"),
        "scoped insert:\n{js}"
    );
    assert!(
        js.contains("_$delegateEvents([\"click\"]);"),
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
        js.contains("import { createComponent as _$createComponent } from \"@solidjs/web\";"),
        "runtime import (createComponent for the user component):\n{js}"
    );
    assert!(!js.contains("delegateEvents("), "no registration:\n{js}");
    // The import of the component module is preserved verbatim (quotes
    // normalized by the strip's print).
    assert!(
        js.contains("import HmrTarget from \"./HmrTarget.tsx\";"),
        "component import:\n{js}"
    );
    // The member-expression hole inserts into the p; the component child
    // rides the marker pair.
    assert!(
        js.contains(
            "var _tmpl$ = /* @__PURE__ */ _$template(`<main><h1>Home</h1><p></p><!$><!/>`);"
        ),
        "main template:\n{js}"
    );
    assert!(
        js.contains("return props.data.message;"),
        "member-expr insert:\n{js}"
    );
    assert!(
        js.contains("_$insert(_el$, _$createComponent(HmrTarget, {}), _el$5, _el$6);"),
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
        js.contains("var _tmpl$ = /* @__PURE__ */ _$template(`<p>a <!$><!/>`);"),
        "fragment child template:\n{js}"
    );
    assert!(
        js.contains("var _tmpl$2 = /* @__PURE__ */ _$template(`<b>b`);"),
        "static child template:\n{js}"
    );
    assert!(
        js.contains("return [(() => {"),
        "array starts with the hole child's IIFE:\n{js}"
    );
    assert!(
        js.contains("})(), _$getNextElement(_tmpl$2)];"),
        "the static child rides the array verbatim:\n{js}"
    );
}

#[test]
fn dom_component_children_cover_static_dynamic_and_component_items() {
    let src = "import { createSignal } from 'solid-js';\nconst Comp = (props) => props.children;\nexport const B = () => {\n  const [x] = createSignal(1);\n  return (\n    <Comp>\n      <p>a</p>\n      {x()}\n      <em>c</em>\n    </Comp>\n  );\n};\n";
    let out = compile_dom(src, "src/B.tsx").expect("dom children");
    let js = &out.js;
    // Multiple children ride the getter-props form; dynamic items are
    // memoized accessors, static elements compile inline.
    assert!(
        js.contains("_$createComponent(Comp, {"),
        "component props object:\n{js}"
    );
    assert!(js.contains("get children() {"), "children getter:\n{js}");
    assert!(
        js.contains("_$memo(() => {") && js.contains("return x();"),
        "dynamic child accessor:\n{js}"
    );
}

#[test]
fn dom_single_expression_child_is_a_direct_getter() {
    let src = "import { createSignal } from 'solid-js';\nconst Comp = (props) => props.children;\nexport const C = () => {\n  const [x] = createSignal(1);\n  return <Comp>{x()}</Comp>;\n};\n";
    let out = compile_dom(src, "src/C.tsx").expect("dom single child");
    let js = &out.js;
    assert!(
        js.contains("_$createComponent(Comp, {"),
        "component props object:\n{js}"
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
    // The static attribute rides the template (unquoted, `omitQuotes`);
    // the dynamic one is set at runtime through the effect/setAttribute
    // pair the 2.0 core emits.
    assert!(
        js.contains("var _tmpl$ = /* @__PURE__ */ _$template(`<div title=static>t`);"),
        "static attrs in template:\n{js}"
    );
    assert!(
        js.contains("_$setAttribute(_el$, \"id\", _v$);"),
        "dynamic attribute:\n{js}"
    );
    assert!(
        js.contains("import { effect as _$effect } from \"@solidjs/web\";"),
        "effect helper imported:\n{js}"
    );
}

#[test]
fn jsx_source_without_typescript_compiles() {
    let src = "export const E = () => <b>bold</b>;\n";
    let out = compile_dom(src, "src/E.jsx").expect("jsx compile");
    assert!(out.js.contains("_$template(`<b>bold`)"), "js:\n{}", out.js);
}

#[test]
fn user_solid_js_web_import_is_rewritten_to_the_2_0_runtime() {
    // The worker's rewrite rule: `solid-js/web` (the Solid 1.x runtime
    // location) → `@solidjs/web`, so Rolldown resolves the import in a
    // Solid 2.0 workspace. The rewrite is offset-neutral (both specifiers
    // are 12 chars), so the upstream source map stays valid.
    let src = "import { template } from 'solid-js/web';\nexport const F = () => <b>bold</b>;\n";
    let out = compile_dom(src, "src/F.tsx").expect("compile");
    assert!(
        out.js
            .contains("import { template } from \"@solidjs/web\";"),
        "user import rewritten (the codegen normalizes quotes):\n{}",
        out.js
    );
}

#[test]
fn dom_spread_compiles_to_the_upstream_spread_helper() {
    // Solid 2.0's core supports spreads natively (`_$spread`); the old
    // in-repo transform refused them — the upstream swap widens the
    // supported surface.
    let src = "export const A = (props) => <div {...props} />;";
    let out = compile_dom(src, "src/A.tsx").expect("spread compiles");
    assert!(
        out.js.contains("_$spread(_el$, props, false);"),
        "spread helper:\n{}",
        out.js
    );
    assert!(
        out.js.contains("_$runHydrationEvents();"),
        "hydration tail:\n{}",
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
    // The SSR helpers, one import per helper with the `_$` alias.
    assert!(
        js.contains("import { escape as _$escape } from \"@solidjs/web\";"),
        "escape import:\n{js}"
    );
    assert!(
        js.contains("import { ssr as _$ssr } from \"@solidjs/web\";"),
        "ssr import:\n{js}"
    );
    assert!(
        js.contains("import { ssrHydrationKey as _$ssrHydrationKey } from \"@solidjs/web\";"),
        "ssrHydrationKey import:\n{js}"
    );
    // SSR templates are string-array literals; hydration markers ride the
    // `<!--$--><!--/-->` comment pairs (the worker compiles with
    // `hydratable: true`).
    assert!(
        js.contains("\" class=\\\"counter\\\"><button type=\\\"button\\\">Clicked <!--$-->"),
        "section ssr chunk:\n{js}"
    );
    assert!(
        js.contains("var _tmpl$2 = [\"<p\", \">over zero</p>\"];"),
        "p ssr template:\n{js}"
    );
    // Text holes ride `escape`.
    assert!(js.contains("_$escape(count())"), "text hole:\n{js}");
    // The module call interpolates every hole in order.
    assert!(
        js.contains("return _$ssr(_tmpl$, _v$, _v$2, _v$3);"),
        "ssr call:\n{js}"
    );
    // `<Show>` lowers to the memo + ternary, returning the SSR string.
    assert!(
        js.contains("_c$() ? (_v$4 = _$ssrHydrationKey(), _$ssr(_tmpl$2, _v$4)) : void 0;"),
        "lowered show ssr:\n{js}"
    );
    // No client-only helpers on the SSR path.
    assert!(!js.contains("delegateEvents"), "no delegation:\n{js}");
    assert!(!js.contains("_$$click"), "no delegated props:\n{js}");
    assert!(!js.contains("_$template("), "no DOM templates:\n{js}");
    assert!(!js.contains("_$insert("), "no DOM inserts:\n{js}");
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
        js.contains("_$escape(props.data.message)"),
        "escaped member-expr hole:\n{js}"
    );
    // SSR interpolates a plain component call wrapped in the escape the
    // template hole carries (the 2.0 SSR shape — no createComponent on
    // the server path).
    assert!(
        js.contains("_$escape(HmrTarget({}))"),
        "component interpolation:\n{js}"
    );
    assert!(js.contains("_$ssrHydrationKey()"), "hydration keys:\n{js}");
}

#[test]
fn ssr_dynamic_attribute_uses_ssr_attribute() {
    let src = "import { createSignal } from 'solid-js';\nexport const G = () => {\n  const [id] = createSignal('x');\n  return <div id={id()}>t</div>;\n};\n";
    let out = compile_ssr(src, "src/G.tsx").expect("ssr dynamic attr");
    let js = &out.js;
    assert!(
        js.contains("_$ssrAttribute(\"id\", _$escape(id(), true))"),
        "ssr attribute hole:\n{js}"
    );
}

#[test]
fn ssr_drops_client_only_props() {
    let src = "import { createSignal } from 'solid-js';\nexport const H = () => {\n  const [n] = createSignal(0);\n  return <button onClick={() => n()}>b</button>;\n};\n";
    let out = compile_ssr(src, "src/H.tsx").expect("ssr compile");
    // Events are client-only: dropped on the SSR output by the upstream
    // core (no warning channel exists on its API — the output is silent).
    assert!(!out.js.contains("_$$click"), "no handler:\n{}", out.js);
    assert!(
        !out.js.contains("delegateEvents"),
        "no delegation:\n{}",
        out.js
    );
    assert!(out.js.contains("_$ssr("), "ssr output:\n{}", out.js);
}

#[test]
fn ssr_fragment_compiles_to_an_array_of_ssr_calls() {
    // Solid 2.0's core supports SSR fragments (array templates per child);
    // the old in-repo transform refused them.
    let src = "export const A = () => (\n  <>\n    <p>a</p>\n    <b>b</b>\n  </>\n);";
    let out = compile_ssr(src, "src/frag.tsx").expect("ssr fragment");
    assert!(
        out.js.contains("var _tmpl$ = [\"<p\", \">a</p>\"];"),
        "p ssr template:\n{}",
        out.js
    );
    assert!(
        out.js.contains("var _tmpl$2 = [\"<b\", \">b</b>\"];"),
        "b ssr template:\n{}",
        out.js
    );
    assert!(out.js.contains("return ["), "array return:\n{}", out.js);
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
fn use_directives_refuse() {
    // `use:` was REMOVED in Solid 2.0 — the upstream core refuses it with
    // the migration note; the adapter maps it onto `CMP-SOL-001` with the
    // Node-pool fix (the pool's babel fallback still compiles 1.x-style
    // sources).
    assert_refused("export const A = () => <div use:tooltip />;", false, "use:");
    assert_refused("export const A = () => <div use:tooltip />;", true, "use:");
}

#[test]
fn broken_sources_refuse() {
    // A source the pipeline cannot parse refuses loudly with the
    // diagnostic riding the message.
    assert_refused("export const X = () => <div>{;};\n}}}{{", false, "parse");
}

// ---------------------------------------------------------------------------
// The dev flags (source names + the upstream source map)
// ---------------------------------------------------------------------------

#[test]
fn dev_compiles_carry_source_names_and_the_upstream_map() {
    let out = compile_opts(COUNTER, "src/components/Counter.tsx", Generate::Dom, true)
        .expect("dev compile");
    // Dev names every compiled binding effect by what it writes and where
    // (the upstream `sourceNames` surface, resolved against `dev`).
    assert!(
        out.js.contains("name: \"button.children\"")
            && out.js.contains("loc: \"src/components/Counter.tsx:"),
        "dev source names:\n{}",
        out.js
    );
    // The upstream V3 map rides the seam output in dev (the production
    // pipeline does not consume per-file native maps yet — the pool emits
    // none).
    let map = out.source_map.expect("dev map");
    assert!(map.contains("\"version\":3"), "v3 map: {map}");
    assert!(
        map.contains("\"sources\":[\"src/components/Counter.tsx\"]"),
        "sources: {map}"
    );
}

#[test]
fn production_compiles_carry_no_map() {
    let out = compile_dom(COUNTER, "src/components/Counter.tsx").expect("prod compile");
    assert!(out.source_map.is_none(), "prod map is None");
}

// ---------------------------------------------------------------------------
// The seam metadata
// ---------------------------------------------------------------------------

#[test]
fn backend_metadata_is_the_seam_contract() {
    assert_eq!(SolidNative.framework(), "solid");
    assert_eq!(SolidNative.id(), "solid-native");
    // The version is the upstream artifact this backend wraps (the npm
    // release at the pinned rev + the short rev) — the D40 CAS dimension.
    assert_eq!(SolidNative.version(), "2.0.0-rc.21+7ea2a02");
    assert_eq!(SolidNative::backend_version(), SolidNative.version());
    assert!(SolidNative.handles("tsx"));
    assert!(SolidNative.handles("jsx"));
    assert!(!SolidNative.handles("vue"));
    assert!(!SolidNative.handles("svelte"));
    // The D40 CAS key spelling folds the version.
    assert_eq!(
        cas_tool_version(&SolidNative),
        "native:solid-native@2.0.0-rc.21+7ea2a02"
    );
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
    // across the module (the upstream core dedupes templates per distinct
    // HTML).
    let src = "import { createSignal } from 'solid-js';\nexport const A = () => {\n  const [n] = createSignal(1);\n  return <li>x {n()}</li>;\n};\nexport const B = () => {\n  const [m] = createSignal(2);\n  return <li>x {m()}</li>;\n};\n";
    let out = compile_dom(src, "src/dedupe.tsx").expect("compile");
    assert_eq!(
        out.js.matches("_$template(`<li>x <!$><!/>`)").count(),
        1,
        "the two functions share one hoisted constant:\n{}",
        out.js
    );
    // Each shape still walks its own instance per call (the walk-variable
    // counter is module-scoped).
    assert_eq!(
        out.js.matches("_$getNextElement(_tmpl$)").count(),
        2,
        "two walk roots:\n{}",
        out.js
    );
    assert_eq!(
        out.js.matches("_$insert(").count(),
        2,
        "two inserts:\n{}",
        out.js
    );
}
