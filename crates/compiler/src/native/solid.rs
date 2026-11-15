//! The native Solid JSX compiler (§V2-C02, re-opened) — behind the
//! `native-solid` feature.
//!
//! # Why the transform is in-repo (the C02 re-open)
//!
//! The upstream artifact is real and official: `@solidjs/compiler`
//! 2.0.0-rc.21 on npm is Solid 2.0's own Oxc-based JSX compiler (the RC
//! cycle's default compiler backend, maintained inside the Solid 2.0
//! release). But its Rust core is not sourceable for a Cargo build —
//! crates.io has no `solidjs-compiler` (the sparse index answers empty),
//! and the `solidjs/solid` `main` tree carries no `packages/compiler`
//! (the npm package ships the NAPI loader plus per-platform binaries
//! only). The earlier kill decision (`docs/decisions/C02-solid-kill.md`)
//! pinned exactly those two facts; the npm release satisfies the
//! re-open condition in spirit (a real Rust/Oxc compiler ships upstream)
//! while the dependency arm it sketched (a crates.io crate or a pin-able
//! rev) stays unmet. So the backend implements the transform in-repo over
//! the workspace oxc stack this crate already pins — the same string-in /
//! string-out seam, no new external dependency, no version skew. The
//! decision record (`docs/decisions/C02-solid-native.md`) holds the full
//! facts and the re-run criteria for the day upstream publishes a crate.
//!
//! # Output contract (the worker oracle)
//!
//! `tooling:solid-compile` in `crates/jshost/worker/oxide-worker.mjs`:
//! the JSX compiles to `babel-preset-solid`-shaped runtime calls with
//! `moduleName: "@solidjs/web"` (the Solid 2.0 runtime location), the
//! `generate` target, `hydratable: true` and the source filename; the
//! TypeScript annotations are stripped; and `solid-js/web` user imports
//! are rewritten to `@solidjs/web` so Rolldown resolves the runtime in a
//! Solid 2.0 workspace. This backend mirrors the contract in-repo:
//!
//! - **DOM** (`Generate::Dom`): per distinct element shape one hoisted
//!   `template()` constant (`<!>` comment markers at the
//!   dynamic child positions), named `firstChild`/`nextSibling` walk
//!   variables, `insert` for dynamic children (before the marker when the
//!   hole sits among static content), `setAttribute` for dynamic
//!   attributes (direct property assignment for the small known-property
//!   set), `$$evt` delegated handlers plus one module-tail
//!   `delegateEvents`, and `createComponent` with getter props for
//!   components. Fragments compile to arrays.
//! - **SSR** (`Generate::Ssr`): `ssr()` template literals with
//!   `${escape(...)}` text holes, `${ssrAttribute(...)}` dynamic
//!   attributes and `${ssrHydrationKey()}` after every tag name (the
//!   worker compiles with `hydratable: true`); components interpolate as
//!   `${createComponent(...)}`. Client-only constructs (events, refs) are
//!   dropped with a warning on the output, never silently.
//! - TypeScript rides the workspace oxc transformer's TypeScript pass
//!   (JSX preserved — the JSX lowering is the separate React pass, which
//!   this pipeline does not run), then the JSX walk, then one codegen
//!   print. The final `solid-js/web` → `@solidjs/web` import rewrite is
//!   the worker's own post-transform rule, applied verbatim.
//!
//! # The v1 scope (refusals are loud, never silent)
//!
//! Supported: static and dynamic attributes (`style`/`class` strings),
//! delegated (`onClick`) and non-delegated (`on:click`) events, dynamic
//! text, nested dynamic elements, components (including member
//! expressions) with getter props, fragments in DOM output (arrays), TSX
//! and JSX sources. Refused with `CMP-SOL-001` (naming the construct and
//! the Node-pool fix): JSX spreads (attributes and children), `ref`,
//! `use:` and `oncapture:` directives, object-literal `style`/`classList`
//! (the string-valued forms are supported; the object literals are
//! syntactically detectable and refuse instead of coercing), `innerHTML`,
//! fragments in SSR output, and unparseable sources. The Node tooling
//! pool compiles everything the v1 scope refuses — `auto` falls back,
//! `native` surfaces the refusal.

use crate::native::{CompileOutput, CompileRequest, Generate, NativeCompiler};
use crate::{OxideError, Result};
use oxc_allocator::{Allocator, Box as ArenaBox, Dummy, TakeIn, Vec as ArenaVec};
use oxc_ast::ast::{
    Expression, JSXAttributeItem, JSXAttributeName, JSXAttributeValue, JSXChild, JSXElement,
    JSXElementName, JSXExpression, JSXExpressionContainer, JSXMemberExpressionObject,
    JSXOpeningElement, ModuleDeclaration, Program, Statement,
};
use oxc_ast_visit::{VisitMut, walk_mut};
use oxc_codegen::{Codegen, CodegenOptions};
use oxc_span::{GetSpan, SourceType};
use oxc_transformer::{TransformOptions, Transformer, TypeScriptOptions};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;

/// The `@solidjs/web` runtime module the emitted code imports (the worker
/// passes `moduleName: "@solidjs/web"` — the Solid 2.0 runtime location).
const RUNTIME_MODULE: &str = "@solidjs/web";

/// HTML void elements — serialized without a closing tag.
const VOID_TAGS: [&str; 14] = [
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// The known-boolean attribute set for the SSR `ssrAttribute(name, value,
/// isBoolean)` third argument (a dynamic boolean attr renders bare, not
/// `="true"`).
const BOOLEAN_ATTRS: [&str; 24] = [
    "allow",
    "async",
    "autofocus",
    "autoplay",
    "checked",
    "controls",
    "default",
    "defer",
    "disabled",
    "formnovalidate",
    "hidden",
    "inert",
    "ismap",
    "loop",
    "multiple",
    "muted",
    "nomodule",
    "novalidate",
    "open",
    "playsinline",
    "readonly",
    "required",
    "reversed",
    "selected",
];

/// The dynamic attributes set as DOM properties (attribute assignment does
/// not update the live value for these; the worker's babel output assigns
/// them as properties too).
const PROPERTY_ATTRS: [&str; 3] = ["checked", "selected", "value"];

/// The native Solid backend (`solid-native`, the in-repo transform).
///
/// The `version()` is this transform's own revision (no upstream crate to
/// track — see the module docs): bump it on any output-shape change so the
/// D40 CAS key invalidates the previous backend's artifacts.
pub struct SolidNative;

impl SolidNative {
    /// The transform revision — the D40 CAS key dimension.
    #[must_use]
    pub fn backend_version() -> &'static str {
        "1.0.0"
    }
}

