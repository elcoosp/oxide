//! TDD pins for the §V2-F02 auto-islands inference — the pure
//! compiler-side interactivity scanner (`oxide_fw_compiler::interactivity`)
//! the `.oxide/island-manifest.json` side-car and the auto hydration set
//! come from.
//!
//! Written BEFORE the implementation; every pin imports public API only.
//!
//! The contracts pinned here:
//!
//! - [`scan_interactivity`] detects the spec's named tokens per framework:
//!   React (`useState`, `useReducer`, `useEffect`, `useLayoutEffect`,
//!   `useRef` **with** `.current` writes, any `on*` JSX attribute), Solid
//!   (`createSignal`, `createEffect`, `createStore`, `on*`), Vue (`ref`,
//!   `reactive`, `computed`, `watch`, `onMounted`, `emit`, `v-on` or
//!   `@event` in the template), Svelte (`$state`, `$:` statements, `on:`
//!   or `onclick` attributes, `bind:`).
//! - Comments and string literals are never hits (the AST arms run on a
//!   real parse, the F16 `infer_reasons` precedent).
//! - A component is **pure** when it has no hits and imports no
//!   interactive component from the same project — the import fixpoint is
//!   transitive and follows only relative same-project specifiers.
//! - A module that does not parse classifies **interactive** (the
//!   conservative arm: a false negative silently breaks interactivity,
//!   the kill criterion; an over-flagged island only costs bytes).
//! - The manifest round-trips as the spec's `{ file: { interactive,
//!   reasons } }` JSON, sorted, compact.
//! - The hydration decision: an explicit registry entry always wins; a
//!   pure classification is the only reason to skip; missing data
//!   hydrates (the conservative arm again).

use std::collections::BTreeMap;

use oxide_fw_compiler::framework::Framework;
use oxide_fw_compiler::interactivity::{
    Interactivity, IslandManifest, hydrates, scan_interactivity,
};

// ------------------------------------------------------------ React

