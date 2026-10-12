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
    inject_hmr_boundary, is_server_module_path, scan_import_spans, transform_server_fn_stubs,
    transform_use_cache, Directive, Env, ImportScan, ImportSpan, Result, ServerFnStub,
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

        let mut diagnostics: Vec<Diagnostic> = parsed
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

        // Remaining passes delegate to the tolerant scanner (documented gap).
        let mut code = input.code.clone();
        let mut directives: Vec<Directive> = Vec::new();
        let mut server_fns: Vec<ServerFnStub> = Vec::new();
        let server = is_server_module_path(&input.path);
        if server && input.env == Env::Ssr {
            let (c, d) = transform_use_cache(&code, &input.path)?;
            code = c;
            directives = d;
        }
        if server {
            let (c, f) =
                transform_server_fn_stubs(&code, &input.path, input.env, &input.build_secret)?;
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