impl NativeCompiler for SolidNative {
    fn framework(&self) -> &'static str {
        "solid"
    }

    fn id(&self) -> &'static str {
        "solid-native"
    }

    fn version(&self) -> String {
        Self::backend_version().to_string()
    }

    fn handles(&self, ext: &str) -> bool {
        ext == "tsx" || ext == "jsx"
    }

    fn compile(&self, req: &CompileRequest<'_>) -> Result<CompileOutput> {
        let source_type = if req.path.extension().is_some_and(|e| e == "jsx") {
            SourceType::jsx()
        } else {
            SourceType::tsx()
        };
        let allocator = Allocator::default();
        let parsed = oxc_parser::Parser::new(&allocator, req.source, source_type).parse();
        if !parsed.diagnostics.is_empty() {
            let first = first_message(parsed.diagnostics.iter().map(ToString::to_string));
            return Err(refusal(
                req.path,
                &format!("the source does not parse ({first})"),
            ));
        }
        let mut program = parsed.program;

        // The TypeScript strip (the worker's sucrase `typescript` pass):
        // annotations removed at the AST level, JSX untouched (the JSX
        // lowering is the separate React pass, which this pipeline does
        // NOT run — the Solid walk below owns the JSX).
        let semantic = oxc_semantic::SemanticBuilder::new().build(&program);
        let scoping = semantic.semantic.into_scoping();
        let options = TransformOptions {
            // `only_remove_type_imports` keeps unused VALUE imports — the
            // sucrase parity (the worker's strip never elides imports).
            typescript: TypeScriptOptions {
                only_remove_type_imports: true,
                ..TypeScriptOptions::default()
            },
            // The React JSX lowering is the one pass this pipeline must
            // NOT run (the Solid walk below owns the JSX; the worker's
            // sucrase pass keeps JSX verbatim too).
            jsx: oxc_transformer::JsxOptions {
                jsx_plugin: false,
                development: false,
                display_name_plugin: false,
                jsx_self_plugin: false,
                jsx_source_plugin: false,
                pure: false,
                ..oxc_transformer::JsxOptions::default()
            },
            ..TransformOptions::default()
        };
        let stripped = Transformer::new(
            &allocator,
            Path::new(&req.path.display().to_string()),
            &options,
        )
        .build_with_scoping(scoping, &mut program);
        if !stripped.diagnostics.is_empty() {
            let first = first_message(stripped.diagnostics.iter().map(ToString::to_string));
            return Err(refusal(
                req.path,
                &format!("the TypeScript strip failed ({first})"),
            ));
        }

        // The JSX walk: every JSX expression the module holds compiles to
        // the Solid runtime shape for the request's target. `dev` carries
        // no Solid-specific options on the worker contract (the handler
        // passes generate/moduleName/hydratable only) — accepted, unused.
        let mode = if req.generate == Generate::Ssr {
            Mode::Ssr
        } else {
            Mode::Dom
        };
        let mut emitter = Emitter::new(mode, &allocator);
        let mut pass = JsxPass {
            emitter: &mut emitter,
            error: None,
        };
        pass.visit_program(&mut program);
        if let Some(r) = pass.error {
            return Err(refusal(req.path, &r.detail));
        }

        // Module assembly: the runtime import after the existing imports,
        // the hoisted templates after it (first-use order), and the
        // delegated-event registration at the tail.
        assemble_module(&mut program, &mut emitter)?;

        let printed = Codegen::new()
            .with_options(CodegenOptions::default())
            .build(&program)
            .code;
        // The worker's post-transform rewrite: `solid-js/web` (the Solid
        // 1.x runtime location some sources still import) → the 2.0
        // runtime, both quote spellings (the codegen normalizes to double
        // quotes; the single-quote arm is the worker's parity).
        let js = printed
            .replace("from \"solid-js/web\"", "from \"@solidjs/web\"")
            .replace("from 'solid-js/web'", "from '@solidjs/web'");
        Ok(CompileOutput {
            js,
            css: None,
            warnings: emitter.warnings,
        })
    }

    fn health_fixture(&self) -> (&'static str, &'static str) {
        ("health-fixture.tsx", "export default () => <p>ok</p>;\n")
    }
}

/// The compile target the emitter works in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    /// The client (DOM) bundle output.
    Dom,
    /// The server-render module output.
    Ssr,
}

/// The first non-empty message of a diagnostic iterator (the one the
/// refusal carries).
fn first_message<I: Iterator<Item = String>>(mut msgs: I) -> String {
    msgs.next()
        .unwrap_or_else(|| "no diagnostic detail".to_string())
}

/// The `CMP-SOL-001` refusal (the Solid backend's one failure shape — the
/// parse/strip/refusal detail rides the message; the fix names the Node
/// tooling pool, the fallback every refused construct compiles with).
fn refusal(path: &Path, detail: &str) -> OxideError {
    let mut d = oxide_fw_diagnostics::Diagnostic::error(
        "CMP-SOL-001",
        format!(
            "native solid compile failed for {}: {detail}",
            path.display()
        ),
    );
    d.fix = Some(
        "set framework.compilerBackend to 'auto' or 'node' to use the Node tooling pool".into(),
    );
    OxideError(Box::new(d))
}

/// One loud refusal (the `CMP-SOL-001` payload before the diagnostic is
/// built): the construct named for the message.
struct Refusal {
    detail: String,
}

impl Refusal {
    fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// The JSX pass — the mutable visitor that replaces every JSX expression
// with its compiled runtime shape.
// ---------------------------------------------------------------------------

struct JsxPass<'a, 'e> {
    emitter: &'e mut Emitter<'a>,
    /// The first refusal, held while the walk finishes so the caller gets
    /// one diagnostic (the walk continues with a `null` placeholder — the
    /// output is discarded when the error surfaces).
    error: Option<Refusal>,
}

impl<'a> VisitMut<'a> for JsxPass<'a, '_> {
    fn visit_expression(&mut self, expr: &mut Expression<'a>) {
        if matches!(expr, Expression::JSXElement(_) | Expression::JSXFragment(_)) {
            let owned = expr.take_in(&self.emitter.alloc);
            let compiled = match self.emitter.compile_root(owned) {
                Ok(js) => js,
                Err(r) => {
                    if self.error.is_none() {
                        self.error = Some(r);
                    }
                    *expr = self.emitter.parse_expression("null").expect("null parses");
                    return;
                }
            };
            match self.emitter.parse_expression(&compiled) {
                Ok(parsed) => *expr = parsed,
                Err(e) => {
                    if self.error.is_none() {
                        self.error = Some(Refusal::new(format!(
                            "the emitted module failed to re-parse ({e}) — a backend bug"
                        )));
                    }
                    *expr = self.emitter.parse_expression("null").expect("null parses");
                }
            }
            return;
        }
        walk_mut::walk_expression(self, expr);
    }
}

// ---------------------------------------------------------------------------
// The emitter — string-shaped output for every JSX construct, parsed back
// into the arena so the module codegen prints one consistent tree.
// ---------------------------------------------------------------------------

struct Emitter<'a> {
    mode: Mode,
    alloc: &'a Allocator,
    /// The hoisted templates, first-use order: `(escaped html, variable)`.
    templates: Vec<(String, String)>,
    /// The delegated event types, sorted at the registration tail.
    events: BTreeSet<String>,
    /// The runtime helpers the emitted code references (the import's named
    /// list, sorted).
    helpers: BTreeSet<&'static str>,
    next_el: u32,
    next_tmpl: u32,
    /// Non-fatal diagnostics (the SSR drops of client-only constructs).
    warnings: Vec<String>,
}

impl<'a> Emitter<'a> {
    fn new(mode: Mode, alloc: &'a Allocator) -> Self {
        Self {
            mode,
            alloc,
            templates: Vec::new(),
            events: BTreeSet::new(),
            helpers: BTreeSet::new(),
            next_el: 0,
            next_tmpl: 0,
            warnings: Vec::new(),
        }
    }

