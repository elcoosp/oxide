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
use oxc_ast::ast::{Program, Statement};
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
                    let frame = oxide_fw_diagnostics::render_code_frame(&input.code, line, Some(col));
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
                    specifier: decl.source.value.as_str().to_string(),
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
                    specifier: decl.source.value.as_str().to_string(),
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
    d.expression.value.as_str().to_string()
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
                    emit_use_cache_edit(
                        code,
                        $func,
                        body,
                        ds,
                        de,
                        $span_start,
                        $exported,
                        $binding,
                        &$name,
                        mod_arg,
                        edits,
                        directives,
                    );
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
                && let Some((ds, de)) = find_use_cache_directive(body) {
                    emit_use_cache_edit(
                        code,
                        func,
                        body,
                        ds,
                        de,
                        func.span.start,
                        exported,
                        false,
                        &name,
                        mod_arg,
                        edits,
                        directives,
                    );
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

#[allow(clippy::too_many_arguments)]
fn emit_use_cache_edit(
    code: &str,
    func: &oxc_ast::ast::Function<'_>,
    body: &oxc_ast::ast::FunctionBody<'_>,
    directive_start: u32,
    directive_end: u32,
    span_start: u32,
    exported: bool,
    binding: bool,
    name: &str,
    mod_arg: &str,
    edits: &mut Vec<(u32, u32, String)>,
    directives: &mut Vec<Directive>,
) {
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
                // `oxide/fn`, which owns the fetch + wire format. Previously
                // this inlined a fetch calling bare `__oxide_encode` /
                // `__oxide_decode` globals that no module defines.
                Env::Client => {
                    format!("{name} = (async (...args) => __oxide_call_fn(\"{id}\", args));")
                }
                Env::Ssr | Env::Edge => {
                    code[d.span.start as usize..d.span.end as usize].to_string()
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
        let import = "import { __oxide_call_fn } from \"oxide/fn\";\n";
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
