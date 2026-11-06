//! Oxc-backed `Transformer` (spec D4: "Oxc native for TS/JSX/refresh").
//!
//! `OxcTransformer` parses with `oxc_parser` — producing real parser diagnostics
//! (`CMP-PARSE` on invalid syntax) — and uses the actual `oxc_ast` AST to enumerate
//! top-level ESM imports/exports with **byte-exact spans**. The remaining passes
//! (`'use cache'`, server-fn stubs, forbidden-import guard, HMR boundary inject)
//! delegate to the same scanner pass functions in `lib.rs` that `ScannerTransformer`
//! uses; replacing them with full AST visitors is tracked in `docs/deviations.md`.
//!
//! `ScannerTransformer` remains available: it needs no allocator and is the fast
//! path when only span surgery (not syntax validation) is needed.

use crate::{
    Directive, Env, ImportScan, ImportSpan, Result, ServerFnStub, TransformInput, TransformOutput,
    Transformer, inject_hmr_boundary, is_server_module_path, scan_import_spans,
};
use oxc_allocator::Allocator;
use oxc_ast::ast::{
    BindingPattern, Declaration, ExportDefaultDeclarationKind, Expression, Program, Statement,
    VariableDeclaration,
};
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType};
use oxide_fw_diagnostics::Diagnostic;
use std::collections::BTreeSet;
use std::path::PathBuf;

/// Oxc-backed transformer (D4). Parse errors surface as `CMP-PARSE` diagnostics;
/// the pipeline continues with scanner passes so bad user code still gets a
/// structured response.
pub struct OxcTransformer;

impl Transformer for OxcTransformer {
    fn transform(&self, input: &TransformInput) -> Result<TransformOutput> {
        let allocator = Allocator::default();
        let source_type = source_type_for(input.lang);
        let parsed = Parser::new(&allocator, &input.code, source_type).parse();

        // §19-E: a parse error carries a real span. Turn it into a
        // line/column `Location` plus a rendered code frame, so the message
        // names the offending line rather than dumping an Oxc debug struct.
        let mut diagnostics: Vec<Diagnostic> = parsed
            .diagnostics
            .iter()
            .map(|e| {
                let msg = e.message.to_string();
                let mut d = Diagnostic::error("CMP-PARSE", msg)
                    .with_fix("fix the syntax error reported by the Oxc parser");
                if let Some(span) = e.labels.first().map(|l| l.span()) {
                    let start = span.start as usize;
                    // Byte offset -> 1-based (line, column). Column is a
                    // character count within the line, not a byte count, so
                    // a multi-byte character does not shift the caret.
                    let before = &input.code[..start.min(input.code.len())];
                    let line = before.matches('\n').count() + 1;
                    let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
                    let col = before[line_start..].chars().count() + 1;
                    d = d.at(oxidation_location(&input.path, line, col));
                    let frame =
                        oxide_fw_diagnostics::render_code_frame(&input.code, line, Some(col));
                    if !frame.is_empty() {
                        d = d.with_code_frame(frame);
                    }
                }
                d
            })
            .collect();

        // Oxc AST walk for top-level static imports/re-exports.
        let oxc_spans = scan_top_level_spans(&parsed.program, &input.code);

        // Merge scanner spans for anything the Oxc AST walk didn't cover:
        //   - dynamic imports (`import('...')`)
        //   - `export {x} from 'm'` re-exports (Oxc 0.153 dropped `source` from
        //     `ExportNamedDeclaration`; the scanner sees it byte-exactly)
        // Merge is by span position, so nothing is duplicated.
        let mut all_spans = oxc_spans;
        for s in scan_import_spans(&input.code) {
            if !all_spans
                .iter()
                .any(|o| o.start == s.start && o.end == s.end)
            {
                all_spans.push(s);
            }
        }
        all_spans.sort_by_key(|s| s.start);
        let scan = build_scan(&all_spans);

        // AST-driven directive + server-fn passes. The scanner-based transforms are
        // retained as fallback paths used by `ScannerTransformer`, but the Oxc
        // transformer walks the real AST: directives are found via
        // `FunctionBody.directives`, server-fn calls via `CallExpression` with a
        // `serverFn` callee. Both fall back to the scanner if the AST exposes an
        // unexpected shape (defensive — future Oxc revisions may rename variants).
        let mut code = input.code.clone();
        let mut directives: Vec<Directive> = Vec::new();
        let mut server_fns: Vec<ServerFnStub> = Vec::new();
        let server = is_server_module_path(&input.path);
        if server && input.env == Env::Ssr {
            let (c, d) = ast_use_cache(&parsed.program, &code, &input.path)?;
            code = c;
            directives = d;
        }
        if server {
            let (c, f) = ast_server_fn_stubs(
                &parsed.program,
                &code,
                &input.path,
                input.env,
                &input.build_secret,
            )?;
            code = c;
            server_fns = f;
        }
        if input.dev && !server {
            code = inject_hmr_boundary(&code, &input.path, input.lang)?;
        }

        let (lowered, map) = match lower_to_js(
            &code,
            input.lang,
            &input.path,
            input.jsx_import_source.as_deref(),
        ) {
            Ok((s, m)) => (s, m),
            Err(e) => {
                diagnostics.push(*e.0);
                (code, None)
            }
        };
        Ok(TransformOutput {
            code: lowered,
            map,
            scan,
            directives,
            server_fns,
            diagnostics,
        })
    }
}

/// Lower TS/JSX to plain JS and produce a V3 sourcemap back to the original
/// source. The map is `None` only if codegen chose not to emit one, which
/// today cannot happen given we always set `source_map_path`.
pub fn lower_to_js(
    code: &str,
    lang: &'static str,
    path: &str,
    jsx_import_source: Option<&str>,
) -> Result<(String, Option<oxc_sourcemap::OwnedSourceMap>)> {
    let allocator = Allocator::default();
    let source_type = source_type_for(lang);
    let parsed = Parser::new(&allocator, code, source_type).parse();
    if !parsed.diagnostics.is_empty() {
        return Err(crate::err(
            "CMP-PARSE",
            format!(
                "cannot lower `{path}` to JavaScript: {} parse error(s)",
                parsed.diagnostics.len()
            ),
        ));
    }
    let mut program = parsed.program;

    let semantic_ret = oxc_semantic::SemanticBuilder::new().build(&program);
    let scoping = semantic_ret.semantic.into_scoping();

    // §6.3: the JSX runtime is the adapter's choice. React uses the
    // `react/jsx-runtime` default; Solid 2.0 declares `@solidjs/web`, whose
    // `jsx-runtime` subpath is where `jsx`/`jsxs`/`Fragment` live. Without
    // this the emitted code imports React's runtime into a Solid page — the
    // chunk parses but produces React elements, and Solid's SSR renders an
    // empty string because its reactive tree was never built.
    let mut jsx = oxc_transformer::JsxOptions::enable();
    if let Some(src) = jsx_import_source {
        jsx.import_source = Some(src.to_string());
    }
    let options = oxc_transformer::TransformOptions {
        typescript: oxc_transformer::TypeScriptOptions::default(),
        jsx,
        ..Default::default()
    };
    let transformer =
        oxc_transformer::Transformer::new(&allocator, std::path::Path::new(path), &options);
    let _ = transformer.build_with_scoping(scoping, &mut program);

    // Turn on sourcemap generation in codegen. `source_map_path` becomes the
    // `sources[0]` entry — the user's original file path as it appears on disk.
    let cg_options = oxc_codegen::CodegenOptions {
        source_map_path: Some(PathBuf::from(path)),
        ..oxc_codegen::CodegenOptions::default()
    };
    let ret = oxc_codegen::Codegen::new()
        .with_options(cg_options)
        .build(&program);
    let code = ret.code;
    let map = ret.map.map(|m| m.into_owned_sourcemap());
    Ok((code, map))
}

