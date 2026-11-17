//! TDD pins for the §V2-F02 dev warn — the props-surface half of the
//! auto-islands inference (`oxide_fw_compiler::props_surface`): the
//! per-component `on*`-props usage sites the dev transform pipeline sees,
//! crossed with the [`IslandManifest`] pure classifications.
//!
//! Written BEFORE the implementation; every pin imports public API only.
//!
//! The contracts pinned here:
//!
//! - [`scan_on_props`] reports every JSX component element (an
//!   identifier or member name with an uppercase base) that carries `on*`
//!   attributes — the same 2-char prefix rule the §V2-F02 classifier
//!   uses — with the props sorted, the 1-based line and the source
//!   snippet. Host elements (`<button onClick>`) are not component
//!   usages; a spread (`{...rest}`) is not a named prop; a source that
//!   does not parse or carries no JSX reports nothing (the surface is
//!   syntactic, never conservative — a warn is an opinion, not a skip
//!   decision).
//! - [`scan_component_imports`] maps the local binding names a JSX usage
//!   can reference (default, named, aliased, namespace) onto their
//!   specifiers.
//! - The warn fires ONLY on a positive pure classification: a defining
//!   file the manifest classified pure that a usage site equips with
//!   `on*` props through a relative same-project import. A defining file
//!   the manifest classifies interactive, does not know (not scanned —
//!   the `hydrates` missing-data rule), or that is the usage file itself
//!   (carrying the `on*` attribute makes the file interactive by its own
//!   scan) never warns. Package imports carry no same-project evidence.
//! - The project pass is deterministic: sorted by usage file, then line,
//!   then component; a fixpoint-promoted defining file (it imports an
//!   interactive child) never warns.

use std::collections::BTreeMap;

use oxide_fw_compiler::framework::Framework;
use oxide_fw_compiler::interactivity::IslandManifest;
use oxide_fw_compiler::props_surface::{
    OnPropsUsage, PureOnPropsWarn, pure_on_props_in_file, pure_on_props_warnings,
    scan_component_imports, scan_on_props,
};

/// A tiny React project: a pure `Button`, an interactive `Counter`, and a
/// page that imports both — the manifest shape every pin builds on.
fn project() -> BTreeMap<String, (Framework, String)> {
    [
        (
            "src/islands/button.tsx".to_string(),
            (
                Framework::React,
                "export default function Button(props: { children?: unknown }) {\n  return <button>{props.children}</button>;\n}\n"
                    .to_string(),
            ),
        ),
        (
            "src/islands/counter.tsx".to_string(),
            (
                Framework::React,
                "import { useState } from 'react';\nexport default function Counter() {\n  const [n, setN] = useState(0);\n  return <button onClick={() => setN(n + 1)}>{n}</button>;\n}\n"
                    .to_string(),
            ),
        ),
        (
            "routes/page.tsx".to_string(),
            (
                Framework::React,
                "import Button from '../src/islands/button';\nimport Counter from '../src/islands/counter';\nexport default function Page() {\n  return <main><Button onClick={() => {}} onInput={() => {}}>go</Button><Counter /></main>;\n}\n"
                    .to_string(),
            ),
        ),
    ]
    .into_iter()
    .collect()
}

fn manifest_of(files: &BTreeMap<String, (Framework, String)>) -> IslandManifest {
    IslandManifest::build(files)
}

// ------------------------------------------------------------ scan_on_props

/// A component element with two `on*` props reports one usage with the
/// props sorted and the line/snippet of the opening tag.
#[test]
fn scan_reports_the_component_its_on_props_and_the_location() {
    let src = "const f = () => {};\nexport default function Page() {\n  return <Card onZ={() => {}} onA={f}>x</Card>;\n}\n";
    let usages = scan_on_props(src);
    assert_eq!(usages.len(), 1, "one usage: {usages:?}");
    let u = &usages[0];
    assert_eq!(u.component, "Card");
    assert_eq!(
        u.props,
        vec!["onA", "onZ"],
        "sorted, deduped: {:?}",
        u.props
    );
    assert_eq!(u.line, 3);
    assert_eq!(
        u.snippet, "return <Card onZ={() => {}} onA={f}>x</Card>;",
        "the trimmed source line, the classifier's enrich shape"
    );
}