    /// Parse one expression snippet back into the arena (the emitted
    /// strings share the main allocator's lifetime via `alloc_str`).
    fn parse_expression(&self, src: &str) -> std::result::Result<Expression<'a>, String> {
        let leaked = self.alloc.alloc_str(src);
        let parsed = oxc_parser::Parser::new(self.alloc, leaked, SourceType::mjs()).parse();
        if !parsed.diagnostics.is_empty() {
            let first = first_message(parsed.diagnostics.iter().map(ToString::to_string));
            return Err(first);
        }
        match parsed.program.body.into_iter().next() {
            Some(Statement::ExpressionStatement(s)) => Ok(s.unbox().expression),
            _ => Err("the snippet is not an expression statement".to_string()),
        }
    }

    /// Parse one or more full statements (imports, template consts, the
    /// delegation tail) back into the arena.
    fn parse_statements(
        &self,
        src: &str,
    ) -> std::result::Result<ArenaVec<'a, Statement<'a>>, String> {
        let leaked = self.alloc.alloc_str(src);
        let parsed = oxc_parser::Parser::new(self.alloc, leaked, SourceType::mjs()).parse();
        if !parsed.diagnostics.is_empty() {
            let first = first_message(parsed.diagnostics.iter().map(ToString::to_string));
            return Err(first);
        }
        Ok(parsed.program.body)
    }

    /// The deduped template variable for `html` (the raw, escaped-for-
    /// template-literal markup), hoisted in first-use order.
    fn tmpl_var(&mut self, html: &str) -> String {
        for (seen, name) in &self.templates {
            if seen == html {
                return name.clone();
            }
        }
        self.next_tmpl += 1;
        let name = if self.next_tmpl == 1 {
            "_tmpl$".to_string()
        } else {
            format!("_tmpl${}", self.next_tmpl)
        };
        self.templates.push((html.to_string(), name.clone()));
        self.helpers.insert("template");
        name
    }

    /// Print one (already TS-stripped) expression back to source text —
    /// the embedded-children mechanism: the printed text rides the
    /// emitted strings, the AST is consumed.
    fn print_expr(&self, expr: Expression<'a>) -> std::result::Result<String, Refusal> {
        let mut program = Program::dummy(self.alloc);
        program.source_type = SourceType::mjs();
        let mut stmt = oxc_ast::ast::ExpressionStatement::dummy(self.alloc);
        stmt.span = expr.span();
        stmt.expression = expr;
        program
            .body
            .push(Statement::ExpressionStatement(ArenaBox::new_in(
                stmt,
                &self.alloc,
            )));
        let code = Codegen::new()
            .with_options(CodegenOptions::default())
            .build(&program)
            .code;
        let trimmed = code.trim_end();
        let trimmed = trimmed.strip_suffix(';').unwrap_or(trimmed);
        if trimmed.is_empty() {
            return Err(Refusal::new(
                "an expression printed empty — a backend bug".to_string(),
            ));
        }
        Ok(trimmed.to_string())
    }

    // -- dispatch ---------------------------------------------------------

    /// Compile one JSX expression root (an element or a fragment).
    fn compile_root(&mut self, expr: Expression<'a>) -> std::result::Result<String, Refusal> {
        match expr {
            Expression::JSXElement(el) => {
                let el = el.unbox();
                let tag = element_tag(&el.opening_element.name);
                if tag.is_empty() {
                    return Err(Refusal::new(
                        "a namespaced element tag (`<ns:tag>`) is outside the native Solid \
                         transform scope",
                    ));
                }
                if is_component_tag(&tag) {
                    self.compile_component(el)
                } else if self.mode == Mode::Ssr {
                    self.compile_element_ssr(el)
                } else {
                    self.compile_element_dom(el)
                }
            }
            Expression::JSXFragment(f) => {
                let f = f.unbox();
                if self.mode == Mode::Ssr {
                    return Err(Refusal::new(
                        "a JSX fragment is outside the native Solid transform scope in SSR \
                         output (DOM output compiles fragments to arrays); keep the Node pool \
                         for this file or give the fragment a wrapping element",
                    ));
                }
                let kept: Vec<JSXChild<'a>> = f.children.into_iter().filter(kept_child).collect();
                let items = self.child_items_vec(kept, true)?;
                Ok(format!("[{}]", items.join(", ")))
            }
            _ => Err(Refusal::new(
                "a non-JSX expression reached the JSX emitter — a backend bug".to_string(),
            )),
        }
    }

    // -- components -------------------------------------------------------

    /// `<Comp ...>children</Comp>` → `createComponent(Name, props)`.
    /// The props object carries static values inline, dynamic values as
    /// getters (the Solid props contract — reading the prop re-runs the
    /// getter), and the children under the `children` key.
    fn compile_component(&mut self, el: JSXElement<'a>) -> std::result::Result<String, Refusal> {
        let JSXElement {
            opening_element,
            children,
            ..
        } = el;
        let JSXOpeningElement {
            name, attributes, ..
        } = opening_element.unbox();
        let comp_name = element_name_string(&name)?;
        let mut props: Vec<String> = Vec::new();
        for item in attributes {
            match item {
                JSXAttributeItem::Attribute(a) => {
                    let a = a.unbox();
                    let key = attribute_key(&a.name)?;
                    props.push(self.component_prop(&key, a.value)?);
                }
                JSXAttributeItem::SpreadAttribute(_) => {
                    return Err(Refusal::new(
                        "a JSX spread attribute (`{...props}`) is outside the native Solid \
                         transform scope (v1 compiles spelled props only)",
                    ));
                }
            }
        }
        if let Some(children) = self.compile_component_children(children)? {
            props.push(children);
        }
        self.helpers.insert("createComponent");
        Ok(format!(
            "_createComponent({comp_name}, {{ {} }})",
            props.join(", ")
        ))
    }

    /// One prop entry: static strings/booleans inline, dynamic values as
    /// getters.
    fn component_prop(
        &mut self,
        key: &str,
        value: Option<JSXAttributeValue<'a>>,
    ) -> std::result::Result<String, Refusal> {
        match value {
            None => Ok(format!("{key}: true")),
            Some(JSXAttributeValue::StringLiteral(s)) => Ok(format!(
                "{key}: {}",
                js_string_literal(s.value.as_str().unwrap_or_default())
            )),
            Some(JSXAttributeValue::ExpressionContainer(c)) => {
                let printed = self.container_print(c)?;
                if printed.is_object && (key == "style" || key == "classList") {
                    return Err(Refusal::new(format!(
                        "an object literal passed to `{key}` is outside the native Solid \
                         transform scope (the backend accepts `{key}` strings only)"
                    )));
                }
                Ok(format!("get {key}() {{ return {}; }}", printed.code))
            }
            Some(_) => Err(Refusal::new(
                "a JSX element used as an attribute value is outside the native Solid \
                 transform scope",
            )),
        }
    }

    /// The `children` prop: a single child rides the getter directly,
    /// multiple children ride the array form (dynamic items as accessors —
    /// the runtime re-reads them).
    fn compile_component_children(
        &mut self,
        children: ArenaVec<'a, JSXChild<'a>>,
    ) -> std::result::Result<Option<String>, Refusal> {
        let kept: Vec<JSXChild<'a>> = children.into_iter().filter(|c| kept_child(c)).collect();
        // The single-child shape rides the getter directly — the value
        // itself, not an accessor around it (the getter IS the accessor).
        if kept.len() == 1 {
            let item = self
                .child_item(kept.into_iter().next().expect("len 1"), false)?
                .expect("the kept child compiles to an item");
            return Ok(Some(format!("get children() {{ return {item}; }}")));
        }
        let mut items = Vec::new();
        for child in kept {
            if let Some(item) = self.child_item(child, true)? {
                items.push(item);
            }
        }
        match items.len() {
            0 => Ok(None),
            _ => Ok(Some(format!(
                "get children() {{ return [{}]; }}",
                items.join(", ")
            ))),
        }
    }

    /// The per-child compiled items (shared by DOM fragments);
    /// `wrap_accessors` puts the dynamic expressions behind `() => ...`
    /// accessors (array items — the runtime re-reads them).
    fn child_items_vec(
        &mut self,
        children: Vec<JSXChild<'a>>,
        wrap_accessors: bool,
    ) -> std::result::Result<Vec<String>, Refusal> {
        let mut items = Vec::new();
        for child in children {
            if let Some(item) = self.child_item(child, wrap_accessors)? {
                items.push(item);
            }
        }
        Ok(items)
    }

    /// One compiled child item (`None` = the whitespace-only text node
    /// the JSX whitespace rules drop).
    fn child_item(
        &mut self,
        child: JSXChild<'a>,
        wrap_accessors: bool,
    ) -> std::result::Result<Option<String>, Refusal> {
        match child {
            JSXChild::Text(t) => {
                let _ = wrap_accessors;
                Ok(jsx_text_html(t.unbox().raw.as_deref()).map(|text| js_string_literal(&text)))
            }
            JSXChild::ExpressionContainer(c) => {
                let printed = self.container_print(c)?;
                if wrap_accessors {
                    Ok(Some(format!("() => {}", printed.code)))
                } else {
                    Ok(Some(printed.code))
                }
            }
            JSXChild::Element(el) => {
                let el = el.unbox();
                let tag = element_tag(&el.opening_element.name);
                if tag.is_empty() {
                    return Err(Refusal::new(
                        "a namespaced element tag (`<ns:tag>`) is outside the native Solid \
                         transform scope",
                    ));
                }
                if is_component_tag(&tag) {
                    self.compile_component(el).map(Some)
                } else if self.mode == Mode::Ssr {
                    self.compile_element_ssr(el).map(Some)
                } else {
                    self.compile_element_dom(el).map(Some)
                }
            }
            JSXChild::Fragment(_) => Err(Refusal::new(
                "a nested JSX fragment is outside the native Solid transform scope \
                 (top-level DOM fragments compile to arrays)",
            )),
            JSXChild::Spread(_) => Err(Refusal::new(
                "a JSX spread child (`{...children}`) is outside the native Solid \
                 transform scope",
            )),
        }
    }

    // -- DOM elements -----------------------------------------------------

    /// `<div ...>` → `template()` / the dynamic IIFE. The element shape is
    /// serialized to one HTML template with `<!>` markers at the dynamic
    /// child positions; the dynamics ride named walk variables.
    fn compile_element_dom(&mut self, el: JSXElement<'a>) -> std::result::Result<String, Refusal> {
        let node = self.serialize_dom(el)?;
        let html = render_dom_html(&node)?;
        let tmpl = self.tmpl_var(&js_template_escape(&html));
        if node.is_static() {
            return Ok(format!("{tmpl}()"));
        }

        // The bind set: the root plus every element that carries actions
        // or dynamic children, in document order (pre-order — the output
        // is deterministic).
        let mut binds: Vec<(Vec<usize>, String)> = Vec::new();
        collect_binds(&node, &[], &mut self.next_el, &mut binds);
        let root_name = binds[0].1.clone();
        self.helpers.insert("insert");

        let mut lines: Vec<String> = Vec::new();
        lines.push(format!("const {root_name} = {tmpl}();"));
        // The binds (root first, then document order); each deeper bind
        // chains from the ROOT variable (deterministic, no dependencies
        // between bind lines).
        for (path, name) in binds.iter().skip(1) {
            lines.push(format!("const {name} = {root_name}{};", child_hop(path)));
        }
        // The actions and inserts, document order.
        emit_node_actions(&node, &binds, &root_name, &mut lines);
        lines.push(format!("return {root_name};"));
        Ok(format!("(() => {{\n  {}\n}})()", lines.join("\n  ")))
    }

    /// The DOM element tree: static attrs in `attrs_html`, dynamic actions
    /// on the node, the emitted children in document order.
    fn serialize_dom(&mut self, el: JSXElement<'a>) -> std::result::Result<DomNode, Refusal> {
        let JSXElement {
            opening_element,
            children,
            ..
        } = el;
        let JSXOpeningElement {
            name, attributes, ..
        } = opening_element.unbox();
        let mut node = DomNode {
            tag: element_tag(&name),
            attrs_html: String::new(),
            actions: Vec::new(),
            children: Vec::new(),
        };
        for item in attributes {
            match item {
                JSXAttributeItem::Attribute(a) => {
                    self.dom_attribute(&mut node, a.unbox())?;
                }
                JSXAttributeItem::SpreadAttribute(_) => {
                    return Err(Refusal::new(
                        "a JSX spread attribute (`{...props}`) is outside the native Solid \
                         transform scope (v1 compiles spelled props only)",
                    ));
                }
            }
        }
        self.dom_children(&mut node, children)?;
        Ok(node)
    }

    /// One attribute on a DOM element (the dispatch: plain identifiers
    /// vs the namespaced spellings).
    fn dom_attribute(
        &mut self,
        node: &mut DomNode,
        a: oxc_ast::ast::JSXAttribute<'a>,
    ) -> std::result::Result<(), Refusal> {
        // The dispatch reads the name off the borrowed attribute, then
        // moves the attribute into the arm's helper.
        // The dispatch reads the names off the borrowed attribute, then
        // moves the attribute into the arm's helper.
        let identifier = if let JSXAttributeName::Identifier(id) = &a.name {
            Some(id.name.to_string())
        } else {
            None
        };
        let namespaced = if let JSXAttributeName::NamespacedName(ns) = &a.name {
            Some((ns.namespace.name.to_string(), ns.name.name.to_string()))
        } else {
            None
        };
        match a.name {
            JSXAttributeName::Identifier(_) => {
                self.dom_identifier_attribute(node, a, identifier.unwrap_or_default())
            }
            JSXAttributeName::NamespacedName(_) => {
                let (nsp, local) = namespaced.unwrap_or_default();
                self.dom_namespaced_attribute(node, a, &nsp, &local)
            }
        }
    }

    /// A plain-identifier attribute: the client-only and object-form
    /// refusals, the delegated events and the static/dynamic value arms.
    fn dom_identifier_attribute(
        &mut self,
        node: &mut DomNode,
        a: oxc_ast::ast::JSXAttribute<'a>,
        name: String,
    ) -> std::result::Result<(), Refusal> {
        {
            // Client-only and object-form constructs refuse.
            if name == "ref" {
                return Err(Refusal::new(
                    "the `ref` attribute is outside the native Solid transform scope \
                         (v1 has no ref plumbing)",
                ));
            }
            if name == "classList" {
                return Err(Refusal::new(
                    "`classList` is outside the native Solid transform scope (the backend \
                         accepts `class` strings only)",
                ));
            }
            if name == "innerHTML" {
                return Err(Refusal::new(
                    "the `innerHTML` attribute is outside the native Solid transform scope \
                         (template markup is parsed as HTML; static markup goes into the \
                         template verbatim)",
                ));
            }
            // Delegated events: `onClick` → the `$$click` property.
            if is_delegated_event(&name) {
                let value = match a.value {
                    Some(JSXAttributeValue::ExpressionContainer(c)) => {
                        self.container_print(c)?.code
                    }
                    _ => {
                        return Err(Refusal::new(format!(
                            "the delegated handler `{name}` needs a `{name}={{fn}}` \
                                 expression (string handlers are outside the native Solid \
                                 transform scope)"
                        )));
                    }
                };
                self.events.insert(name[2..].to_ascii_lowercase());
                node.actions.push(DomAction::Delegate {
                    evt: name[2..].to_ascii_lowercase(),
                    value,
                });
                return Ok(());
            }
            match a.value {
                None => {
                    // A bare boolean attribute rides the template.
                    let _ = write!(node.attrs_html, " {name}");
                }
                Some(JSXAttributeValue::StringLiteral(s)) => {
                    let _ = write!(
                        node.attrs_html,
                        " {}=\"{}\"",
                        name,
                        html_attr_escape(s.value.as_str().unwrap_or_default())
                    );
                }
                Some(JSXAttributeValue::ExpressionContainer(c)) => {
                    let printed = self.container_print(c)?;
                    if printed.is_object && (name == "style" || name == "classList") {
                        return Err(Refusal::new(format!(
                            "an object literal passed to `{name}` is outside the native \
                                 Solid transform scope (the backend accepts `{name}` strings \
                                 only)"
                        )));
                    }
                    if PROPERTY_ATTRS.contains(&name.as_str()) {
                        node.actions.push(DomAction::Prop {
                            name,
                            value: printed.code,
                        });
                    } else {
                        self.helpers.insert("setAttribute");
                        node.actions.push(DomAction::Attr {
                            name,
                            value: printed.code,
                        });
                    }
                }
                Some(_) => {
                    return Err(Refusal::new(
                        "a JSX element used as an attribute value is outside the native \
                             Solid transform scope",
                    ));
                }
            }
            Ok(())
        }
    }

    /// A namespaced attribute: the `on:` listener form plus the
    /// directives the v1 scope refuses.
    fn dom_namespaced_attribute(
        &mut self,
        node: &mut DomNode,
        a: oxc_ast::ast::JSXAttribute<'a>,
        nsp: &str,
        local: &str,
    ) -> std::result::Result<(), Refusal> {
        {
            match nsp {
                "on" => {
                    // The non-delegated listener form: `on:click`.
                    let value = match a.value {
                        Some(JSXAttributeValue::ExpressionContainer(c)) => {
                            self.container_print(c)?.code
                        }
                        _ => {
                            return Err(Refusal::new(format!(
                                "the listener `on:{local}` needs an `on:{local}={{fn}}` \
                                     expression"
                            )));
                        }
                    };
                    node.actions.push(DomAction::Listen {
                        evt: local.to_string(),
                        value,
                    });
                    Ok(())
                }
                "oncapture" => Err(Refusal::new(format!(
                    "the `oncapture:{local}` event directive is outside the native Solid \
                         transform scope (v1 compiles delegated and plain listeners only)"
                ))),
                "use" => Err(Refusal::new(format!(
                    "the `use:{local}` directive is outside the native Solid transform \
                         scope (directives need the runtime's action plumbing)"
                ))),
                _ => Err(Refusal::new(format!(
                    "the namespaced attribute `{nsp}:{local}` is outside the native Solid \
                         transform scope"
                ))),
            }
        }
    }

    /// The element's children (fragments are transparent — their children
    /// splice into the parent's list; the DOM template carries no trace).
    fn dom_children(
        &mut self,
        node: &mut DomNode,
        children: ArenaVec<'a, JSXChild<'a>>,
    ) -> std::result::Result<(), Refusal> {
        for child in children {
            match child {
                JSXChild::Text(t) => {
                    if let Some(text) = jsx_text_html(t.unbox().raw.as_deref()) {
                        node.children.push(DomChild::Text(text));
                    }
                }
                JSXChild::ExpressionContainer(c) => {
                    let printed = self.container_print(c)?;
                    node.children.push(DomChild::Hole(printed.code));
                }
                JSXChild::Element(el) => {
                    let el = el.unbox();
                    let tag = element_tag(&el.opening_element.name);
                    if tag.is_empty() {
                        return Err(Refusal::new(
                            "a namespaced element tag (`<ns:tag>`) is outside the native Solid \
                             transform scope",
                        ));
                    }
                    if is_component_tag(&tag) {
                        let compiled = self.compile_component(el)?;
                        node.children.push(DomChild::Comp(compiled));
                    } else {
                        let child_node = self.serialize_dom(el)?;
                        node.children.push(DomChild::Elem(child_node));
                    }
                }
                JSXChild::Fragment(f) => {
                    let f = f.unbox();
                    self.dom_children(node, f.children)?;
                }
                JSXChild::Spread(_) => {
                    return Err(Refusal::new(
                        "a JSX spread child (`{...children}`) is outside the native Solid \
                         transform scope",
                    ));
                }
            }
        }
        Ok(())
    }

    // -- SSR elements -----------------------------------------------------

    /// `<div ...>` → the CONTENT of an `ssr()` template literal:
    /// static attrs in-string, dynamic attrs as `${ssrAttribute(...)}`,
    /// text holes as `${escape(...)}`, hydration keys after every tag
    /// name, components interpolated as `${createComponent(...)}`.
    fn compile_element_ssr(&mut self, el: JSXElement<'a>) -> std::result::Result<String, Refusal> {
        let content = self.serialize_ssr(el)?;
        self.helpers.insert("ssr");
        Ok(format!("_ssr(`{content}`)"))
    }

    fn serialize_ssr(&mut self, el: JSXElement<'a>) -> std::result::Result<String, Refusal> {
        let JSXElement {
            opening_element,
            children,
            ..
        } = el;
        let JSXOpeningElement {
            name, attributes, ..
        } = opening_element.unbox();
        let tag = element_tag(&name);
        let void = VOID_TAGS.contains(&tag.as_str());
        let mut out = String::new();
        out.push('<');
        out.push_str(&tag);
        // The worker compiles with `hydratable: true`: every element
        // carries its hydration key.
        self.helpers.insert("ssrHydrationKey");
        out.push_str("${_ssrHydrationKey()}");
        for item in attributes {
            match item {
                JSXAttributeItem::Attribute(a) => {
                    self.ssr_attribute(&mut out, a.unbox())?;
                }
                JSXAttributeItem::SpreadAttribute(_) => {
                    return Err(Refusal::new(
                        "a JSX spread attribute (`{...props}`) is outside the native Solid \
                         transform scope (v1 compiles spelled props only)",
                    ));
                }
            }
        }
        out.push('>');
        for child in children {
            match child {
                JSXChild::Text(t) => {
                    if let Some(text) = jsx_text_html(t.unbox().raw.as_deref()) {
                        out.push_str(&text);
                    }
                }
                JSXChild::ExpressionContainer(c) => {
                    let printed = self.container_print(c)?;
                    self.helpers.insert("escape");
                    let _ = write!(out, "${{_escape({})}}", printed.code);
                }
                JSXChild::Element(child_el) => {
                    let child_el = child_el.unbox();
                    let child_tag = element_tag(&child_el.opening_element.name);
                    if child_tag.is_empty() {
                        return Err(Refusal::new(
                            "a namespaced element tag (`<ns:tag>`) is outside the native Solid \
                             transform scope",
                        ));
                    }
                    if is_component_tag(&child_tag) {
                        let compiled = self.compile_component(child_el)?;
                        let _ = write!(out, "${{{compiled}}}");
                    } else {
                        let inner = self.serialize_ssr(child_el)?;
                        out.push_str(&inner);
                    }
                }
                JSXChild::Fragment(_) => {
                    return Err(Refusal::new(
                        "a JSX fragment is outside the native Solid transform scope in SSR \
                         output (DOM output compiles fragments to arrays); keep the Node pool \
                         for this file or give the fragment a wrapping element",
                    ));
                }
                JSXChild::Spread(_) => {
                    return Err(Refusal::new(
                        "a JSX spread child (`{...children}`) is outside the native Solid \
                         transform scope",
                    ));
                }
            }
        }
        if !void {
            let _ = write!(out, "</{tag}>");
        }
        Ok(out)
    }

    /// One attribute on an SSR element: statics in-string, dynamics as
    /// `${ssrAttribute(...)}` holes; client-only constructs (events,
    /// refs) drop with a warning — never silently.
    fn ssr_attribute(
        &mut self,
        out: &mut String,
        a: oxc_ast::ast::JSXAttribute<'a>,
    ) -> std::result::Result<(), Refusal> {
        match a.name {
            JSXAttributeName::Identifier(id) => {
                let name = id.name.to_string();
                if name == "classList" {
                    return Err(Refusal::new(
                        "`classList` is outside the native Solid transform scope (the backend \
                         accepts `class` strings only)",
                    ));
                }
                if name == "innerHTML" {
                    return Err(Refusal::new(
                        "the `innerHTML` attribute is outside the native Solid transform scope",
                    ));
                }
                if is_delegated_event(&name) || name == "ref" {
                    self.warnings.push(format!(
                        "native solid ssr output dropped the client-only `{name}` attribute \
                         (it never runs on the server)"
                    ));
                    return Ok(());
                }
                match a.value {
                    None => {
                        let _ = write!(out, " {name}");
                    }
                    Some(JSXAttributeValue::StringLiteral(s)) => {
                        let _ = write!(
                            out,
                            " {}=\"{}\"",
                            name,
                            html_attr_escape(s.value.as_str().unwrap_or_default())
                        );
                    }
                    Some(JSXAttributeValue::ExpressionContainer(c)) => {
                        let printed = self.container_print(c)?;
                        if printed.is_object && (name == "style" || name == "classList") {
                            return Err(Refusal::new(format!(
                                "an object literal passed to `{name}` is outside the native \
                                 Solid transform scope (the backend accepts `{name}` strings \
                                 only)"
                            )));
                        }
                        let is_bool = BOOLEAN_ATTRS.contains(&name.as_str());
                        self.helpers.insert("ssrAttribute");
                        self.helpers.insert("escape");
                        let _ = write!(
                            out,
                            "${{_ssrAttribute(\"{name}\", _escape({}), {is_bool})}}",
                            printed.code
                        );
                    }
                    Some(_) => {
                        return Err(Refusal::new(
                            "a JSX element used as an attribute value is outside the native \
                             Solid transform scope",
                        ));
                    }
                }
                Ok(())
            }
            JSXAttributeName::NamespacedName(ns) => {
                let nsp = ns.namespace.name.to_string();
                let local = ns.name.name.to_string();
                match nsp.as_str() {
                    "on" => {
                        self.warnings.push(format!(
                            "native solid ssr output dropped the client-only `on:{local}` \
                             listener (it never runs on the server)"
                        ));
                        Ok(())
                    }
                    "use" => Err(Refusal::new(format!(
                        "the `use:{local}` directive is outside the native Solid transform \
                         scope (directives need the runtime's action plumbing)"
                    ))),
                    _ => Err(Refusal::new(format!(
                        "the namespaced attribute `{nsp}:{local}` is outside the native Solid \
                         transform scope"
                    ))),
                }
            }
        }
    }

    /// Print a container's inner expression: `None` for the empty `{}` (it
    /// renders nothing), the printed source text otherwise — with the
    /// object-literal flag the `style`/`classList` refusals key on.
    fn container_print(
        &mut self,
        c: ArenaBox<'a, JSXExpressionContainer<'a>>,
    ) -> std::result::Result<Printed, Refusal> {
        let c = c.unbox();
        match c.expression {
            JSXExpression::EmptyExpression(_) => Err(Refusal::new(
                "an empty JSX expression (`{}`) is outside the native Solid transform scope \
                 (render it as nothing by removing the container)",
            )),
            other => {
                let expr = Expression::try_from(other).map_err(|()| {
                    Refusal::new(
                        "a JSX expression slot held a non-expression — a backend bug".to_string(),
                    )
                })?;
                let is_object = matches!(expr, Expression::ObjectExpression(_));
                let code = self.print_expr(expr)?;
                Ok(Printed { code, is_object })
            }
        }
    }
}