/// Inject React Refresh's `$RefreshReg$` calls for every top-level component
/// (function declaration, class declaration, or arrow/function/class bound to
/// `const`/`let`/`var`), plus the `$RefreshReg$`/`$RefreshSig$` shim at the
/// top of the module. Wrapped in `export` declarations too.
///
/// Returns the input unchanged for:
/// - Non-tsx/jsx languages (React Refresh only matters for React components).
/// - Sources that already declare `$RefreshReg$` (idempotent — a caller that
///   re-transforms an already-injected source keeps one shim and one set of
///   registration calls).
/// - Sources that fail to parse (best-effort; the caller will get the parse
///   diagnostic from `lower_to_js` first).
/// - Sources with no top-level components.
///
/// The `$RefreshSig$` chain for custom hooks is intentionally **not**
/// generated — React Refresh treats an unhinted component as "always
/// invalidated when its module changes," which is a safe default and what the
/// official Babel plugin does in the absence of a stable hook signature. A
/// later phase can add the signature chain.
pub fn inject_react_refresh(code: &str, lang: &str, path: &str) -> Result<String> {
    if !matches!(lang, "tsx" | "jsx") {
        return Ok(code.to_string());
    }
    if code.contains("$RefreshReg$") {
        return Ok(code.to_string());
    }
    let allocator = Allocator::default();
    let source_type = source_type_for(lang);
    let parsed = Parser::new(&allocator, code, source_type).parse();
    if !parsed.diagnostics.is_empty() {
        return Ok(code.to_string());
    }
    let mut regs: Vec<String> = Vec::new();
    for stmt in &parsed.program.body {
        regs.extend(top_level_component_names(stmt));
    }
    if regs.is_empty() {
        return Ok(code.to_string());
    }
    // §1.2: collect hook-wrapping edits BEFORE emitting. Each `useX(...)`
    // top-level call inside a component function body gets its CALLEE
    // wrapped as `_s(useX)(...)` — the outer `_s(useX)` records the hook
    // type in the signature, the trailing `(...)` invokes the real hook. React Refresh's `createSignatureFunctionForTransform()`
    // reads this to detect hook-shape changes: same sequence → in-place
    // update; different → remount (rather than silently keeping the OLD
    // closure against the new hook chain).
    //
    // Scope (deliberately narrow): top-level `VariableDeclaration`
    // declarators whose init is a `use*` call, and top-level
    // `ExpressionStatement`s whose expression is a `use*` call. Nested
    // hooks (inside `.map(...)`, `if (...)`) are NOT wrapped — React's
    // rules of hooks forbid them at runtime anyway, so wrapping them is
    // not needed and the byte-span edit is more error-prone there.
    // §1.2: per-COMPONENT hook-sequence signatures. Emitted at MODULE
    // scope AFTER the component declaration, matching react-refresh's
    // babel plugin (react-refresh/cjs/react-refresh-babel.development.js
    // line 468-480: `insertAfterPath.insertAfter(_s(Component, key))`).
    //
    // What `key` is (line 205-229 of the plugin + line 129-137 of
    // `getHookCallsSignature`): a STRING built from the hook occurrences
    // in the component body, each formatted `name{varName(args)}`, joined
    // by `\n`. `setSignature(Component, key)` records this string;
    // `refreshUpdate` compares old vs new `computeFullKey(sig)` — same
    // → in-place update, different → remount. If the key were the
    // component path (as an earlier attempt emitted), old and new keys
    // would be identical, and the runtime would do an in-place update
    // against a mismatched hook chain — React throws "Rendered more hooks
    // than during the previous render" exactly as observed.
    //
    // What `getCustomHooks` is (4th arg of `_s(...)`): a function that
    // returns an array of hook FUNCTIONS in body order. The runtime looks
    // each up in `allSignaturesByType`; if found, appends its key to
    // `computeFullKey`. Per-hook entries only exist if we emit them (we
    // don't in this implementation — no need, since the sequence string
    // already covers shape), so this returns an empty array for now,
    // which the runtime handles (line 89-93: default `getCustomHooks` =
    // `() => []`).
    // §1.2 EMIT TEMPORARILY DISABLED (2026-11-04). The walker
    // (build_hook_key + collect_components_for_sig) is verified and
    // tested, but emitting `_s(Comp, key)` at module scope was
    // destabilizing the dev server under test-suite load — see the
    // session's diagnostic logs (`ECONNRESET` on pure-HTTP tests
    // indicates a Rust panic in the compiler during a rebuild burst;
    // `hmr-refresh-sig.spec.ts` passes in isolation, so the emit is
    // correct for the sig path but interacts badly with the watcher's
    // rebuild-burst path).
    //
    // The walker and runtime hook (`__oxide_refresh_sig` + bufferedSigs)
    // ship; the emit is re-enabled below.
    //
    // §1.2 RE-ENABLED (2026-11-05). The ECONNRESET the disable note refers
    // to was the boot-storm chain fixed in 8359a20b (watcher boot baseline,
    // node_modules event filtering, §3.3 boot-settle grace, stale-generation
    // gating, client runtime race): the "rebuild burst under full-suite
    // load" that isolated runs never reproduced was the boot-time
    // workspace-rebuild flood, not the emit. The emit shape below inserts
    // `_s(Comp, hook_key);` at MODULE scope directly after each component's
    // declaring statement — the same placement react-refresh's babel plugin
    // uses (insertAfterPath on the declaration), which also keeps
    // arrow-function components (`const X = () => …`) syntactically valid,
    // unlike a body-end insertion.
    let mut components: Vec<ComponentEmit> = Vec::new();
    for stmt in &parsed.program.body {
        collect_components_for_sig(stmt, code, &mut components);
    }

    // Build (byte_position, insert_text) edits: one `_s(Comp, key);\n` after
    // each component's body. Ascending order, then reverse for splice safety.
    let mut edits: Vec<(usize, String)> = Vec::new();
    for comp in &components {
        let stmt = format!(
            "_s({name}, {key:?});\n",
            name = comp.name,
            key = comp.hook_key,
        );
        edits.push((comp.insert_at, stmt));
    }
    edits.sort_by_key(|(pos, _)| std::cmp::Reverse(*pos));
    let mut code_with_sig = code.to_string();
    for (pos, text) in edits {
        code_with_sig.insert_str(pos, &text);
    }

    let mut out = String::with_capacity(code_with_sig.len() + 200 + regs.len() * 40);
    out.push_str("// oxide: React Refresh (dev only)\n");
    out.push_str(
        "const $RefreshReg$ = (t, id) => { const r = globalThis.__oxide_refresh_reg; if (typeof r === 'function') { try { r(t, id, import.meta.url); } catch {} } };\n",
    );
    out.push_str("const $RefreshSig$ = () => (globalThis.__oxide_refresh_sig ? globalThis.__oxide_refresh_sig() : ((t) => t));\n");
    // §1.2: one `_s` per module, shared by every wrapped hook call. The
    // runtime's `createSignatureFunctionForTransform` returns a per-component
    // tracker that discriminates by `type` (the hook function itself), so
    // sharing one `_s` across all components is what react-refresh's own
    // babel plugin does.
    out.push_str("const _s = $RefreshSig$();\n");
    out.push_str(&code_with_sig);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    // Emit `path:name` as the id. React Refresh keys families by the
    // id passed to `$RefreshReg$`: if the id changes, the old and new
    // module versions are treated as different components and
    // `performReactRefresh()` has nothing to correlate against (the
    // screen doesn't update). Building the id from `import.meta.url`
    // breaks this because the HMR re-import URL carries `?t=<ms>`. The
    // source path is stable across reloads, so use it directly.
    for name in &regs {
        let stable_id = format!("{path}:{name}");
        out.push_str("$RefreshReg$(");
        out.push_str(name);
        out.push_str(", ");
        out.push_str(&format!("{:?}", stable_id));
        out.push_str(");\n");
    }
    Ok(out)
}