/// Host elements are not component usages, even when they carry event
/// attributes — the warn targets the pure-classified DEFINING file of a
/// component, and a host tag has none.
#[test]
fn host_elements_are_not_component_usages() {
    let src = "export default function P() {\n  return <button onClick={() => {}} onMouseOver={() => {}}>x</button>;\n}\n";
    assert!(scan_on_props(src).is_empty(), "{:?}", scan_on_props(src));
}

/// Member JSX names attribute to the base binding: `<Icons.Cog onClick>`
/// is a usage of `Icons`.
#[test]
fn member_names_attribute_to_the_base_binding() {
    let src = "export default function P() {\n  return <Icons.Cog onClick={() => {}} />;\n}\n";
    let usages = scan_on_props(src);
    assert_eq!(usages.len(), 1, "{usages:?}");
    assert_eq!(usages[0].component, "Icons");
    assert_eq!(usages[0].props, vec!["onClick"]);
}

/// A spread is not a named prop: the surface never claims `{...rest}`
/// carries events (a warn is an opinion; an unprovable one is noise).
#[test]
fn spreads_are_not_named_props() {
    let src = "export default function P(props: { onClick?: unknown }) {\n  return <Card {...props}>x</Card>;\n}\n";
    assert!(scan_on_props(src).is_empty(), "{:?}", scan_on_props(src));
}

/// A non-`on` attribute is not an event prop (`only` shares no prefix
/// with `onClick`'s second character... it shares both — the classifier's
/// case-insensitive 2-char rule is mirrored verbatim, so the pin records
/// the contract instead of a second opinion).
#[test]
fn the_on_prefix_rule_is_the_classifier_rule() {
    let src = "export default function P() {\n  return <Card onClick={() => {}} only={1} On={2}>x</Card>;\n}\n";
    let usages = scan_on_props(src);
    assert_eq!(usages.len(), 1, "{usages:?}");
    assert_eq!(usages[0].props, vec!["On", "onClick", "only"]);
}

/// Comments, strings and non-JSX sources report nothing — the surface is
/// a real parse (the F16 precedent), never a token scan.
#[test]
fn comments_strings_and_non_jsx_report_nothing() {
    let commented =
        "// <Card onClick={() => {}} />\nconst s = '<Card onClick={() => {}} />';\nexport {};\n";
    assert!(
        scan_on_props(commented).is_empty(),
        "{:?}",
        scan_on_props(commented)
    );
    assert!(
        scan_on_props("const n = 1; export default n;\n").is_empty(),
        "non-JSX reports nothing: {:?}",
        scan_on_props("const n = 1; export default n;\n")
    );
    // A parse failure is not a conservative hit here: the warn surface
    // under-reports by design (a skipped warn costs nothing; a false one
    // trains the developer to ignore the channel).
    assert!(
        scan_on_props("export default function <(").is_empty(),
        "a parse failure reports nothing: {:?}",
        scan_on_props("export default function <(")
    );
}

/// Multiple usage sites report in pre-order.
#[test]
fn multiple_usages_report_in_pre_order() {
    let src = "export default function P() {\n  return (\n    <>\n      <A onClick={() => {}} />\n      <B onInput={() => {}} />\n      <A onSubmit={() => {}} />\n    </>\n  );\n}\n";
    let usages = scan_on_props(src);
    let names: Vec<&str> = usages.iter().map(|u| u.component.as_str()).collect();
    assert_eq!(names, vec!["A", "B", "A"], "{usages:?}");
}

// ---------------------------------------------------- scan_component_imports

/// Default, named, aliased and namespace bindings all land on their
/// specifier; the map is local-name → specifier.
#[test]
fn imports_map_local_names_onto_specifiers() {
    let src = "import Button from './button';\nimport { Cog as Wheel } from './cog';\nimport * as Icons from './icons';\nimport { useState } from 'react';\nexport default function P() {\n  return null;\n}\n";
    let imports = scan_component_imports(src);
    assert_eq!(imports.get("Button").map(String::as_str), Some("./button"));
    assert_eq!(imports.get("Wheel").map(String::as_str), Some("./cog"));
    assert_eq!(imports.get("Icons").map(String::as_str), Some("./icons"));
    assert_eq!(
        imports.get("useState").map(String::as_str),
        Some("react"),
        "package bindings land too — dropping them happens in the resolver (no same-project evidence), not in the surface: {imports:?}"
    );
}