/// One printed child expression: the source text plus the object-literal
/// flag the `style`/`classList` refusal checks.
struct Printed {
    code: String,
    is_object: bool,
}

// ---------------------------------------------------------------------------
// DOM node model + emission helpers
// ---------------------------------------------------------------------------

/// One element in the serialized DOM tree.
struct DomNode {
    tag: String,
    /// The static attributes, pre-serialized (` class="counter"`).
    attrs_html: String,
    actions: Vec<DomAction>,
    /// The EMITTED children in document order (whitespace-only text nodes
    /// are absent — the indices are the template's child indices).
    children: Vec<DomChild>,
}

enum DomChild {
    /// Static markup text.
    Text(String),
    /// A dynamic text hole (the template carries a `<!>` marker at its
    /// position unless it is the only child).
    Hole(String),
    /// A compiled component call in child position (same marker rule).
    Comp(String),
    /// A nested element (may carry dynamics of its own).
    Elem(DomNode),
}

enum DomAction {
    /// `onClick` → the `$$click` delegated property.
    Delegate { evt: String, value: String },
    /// `on:click` → a plain `addEventListener`.
    Listen { evt: String, value: String },
    /// A dynamic known property (`value`, `checked`, `selected`).
    Prop { name: String, value: String },
    /// A dynamic attribute → the runtime `setAttribute` helper.
    Attr { name: String, value: String },
}

