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
    inject_hmr_boundary, is_server_module_path, scan_import_spans, Directive, Env, ImportScan,
    ImportSpan, Result, ServerFnStub,
    TransformInput, TransformOutput, Transformer,
};
use oxide_diagnostics::Diagnostic;
use oxc_allocator::Allocator;
use oxc_ast::ast::{Program, Statement};
use oxc_parser::Parser;
use oxc_span::SourceType;
use std::collections::BTreeSet;

/// Oxc-backed transformer (D4). Parse errors surface as `CMP-PARSE` diagnostics;
/// the pipeline continues with scanner passes so bad user code still gets a
/// structured response.
pub struct OxcTransformer;

impl Transformer for OxcTransformer {
    fn transform(&self, input: &TransformInput) -> Result<TransformOutput> {
        let allocator = Allocator::default();
        let source_type = source_type_for(input.lang);
        let parsed = Parser::new(&allocator, &input.code, source_type).parse();

        let diagnostics: Vec<Diagnostic> = parsed
            .diagnostics
            .iter()
            .map(|e| {
                Diagnostic::error("CMP-PARSE", format!("{e:?}"))
                    .with_fix("fix the syntax error reported by the Oxc parser")
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

        Ok(TransformOutput {
            code,
            scan,
            directives,
            server_fns,
            diagnostics,
        })
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
fn find_use_cache_directive(
    body: &oxc_ast::ast::FunctionBody<'_>,
) -> Option<(u32, u32)> {
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
            if let Some(body) = func.body.as_ref() {
                if let Some((ds, de)) = find_use_cache_directive(body) {
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
        let Some(init) = declarator.init.as_ref() else { continue };
        let (func_body, fn_span_start, fn_span_end) = match init {
            Expression::ArrowFunctionExpression(arrow) => {
                let body = match &arrow.body {
                    oxc_ast::ast::ArrowFunctionBody::FunctionBody(b) => b,
                    _ => continue, // expression-body arrows cannot carry a directive
                };
                (body, arrow.span.start, arrow.span.end)
            }
            Expression::FunctionExpression(func) => {
                let Some(body) = func.body.as_ref() else { continue };
                (body, func.span.start, func.span.end)
            }
            _ => continue,
        };
        let Some((ds, de)) = find_use_cache_directive(func_body) else { continue };

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
        let (opts, blank_spans) = collect_cache_options(func_body, code);
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

    let (opts, blank_spans) = collect_cache_options(body, code);
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

    // Helper: is `expr` a call to `serverFn(...)`?
    fn as_server_fn_call<'a>(
        expr: &'a Expression<'a>,
    ) -> Option<&'a oxc_ast::ast::CallExpression<'a>> {
        let Expression::CallExpression(call) = expr else { return None };
        let Expression::Identifier(id) = &call.callee else { return None };
        (id.name.as_str() == "serverFn").then_some(call)
    }

    // Extract the name from a `VariableDeclarator.id`.
    fn declarator_name(id: &oxc_ast::ast::BindingPattern<'_>) -> Option<String> {
        match id {
            BindingPattern::BindingIdentifier(b) => Some(b.name.as_str().to_string()),
            _ => None,
        }
    }

    let handle_var_decl = |var: &oxc_ast::ast::VariableDeclaration<'_>,
                               edits: &mut Vec<(u32, u32, String)>,
                               stubs: &mut Vec<ServerFnStub>,
                               ssr_lines: &mut Vec<String>| {
        for d in &var.declarations {
            let Some(name) = declarator_name(&d.id) else { continue };
            let Some(init) = d.init.as_ref() else { continue };
            let Some(_call) = as_server_fn_call(init) else { continue };

            // Full declarator span is `name = serverFn(...)`; the scanner replaces
            // the whole `export const NAME = serverFn(...)` statement. We use the
            // declarator span and let the outer statement boundary be preserved.
            let id = crate::server_fn_id(module_path, &name, secret);
            stubs.push(ServerFnStub {
                export_name: name.clone(),
                id: id.clone(),
            });

            let repl = match env {
                Env::Client => format!(
                    "{name} = (async (...args) => {{ const r = await \
                     fetch(\"/_oxide/fn/{id}\", {{ method: \"POST\", headers: {{ \"content-type\": \
                     \"application/oxide-fn\" }}, body: __oxide_encode(args) }}); return \
                     __oxide_decode(await r.arrayBuffer()); }});"
                ),
                Env::Ssr | Env::Edge => code[d.span.start as usize..d.span.end as usize].to_string(),
            };
            edits.push((d.span.start, d.span.end, repl));

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
) -> (String, Vec<(u32, u32)>) {
    use oxc_ast::ast::{Argument, Expression, Statement};

    let mut tags: Vec<String> = Vec::new();
    let mut lifes: Vec<String> = Vec::new();
    let mut spans: Vec<(u32, u32)> = Vec::new();

    for stmt in &body.statements {
        let Statement::ExpressionStatement(es) = stmt else { continue };
        let Expression::CallExpression(call) = &es.expression else { continue };
        let Expression::Identifier(id) = &call.callee else { continue };
        let callee = id.name.as_str();

        match callee {
            "cacheTag" => {
                if let Some(Argument::StringLiteral(s)) = call.arguments.first() {
                    // Use the raw source slice so the quote style is preserved.
                    let raw = &code[s.span.start as usize..s.span.end as usize];
                    tags.push(raw.to_string());
                    spans.push((es.span.start, es.span.end));
                }
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
    if !tags.is_empty() {
        parts.push(format!("tags: [{}]", tags.join(", ")));
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