#[test]
fn react_state_hooks_flip_interactive() {
    for hook in ["useState", "useReducer", "useEffect", "useLayoutEffect"] {
        let src = format!(
            "import {{ {hook} }} from 'react';\nexport default function C() {{\n  {hook}(() => {{}}, []);\n  return null;\n}}\n"
        );
        let inter = scan_interactivity(&src, Framework::React);
        assert!(inter.interactive, "{hook} must flip interactive");
        assert!(
            inter.reasons.iter().any(|r| r.kind == hook),
            "the reason names the hook (got {:?})",
            inter
                .reasons
                .iter()
                .map(|r| r.kind.clone())
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn react_on_jsx_attribute_flips_interactive() {
    let src =
        "export default function C() {\n  return <button onClick={() => {}}>hi</button>;\n}\n";
    let inter = scan_interactivity(src, Framework::React);
    assert!(inter.interactive, "an `on*` JSX attribute is a hit");
    assert!(inter.reasons.iter().any(|r| r.kind == "on-attribute"));
}

#[test]
fn react_use_ref_without_current_write_stays_pure() {
    // The spec's direct detection: `useRef` counts only with a `.current`
    // write — a ref held for a DOM handle alone is not interactivity.
    let src = "import { useRef } from 'react';\nexport default function C() {\n  const el = useRef(null);\n  return <div ref={el} />;\n}\n";
    let inter = scan_interactivity(src, Framework::React);
    assert!(!inter.interactive, "got reasons {:?}", inter.reasons);
}

#[test]
fn react_ref_current_write_flips_interactive() {
    let src = "import { useRef } from 'react';\nexport default function C() {\n  const seen = useRef(false);\n  const mark = () => { seen.current = true; };\n  return <div onMouseEnter={mark} />;\n}\n";
    let inter = scan_interactivity(src, Framework::React);
    assert!(inter.interactive);
    assert!(inter.reasons.iter().any(|r| r.kind == "ref-current-write"));
}

#[test]
fn react_comments_and_strings_are_not_hits() {
    let src = "// useState mentions in a comment\n// useEffect in a comment\nexport default function C() {\n  const note = 'useState in a string';\n  return <p>{note}</p>;\n}\n";
    let inter = scan_interactivity(src, Framework::React);
    assert!(!inter.interactive, "got reasons {:?}", inter.reasons);
}

// ------------------------------------------------------------ Solid

#[test]
fn solid_primitives_flip_interactive() {
    for prim in ["createSignal", "createEffect", "createStore"] {
        let src = format!(
            "import {{ {prim} }} from 'solid-js';\nexport default function C() {{\n  {prim}();\n  return null;\n}}\n"
        );
        let inter = scan_interactivity(&src, Framework::Solid);
        assert!(inter.interactive, "{prim} must flip interactive");
    }
}

#[test]
fn solid_on_attribute_flips_interactive() {
    let src =
        "export default function C() {\n  return <button onclick={() => {}}>hi</button>;\n}\n";
    let inter = scan_interactivity(src, Framework::Solid);
    assert!(inter.interactive);
}

// ------------------------------------------------------------ Vue

#[test]
fn vue_script_primitives_flip_interactive() {
    for prim in ["ref", "reactive", "computed", "watch", "onMounted"] {
        let src = format!(
            "<script setup>\nimport {{ {prim} }} from 'vue';\n{prim}(() => {{}});\n</script>\n<template><p>hi</p></template>\n"
        );
        let inter = scan_interactivity(&src, Framework::Vue);
        assert!(inter.interactive, "{prim} must flip interactive");
    }
}

#[test]
fn vue_emit_flips_interactive() {
    let src = "<script setup>\nconst emit = defineEmits(['change']);\n</script>\n<template><p>hi</p></template>\n";
    let inter = scan_interactivity(src, Framework::Vue);
    assert!(inter.interactive, "defineEmits/emit declares interactivity");
}

#[test]
fn vue_template_v_on_flips_interactive() {
    let src = "<template>\n  <button v-on:click=\"go\">hi</button>\n</template>\n";
    let inter = scan_interactivity(src, Framework::Vue);
    assert!(inter.interactive, "`v-on:` in the template is a hit");
    assert!(inter.reasons.iter().any(|r| r.kind == "vue-template-v-on"));
}

#[test]
fn vue_template_at_shorthand_flips_interactive() {
    let src = "<template>\n  <button @click=\"go\">hi</button>\n</template>\n";
    let inter = scan_interactivity(src, Framework::Vue);
    assert!(inter.interactive, "`@event` in the template is a hit");
}

#[test]
fn vue_pure_template_and_script_stay_pure() {
    let src = "<script setup>\nimport Badge from './Badge.vue';\nconst label = 'static';\n</script>\n<template><p>{{ label }}</p></template>\n";
    let inter = scan_interactivity(src, Framework::Vue);
    assert!(!inter.interactive, "got reasons {:?}", inter.reasons);
}

// ------------------------------------------------------------ Svelte

#[test]
fn svelte_state_rune_flips_interactive() {
    let src = "<script>\nlet count = $state(0);\n</script>\n<p>{count}</p>\n";
    let inter = scan_interactivity(src, Framework::Svelte);
    assert!(inter.interactive, "`$state` is a hit");
    assert!(inter.reasons.iter().any(|r| r.kind == "svelte-state"));
}

#[test]
fn svelte_reactive_statement_flips_interactive() {
    let src = "<script>\nlet doubled;\n$: doubled = 2;\n</script>\n<p>{doubled}</p>\n";
    let inter = scan_interactivity(src, Framework::Svelte);
    assert!(inter.interactive, "a `$:` statement is a hit");
    assert!(
        inter
            .reasons
            .iter()
            .any(|r| r.kind == "svelte-reactive-statement")
    );
}

#[test]
fn svelte_on_directive_and_bind_flip_interactive() {
    for (markup, kind) in [
        ("<button on:click={go}>hi</button>", "svelte-on-directive"),
        ("<input bind:value={name} />", "svelte-bind"),
        ("<button onclick={go}>hi</button>", "svelte-template-event"),
    ] {
        let src = format!("<script>\nlet v = 1;\n</script>\n{markup}\n");
        let inter = scan_interactivity(&src, Framework::Svelte);
        assert!(inter.interactive, "{markup} must flip interactive");
        assert!(
            inter.reasons.iter().any(|r| r.kind == kind),
            "{markup}: expected {kind}, got {:?}",
            inter
                .reasons
                .iter()
                .map(|r| r.kind.clone())
                .collect::<Vec<_>>()
        );
    }
}

// --------------------------------------------------- conservative arms

#[test]
fn unparseable_source_classifies_interactive() {
    // The kill criterion is the false negative: content the parser
    // cannot read is content we cannot vouch for, so it hydrates.
    let src = "export default function C( {{{ <not js at all\n";
    let inter = scan_interactivity(src, Framework::React);
    assert!(inter.interactive);
    assert!(inter.reasons.iter().any(|r| r.kind == "unparsed"));
}

#[test]
fn framework_none_has_no_direct_hits() {
    // A non-component module is pure on its own; the fixpoint still
    // promotes it when it imports an interactive component.
    let src = "export const one = 1;\n";
    let inter = scan_interactivity(src, Framework::None);
    assert!(!inter.interactive);
}

// -------------------------------------------------- the project fixpoint

/// Build the scan inputs for `IslandManifest::build`: relative path →
/// (framework, source).
fn files(entries: &[(&str, Framework, &str)]) -> BTreeMap<String, (Framework, String)> {
    entries
        .iter()
        .map(|(p, fw, src)| ((*p).to_string(), (*fw, (*src).to_string())))
        .collect()
}

#[test]
fn manifest_marks_hits_interactive_and_rest_pure() {
    let manifest = IslandManifest::build(&files(&[
        (
            "src/islands/counter.tsx",
            Framework::React,
            "import { useState } from 'react';\nexport default function C() {\n  const [n] = useState(0);\n  return <p>{n}</p>;\n}\n",
        ),
        (
            "src/islands/label.tsx",
            Framework::React,
            "export default function L() {\n  return <p>static</p>;\n}\n",
        ),
    ]));
    assert!(
        manifest
            .get("src/islands/counter.tsx")
            .expect("scanned file present")
            .interactive
    );
    assert!(
        !manifest
            .get("src/islands/label.tsx")
            .expect("scanned file present")
            .interactive
    );
}

#[test]
fn importing_an_interactive_component_flips_interactive() {
    let manifest = IslandManifest::build(&files(&[
        (
            "src/islands/counter.tsx",
            Framework::React,
            "import { useState } from 'react';\nexport default function C() {\n  useState(0);\n  return null;\n}\n",
        ),
        (
            "src/islands/wrapper.tsx",
            Framework::React,
            "import Counter from './counter';\nexport default function W() {\n  return <Counter />;\n}\n",
        ),
    ]));
    let wrapper = manifest.get("src/islands/wrapper.tsx").unwrap();
    assert!(
        wrapper.interactive,
        "importing an interactive same-project component is a hit"
    );
    assert!(
        wrapper
            .reasons
            .iter()
            .any(|r| r.kind == "imports-interactive"),
        "the reason names the propagation kind"
    );
}

#[test]
fn the_fixpoint_is_transitive() {
    // A imports B, B imports C, C holds the hit: A and B are interactive.
    let manifest = IslandManifest::build(&files(&[
        (
            "src/a.tsx",
            Framework::React,
            "import B from './b';\nexport default function A() {\n  return <B />;\n}\n",
        ),
        (
            "src/b.tsx",
            Framework::React,
            "import C from './c';\nexport default function B() {\n  return <C />;\n}\n",
        ),
        (
            "src/c.tsx",
            Framework::React,
            "import { useState } from 'react';\nexport default function C() {\n  useState(0);\n  return null;\n}\n",
        ),
    ]));
    assert!(manifest.get("src/a.tsx").unwrap().interactive);
    assert!(manifest.get("src/b.tsx").unwrap().interactive);
    assert!(manifest.get("src/c.tsx").unwrap().interactive);
}

#[test]
fn package_imports_do_not_propagate() {
    // Only same-project relative imports carry the fixpoint; a package
    // import of an (unknown) interactive-looking module does not.
    let manifest = IslandManifest::build(&files(&[
        (
            "src/page.tsx",
            Framework::React,
            "import { Carousel } from '@oxide/vendor';\nexport default function P() {\n  return <Carousel />;\n}\n",
        ),
        (
            "src/other.tsx",
            Framework::React,
            "export default function O() {\n  return <p>x</p>;\n}\n",
        ),
    ]));
    assert!(!manifest.get("src/page.tsx").unwrap().interactive);
}

// ------------------------------------------------------ the manifest JSON

#[test]
fn manifest_json_round_trips_the_spec_shape() {
    let manifest = IslandManifest::build(&files(&[(
        "src/islands/counter.tsx",
        Framework::React,
        "import { useState } from 'react';\nexport default function C() {\n  const [n] = useState(0);\n  return <p>{n}</p>;\n}\n",
    )]));
    let json = manifest.to_json();
    assert!(
        json.contains("\"src/islands/counter.tsx\""),
        "files are the keys (got {json})"
    );
    assert!(json.contains("\"interactive\":true"));
    assert!(json.contains("\"kind\":\"useState\""));
    assert!(!json.contains('\n'), "compact, not pretty (got {json})");
    let back = IslandManifest::from_json(&json).expect("round trip");
    assert_eq!(back, manifest);
    assert_eq!(
        back.get("src/islands/counter.tsx"),
        manifest.get("src/islands/counter.tsx")
    );
}

#[test]
fn manifest_json_is_sorted_by_file() {
    let manifest = IslandManifest::build(&files(&[
        (
            "src/z.tsx",
            Framework::React,
            "import { useState } from 'react';\nexport default function Z() {\n  useState(0);\n  return null;\n}\n",
        ),
        (
            "src/a.tsx",
            Framework::React,
            "import { useState } from 'react';\nexport default function A() {\n  useState(0);\n  return null;\n}\n",
        ),
    ]));
    let json = manifest.to_json();
    let z = json.find("\"src/z.tsx\"").expect("z present");
    let a = json.find("\"src/a.tsx\"").expect("a present");
    assert!(a < z, "sorted keys (got {json})");
}

#[test]
fn manifest_from_json_refuses_garbage() {
    assert!(IslandManifest::from_json("not json").is_err());
    assert!(IslandManifest::from_json("{\"a.tsx\": 3}").is_err());
}

// ------------------------------------------------- the hydration decision

#[test]
fn explicit_registration_always_wins() {
    let pure = Interactivity {
        interactive: false,
        reasons: Vec::new(),
    };
    assert!(
        hydrates(true, Some(&pure)),
        "an explicit registry entry hydrates even when the scanner says pure"
    );
}

#[test]
fn pure_classification_is_the_only_skip() {
    let pure = Interactivity {
        interactive: false,
        reasons: Vec::new(),
    };
    let interactive = Interactivity {
        interactive: true,
        reasons: Vec::new(),
    };
    assert!(!hydrates(false, Some(&pure)), "pure files skip hydration");
    assert!(hydrates(false, Some(&interactive)));
}

#[test]
fn missing_manifest_data_hydrates() {
    // The conservative arm: no classification means no vouch, and a
    // skipped island that should have hydrated is the silent breakage
    // the kill criterion forbids.
    assert!(hydrates(false, None));
}