impl DomNode {
    /// Whether the subtree needs no runtime work (a plain `template()`
    /// clone suffices).
    fn is_static(&self) -> bool {
        if !self.actions.is_empty() {
            return false;
        }
        self.children.iter().all(|c| match c {
            DomChild::Text(_) => true,
            DomChild::Hole(_) | DomChild::Comp(_) => false,
            DomChild::Elem(n) => n.is_static(),
        })
    }

    /// Whether this node carries actions or dynamic children (the bind set).
    fn needs_bind(&self) -> bool {
        !self.actions.is_empty()
            || self
                .children
                .iter()
                .any(|c| matches!(c, DomChild::Hole(_) | DomChild::Comp(_)))
    }
}

/// The next `_el$`-shaped walk-variable name (`_el$`, `_el$2`, ...).
fn next_el_name(counter: &mut u32) -> String {
    *counter += 1;
    if *counter == 1 {
        "_el$".to_string()
    } else {
        format!("_el${counter}")
    }
}

/// The bind walk: the root plus every needs-bind element, pre-order —
/// `(path from the root, variable name)`.
fn collect_binds(
    node: &DomNode,
    path: &[usize],
    next_el: &mut u32,
    out: &mut Vec<(Vec<usize>, String)>,
) {
    if path.is_empty() || node.needs_bind() {
        let name = next_el_name(next_el);
        out.push((path.to_vec(), name));
    }
    for (i, child) in node.children.iter().enumerate() {
        if let DomChild::Elem(child_node) = child {
            let mut child_path = path.to_vec();
            child_path.push(i);
            collect_binds(child_node, &child_path, next_el, out);
        }
    }
}