// ------------------------------------------------------- pure_on_props_in_file

/// The core warn: the pure-classified `button.tsx` receives `onClick` +
/// `onInput` from the page through a relative import — one warn naming
/// the usage file, the component, the defining file, the props.
#[test]
fn a_pure_defining_file_receiving_on_props_warns() {
    let files = project();
    let manifest = manifest_of(&files);
    let page = files.get("routes/page.tsx").unwrap().1.clone();
    let warns = pure_on_props_in_file("routes/page.tsx", &page, &manifest);
    assert_eq!(warns.len(), 1, "{warns:?}");
    let w = &warns[0];
    assert_eq!(w.usage_file, "routes/page.tsx");
    assert_eq!(w.component, "Button");
    assert_eq!(w.defining_file, "src/islands/button.tsx");
    assert_eq!(w.props, vec!["onClick", "onInput"]);
    assert_eq!(w.line, 4, "the usage line: {w:?}");
    assert!(w.snippet.contains("<Button"), "{:?}", w.snippet);
}

/// The interactive defining file (`counter.tsx`, a `useState` hit) never
/// warns — its classification is the hydration decision, not a mismatch.
#[test]
fn an_interactive_defining_file_never_warns() {
    let files = project();
    let manifest = manifest_of(&files);
    let page = files.get("routes/page.tsx").unwrap().1.clone();
    let warns = pure_on_props_in_file("routes/page.tsx", &page, &manifest);
    assert!(
        !warns.iter().any(|w| w.component == "Counter"),
        "the interactive counter never warns: {warns:?}"
    );
}

/// A defining file the manifest does not know (not scanned) carries no
/// vouch in either direction — the `hydrates` missing-data rule, mirrored
/// as no-warn. A package import resolves to nothing same-project.
#[test]
fn unknown_or_package_defining_files_never_warn() {
    let mut files = project();
    files.insert(
        "routes/other.tsx".to_string(),
        (
            Framework::React,
            "import Widget from './nowhere';\nimport Chip from '@ui/chip';\nexport default function O() {\n  return <><Widget onX={() => {}} /><Chip onY={() => {}} /></>;\n}\n"
                .to_string(),
        ),
    );
    let manifest = manifest_of(&files);
    let other = files.get("routes/other.tsx").unwrap().1.clone();
    assert!(
        pure_on_props_in_file("routes/other.tsx", &other, &manifest).is_empty(),
        "no vouch in either direction: {:?}",
        pure_on_props_in_file("routes/other.tsx", &other, &manifest)
    );
}

/// The usage file's own components are skipped: carrying the `on*`
/// attribute makes the file interactive by its own scan, so a same-file
/// "pure defining file" is a contradiction, never a warn.
#[test]
fn same_file_usages_never_warn() {
    let files = project();
    let manifest = manifest_of(&files);
    let src = "function Local(props: { children?: unknown }) {\n  return <span>{props.children}</span>;\n}\nexport default function Page() {\n  return <Local onClick={() => {}} />;\n}\n";
    assert!(
        pure_on_props_in_file("routes/local.tsx", src, &manifest).is_empty(),
        "{:?}",
        pure_on_props_in_file("routes/local.tsx", src, &manifest)
    );
}

/// The resolution follows the manifest's specifier rule: extensionless,
/// extension-carrying and `..` specifiers all land on the defining file;
/// a specifier naming no scanned file is not same-project evidence.
#[test]
fn relative_specifiers_resolve_like_the_fixpoint() {
    let mut files = project();
    files.insert(
        "src/islands/wrap.tsx".to_string(),
        (
            Framework::React,
            "import Pure from './button.tsx';\nimport Up from '../islands/button';\nexport default function W() {\n  return <><Pure onA={() => {}} /><Up onB={() => {}} /></>;\n}\n"
                .to_string(),
        ),
    );
    let manifest = manifest_of(&files);
    let wrap = files.get("src/islands/wrap.tsx").unwrap().1.clone();
    let warns = pure_on_props_in_file("src/islands/wrap.tsx", &wrap, &manifest);
    assert_eq!(
        warns.len(),
        2,
        "both spellings land on button.tsx: {warns:?}"
    );
    assert!(
        warns
            .iter()
            .all(|w| w.defining_file == "src/islands/button.tsx")
    );
}