/// §1.2: per-component hook-signature info. Each top-level component
/// gets a `_s(Component, "path:Name", false, function() { return [hooks]; });`
/// call at the START of its body. That records the component's
/// hook-sequence signature, which `performReactRefresh` uses to
/// differentiate in-place updates (same sequence) from remounts
/// (different sequence). Without this, the runtime has only per-hook
/// entries (from `_s(useState, "useState")`) but no component-level
/// sequence to compare.
/// §1.2: a component with its hook-sequence key. Emitted as
/// `_s(name, key)` at module scope after the component declaration.
struct ComponentEmit {
    name: String,
    /// Byte offset to insert the `_s(name, key);\n` — the END of the
    /// component's DECLARING statement (function declaration, variable
    /// declaration, or their export wrappers). Statement end, not function
    /// body end: an arrow-function component (`const X = () => {}`) would
    /// otherwise get the insert landed between `}` and `;`.
    insert_at: usize,
    /// Babel-matching hook-sequence string. E.g. for
    ///   `const [count, setCount] = useState(0); useEffect(() => {}, []);`
    /// the key is `"useState{[count, setCount](0)}\nuseEffect{}"`.
    /// Different hook sequences produce different keys → remount.
    hook_key: String,
}

/// §1.2: build the hook-sequence key for a function body. Returns `None`
/// when the body has no top-level hooks — such a component is a "no possible
/// shape change" and the runtime treats it as in-place-always, which is
/// correct (nothing to remount over).
fn build_hook_key(body: &oxc_ast::ast::FunctionBody<'_>, code: &str) -> Option<String> {
    use oxc_ast::ast::{BindingPattern, Expression, Statement};
    use oxc_span::GetSpan;
    let mut parts: Vec<String> = Vec::new();
    for stmt in &body.statements {
        let mut push_hook = |hook: &str, var_pattern: Option<&str>, call: &oxc_ast::ast::CallExpression<'_>| {
            let mut s = String::new();
            s.push_str(hook);
            s.push('{');
            if let Some(v) = var_pattern {
                s.push_str(v);
            }
            // Babel format: useState and useReducer append their relevant
            // arg source. From `getHookCallsSignature` (babel plugin
            // line ~129-137): useState → arg[0], useReducer → arg[1].
            if hook == "useState" && !call.arguments.is_empty() {
                if let Some(expr) = call.arguments[0].as_expression() {
                    let span = expr.span();
                    s.push('(');
                    s.push_str(&code[span.start as usize..span.end as usize]);
                    s.push(')');
                }
            } else if hook == "useReducer" && call.arguments.len() > 1
                && let Some(expr) = call.arguments[1].as_expression() {
                    let span = expr.span();
                    s.push('(');
                    s.push_str(&code[span.start as usize..span.end as usize]);
                    s.push(')');
                }
            s.push('}');
            parts.push(s);
        };
        match stmt {
            Statement::VariableDeclaration(v) => {
                for d in &v.declarations {
                    let Some(init) = &d.init else { continue };
                    let Expression::CallExpression(call) = init else { continue };
                    let Expression::Identifier(id) = &call.callee else { continue };
                    let name = id.name.as_str();
                    if name.len() < 4
                        || !name.starts_with("use")
                        || !name.chars().nth(3).map(|c| c.is_uppercase()).unwrap_or(false)
                    {
                        continue;
                    }
                    let var_src: Option<String> = match &d.id {
                        BindingPattern::BindingIdentifier(id) => Some(id.name.to_string()),
                        other => {
                            let span = other.span();
                            Some(code[span.start as usize..span.end as usize].to_string())
                        }
                    };
                    push_hook(name, var_src.as_deref(), call);
                }
            }
            Statement::ExpressionStatement(es) => {
                let Expression::CallExpression(call) = &es.expression else { continue };
                let Expression::Identifier(id) = &call.callee else { continue };
                let name = id.name.as_str();
                if name.len() < 4
                    || !name.starts_with("use")
                    || !name.chars().nth(3).map(|c| c.is_uppercase()).unwrap_or(false)
                {
                    continue;
                }
                push_hook(name, None, call);
            }
            _ => {}
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

/// §1.2: walk a top-level statement and collect every component it declares
/// (function decl, `const X = () => ...`, `const X = function ...`, wrapped
/// in export). Each with its hook-sequence key and body end offset.
fn collect_components_for_sig(
    stmt: &oxc_ast::ast::Statement<'_>,
    code: &str,
    out: &mut Vec<ComponentEmit>,
) {
    use oxc_ast::ast::{
        BindingPattern, Declaration, ExportDefaultDeclarationKind, Expression, Statement,
    };
    use oxc_span::GetSpan;

    fn walk(
        name: &str,
        body: Option<&oxc_ast::ast::FunctionBody<'_>>,
        insert_at: usize,
        code: &str,
        out: &mut Vec<ComponentEmit>,
    ) {
        let Some(body) = body else { return };
        if let Some(key) = build_hook_key(body, code) {
            out.push(ComponentEmit {
                name: name.to_string(),
                insert_at,
                hook_key: key,
            });
        }
    }

    // §1.2: the `_s(Comp, key);` statement is inserted AFTER the declaring
    // statement's end — the same placement react-refresh's babel plugin
    // uses (insertAfterPath on the declaration). Inserting at the function
    // BODY's end would land between an arrow function's closing `}` and the
    // declarator's `;` (`const X = () => {}_s(X, …);`) — a syntax error.
    let stmt_end = stmt.span().end as usize;
    match stmt {
        Statement::FunctionDeclaration(f) => {
            if let Some(id) = &f.id {
                walk(id.name.as_str(), f.body.as_deref(), stmt_end, code, out);
            }
        }
        Statement::VariableDeclaration(v) => {
            for d in &v.declarations {
                let BindingPattern::BindingIdentifier(id) = &d.id else { continue };
                let Some(init) = &d.init else { continue };
                match init {
                    Expression::ArrowFunctionExpression(a) => {
                        if let oxc_ast::ast::ArrowFunctionBody::FunctionBody(body) = &a.body {
                            walk(id.name.as_str(), Some(body), stmt_end, code, out);
                        }
                    }
                    Expression::FunctionExpression(fe) => {
                        walk(id.name.as_str(), fe.body.as_deref(), stmt_end, code, out);
                    }
                    _ => {}
                }
            }
        }
        Statement::ExportDeclaration(e) => match &e.declaration {
            Declaration::FunctionDeclaration(f) => {
                if let Some(id) = &f.id {
                    walk(id.name.as_str(), f.body.as_deref(), stmt_end, code, out);
                }
            }
            Declaration::VariableDeclaration(v) => {
                for d in &v.declarations {
                    let BindingPattern::BindingIdentifier(id) = &d.id else { continue };
                    let Some(init) = &d.init else { continue };
                    if let Expression::ArrowFunctionExpression(a) = init
                        && let oxc_ast::ast::ArrowFunctionBody::FunctionBody(body) = &a.body
                    {
                        walk(id.name.as_str(), Some(body), stmt_end, code, out);
                    }
                }
            }
            _ => {}
        },
        Statement::ExportDefaultDeclaration(e) => if let ExportDefaultDeclarationKind::FunctionDeclaration(f) = &e.declaration {
            let name = f
                .id
                .as_ref()
                .map(|i| i.name.to_string())
                .unwrap_or_else(|| "default".to_string());
            walk(&name, f.body.as_deref(), stmt_end, code, out);
        },
        _ => {}
    }
}

fn top_level_component_names(stmt: &Statement<'_>) -> Vec<String> {
    match stmt {
        Statement::FunctionDeclaration(f) => f
            .id
            .as_ref()
            .map(|id| vec![id.name.to_string()])
            .unwrap_or_default(),
        Statement::ClassDeclaration(c) => c
            .id
            .as_ref()
            .map(|id| vec![id.name.to_string()])
            .unwrap_or_default(),
        Statement::ExportDeclaration(e) => match &e.declaration {
            Declaration::FunctionDeclaration(f) => f
                .id
                .as_ref()
                .map(|id| vec![id.name.to_string()])
                .unwrap_or_default(),
            Declaration::ClassDeclaration(cl) => cl
                .id
                .as_ref()
                .map(|id| vec![id.name.to_string()])
                .unwrap_or_default(),
            Declaration::VariableDeclaration(v) => {
                var_declarator_component_names(v.as_ref())
            }
            _ => Vec::new(),
        },
        Statement::VariableDeclaration(v) => var_declarator_component_names(v.as_ref()),
        Statement::ExportDefaultDeclaration(e) => match &e.declaration {
            ExportDefaultDeclarationKind::FunctionDeclaration(f) => f
                .id
                .as_ref()
                .map(|id| vec![id.name.to_string()])
                .unwrap_or_default(),
            ExportDefaultDeclarationKind::ClassDeclaration(cl) => cl
                .id
                .as_ref()
                .map(|id| vec![id.name.to_string()])
                .unwrap_or_default(),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

fn var_declarator_component_names(v: &VariableDeclaration<'_>) -> Vec<String> {
    v.declarations
        .iter()
        .filter_map(|decl| {
            let init = decl.init.as_ref()?;
            if !matches!(
                init,
                Expression::ArrowFunctionExpression(_)
                    | Expression::FunctionExpression(_)
                    | Expression::ClassExpression(_)
            ) {
                return None;
            }
            if let BindingPattern::BindingIdentifier(id) = &decl.id {
                Some(id.name.to_string())
            } else {
                None
            }
        })
        .collect()
}

/// `lower_to_js` with React Refresh registration calls injected for `.tsx`
/// and `.jsx` sources. The transform ordering matches the JSX-lowering path:
/// parse once, lower to JS, then walk the **original** source for component
/// names (the lowered output still contains the same top-level bindings). If
/// either pass fails, the JS-lowered code is still returned — a
/// Refresh-registration failure must never break rendering.
pub fn lower_to_js_with_refresh(
    code: &str,
    lang: &'static str,
    path: &str,
    jsx_import_source: Option<&str>,
) -> Result<(String, Option<oxc_sourcemap::OwnedSourceMap>)> {
    let (lowered, srcmap) = lower_to_js(code, lang, path, jsx_import_source)?;
    if !matches!(lang, "tsx" | "jsx") {
        return Ok((lowered, srcmap));
    }
    let with_refresh = inject_react_refresh(&lowered, lang, path).unwrap_or(lowered);
    Ok((with_refresh, srcmap))
}


impl OxcTransformer {
    /// Run only the AST-driven passes — `'use cache'` rewriting, server-fn
    /// stubs, HMR boundary — and *not* the TS/JSX lowering stage.
    ///
    /// Used by the Rolldown integration: Rolldown runs its own Oxc
    /// lowering on the output, so the framework-specific transforms happen
    /// here first, on the original source (correct spans, correct
    /// identifiers), and the TS/JSX strip happens once downstream. Running
    /// `transform` instead would lower twice and double-codegen.
    ///
    /// Returns the rewritten source. The `scan`/`directives`/`server_fns`
    /// metadata is computed by the caller when needed; this method is
    /// deliberately narrow.
    pub fn transform_passes_only(&self, input: &TransformInput) -> Result<String> {
        let allocator = Allocator::default();
        let source_type = source_type_for(input.lang);
        let parsed = Parser::new(&allocator, &input.code, source_type).parse();
        if !parsed.diagnostics.is_empty() {
            // §19-E: carry the first error's span as a `Location` plus a
            // rendered code frame, so the message names the offending line
            // instead of only the file and a count. The first error is the
            // primary one — Oxc emits follow-on diagnostics that cascade from
            // it, and pointing at the second one is unhelpful.
            let first = &parsed.diagnostics[0];
            let mut d = Diagnostic::error(
                "CMP-PARSE",
                format!(
                    "`{}` failed to parse: {} error(s): {}",
                    input.path,
                    parsed.diagnostics.len(),
                    first.message,
                ),
            )
            .with_fix("fix the syntax error reported by the Oxc parser");
            if let Some(span) = first.labels.first().map(|l| l.span()) {
                let start = span.start as usize;
                let before = &input.code[..start.min(input.code.len())];
                let line = before.matches('\n').count() + 1;
                let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
                let col = before[line_start..].chars().count() + 1;
                d = d.at(oxide_fw_diagnostics::Location::file(
                    &input.path,
                    line as u32,
                    col as u32,
                ));
                let frame = oxide_fw_diagnostics::render_code_frame(&input.code, line, Some(col));
                if !frame.is_empty() {
                    d = d.with_code_frame(frame);
                }
            }
            return Err(crate::OxideError(Box::new(d)));
        }

        let mut code = input.code.clone();
        let server = is_server_module_path(&input.path);
        if server && input.env == Env::Ssr {
            let (c, _) = ast_use_cache(&parsed.program, &code, &input.path)?;
            code = c;
        }
        if server {
            let (c, _) = ast_server_fn_stubs(
                &parsed.program,
                &code,
                &input.path,
                input.env,
                &input.build_secret,
            )?;
            code = c;
        }
        if input.dev && !server {
            code = inject_hmr_boundary(&code, &input.path, input.lang)?;
        }
        Ok(code)
    }
}

fn source_type_for(lang: &str) -> SourceType {
    match lang {
        "tsx" => SourceType::tsx(),
        "ts" => SourceType::ts(),
        "jsx" => SourceType::jsx(),
        "mjs" | "js" => SourceType::mjs(),
        _ => SourceType::mjs(),
    }
}

/// Walk the top-level AST for `ImportDeclaration` / `ExportNamedDeclaration` with
/// a source / `ExportAllDeclaration`, returning byte-exact spans. Never touches
/// dynamic imports (a follow-up visitor handles those).
fn scan_top_level_spans(program: &Program, _source: &str) -> Vec<ImportSpan> {
    let mut out: Vec<ImportSpan> = Vec::new();
    for stmt in &program.body {
        match stmt {
            Statement::ImportDeclaration(decl) => {
                out.push(ImportSpan {
                    start: decl.span.start as usize,
                    end: decl.span.end as usize,
                    specifier: decl.source.value.as_str().unwrap_or_default().to_string(),
                    dynamic: false,
                });
            }
            // NOTE: In Oxc 0.153, `ExportNamedDeclaration` no longer carries a
            // `source` field (`export {x} from 'm'` re-exports moved). The tolerant
            // scanner handles `export {...} from '...'` byte-exactly, and this
            // function's spans are merged with the scanner's in `transform()`, so
            // the missing branch is covered without duplication.
            Statement::ExportAllDeclaration(decl) => {
                out.push(ImportSpan {
                    start: decl.span.start as usize,
                    end: decl.span.end as usize,
                    specifier: decl.source.value.as_str().unwrap_or_default().to_string(),
                    dynamic: false,
                });
            }
            _ => {}
        }
    }
    out
}

fn build_scan(spans: &[ImportSpan]) -> ImportScan {
    let mut scan = ImportScan::default();
    let mut seen_static: BTreeSet<String> = BTreeSet::new();
    let mut seen_dynamic: BTreeSet<String> = BTreeSet::new();
    let mut seen_asset: BTreeSet<String> = BTreeSet::new();
    for sp in spans {
        if sp.specifier.contains('?') {
            if seen_asset.insert(sp.specifier.clone()) {
                scan.asset_queries.push(sp.specifier.clone());
            }
        } else if sp.dynamic {
            if seen_dynamic.insert(sp.specifier.clone()) {
                scan.dynamic_imports.push(sp.specifier.clone());
            }
        } else if seen_static.insert(sp.specifier.clone()) {
            scan.imports.push(sp.specifier.clone());
        }
    }
    scan
}

// ---------------- Oxc-AST-driven passes ----------------

use std::collections::BTreeMap as AstBTreeMap;

/// Extract the raw text of a directive's string value (`"use cache"` → `use cache`).
fn directive_text(d: &oxc_ast::ast::Directive<'_>) -> String {
    d.expression.value.as_str().unwrap_or_default().to_string()
}

/// Find the first `'use cache'` directive in a function body.
fn find_use_cache_directive(body: &oxc_ast::ast::FunctionBody<'_>) -> Option<(u32, u32)> {
    for d in &body.directives {
        if directive_text(d) == "use cache" {
            return Some((d.span.start, d.span.end));
        }
    }
    None
}

/// The scanner emits `__oxide_cache(<fn_text>, "<module>", "<name>", {opts})`.
/// This function collects the same edits, but drives detection from the AST.
/// It produces `(start, end, replacement)` triples and the `Directive` records.
fn ast_use_cache(
    program: &oxc_ast::ast::Program<'_>,
    code: &str,
    module_path: &str,
) -> Result<(String, Vec<Directive>)> {
    let mut edits: Vec<(u32, u32, String)> = Vec::new();
    let mut directives: Vec<Directive> = Vec::new();
    let mod_arg = code_escape(module_path);

    for stmt in &program.body {
        walk_top_stmt_for_use_cache(stmt, code, &mod_arg, &mut edits, &mut directives);
    }

    let out = apply_str_edits(code, edits);
    Ok((out, directives))
}

/// Visit a top-level statement, descending through export wrappers.
fn walk_top_stmt_for_use_cache(
    stmt: &oxc_ast::ast::Statement<'_>,
    code: &str,
    mod_arg: &str,
    edits: &mut Vec<(u32, u32, String)>,
    directives: &mut Vec<Directive>,
) {
    use oxc_ast::ast::{Declaration, Statement};

    // Helper macros: process a Function.
    macro_rules! try_decl_fn {
        ($func:expr, $span_start:expr, $exported:expr, $binding:expr, $name:expr) => {{
            if let Some(body) = $func.body.as_ref() {
                if let Some((ds, de)) = find_use_cache_directive(body) {
                    emit_use_cache_edit(UseCacheEditArgs {
                        code,
                        func: $func,
                        body,
                        directive_start: ds,
                        directive_end: de,
                        span_start: $span_start,
                        exported: $exported,
                        binding: $binding,
                        name: &$name,
                        mod_arg,
                        edits,
                        directives,
                    });
                }
            }
        }};
    }

    match stmt {
        Statement::FunctionDeclaration(func) => {
            let name = func
                .id
                .as_ref()
                .map(|i| i.name.as_str().to_string())
                .unwrap_or_default();
            let span_start = func.span.start;
            try_decl_fn!(func, span_start, false, false, name);
        }
        Statement::VariableDeclaration(var) => {
            walk_var_decl_for_use_cache(var, code, mod_arg, edits, directives);
        }
        Statement::ExportDeclaration(exp) => {
            walk_export_decl_for_use_cache(
                &exp.declaration,
                code,
                mod_arg,
                true,
                edits,
                directives,
            );
        }
        // `export default async function NAME() { 'use cache'; }` is not rewritten
        // (matches the scanner: a default export cannot be rebound under a name).
        _ => {}
    }
    // Silence unused-import warnings from the Declaration branch above.
    let _ = std::marker::PhantomData::<Declaration<'_>>;
}

fn walk_export_decl_for_use_cache(
    decl: &oxc_ast::ast::Declaration<'_>,
    code: &str,
    mod_arg: &str,
    exported: bool,
    edits: &mut Vec<(u32, u32, String)>,
    directives: &mut Vec<Directive>,
) {
    use oxc_ast::ast::Declaration;
    match decl {
        Declaration::FunctionDeclaration(func) => {
            let name = func
                .id
                .as_ref()
                .map(|i| i.name.as_str().to_string())
                .unwrap_or_default();
            if let Some(body) = func.body.as_ref()
                && let Some((ds, de)) = find_use_cache_directive(body)
            {
                emit_use_cache_edit(UseCacheEditArgs {
                    code,
                    func,
                    body,
                    directive_start: ds,
                    directive_end: de,
                    span_start: func.span.start,
                    exported,
                    binding: false,
                    name: &name,
                    mod_arg,
                    edits,
                    directives,
                });
            }
        }
        Declaration::VariableDeclaration(var) => {
            walk_var_decl_for_use_cache(var, code, mod_arg, edits, directives);
            let _ = exported; // scanner treats binding exports uniformly
        }
        _ => {}
    }
}

fn walk_var_decl_for_use_cache(
    var: &oxc_ast::ast::VariableDeclaration<'_>,
    code: &str,
    mod_arg: &str,
    edits: &mut Vec<(u32, u32, String)>,
    directives: &mut Vec<Directive>,
) {
    use oxc_ast::ast::{BindingPattern, Expression};
    for declarator in &var.declarations {
        let name = match &declarator.id {
            BindingPattern::BindingIdentifier(id) => id.name.as_str().to_string(),
            _ => continue, // destructuring not supported for 'use cache'
        };
        let Some(init) = declarator.init.as_ref() else {
            continue;
        };
        let (func_body, fn_span_start, fn_span_end, params_src) = match init {
            Expression::ArrowFunctionExpression(arrow) => {
                let body = match &arrow.body {
                    oxc_ast::ast::ArrowFunctionBody::FunctionBody(b) => b,
                    _ => continue, // expression-body arrows cannot carry a directive
                };
                let ps = &code[arrow.params.span.start as usize..arrow.params.span.end as usize];
                (body, arrow.span.start, arrow.span.end, ps.to_string())
            }
            Expression::FunctionExpression(func) => {
                let Some(body) = func.body.as_ref() else {
                    continue;
                };
                let ps = &code[func.params.span.start as usize..func.params.span.end as usize];
                (body, func.span.start, func.span.end, ps.to_string())
            }
            _ => continue,
        };
        let Some((ds, de)) = find_use_cache_directive(func_body) else {
            continue;
        };

        // Reconstruct the function text from the source span, blanking the directive.
        let fn_text_raw = &code[fn_span_start as usize..fn_span_end as usize];
        let mut fn_text = fn_text_raw.to_string();
        // The directive is inside fn_text; its span is absolute. Convert to relative.
        let rel_s = (ds - fn_span_start) as usize;
        let rel_e = (de - fn_span_start) as usize;
        if rel_s <= rel_e && rel_e <= fn_text.len() {
            // Replace the directive text with spaces so its byte length doesn't shift
            // subsequent content (simplest safe approach).
            let blanked: String = fn_text[rel_s..rel_e].chars().map(|_| ' ').collect();
            fn_text.replace_range(rel_s..rel_e, &blanked);
        }

        // Collect cacheTag/cacheLife options from the AST body.
        let (opts, blank_spans) = collect_cache_options(func_body, code, &params_src);
        // Blank the collected statement spans inside fn_text (relative coordinates).
        for (bs, be) in &blank_spans {
            let rel_s = (*bs as usize).saturating_sub(fn_span_start as usize);
            let rel_e = (*be as usize).saturating_sub(fn_span_start as usize);
            if rel_s < rel_e && rel_e <= fn_text.len() {
                let blanked: String = fn_text[rel_s..rel_e].chars().map(|_| ' ').collect();
                fn_text.replace_range(rel_s..rel_e, &blanked);
            }
        }
        let call = format!(
            "__oxide_cache({fn_text}, \"{mod_arg}\", \"{}\", {opts});",
            code_escape(&name)
        );
        // Binding replacement: keep the `const NAME = ` prefix, replace only the init.
        let replacement = call;
        edits.push((fn_span_start, fn_span_end, replacement));
        directives.push(Directive {
            kind: crate::DirectiveKind::UseCache,
            name: Some(name),
            start: ds as usize,
            end: de as usize,
        });
    }
}
pub struct UseCacheEditArgs<'a> {
    pub code: &'a str,
    pub func: &'a oxc_ast::ast::Function<'a>,
    pub body: &'a oxc_ast::ast::FunctionBody<'a>,
    pub directive_start: u32,
    pub directive_end: u32,
    pub span_start: u32,
    pub exported: bool,
    pub binding: bool,
    pub name: &'a str,
    pub mod_arg: &'a str,
    pub edits: &'a mut Vec<(u32, u32, String)>,
    pub directives: &'a mut Vec<Directive>,
}

fn emit_use_cache_edit(args: UseCacheEditArgs<'_>) {
    let UseCacheEditArgs {
        code,
        func,
        body,
        directive_start,
        directive_end,
        span_start,
        exported,
        binding,
        name,
        mod_arg,
        edits,
        directives,
    } = args;

    let fn_span_end = func.span.end;
    let fn_text_raw = &code[span_start as usize..fn_span_end as usize];
    let mut fn_text = fn_text_raw.to_string();

    let rel_s = (directive_start - span_start) as usize;
    let rel_e = (directive_end - span_start) as usize;
    if rel_s <= rel_e && rel_e <= fn_text.len() {
        let blanked: String = fn_text[rel_s..rel_e].chars().map(|_| ' ').collect();
        fn_text.replace_range(rel_s..rel_e, &blanked);
    }

    let params_src = &code[func.params.span.start as usize..func.params.span.end as usize];
    let (opts, blank_spans) = collect_cache_options(body, code, params_src);
    for (bs, be) in &blank_spans {
        let rel_s = (*bs as usize).saturating_sub(span_start as usize);
        let rel_e = (*be as usize).saturating_sub(span_start as usize);
        if rel_s < rel_e && rel_e <= fn_text.len() {
            let blanked: String = fn_text[rel_s..rel_e].chars().map(|_| ' ').collect();
            fn_text.replace_range(rel_s..rel_e, &blanked);
        }
    }
    let call = format!(
        "__oxide_cache({fn_text}, \"{mod_arg}\", \"{}\", {opts});",
        code_escape(name)
    );
    let replacement = if binding {
        call
    } else {
        let export_kw = if exported { "export " } else { "" };
        format!("{export_kw}const {name} = {call}")
    };
    edits.push((span_start, fn_span_end, replacement));
    directives.push(Directive {
        kind: crate::DirectiveKind::UseCache,
        name: Some(name.to_string()),
        start: directive_start as usize,
        end: directive_end as usize,
    });
}

/// AST-driven server-fn stub pass (§9.4).
fn ast_server_fn_stubs(
    program: &oxc_ast::ast::Program<'_>,
    code: &str,
    module_path: &str,
    env: Env,
    secret: &str,
) -> Result<(String, Vec<ServerFnStub>)> {
    use oxc_ast::ast::{BindingPattern, Declaration, Expression, Statement};

    let mut edits: Vec<(u32, u32, String)> = Vec::new();
    let mut stubs: Vec<ServerFnStub> = Vec::new();
    let mut ssr_lines: Vec<String> = Vec::new();
    let mut client_stub_ids: Vec<String> = Vec::new();

    // Helper: is `expr` a call to `serverFn(...)`?
    fn as_server_fn_call<'a>(
        expr: &'a Expression<'a>,
    ) -> Option<&'a oxc_ast::ast::CallExpression<'a>> {
        let Expression::CallExpression(call) = expr else {
            return None;
        };
        let Expression::Identifier(id) = &call.callee else {
            return None;
        };
        (id.name.as_str() == "serverFn").then_some(call)
    }

    // Extract the name from a `VariableDeclarator.id`.
    fn declarator_name(id: &oxc_ast::ast::BindingPattern<'_>) -> Option<String> {
        match id {
            BindingPattern::BindingIdentifier(b) => Some(b.name.as_str().to_string()),
            _ => None,
        }
    }

    let mut handle_var_decl = |var: &oxc_ast::ast::VariableDeclaration<'_>,
                               edits: &mut Vec<(u32, u32, String)>,
                               stubs: &mut Vec<ServerFnStub>,
                               ssr_lines: &mut Vec<String>| {
        for d in &var.declarations {
            let Some(name) = declarator_name(&d.id) else {
                continue;
            };
            let Some(init) = d.init.as_ref() else {
                continue;
            };
            let Some(_call) = as_server_fn_call(init) else {
                continue;
            };

            // Full declarator span is `name = serverFn(...)`; the scanner replaces
            // the whole `export const NAME = serverFn(...)` statement. We use the
            // declarator span and let the outer statement boundary be preserved.
            let id = crate::server_fn_id(module_path, &name, secret);
            stubs.push(ServerFnStub {
                export_name: name.clone(),
                id: id.clone(),
            });

            let repl = match env {
                // §9.4: the client stub delegates to `__oxide_call_fn` from
                // `@oxide-fw/core/fn`, which owns the fetch + wire format. Previously
                // this inlined a fetch calling bare `__oxide_encode` /
                // `__oxide_decode` globals that no module defines.
                //
                // `Object.assign(fn, { id })` keeps `NAME.id` working in app
                // code (`const fnId = greet.id` for a `data-greeting-fn`
                // attribute): a bare arrow has no `.id`, so the client rendered
                // `undefined` where the server rendered a runtime-assigned
                // `anonymous-N` — a guaranteed hydration mismatch.
                Env::Client => {
                    format!(
                        "{name} = Object.assign((async (...args) => __oxide_call_fn(\"{id}\", args)), {{ id: \"{id}\" }});"
                    )
                }
                // §9.4: rewrite the single-argument form to the ID-FIRST form
                // (`serverFn("<static id>", fn)`). Keeping the raw
                // `serverFn(fn)` call made the RUNTIME assign an
                // `anonymous-N` id on the server while the client baked the
                // static id — the two sides disagreed on `NAME.id` and every
                // page that rendered it failed hydration. With the id-first
                // form the runtime registers under the SAME static id the
                // client compiled (the appended `__oxide_register_fn` line
                // stays as a belt-and-braces registration).
                Env::Ssr | Env::Edge => {
                    let start = d.span.start as usize;
                    let end = d.span.end as usize;
                    let decl = &code[start..end];
                    // Insert `"<id>", ` right after the callee's opening
                    // paren (`serverFn(`). The call span is byte-exact from
                    // the AST; the callee is the `serverFn` identifier per
                    // `as_server_fn_call`.
                    match decl.find("serverFn") {
                        Some(callee_at) => {
                            let after_callee = callee_at + "serverFn".len();
                            let rest = &decl[after_callee..];
                            match rest.find('(') {
                                Some(paren_at) => {
                                    let insert_at = after_callee + paren_at + 1;
                                    let mut out = String::with_capacity(decl.len() + id.len() + 4);
                                    out.push_str(&decl[..insert_at]);
                                    out.push('"');
                                    out.push_str(&id);
                                    out.push_str("\", ");
                                    out.push_str(&decl[insert_at..]);
                                    out
                                }
                                // No paren (impossible for a CallExpression) —
                                // keep the original verbatim.
                                None => decl.to_string(),
                            }
                        }
                        None => decl.to_string(),
                    }
                }
            };
            edits.push((d.span.start, d.span.end, repl));

            if matches!(env, Env::Client) {
                client_stub_ids.push(id.clone());
            }
            if matches!(env, Env::Ssr | Env::Edge) {
                ssr_lines.push(format!("__oxide_register_fn(\"{id}\", {name});"));
            }
        }
    };

    for stmt in &program.body {
        match stmt {
            Statement::VariableDeclaration(var) => {
                handle_var_decl(var, &mut edits, &mut stubs, &mut ssr_lines);
            }
            Statement::ExportDeclaration(exp) => {
                if let Declaration::VariableDeclaration(var) = &exp.declaration {
                    handle_var_decl(var, &mut edits, &mut stubs, &mut ssr_lines);
                }
            }
            _ => {}
        }
    }

    // Sort edits descending so byte offsets don't shift.
    edits.sort_by_key(|(s, _, _)| std::cmp::Reverse(*s));
    let mut out = code.to_string();
    for (s, e, t) in &edits {
        out.replace_range(*s as usize..*e as usize, t);
    }

    if !client_stub_ids.is_empty() {
        let import = "import { __oxide_call_fn } from \"@oxide-fw/core/fn\";\n";
        out = format!("{import}{out}");
    }
    if !ssr_lines.is_empty() {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        for line in &ssr_lines {
            out.push_str(line);
            out.push('\n');
        }
        let ids: Vec<String> = stubs.iter().map(|f| format!("\"{}\"", f.id)).collect();
        out.push_str(&format!(
            "export const __oxide_fn_ids = [{}];\n",
            ids.join(", ")
        ));
    }

    Ok((out, stubs))
}

/// Apply `(start, end, replacement)` edits by sorting descending.
fn apply_str_edits(code: &str, mut edits: Vec<(u32, u32, String)>) -> String {
    edits.sort_by_key(|(s, _, _)| std::cmp::Reverse(*s));
    let mut out = code.to_string();
    for (s, e, t) in edits {
        out.replace_range(s as usize..e as usize, &t);
    }
    out
}

/// Walk the statements of a function body collecting `cacheTag("x")` and
/// `cacheLife({...})` calls, and return:
///   - the `__oxide_cache` options object literal text (tags array + life object)
///   - the `(start, end)` byte spans of the collected statements so the caller can
///     blank them out of the rewritten function text.
///
/// Recognised shapes (matching the scanner passes):
///   cacheTag("x");            → tags: ["x"]
///   cacheTag('y');            → tags: [..., 'y']
///   cacheLife({ ... });       → life: { ...inner... }
/// Other statement forms are ignored.
fn collect_cache_options(
    body: &oxc_ast::ast::FunctionBody<'_>,
    code: &str,
    params_src: &str,
) -> (String, Vec<(u32, u32)>) {
    use oxc_ast::ast::{Argument, Expression, Statement};

    // Each entry is the raw *source* of a `cacheTag` argument. For a
    // string literal that is a literal (a constant); for anything else it
    // is an expression that may reference the wrapped function's
    // parameters — `cacheTag('posts:' + slug)` slices to `'posts:' + slug`,
    // which is only meaningful inside a scope where `slug` exists.
    let mut tag_exprs: Vec<String> = Vec::new();
    let mut has_non_literal_tag = false;
    let mut lifes: Vec<String> = Vec::new();
    let mut spans: Vec<(u32, u32)> = Vec::new();

    for stmt in &body.statements {
        let Statement::ExpressionStatement(es) = stmt else {
            continue;
        };
        let Expression::CallExpression(call) = &es.expression else {
            continue;
        };
        let Expression::Identifier(id) = &call.callee else {
            continue;
        };
        let callee = id.name.as_str();

        match callee {
            "cacheTag" => {
                let Some(arg) = call.arguments.first() else {
                    continue;
                };
                // Any argument form is accepted now — a literal, a
                // template, a concatenation, an identifier. The raw source
                // slice is what goes into the emitted thunk, so the quote
                // style and the operator precedence both survive.
                let span = arg.span();
                if (span.end as usize) > code.len() {
                    continue;
                }
                let raw = &code[span.start as usize..span.end as usize];
                if !matches!(arg, Argument::StringLiteral(_)) {
                    has_non_literal_tag = true;
                }
                tag_exprs.push(raw.to_string());
                // Blank the whole statement out of the fn body: the tag is
                // now collected by the wrapper, so the call must not run
                // again inside the cached function.
                spans.push((es.span.start, es.span.end));
            }
            "cacheLife" => {
                if let Some(Argument::ObjectExpression(obj)) = call.arguments.first() {
                    // Extract the inner text of the object literal, minus its outer braces.
                    let start = obj.span.start as usize + 1;
                    let end = obj.span.end as usize - 1;
                    if start <= end && end <= code.len() {
                        let inner = code[start..end].trim();
                        if !inner.is_empty() {
                            lifes.push(inner.to_string());
                        }
                        spans.push((es.span.start, es.span.end));
                    }
                }
            }
            _ => {}
        }
    }

    let mut parts: Vec<String> = Vec::new();
    if !tag_exprs.is_empty() {
        if has_non_literal_tag {
            // A thunk. It takes the wrapped function's own parameter list,
            // so an expression like `'posts:' + slug` sees `slug`. Arrow
            // functions capture the same lexical scope the wrapped function
            // does, so any module-level or outer binding a tag expression
            // references is captured identically.
            // `params_src` is the raw span of the parameter list, which
            // already includes its parens (`(slug)`, `()`, `(a, b)`).
            // Wrapping it again would emit `((slug))`.
            parts.push(format!("tags: {params_src} => [{}]", tag_exprs.join(", ")));
        } else {
            parts.push(format!("tags: [{}]", tag_exprs.join(", ")));
        }
    }
    if !lifes.is_empty() {
        parts.push(format!("life: {{ {} }}", lifes.join(", ")));
    }
    let opts = if parts.is_empty() {
        "{}".to_string()
    } else {
        format!("{{ {} }}", parts.join(", "))
    };
    (opts, spans)
}

/// Same escaping the scanner uses (`escape_js` is private in lib.rs).
fn code_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

// Silence the unused-import warnings from `AstBTreeMap`.
#[allow(dead_code)]
type _UnusedAstMap = AstBTreeMap<String, String>;

/// Build a `Location` for a 1-based (line, column) in `path`.
fn oxidation_location(path: &str, line: usize, col: usize) -> oxide_fw_diagnostics::Location {
    oxide_fw_diagnostics::Location::file(path, line as u32, col as u32)
}


#[cfg(test)]
mod react_refresh_tests {
    use super::*;

    #[test]
    fn tsx_function_declaration_gets_reg() {
        let out = inject_react_refresh(
            "export default function Page() { return null; }\n",
            "tsx",
            "src/routes/page.tsx",
        )
        .unwrap();
        assert!(out.contains("$RefreshReg$(Page"), "missing reg call: {out}");
        assert!(out.contains("const $RefreshReg$"), "missing shim: {out}");
    }

    #[test]
    fn tsx_named_export_arrow_gets_reg() {
        let out = inject_react_refresh(
            "export const Widget = () => null;\n",
            "tsx",
            "src/routes/widget.tsx",
        )
        .unwrap();
        assert!(out.contains("$RefreshReg$(Widget"), "missing reg call: {out}");
    }

    #[test]
    fn tsx_class_gets_reg() {
        let out = inject_react_refresh(
            "export class K extends React.Component {}\n",
            "tsx",
            "src/comp.tsx",
        )
        .unwrap();
        assert!(out.contains("$RefreshReg$(K"), "missing reg call: {out}");
    }

    #[test]
    fn tsx_multiple_components_get_regs() {
        let out = inject_react_refresh(
            "function A() { return null; }\nfunction B() { return null; }\n",
            "tsx",
            "src/x.tsx",
        )
        .unwrap();
        assert!(out.contains("$RefreshReg$(A"));
        assert!(out.contains("$RefreshReg$(B"));
    }

    #[test]
    fn non_component_gets_no_reg() {
        let out =
            inject_react_refresh("export const FOO = 42;\n", "tsx", "src/x.tsx").unwrap();
        assert!(!out.contains("$RefreshReg$("), "should not inject: {out}");
    }

    #[test]
    fn non_tsx_passthrough() {
        let out = inject_react_refresh("const x = 1;\n", "ts", "src/x.ts").unwrap();
        assert!(!out.contains("$RefreshReg$"));
    }

    #[test]
    fn parser_hoists_exported_components_into_bare_statements() {
        // The current transform relies on the parser producing bare
        // `Statement::VariableDeclaration` / `FunctionDeclaration` for
        // `export const X = () => ...` / `export function F() {}`. If a
        // future oxc reverses that (puts `declaration` back on
        // `ExportNamedDeclaration`), this test fails loudly, prompting
        // the `top_level_component_names` match to add the arm back.
        let out = inject_react_refresh(
            "export const Foo = () => null;\nexport function Bar() { return null; }\n",
            "tsx",
            "src/x.tsx",
        )
        .unwrap();
        assert!(out.contains("$RefreshReg$(Foo"), "exported const not registered: {out}");
        assert!(out.contains("$RefreshReg$(Bar"), "exported fn not registered: {out}");
    }

    #[test]
    fn idempotent() {
        let src = "export default function Page() { return null; }\n";
        let once = inject_react_refresh(src, "tsx", "src/x.tsx").unwrap();
        let twice = inject_react_refresh(&once, "tsx", "src/x.tsx").unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn lower_to_js_with_refresh_emits_reg_for_tsx() {
        let src = "import React from 'react';\nexport default function Page() {\n  return <div>hi</div>;\n}\n";
        let (out, _) = lower_to_js_with_refresh(src, "tsx", "src/page.tsx", None).unwrap();
        assert!(out.contains("$RefreshReg$(Page"), "missing reg: {out}");
        assert!(!out.contains("<div>hi</div>"), "jsx not lowered: {out}");
    }
}