/// The `firstChild`/`nextSibling` chain for a child-index path (each level
/// descends into the parent's children).
fn child_hop(path: &[usize]) -> String {
    let mut s = String::new();
    for idx in path {
        s.push_str(".firstChild");
        for _ in 0..*idx {
            s.push_str(".nextSibling");
        }
    }
    s
}

/// The actions and inserts of the whole tree (pre-order), appended to
/// `lines`.
fn emit_node_actions(
    node: &DomNode,
    binds: &[(Vec<usize>, String)],
    root_name: &str,
    lines: &mut Vec<String>,
) {
    emit_node_actions_rec(node, &[], binds, root_name, lines);
}

fn emit_node_actions_rec(
    node: &DomNode,
    path: &[usize],
    binds: &[(Vec<usize>, String)],
    root_name: &str,
    lines: &mut Vec<String>,
) {
    let name = node_var(path, binds, root_name);
    for action in &node.actions {
        match action {
            DomAction::Delegate { evt, value } => {
                lines.push(format!("{name}.$${evt} = {value};"));
            }
            DomAction::Listen { evt, value } => {
                lines.push(format!("{name}.addEventListener(\"{evt}\", {value});"));
            }
            DomAction::Prop { name: prop, value } => {
                lines.push(format!("{name}.{prop} = {value};"));
            }
            DomAction::Attr { name: attr, value } => {
                lines.push(format!("_setAttribute({name}, \"{attr}\", () => {value});"));
            }
        }
    }
    for (i, child) in node.children.iter().enumerate() {
        match child {
            DomChild::Hole(value) => {
                let marker = insert_marker(&node.children, i, &name);
                lines.push(format!("_insert({name}, () => {value}{marker});"));
            }
            DomChild::Comp(compiled) => {
                let marker = insert_marker(&node.children, i, &name);
                lines.push(format!("_insert({name}, {compiled}{marker});"));
            }
            DomChild::Elem(child_node) => {
                let mut child_path = path.to_vec();
                child_path.push(i);
                emit_node_actions_rec(child_node, &child_path, binds, root_name, lines);
            }
            DomChild::Text(_) => {}
        }
    }
}