/// A namespace import attributes member usages to the namespace's
/// defining file.
#[test]
fn namespace_imports_attribute_to_their_defining_file() {
    let mut files = project();
    files.insert(
        "src/islands/icons.tsx".to_string(),
        (
            Framework::React,
            "export function Cog() {\n  return <svg />;\n}\n".to_string(),
        ),
    );
    files.insert(
        "routes/iconic.tsx".to_string(),
        (
            Framework::React,
            "import * as Icons from '../src/islands/icons';\nexport default function I() {\n  return <Icons.Cog onSpin={() => {}} />;\n}\n"
                .to_string(),
        ),
    );
    let manifest = manifest_of(&files);
    let iconic = files.get("routes/iconic.tsx").unwrap().1.clone();
    let warns = pure_on_props_in_file("routes/iconic.tsx", &iconic, &manifest);
    assert_eq!(warns.len(), 1, "{warns:?}");
    assert_eq!(warns[0].defining_file, "src/islands/icons.tsx");
    assert_eq!(warns[0].component, "Icons");
}

// ------------------------------------------------------ pure_on_props_warnings

/// The project pass crosses every file's usages with the manifest and
/// sorts deterministically by (usage file, line, component).
#[test]
fn the_project_pass_is_deterministic_and_sorted() {
    let mut files = project();
    files.insert(
        "routes/other.tsx".to_string(),
        (
            Framework::React,
            "import Button from '../src/islands/button';\nexport default function O() {\n  return <Button onSubmit={() => {}} />;\n}\n"
                .to_string(),
        ),
    );
    let manifest = manifest_of(&files);
    let warns = pure_on_props_warnings(&files, &manifest);
    assert_eq!(warns.len(), 2, "{warns:?}");
    // `routes/other.tsx` sorts before `routes/page.tsx`.
    assert_eq!(warns[0].usage_file, "routes/other.tsx");
    assert_eq!(warns[0].component, "Button");
    assert_eq!(warns[1].usage_file, "routes/page.tsx");
    let again = pure_on_props_warnings(&files, &manifest);
    assert_eq!(warns, again, "identical bytes on re-run");
}

/// A fixpoint-promoted defining file (pure on its own hits, interactive
/// through an interactive child) never warns — the manifest's
/// classification is the one the hydration decision consumes.
#[test]
fn a_fixpoint_promoted_defining_file_never_warns() {
    let mut files = project();
    // `chain.tsx` has no hits of its own but imports the interactive
    // counter — the fixpoint promotes it, so receiving `on*` props is the
    // expected shape, not a mismatch.
    files.insert(
        "src/islands/chain.tsx".to_string(),
        (
            Framework::React,
            "import Counter from './counter';\nexport default function Chain() {\n  return <Counter />;\n}\n"
                .to_string(),
        ),
    );
    files.insert(
        "routes/chained.tsx".to_string(),
        (
            Framework::React,
            "import Chain from '../src/islands/chain';\nexport default function C() {\n  return <Chain onClick={() => {}} />;\n}\n"
                .to_string(),
        ),
    );
    let manifest = manifest_of(&files);
    let warns = pure_on_props_warnings(&files, &manifest);
    assert!(
        !warns
            .iter()
            .any(|w| w.defining_file == "src/islands/chain.tsx"),
        "the promoted chain never warns: {warns:?}"
    );
    assert!(
        warns
            .iter()
            .any(|w| w.defining_file == "src/islands/button.tsx"),
        "the pure button still warns: {warns:?}"
    );
}

/// The warn shape is the wire the dev pipeline logs: every field the
/// `tracing` event names is on the struct.
#[test]
fn the_warn_carries_every_field_the_event_logs() {
    let files = project();
    let manifest = manifest_of(&files);
    let warns: Vec<PureOnPropsWarn> = pure_on_props_warnings(&files, &manifest);
    assert_eq!(warns.len(), 1);
    let w = &warns[0];
    let _ = (
        w.usage_file.as_str(),
        w.component.as_str(),
        w.defining_file.as_str(),
        w.props.as_slice(),
        w.line,
        w.snippet.as_str(),
    );
    let _ = OnPropsUsage {
        component: w.component.clone(),
        props: w.props.clone(),
        line: w.line,
        snippet: w.snippet.clone(),
    };
}