/// The marker argument for the hole at child index `i`: empty when the
/// hole is the parent's only child (the insert appends), else the `<!>`
/// comment node's walk path from the parent.
fn insert_marker(children: &[DomChild], i: usize, parent_name: &str) -> String {
    if children.len() == 1 {
        return String::new();
    }
    format!(", {parent_name}{}", child_hop(&[i]))
}

/// The variable name bound for the node at `path` (the root falls back to
/// its own name).
fn node_var(path: &[usize], binds: &[(Vec<usize>, String)], root_name: &str) -> String {
    binds
        .iter()
        .find(|(p, _)| p == path)
        .map_or_else(|| root_name.to_string(), |(_, n)| n.clone())
}

/// The HTML serialization of the serialized tree (the template source).
fn render_dom_html(node: &DomNode) -> std::result::Result<String, Refusal> {
    let void = VOID_TAGS.contains(&node.tag.as_str());
    let mut out = String::new();
    out.push('<');
    out.push_str(&node.tag);
    out.push_str(&node.attrs_html);
    if void {
        if !node.children.is_empty() {
            return Err(Refusal::new(format!(
                "the void element `<{}>` carries children — a template serialization bug",
                node.tag
            )));
        }
        out.push('>');
        return Ok(out);
    }
    out.push('>');
    for child in &node.children {
        match child {
            DomChild::Text(text) => out.push_str(text),
            // The `<!>` marker exists for positional inserts; a parent's
            // sole dynamic child needs none (the insert appends).
            DomChild::Hole(_) | DomChild::Comp(_) => {
                if node.children.len() > 1 {
                    out.push_str("<!>");
                }
            }
            DomChild::Elem(child_node) => out.push_str(&render_dom_html(child_node)?),
        }
    }
    let _ = write!(out, "</{}>", node.tag);
    Ok(out)
}

// ---------------------------------------------------------------------------
// Module assembly
// ---------------------------------------------------------------------------

/// Insert the runtime import, the hoisted templates and the delegation
/// tail into the module body.
fn assemble_module<'a>(program: &mut Program<'a>, em: &mut Emitter<'a>) -> Result<()> {
    if !em.events.is_empty() {
        em.helpers.insert("delegateEvents");
    }
    let mut prelude: Vec<String> = Vec::new();
    if !em.helpers.is_empty() {
        let names: Vec<String> = em.helpers.iter().map(|h| format!("{h} as _{h}")).collect();
        prelude.push(format!(
            "import {{ {} }} from \"{RUNTIME_MODULE}\";",
            names.join(", ")
        ));
    }
    for (html, name) in &em.templates {
        prelude.push(format!("const {name} = _template(`{html}`);"));
    }
    if !prelude.is_empty() {
        let src = prelude.join("\n");
        let stmts = em.parse_statements(&src).map_err(|e| {
            OxideError(Box::new(oxide_fw_diagnostics::Diagnostic::error(
                "CMP-SOL-001",
                format!("the emitted module prelude failed to re-parse ({e}) — a backend bug"),
            )))
        })?;
        // After the leading imports (the runtime import and the templates
        // ride the same insertion point, in that order).
        let at = program
            .body
            .iter()
            .position(|s| !is_import(s))
            .unwrap_or(program.body.len());
        for (i, stmt) in stmts.into_iter().enumerate() {
            program.body.insert(at + i, stmt);
        }
    }
    if !em.events.is_empty() {
        let list: Vec<String> = em.events.iter().map(|e| format!("\"{e}\"")).collect();
        let src = format!("_delegateEvents([{}]);", list.join(", "));
        let stmts = em.parse_statements(&src).map_err(|e| {
            OxideError(Box::new(oxide_fw_diagnostics::Diagnostic::error(
                "CMP-SOL-001",
                format!("the delegation tail failed to re-parse ({e}) — a backend bug"),
            )))
        })?;
        program.body.extend(stmts);
    }
    Ok(())
}

/// Whether the statement is an import declaration (the prelude insertion
/// point skips them).
fn is_import(s: &Statement<'_>) -> bool {
    s.as_module_declaration()
        .is_some_and(|m| matches!(m, ModuleDeclaration::ImportDeclaration(_)))
}

// ---------------------------------------------------------------------------
// Small helpers — JSX naming, whitespace, escaping
// ---------------------------------------------------------------------------

/// The element's tag path's first segment (`div`, `Foo`, `a.b.c` → `a`,
/// namespaced → empty — the caller refuses).
fn element_tag(name: &JSXElementName<'_>) -> String {
    match name {
        JSXElementName::Identifier(id) => id.name.to_string(),
        JSXElementName::IdentifierReference(id) => id.name.to_string(),
        JSXElementName::MemberExpression(m) => member_object_base(&m.object),
        JSXElementName::ThisExpression(_) => "this".to_string(),
        JSXElementName::NamespacedName(_) => String::new(),
    }
}

/// The full component name for `createComponent` (`Show`, `a.b.c`).
fn element_name_string(name: &JSXElementName<'_>) -> std::result::Result<String, Refusal> {
    match name {
        JSXElementName::Identifier(id) => Ok(id.name.to_string()),
        JSXElementName::IdentifierReference(id) => Ok(id.name.to_string()),
        JSXElementName::MemberExpression(m) => {
            let mut s = member_object_base(&m.object);
            s.push('.');
            s.push_str(&m.property.name);
            Ok(s)
        }
        JSXElementName::ThisExpression(_) => Err(Refusal::new(
            "`<this.Component>` is outside the native Solid transform scope",
        )),
        JSXElementName::NamespacedName(ns) => Err(Refusal::new(format!(
            "the namespaced element `<{}:{}>` is outside the native Solid transform scope",
            ns.namespace.name, ns.name.name
        ))),
    }
}

fn member_object_base(object: &JSXMemberExpressionObject<'_>) -> String {
    match object {
        JSXMemberExpressionObject::IdentifierReference(id) => id.name.to_string(),
        JSXMemberExpressionObject::MemberExpression(m) => {
            let mut s = member_object_base(&m.object);
            s.push('.');
            s.push_str(&m.property.name);
            s
        }
        JSXMemberExpressionObject::ThisExpression(_) => "this".to_string(),
    }
}

/// Capitalized (or member/`this`) tags are components.
fn is_component_tag(tag: &str) -> bool {
    tag.chars().next().is_some_and(|c| c.is_ascii_uppercase()) || tag == "this" || tag.contains('.')
}

/// The prop key for a component attribute (namespaced keys refuse — the
/// component props contract carries plain keys).
fn attribute_key(name: &JSXAttributeName<'_>) -> std::result::Result<String, Refusal> {
    match name {
        JSXAttributeName::Identifier(id) => Ok(id.name.to_string()),
        JSXAttributeName::NamespacedName(ns) => Err(Refusal::new(format!(
            "the namespaced prop `{}:{}` is outside the native Solid transform scope \
             (component props carry plain keys)",
            ns.namespace.name, ns.name.name
        ))),
    }
}

/// `onClick`, `onInput`… — the delegated event spelling. The event type is
/// the remainder lowercased (`onClick` → `click`, `onMouseDown` →
/// `mousedown` — the DOM event names are all-lowercase).
fn is_delegated_event(name: &str) -> bool {
    name.len() > 2
        && name.starts_with("on")
        && name.as_bytes().get(2).is_some_and(u8::is_ascii_uppercase)
}

/// Whether the child survives the JSX whitespace rules (the
/// whitespace-only-across-lines text nodes render nothing and are
/// filtered before the single/multi child branching).
fn kept_child(child: &JSXChild<'_>) -> bool {
    match child {
        JSXChild::Text(t) => jsx_text_html(t.raw.as_deref()).is_some(),
        _ => true,
    }
}

/// The JSX text node's HTML form, babel's whitespace rules: leading
/// whitespace runs containing a newline are removed, trailing runs
/// containing a newline are removed, internal runs containing a newline
/// collapse to one space; same-line whitespace stays verbatim. `None` for
/// the whitespace-only-across-lines nodes (they render nothing).
fn jsx_text_html(raw: Option<&str>) -> Option<String> {
    let raw = raw?;
    if !raw.contains('\n') {
        return if raw.is_empty() {
            None
        } else {
            Some(raw.to_string())
        };
    }
    // Leading whitespace-through-the-first-newline removal.
    let mut s = raw;
    let first_non_ws = s.find(|c: char| !c.is_whitespace()).unwrap_or(s.len());
    if s[..first_non_ws].contains('\n') {
        s = &s[first_non_ws..];
    }
    // Trailing whitespace-from-the-last-newline removal.
    let last_non_ws = s.rfind(|c: char| !c.is_whitespace()).map_or(0, |i| i + 1);
    if s[last_non_ws..].contains('\n') {
        s = &s[..last_non_ws];
    }
    // Internal whitespace runs containing a newline collapse to one space;
    // same-line runs stay verbatim.
    let mut out = String::with_capacity(s.len());
    let mut iter = s.chars().peekable();
    while let Some(c) = iter.next() {
        if c.is_whitespace() {
            let mut run = String::new();
            run.push(c);
            let mut has_nl = c == '\n';
            while let Some(&next) = iter.peek() {
                if !next.is_whitespace() {
                    break;
                }
                if next == '\n' {
                    has_nl = true;
                }
                run.push(next);
                iter.next();
            }
            if has_nl {
                out.push(' ');
            } else {
                out.push_str(&run);
            }
        } else {
            out.push(c);
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

/// Escape a static string into a JS string literal (double quotes — the
/// codegen's normalized spelling).
fn js_string_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Escape static markup into a JS template literal (the emitted templates
/// are backtick literals: the content must not terminate them or start an
/// interpolation).
fn js_template_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('`', "\\`")
        .replace("${", "\\${")
}

/// Escape an attribute VALUE back into HTML (the JSX parser decoded the
/// entities; the template re-parses them).
fn html_attr_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delegated_event_names_lowercase_the_dom_type() {
        assert!(is_delegated_event("onClick"));
        assert!(!is_delegated_event("on")); // bare `on` is not an event
        assert!(!is_delegated_event("only")); // not the `on` prefix
        assert!(!is_delegated_event("class"));
    }

    #[test]
    fn jsx_text_whitespace_follows_the_babel_rules() {
        // Leading newline+indent strips, trailing single space stays.
        assert_eq!(
            jsx_text_html(Some("\n        Clicked ")).as_deref(),
            Some("Clicked ")
        );
        // Leading single space stays, trailing newline+indent strips.
        assert_eq!(
            jsx_text_html(Some(" times\n      ")).as_deref(),
            Some(" times")
        );
        // Whitespace-only across lines renders nothing.
        assert_eq!(jsx_text_html(Some("\n      ")), None);
        // Same-line text stays verbatim.
        assert_eq!(jsx_text_html(Some("Home")).as_deref(), Some("Home"));
        // Internal multi-line runs collapse to one space.
        assert_eq!(jsx_text_html(Some("a\n   b\nc")).as_deref(), Some("a b c"));
        assert_eq!(jsx_text_html(None), None);
    }

    #[test]
    fn escaping_is_round_trip_safe() {
        // The template-literal escape keeps the backtick/`${` markup
        // verbatim through the emitted literal.
        assert_eq!(
            js_template_escape("<p title=\"a`b${c}d\">"),
            "<p title=\"a\\`b\\${c}d\">"
        );
        // The attribute escape re-encodes the decoded entities.
        assert_eq!(
            html_attr_escape("a & b \" c < d"),
            "a &amp; b &quot; c &lt; d"
        );
        // The string-literal escape keeps text values byte-identical.
        assert_eq!(js_string_literal("say \"hi\"\n"), "\"say \\\"hi\\\"\\n\"");
    }

    #[test]
    fn component_tag_detection() {
        assert!(is_component_tag("Show"));
        assert!(is_component_tag("a.b"));
        assert!(is_component_tag("this"));
        assert!(!is_component_tag("div"));
        assert!(!is_component_tag("section"));
    }

    #[test]
    fn child_hop_chains_the_walk() {
        assert_eq!(child_hop(&[0]), ".firstChild");
        assert_eq!(child_hop(&[1]), ".firstChild.nextSibling");
        assert_eq!(child_hop(&[2]), ".firstChild.nextSibling.nextSibling");
        assert_eq!(child_hop(&[0, 1]), ".firstChild.firstChild.nextSibling");
    }

    #[test]
    fn bind_walk_names_the_root_and_the_dynamic_carriers() {
        // h1 (static) is skipped; the p (hole parent) binds; the component
        // child's parent is the root.
        let node = DomNode {
            tag: "main".into(),
            attrs_html: String::new(),
            actions: Vec::new(),
            children: vec![
                DomChild::Elem(DomNode {
                    tag: "h1".into(),
                    attrs_html: String::new(),
                    actions: Vec::new(),
                    children: vec![DomChild::Text("Home".into())],
                }),
                DomChild::Elem(DomNode {
                    tag: "p".into(),
                    attrs_html: String::new(),
                    actions: Vec::new(),
                    children: vec![DomChild::Hole("props.data.message".into())],
                }),
                DomChild::Comp("_createComponent(HmrTarget, {})".into()),
            ],
        };
        let mut next = 0;
        let mut binds = Vec::new();
        collect_binds(&node, &[], &mut next, &mut binds);
        assert_eq!(binds.len(), 2, "root + p only");
        assert_eq!(binds[0], (vec![], "_el$".to_string()));
        assert_eq!(binds[1], (vec![1], "_el$2".to_string()));
    }

    #[test]
    fn markers_appear_only_for_multi_child_parents() {
        let children = vec![
            DomChild::Text("Clicked ".into()),
            DomChild::Hole("count()".into()),
            DomChild::Text(" times".into()),
        ];
        assert_eq!(
            insert_marker(&children, 1, "_el$2"),
            ", _el$2.firstChild.nextSibling"
        );
        let sole = vec![DomChild::Hole("count()".into())];
        assert_eq!(insert_marker(&sole, 0, "_el$"), "");
    }
}
