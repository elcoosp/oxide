#![doc = include_str!("../README.md")]

//! oxide-fw-compiler — Oxc passes (spec §6.2–6.4, §9.3–9.4, D4).
//!
//! This crate implements the compiler passes behind a `Transformer` trait so the
//! Oxc native backend (D4) can slot in; the default engine is a tolerant scanner
//! covering the exact passes the spec defines:
//! 1. `'use cache'` directive → cache wrapper (§9.3)
//! 2. server-function stubs (§9.4) — client import compiles to `POST /_oxide/fn/<id>`
//! 3. import scan for the graph + preload hints (§6.2 step 7)
//! 4. forbidden-import guard (§6.4) — server-only modules unreachable from client
//! 5. HMR/refresh boundary injection (dev only, §6.2 step 6)
//! 6. why-dynamic reason engine (§V2-F16) — [`infer_reasons`] reports every
//!    request-scoped read (and every uncached `fetch(`) as a [`DynamicReason`]
//!    with a source span; the unit behind `oxide why --route`, the `OX-DYN`
//!    diagnostics and the `.oxide/routes.reasons.json` side-car
//!
//! **Backends (D4):** [`OxcTransformer`] uses the real `oxc_parser` + `oxc_ast`
//! (spec D4) for syntax validation and top-level import/exports; [`ScannerTransformer`]
//! is the tolerant, allocator-free fallback and still services the remaining passes. A small hand-written tokenizer ([`lexer`]) plus token-pattern
//! passes implement every pass over tame, realistic JS/TS; full syntax fidelity
//! (arbitrary TS types, regex edge cases, exotic directive placement) is the Oxc
//! backend's job. Oxc slots in behind [`Transformer`] without changing any
//! caller, type, or pass contract documented here.

#![forbid(unsafe_code)]

use blake3::Hasher;
use oxide_fw_diagnostics::{Diagnostic, OxideError};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub mod cjs_exports;
pub mod framework;
pub mod generate_params;
pub mod lexer;
pub mod native;
mod oxc_backend;
pub mod precompile_engine;

pub use cjs_exports::extract_cjs_export_names;
pub use framework::{
    Framework, FrameworkCompiler, FrameworkInput, FrameworkOutput, ReactInline, SolidInline,
    ToolingDispatcher, compiler_for,
};
pub use generate_params::{GenerateParamsResult, extract_generate_params};
pub use native::{Backend, CompileOutput, CompileRequest, NativeCompiler, NativeRegistry};
pub use oxc_backend::{
    OxcTransformer, inject_react_refresh, lower_to_js, lower_to_js_with_refresh,
};

pub type Result<T> = std::result::Result<T, OxideError>;

pub fn err(code: &'static str, msg: impl Into<String>) -> OxideError {
    OxideError(Box::new(Diagnostic::error(code, msg)))
}

/// Target environment of a transform (§6.1: client, ssr, edge).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Env {
    Client,
    Ssr,
    Edge,
}

/// Transform input.
#[derive(Debug, Clone)]
pub struct TransformInput {
    pub path: String,
    pub code: String,
    pub env: Env,
    /// ts | tsx | js | jsx | mjs
    pub lang: &'static str,
    /// Per-build secret for server-fn ids (§9.4 unguessable, rotating).
    pub build_secret: String,
    pub dev: bool,
    /// §6.3: the JSX runtime the framework adapter declares via
    /// `FrameworkAdapter.jsxImportSource`. `None` means the Oxc default
    /// (React's `react/jsx-runtime`). Solid 2.0 declares
    /// `@solidjs/web`; React declares `react`. Vue and Svelte entries
    /// arrive precompiled, so this is unread for them.
    pub jsx_import_source: Option<String>,
}

/// Transform output: new code + discovered metadata.
#[derive(Debug, Clone, Default)]
pub struct TransformOutput {
    pub code: String,
    /// V3 sourcemap from `code` back to the original source. `None` only when
    /// the lower stage is skipped or fails; a successful lower always
    /// produces one.
    pub map: Option<oxc_sourcemap::OwnedSourceMap>,
    pub scan: ImportScan,
    pub directives: Vec<Directive>,
    pub server_fns: Vec<ServerFnStub>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Import scan result (§6.2 step 7): specifiers + preload hints.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImportScan {
    pub imports: Vec<String>,
    /// Dynamic `import('...')` specifiers.
    pub dynamic_imports: Vec<String>,
    /// Asset queries seen, e.g. `./hero.png?w=640&format=avif` (§6.7). Specifiers
    /// containing `?` land here instead of `imports`.
    pub asset_queries: Vec<String>,
}

/// A `'use cache'`-marked function discovered by the directive pass (§9.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Directive {
    pub kind: DirectiveKind,
    pub name: Option<String>,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DirectiveKind {
    UseCache,
}

/// A `serverFn(...)` discovered in a `*.server.ts` module (§9.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerFnStub {
    pub export_name: String,
    /// Stable hash of file + export name + per-build secret.
    pub id: String,
}

/// Compute a server-fn id (§9.4): blake3(file, export, secret) → url-safe 24 chars.
#[must_use]
pub fn server_fn_id(file: &str, export: &str, secret: &str) -> String {
    let mut h = Hasher::new();
    h.update(file.as_bytes());
    h.update(&[0u8]);
    h.update(export.as_bytes());
    h.update(&[0u8]);
    h.update(secret.as_bytes());
    h.finalize().to_hex().to_string()[..24].to_string()
}

/// Span-annotated import statement (additive API used by the bundler for surgery).
#[derive(Debug, Clone, PartialEq)]
pub struct ImportSpan {
    pub start: usize,
    pub end: usize,
    pub specifier: String,
    pub dynamic: bool,
}

/// Scan import-like statements with byte spans: `import x from '...'`,
/// `import '...'`, `import {a, b as c} from '...'`, `import * as ns from '...'`,
/// `export ... from '...'`, dynamic `import('...')`. Strings, template literals
/// (with `${}` nesting) and comments are skipped by the tokenizer ([`lexer`]).
#[must_use]
pub fn scan_import_spans(code: &str) -> Vec<ImportSpan> {
    use lexer::Tok;
    let toks = lexer::lex(code);
    let count = toks.len();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < count {
        let t = &toks[i];
        if is_word(code, t, "import") {
            match toks.get(i + 1).map(|x| &x.kind) {
                // side-effect: `import '...'`
                Some(Tok::Str { value }) => {
                    let end_idx = stmt_end_idx(&toks, i + 1);
                    out.push(ImportSpan {
                        start: t.start,
                        end: toks[end_idx - 1].end,
                        specifier: value.clone(),
                        dynamic: false,
                    });
                    i = end_idx;
                    continue;
                }
                // dynamic: `import('...')`
                Some(Tok::Punct('(')) => {
                    if let Some(Tok::Str { value }) = toks.get(i + 2).map(|x| &x.kind) {
                        let end = if is_punct(toks.get(i + 3), ')') {
                            toks[i + 3].end
                        } else {
                            toks[i + 2].end
                        };
                        out.push(ImportSpan {
                            start: t.start,
                            end,
                            specifier: value.clone(),
                            dynamic: true,
                        });
                    }
                    i += 2; // `import.meta` and unquoted dynamic args fall through here too
                    continue;
                }
                _ => {
                    // clause import: `import [type] default?, {..}?, * as ns?, from '...'`
                    i = clause_import_end(code, &toks, i, t, &mut out);
                    continue;
                }
            }
        }
        if is_word(code, t, "export") {
            // re-export: `export {..} from '...'` / `export * from '...'` / `export * as ns from '...'`
            if let Some(next) = export_reexport_end(code, &toks, i, t, &mut out) {
                i = next;
                continue;
            }
            i += 1;
            continue;
        }
        i += 1;
    }
    out
}

/// Consume one clause import (`import [type] default?, {..}?, * as ns?,
/// from '...'`) starting at the `import` token `start`; pushes the
/// `ImportSpan` when a `from '<spec>'` clause follows and returns the
/// next scan position (past the statement, or +1 for the
/// `import x = require('y')` shape that is not an ESM import).
fn clause_import_end(
    code: &str,
    toks: &[lexer::Token],
    start: usize,
    t: &lexer::Token,
    out: &mut Vec<ImportSpan>,
) -> usize {
    use lexer::Tok;
    let mut j = start + 1;
    if matches!(toks.get(j).map(|x| &x.kind), Some(Tok::Ident)) {
        j += 1; // default binding (also absorbs TS `import type`)
    }
    if is_punct(toks.get(j), ',') {
        j += 1;
    }
    if is_punct(toks.get(j), '{') {
        j = skip_balanced(toks, j, '{', '}');
    }
    if is_punct(toks.get(j), '*') {
        j += 1;
        if matches!(toks.get(j).map(|x| &x.kind), Some(Tok::Ident)) && is_word(code, &toks[j], "as")
        {
            j += 1;
        }
        if matches!(toks.get(j).map(|x| &x.kind), Some(Tok::Ident)) {
            j += 1;
        }
    }
    if is_word_opt(code, toks.get(j), "from")
        && let Some(Tok::Str { value }) = toks.get(j + 1).map(|x| &x.kind)
    {
        let end_idx = stmt_end_idx(toks, j + 1);
        out.push(ImportSpan {
            start: t.start,
            end: toks[end_idx - 1].end,
            specifier: value.clone(),
            dynamic: false,
        });
        return end_idx;
    }
    start + 1 // e.g. `import x = require('y')` — not an ESM import
}

/// Consume one `export` statement starting at token `start` when it is a
/// re-export (`export {..} from '...'` / `export * from '...'` /
/// `export * as ns from '...'`); pushes the `ImportSpan` and returns the
/// next scan position. `None` when the statement is not a re-export (the
/// caller advances by one token).
fn export_reexport_end(
    code: &str,
    toks: &[lexer::Token],
    start: usize,
    t: &lexer::Token,
    out: &mut Vec<ImportSpan>,
) -> Option<usize> {
    use lexer::Tok;
    let reexport = matches!(
        toks.get(start + 1).map(|x| &x.kind),
        Some(Tok::Punct('{' | '*'))
    );
    if !reexport {
        return None;
    }
    let mut j = start + 1;
    if is_punct(toks.get(j), '{') {
        j = skip_balanced(toks, j, '{', '}');
    } else {
        j += 1; // '*'
        if is_word_opt(code, toks.get(j), "as") {
            j += 2;
        }
    }
    let mut k = j;
    while k < toks.len() {
        if is_punct(toks.get(k), ';') {
            break;
        }
        if is_word_opt(code, toks.get(k), "from") {
            if let Some(Tok::Str { value }) = toks.get(k + 1).map(|x| &x.kind) {
                let end_idx = stmt_end_idx(toks, k + 1);
                out.push(ImportSpan {
                    start: t.start,
                    end: toks[end_idx - 1].end,
                    specifier: value.clone(),
                    dynamic: false,
                });
                return Some(end_idx);
            }
            break;
        }
        k += 1;
    }
    // LOCAL re-export (`export { a, b };`) or `export *;` has no `from`
    // clause. Without an explicit advance the outer `while` would
    // re-enter this branch on the same `export` token forever. `j` is
    // guaranteed > start (past the `{...}` or `*`/`* as X`), so `i = j`
    // always advances.
    Some(j)
}

/// Import scan (§6.2 step 7): static imports, dynamic imports, asset queries.
/// Specifiers containing `?` are asset queries (§6.7) and excluded from `imports`.
#[must_use]
pub fn scan_imports(code: &str) -> ImportScan {
    let mut scan = ImportScan::default();
    let mut seen_static: BTreeSet<String> = BTreeSet::new();
    let mut seen_dynamic: BTreeSet<String> = BTreeSet::new();
    let mut seen_asset: BTreeSet<String> = BTreeSet::new();
    for sp in scan_import_spans(code) {
        if sp.specifier.contains('?') {
            if seen_asset.insert(sp.specifier.clone()) {
                scan.asset_queries.push(sp.specifier);
            }
        } else if sp.dynamic {
            if seen_dynamic.insert(sp.specifier.clone()) {
                scan.dynamic_imports.push(sp.specifier);
            }
        } else if seen_static.insert(sp.specifier.clone()) {
            scan.imports.push(sp.specifier);
        }
    }
    scan
}

/// True when `path` names a server-only module (§6.4): `<name>.server.<ext>`.
#[must_use]
pub fn is_server_module_path(path: &str) -> bool {
    ["ts", "tsx", "js", "jsx", "mjs"]
        .iter()
        .any(|ext| path.ends_with(&format!(".server.{ext}")))
}

/// The compiler trait (D4 isolation point; Oxc backend slots in here).
pub trait Transformer: Send + Sync {
    /// Run the scanner passes over one module input.
    ///
    /// # Errors
    ///
    /// When a pass cannot rewrite the module (`CMP-CACHE` for a `'use cache'`
    /// function without a binding name; the Oxc backend surfaces
    /// `CMP-PARSE` for a syntax error).
    fn transform(&self, input: &TransformInput) -> Result<TransformOutput>;
}

/// Default scanner-based transformer implementing the five passes above.
pub struct ScannerTransformer;

impl Transformer for ScannerTransformer {
    fn transform(&self, input: &TransformInput) -> Result<TransformOutput> {
        let scan = scan_imports(&input.code);
        let mut code = input.code.clone();
        let mut directives = Vec::new();
        let mut server_fns = Vec::new();
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
            // ScannerTransformer does not lower to JS, so it produces no map.
            map: None,
            scan,
            directives,
            server_fns,
            diagnostics: Vec::new(),
        })
    }
}

/// Directive pass (§9.3): in `*.server.ts`, async functions marked `'use cache'`
/// compile to a cache wrapper; the key is `hash(module, export, args)` with optional
/// `cacheTag()`/`cacheLife()` calls. Equivalent explicit form: `cache(fn, {ttl, tags, swr})`.
///
/// Supported shapes (tolerant scanner, tame code):
/// - `async function foo() { 'use cache'; ... }`
/// - `export async function foo() { 'use cache'; ... }`
/// - `export const bar = async () => { 'use cache'; ... }` (block-body arrows)
///
/// Each marked function is rewritten to
/// `__oxide_cache(<fn>, "<module>", "<export>", { tags: [...], life: {...} })`
/// with `cacheTag`/`cacheLife` statements hoisted into the options object and the
/// directive statement removed. Zero or many tag/life calls are supported.
///
/// # Errors
///
/// `CMP-CACHE` when a `'use cache'`-marked function has no binding name to
/// register the cache wrapper under.
pub fn transform_use_cache(code: &str, module_path: &str) -> Result<(String, Vec<Directive>)> {
    use lexer::Tok;
    let toks = lexer::lex(code);
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let mut directives: Vec<Directive> = Vec::new();
    let mut taken: Vec<(usize, usize)> = Vec::new();
    let mod_arg = escape_js(module_path);

    let mut i = 0usize;
    while i < toks.len() {
        if !is_word(code, &toks[i], "async") {
            i += 1;
            continue;
        }
        let Some(af) = parse_async_fn(code, &toks, i) else {
            i += 1;
            continue;
        };
        // The directive must be the body's first statement.
        let first = af.body_open + 1;
        let marked = matches!(
            toks.get(first).map(|t| &t.kind),
            Some(Tok::Str { value }) if value.trim() == "use cache"
        );
        if !marked {
            i += 1;
            continue;
        }
        let start = toks[af.fn_start].start;
        let end = toks[af.body_close].end;
        if taken.iter().any(|&(ts, te)| start < te && ts < end) {
            i = af.body_close + 1; // nested inside an already-rewritten function
            continue;
        }
        taken.push((start, end));

        // Collect spans to blank: the directive statement + cacheTag/cacheLife calls.
        let (blanks, tags, life) = collect_cache_markers(code, &toks, first, af.body_close);

        // Rebuild the function with cleaned body text.
        let body_from = toks[af.body_open].end;
        let body_to = toks[af.body_close].start;
        let mut inner = code[body_from..body_to].to_string();
        for &(bs, be) in blanks.iter().rev() {
            inner.replace_range(bs - body_from..be - body_from, "");
        }
        let fn_text = format!(
            "{}{}}}",
            &code[toks[af.fn_start].start..toks[af.body_open].end],
            inner
        );

        let name = match &af.name {
            Some(n) => n.clone(),
            None => {
                return Err(err("CMP-CACHE", "'use cache' function has no binding name"));
            }
        };
        let mut opts_parts: Vec<String> = Vec::new();
        if !tags.is_empty() {
            opts_parts.push(format!("tags: [{}]", tags.join(", ")));
        }
        if !life.is_empty() {
            opts_parts.push(format!("life: {{ {} }}", life.join(", ")));
        }
        let opts = if opts_parts.is_empty() {
            "{}".to_string()
        } else {
            format!("{{ {} }}", opts_parts.join(", "))
        };
        let call = format!(
            "__oxide_cache({fn_text}, \"{mod_arg}\", \"{}\", {opts});",
            escape_js(&name)
        );

        let (span_start, replacement) = if af.binding {
            (start, call)
        } else {
            let export_kw = if af.exported { "export " } else { "" };
            (
                toks[af.span_start].start,
                format!("{export_kw}const {name} = {call}"),
            )
        };
        edits.push((span_start, end, replacement));
        directives.push(Directive {
            kind: DirectiveKind::UseCache,
            name: Some(name),
            start,
            end,
        });
        i = af.body_close + 1;
    }

    let mut out = code.to_string();
    edits.sort_by_key(|&(s, _, _)| std::cmp::Reverse(s));
    for (s, e, text) in edits {
        out.replace_range(s..e, &text);
    }
    Ok((out, directives))
}

/// Collect the spans to blank out for one `'use cache'` function: the
/// directive statement itself plus every `cacheTag("...")` / `cacheLife({...})`
/// call inside the body (§9.3). Returns the blank spans (directive first),
/// the raw quoted tag strings and the merged `cacheLife` object bodies.
fn collect_cache_markers(
    code: &str,
    toks: &[lexer::Token],
    first: usize,
    body_close: usize,
) -> (Vec<(usize, usize)>, Vec<String>, Vec<String>) {
    use lexer::Tok;
    let mut blanks: Vec<(usize, usize)> = Vec::new();
    let mut tags: Vec<String> = Vec::new();
    let mut life: Vec<String> = Vec::new();
    let d_end = stmt_end_idx(toks, first);
    blanks.push((toks[first].start, toks[d_end - 1].end));
    let mut j = first + 1;
    while j < body_close {
        if is_word(code, &toks[j], "cacheTag")
            && is_punct(toks.get(j + 1), '(')
            && matches!(toks.get(j + 2).map(|t| &t.kind), Some(Tok::Str { .. }))
            && is_punct(toks.get(j + 3), ')')
        {
            tags.push(code[toks[j + 2].start..toks[j + 2].end].to_string());
            let e = stmt_end_idx(toks, j + 3);
            blanks.push((toks[j].start, toks[e - 1].end));
            j = e;
            continue;
        }
        if is_word(code, &toks[j], "cacheLife") && is_punct(toks.get(j + 1), '(') {
            let close = skip_balanced(toks, j + 1, '(', ')');
            // strip the outer braces of the object literal → mergeable key/value text
            let inner = code[toks[j + 2].start..toks[close - 1].start]
                .trim()
                .trim_start_matches('{')
                .trim_end_matches('}')
                .trim()
                .to_string();
            if !inner.is_empty() {
                life.push(inner);
            }
            let e = stmt_end_idx(toks, close - 1);
            blanks.push((toks[j].start, toks[e - 1].end));
            j = e;
            continue;
        }
        j += 1;
    }
    (blanks, tags, life)
}

/// Server-fn stub pass (§9.4): on the **client** environment, importing `addToCart`
/// from `cart.server.ts` compiles to a stub calling `POST /_oxide/fn/<id>`.
/// On the **server** environment the body is kept and the id registry emitted.
///
/// Recognized: `export const <name> = serverFn(async (...) => {...})` and
/// `serverFn(async function (...) {...})`; an optional schema first argument
/// (`serverFn(schema, async ...)`) is supported (the schema arg is dropped in the
/// client stub, which only forwards `...args`).
///
/// # Errors
///
/// `CMP-CACHE` when a server-fn declaration carries no binding name.
pub fn transform_server_fn_stubs(
    code: &str,
    module_path: &str,
    env: Env,
    secret: &str,
) -> Result<(String, Vec<ServerFnStub>)> {
    use lexer::Tok;
    let toks = lexer::lex(code);
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let mut stubs: Vec<ServerFnStub> = Vec::new();
    let mut ssr_lines: Vec<String> = Vec::new();
    let mut client_stub_ids: Vec<String> = Vec::new();
    let mut i = 0usize;
    while i + 5 < toks.len() {
        let matches_shape = is_word(code, &toks[i], "export")
            && is_word(code, &toks[i + 1], "const")
            && toks[i + 2].kind == Tok::Ident
            && is_punct(toks.get(i + 3), '=')
            && is_word(code, &toks[i + 4], "serverFn")
            && is_punct(toks.get(i + 5), '(');
        if !matches_shape {
            i += 1;
            continue;
        }
        let name = code[toks[i + 2].start..toks[i + 2].end].to_string();
        let open = i + 5;
        // Optional schema first argument: verify the arg list eventually contains
        // `async` after a top-level comma (schema-first shape §9.4).
        if !is_word_opt(code, toks.get(open + 1), "async") {
            let mut depth = 1i32;
            let mut m = open + 1;
            let mut found = false;
            while m < toks.len() {
                match toks[m].kind {
                    Tok::Punct('(' | '{' | '[') => depth += 1,
                    Tok::Punct(')' | '}' | ']') => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    Tok::Punct(',') if depth == 1 => {
                        found = is_word_opt(code, toks.get(m + 1), "async");
                        break;
                    }
                    _ => {}
                }
                m += 1;
            }
            if !found {
                i += 1;
                continue;
            }
        }
        let close = skip_balanced(&toks, open, '(', ')') - 1;
        let mut end = toks[close].end;
        if is_punct(toks.get(close + 1), ';') {
            end = toks[close + 1].end;
        }
        let id = server_fn_id(module_path, &name, secret);
        stubs.push(ServerFnStub {
            export_name: name.clone(),
            id: id.clone(),
        });
        match env {
            Env::Client => {
                // The stub calls `__oxide_call_fn` from `@oxide-fw/core/fn` rather
                // than inlining the fetch. The previous form referenced
                // bare `__oxide_encode`/`__oxide_decode` globals that no
                // module defined; emitting an import makes the stub
                // self-contained and lets the runtime own the wire format
                // (JSON today, ODF once the browser decoder ships).
                let stub = format!(
                    "export const {name} = (async (...args) => \
                     __oxide_call_fn(\"{id}\", args));"
                );
                edits.push((toks[i].start, end, stub));
                // Collect the ids so the transform tail can prepend a single
                // import when at least one stub was emitted.
                client_stub_ids.push(id.clone());
            }
            Env::Ssr | Env::Edge => {
                ssr_lines.push(format!("__oxide_register_fn(\"{id}\", {name});"));
            }
        }
        i = close + 1;
    }

    let mut out = code.to_string();
    edits.sort_by_key(|&(s, _, _)| std::cmp::Reverse(s));
    for (s, e, t) in edits {
        out.replace_range(s..e, &t);
    }
    if !client_stub_ids.is_empty() {
        // Strip the `serverFn` (and only `serverFn`) import. After the
        // stub replacement above, every `serverFn(...)` call has become a
        // `__oxide_call_fn(...)` call — so the import is dead. Leaving it
        // in would make the client-side stub import
        // `@oxide-fw/core/server` (which transitively imports `node:fs`,
        // `node:crypto`, `node:path`) and the browser would reject the
        // module graph on CORS. This is the same tree-shaking the
        // production bundler does automatically; in dev (native ESM, no
        // bundler) we do it here.
        out = strip_server_fn_imports(&out);
        // One import regardless of how many stubs were emitted. Placed at
        // the top so it sits before any code the transform left in place.
        let import = "import { __oxide_call_fn } from \"@oxide-fw/core/fn\";\n";
        out = format!("{import}{out}");
    }
    if !ssr_lines.is_empty() {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        for line in &ssr_lines {
            out.push_str(line);
            out.push('\n');
        }
        let ids: Vec<String> = stubs.iter().map(|f| format!("\"{}\"", f.id)).collect();
        out.push_str("export const __oxide_fn_ids = [");
        out.push_str(&ids.join(", "));
        out.push_str("];\n");
    }
    Ok((out, stubs))
}

/// Forbidden-import guard (§6.4): a compile-time check that no `*.server.ts` module
/// is reachable from the client entry except via the server-fn stub transform.
/// Returns diagnostics naming the full import chain (§19.4).
/// Strip every `serverFn` binding from `out`'s import statements. Called
/// by [`transform_server_fn_stubs`] after the stub replacement pass: every
/// `serverFn(...)` call has become a `__oxide_call_fn(...)` call, so any
/// remaining reference to `serverFn` is dead code that (in native ESM dev
/// mode) causes the browser to try importing `node:fs`, `node:crypto`, etc.
///
/// Only the *name* `serverFn` is removed. If an import line also brings in
/// other names (`import { serverFn, defineLoad } from …`), the line is
/// rewritten to `import { defineLoad } from …` — the other helpers are
/// legitimate client-side code (a loader does `defineLoad(fn)` and needs it
/// to create the wrapper).
///
/// A line becomes `import {} from …` after stripping → drop the whole line
/// (an empty import has no side effects we can rely on — the module it
/// imported from is `@oxide-fw/core/server`, whose side effects are all
/// server-only anyway).
fn strip_server_fn_imports(out: &str) -> String {
    let spans = scan_import_spans(out);
    if spans.is_empty() {
        return out.to_string();
    }
    let mut result = out.to_string();
    for sp in spans.into_iter().rev() {
        if sp.dynamic {
            continue;
        }
        let stmt = &result[sp.start..sp.end];
        match rewrite_import_stripping_server_fn(stmt) {
            ImportEdit::Unchanged => {}
            ImportEdit::Delete => {
                let mut end = sp.end;
                let bytes = result.as_bytes();
                if bytes.get(end) == Some(&b';') {
                    end += 1;
                }
                if bytes.get(end) == Some(&b'\n') {
                    end += 1;
                } else if bytes.get(end) == Some(&b'\r') && bytes.get(end + 1) == Some(&b'\n') {
                    end += 2;
                }
                result.replace_range(sp.start..end, "");
            }
            ImportEdit::Replace(new_stmt) => {
                result.replace_range(sp.start..sp.end, &new_stmt);
            }
        }
    }
    result
}

enum ImportEdit {
    Unchanged,
    Delete,
    Replace(String),
}

fn rewrite_import_stripping_server_fn(stmt: &str) -> ImportEdit {
    let trimmed = stmt.trim_start();
    let Some(after_import) = trimmed.strip_prefix("import") else {
        return ImportEdit::Unchanged;
    };
    let Some(first) = after_import.chars().next() else {
        return ImportEdit::Unchanged;
    };
    if !(first.is_whitespace() || first == '{' || first == '*' || first == '"' || first == '\'') {
        return ImportEdit::Unchanged;
    }
    let Some(from_pos) = find_top_level_from(trimmed) else {
        return ImportEdit::Unchanged;
    };
    let tail = &trimmed[from_pos..];
    let mut clause = trimmed["import".len()..from_pos].trim().to_string();
    let is_type = clause.starts_with("type ") || clause == "type";
    if is_type {
        clause = clause["type".len()..].trim_start().to_string();
    }
    let (mut default_part, named_part, namespace_part) = if let Some(open) = clause.find('{') {
        let Some(close) = find_balanced_close(&clause, open) else {
            return ImportEdit::Unchanged;
        };
        (
            clause[..open]
                .trim()
                .trim_end_matches(',')
                .trim()
                .to_string(),
            clause[open + 1..close].to_string(),
            clause[close + 1..]
                .trim()
                .trim_start_matches(',')
                .trim()
                .to_string(),
        )
    } else {
        (
            clause.trim().trim_end_matches(',').trim().to_string(),
            String::new(),
            String::new(),
        )
    };
    let mut changed = false;
    if !default_part.is_empty() && is_server_fn_reference(&default_part) {
        default_part.clear();
        changed = true;
    }
    let mut kept: Vec<String> = Vec::new();
    for piece in split_top_level_commas(&named_part) {
        let t = piece.trim();
        if t.is_empty() {
            continue;
        }
        if is_server_fn_reference(t) {
            changed = true;
        } else {
            kept.push(t.to_string());
        }
    }
    if !changed {
        return ImportEdit::Unchanged;
    }
    let named_out = kept.join(", ");
    let mut segments: Vec<String> = Vec::new();
    if !default_part.is_empty() {
        segments.push(default_part);
    }
    if !named_out.is_empty() {
        segments.push(format!("{{ {named_out} }}"));
    }
    if !namespace_part.is_empty() {
        segments.push(namespace_part);
    }
    if segments.is_empty() {
        return ImportEdit::Delete;
    }
    let mut rebuilt = String::from("import ");
    if is_type {
        rebuilt.push_str("type ");
    }
    rebuilt.push_str(&segments.join(", "));
    rebuilt.push(' ');
    rebuilt.push_str(tail);
    ImportEdit::Replace(rebuilt)
}

fn find_balanced_close(s: &str, open: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (i, c) in s[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + i);
                }
            }
            _ => {}
        }
    }
    None
}

fn split_top_level_commas(inner: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (i, c) in inner.char_indices() {
        match c {
            '{' | '[' | '(' | '<' => depth += 1,
            '}' | ']' | ')' | '>' => depth -= 1,
            ',' if depth == 0 => {
                out.push(inner[start..i].to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    if start < inner.len() {
        out.push(inner[start..].to_string());
    }
    out
}

fn is_server_fn_reference(text: &str) -> bool {
    let s = text.trim();
    let type_stripped = s
        .strip_prefix("type")
        .filter(|rest| rest.starts_with(char::is_whitespace));
    let s = type_stripped.map_or(s, str::trim_start);
    let word = s.split_whitespace().next().unwrap_or("");
    word.trim_end_matches(',').trim_end_matches('}').trim() == "serverFn"
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

fn find_top_level_from(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut depth = 0i32;
    let mut in_str: Option<u8> = None;
    let mut i = 0usize;
    while i < bytes.len() {
        if let Some(q) = in_str {
            if bytes[i] == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            if bytes[i] == q {
                in_str = None;
            }
            i += 1;
            continue;
        }
        match bytes[i] {
            b'"' | b'\'' => {
                in_str = Some(bytes[i]);
                i += 1;
                continue;
            }
            b'{' | b'(' | b'[' => depth += 1,
            b'}' | b')' | b']' => depth -= 1,
            _ => {}
        }
        if depth == 0 && i + 4 <= bytes.len() && &bytes[i..i + 4] == b"from" {
            let before_ok = i == 0 || !is_ident_byte(bytes[i - 1]);
            let after_ok = i + 4 >= bytes.len() || !is_ident_byte(bytes[i + 4]);
            if before_ok && after_ok {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

#[must_use]
pub fn guard_server_only(
    client_graph: &BTreeMap<String, Vec<String>>,
    server_modules: &BTreeMap<String, bool>,
) -> Vec<Diagnostic> {
    let is_server = |m: &str| server_modules.get(m).copied().unwrap_or(false);
    let mut out = Vec::new();
    // BFS from every client entry (graph keys that are not server-only).
    for (entry, deps) in client_graph {
        if is_server(entry) {
            continue;
        }
        let mut parent: BTreeMap<String, Option<String>> = BTreeMap::new();
        parent.insert(entry.clone(), None);
        let mut queue: VecDeque<String> = VecDeque::new();
        for d in deps {
            if parent.insert(d.clone(), Some(entry.clone())).is_none() {
                queue.push_back(d.clone());
            }
        }
        while let Some(cur) = queue.pop_front() {
            if is_server(&cur) {
                let mut chain = vec![cur.clone()];
                let mut p = parent.get(&cur).cloned().flatten();
                while let Some(pp) = p {
                    chain.push(pp.clone());
                    p = parent.get(&pp).cloned().flatten();
                }
                chain.reverse();
                let msg = format!(
                    "forbidden server-only import reachable from the client graph: {}",
                    chain.join(" -> ")
                );
                out.push(Diagnostic::error("CMP-GUARD", msg).with_fix(
                    "move the import into a `*.server.ts` module or expose it via a server \
                         function (§9.4)",
                ));
            }
            if let Some(next) = client_graph.get(&cur) {
                for d in next {
                    if !parent.contains_key(d) {
                        parent.insert(d.clone(), Some(cur.clone()));
                        queue.push_back(d.clone());
                    }
                }
            }
        }
    }
    out
}

/// HMR boundary injection (§6.2 step 6, dev only): append `import.meta.hot` accept
/// code appropriate for the module kind (component / css / server module).
/// Idempotent: code that already references `import.meta.hot` is returned unchanged.
///
/// # Errors
///
/// Never in the current implementation — the signature keeps `Result` so
/// backend swaps (the D4 seam) can surface parse diagnostics unchanged.
pub fn inject_hmr_boundary(code: &str, path: &str, lang: &str) -> Result<String> {
    if code.contains("import.meta.hot") {
        return Ok(code.to_string());
    }
    let snippet = if path.contains(".server.") {
        format!(
            "if (import.meta.hot) {{ import.meta.hot.accept(() => {{ \
             __oxide_server_swap(\"{}\"); }}); }}",
            escape_js(path)
        )
    } else if lang == "tsx" || lang == "jsx" {
        "if (import.meta.hot) { import.meta.hot.accept((mod) => { __oxide_refresh(mod); }); }"
            .to_string()
    } else {
        // css and everything else self-accepts (css swap happens in the runtime)
        "if (import.meta.hot) { import.meta.hot.accept(); }".to_string()
    };
    let mut out = String::from(code);
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&snippet);
    out.push('\n');
    Ok(out)
}

// ------------------------------------------------------------------ helpers

fn is_word(code: &str, t: &lexer::Token, w: &str) -> bool {
    t.kind == lexer::Tok::Ident && &code[t.start..t.end] == w
}

fn is_word_opt(code: &str, t: Option<&lexer::Token>, w: &str) -> bool {
    matches!(t, Some(tt) if is_word(code, tt, w))
}

fn is_punct(t: Option<&lexer::Token>, c: char) -> bool {
    matches!(t, Some(tt) if tt.kind == lexer::Tok::Punct(c))
}

/// Index one past the statement end: a `;` directly after `idx` is included.
fn stmt_end_idx(toks: &[lexer::Token], idx: usize) -> usize {
    if is_punct(toks.get(idx + 1), ';') {
        idx + 2
    } else {
        idx + 1
    }
}

/// `skip_balanced(toks, open_idx, '(', ')')` → index just after the matching close.
fn skip_balanced(toks: &[lexer::Token], open_idx: usize, open: char, close: char) -> usize {
    let mut depth = 0usize;
    let mut i = open_idx;
    while i < toks.len() {
        match toks[i].kind {
            lexer::Tok::Punct(c) if c == open => depth += 1,
            lexer::Tok::Punct(c) if c == close => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    toks.len()
}

/// Skip an optional TS return-type annotation, then return the body `{` index.
/// (Heuristic: object-literal return types are not supported — tame code only.)
fn find_body_open(toks: &[lexer::Token], mut j: usize) -> Option<usize> {
    if is_punct(toks.get(j), ':') {
        j += 1;
        let mut depth = 0i32;
        while j < toks.len() {
            match &toks[j].kind {
                lexer::Tok::Punct('{') if depth == 0 => return Some(j),
                lexer::Tok::Punct('(' | '[' | '<' | '{') => depth += 1,
                lexer::Tok::Punct(')' | ']' | '>' | '}') => depth -= 1,
                lexer::Tok::Punct(';' | ',') if depth == 0 => return None,
                _ => {}
            }
            j += 1;
        }
        return None;
    }
    if is_punct(toks.get(j), '{') {
        return Some(j);
    }
    None
}

/// Find the `=>` after an arrow's parameters (skipping a TS return type).
fn find_arrow(toks: &[lexer::Token], mut j: usize) -> Option<usize> {
    let mut depth = 0i32;
    while j < toks.len() {
        match &toks[j].kind {
            lexer::Tok::Arrow if depth == 0 => return Some(j),
            lexer::Tok::Punct('(' | '[' | '<' | '{') => depth += 1,
            lexer::Tok::Punct(')' | ']' | '>' | '}') => depth -= 1,
            lexer::Tok::Punct(';' | ',') if depth == 0 => return None,
            _ => {}
        }
        j += 1;
    }
    None
}

/// Shape of a parsed async function/arrow (tolerant, tame code).
struct AsyncFnShape {
    /// Token index of the `async` keyword.
    fn_start: usize,
    /// Token index where the replaced span starts (`export` for exported decls).
    span_start: usize,
    body_open: usize,
    body_close: usize,
    name: Option<String>,
    exported: bool,
    /// True when the function is assigned to a binding (`const NAME = async ...`).
    binding: bool,
}

/// Parse `[export] async function NAME(..) {..}`, `const N = async (..) => {..}`,
/// `const N = async function NAME(..) {..}` and `const N = async x => {..}`.
fn parse_async_fn(code: &str, toks: &[lexer::Token], i: usize) -> Option<AsyncFnShape> {
    use lexer::Tok;
    // async function NAME(params) { ... }
    if is_word(code, toks.get(i + 1)?, "function") {
        let name = match toks.get(i + 2) {
            Some(t) if t.kind == Tok::Ident => code[t.start..t.end].to_string(),
            _ => return None, // anonymous expression — cannot be registered
        };
        if !is_punct(toks.get(i + 3), '(') {
            return None;
        }
        let close = skip_balanced(toks, i + 3, '(', ')');
        let body_open = find_body_open(toks, close)?;
        let body_close = skip_balanced(toks, body_open, '{', '}') - 1;
        // `export const N = async function F(..)` keeps its binding prefix.
        if let Some((bname, bexport)) = binding_back(code, toks, i) {
            return Some(AsyncFnShape {
                fn_start: i,
                span_start: i,
                body_open,
                body_close,
                name: Some(bname),
                exported: bexport,
                binding: true,
            });
        }
        // `export default async function` is not supported (cannot re-bind default).
        let exported = i > 0 && is_word(code, &toks[i - 1], "export");
        if i > 0 && is_word(code, &toks[i - 1], "default") {
            return None;
        }
        let span_start = if exported { i - 1 } else { i };
        return Some(AsyncFnShape {
            fn_start: i,
            span_start,
            body_open,
            body_close,
            name: Some(name),
            exported,
            binding: false,
        });
    }
    // async (params) => { ... }
    if is_punct(toks.get(i + 1), '(') {
        let close = skip_balanced(toks, i + 1, '(', ')');
        let arrow = find_arrow(toks, close)?;
        if !is_punct(toks.get(arrow + 1), '{') {
            return None; // expression-body arrows cannot carry the directive
        }
        let body_open = arrow + 1;
        let body_close = skip_balanced(toks, body_open, '{', '}') - 1;
        let (name, exported) = binding_back(code, toks, i)?;
        return Some(AsyncFnShape {
            fn_start: i,
            span_start: i,
            body_open,
            body_close,
            name: Some(name),
            exported,
            binding: true,
        });
    }
    // async x => { ... }
    if matches!(toks.get(i + 1).map(|t| &t.kind), Some(Tok::Ident))
        && matches!(toks.get(i + 2).map(|t| &t.kind), Some(Tok::Arrow))
        && is_punct(toks.get(i + 3), '{')
    {
        let body_open = i + 3;
        let body_close = skip_balanced(toks, body_open, '{', '}') - 1;
        let (name, exported) = binding_back(code, toks, i)?;
        return Some(AsyncFnShape {
            fn_start: i,
            span_start: i,
            body_open,
            body_close,
            name: Some(name),
            exported,
            binding: true,
        });
    }
    None
}

/// Match `const|let|var NAME =` immediately before the `async` token at `i`
/// (with an optional leading `export`). Returns the binding name + export flag.
fn binding_back(code: &str, toks: &[lexer::Token], i: usize) -> Option<(String, bool)> {
    use lexer::Tok;
    if i >= 3
        && is_punct(toks.get(i - 1), '=')
        && toks[i - 2].kind == Tok::Ident
        && ["const", "let", "var"]
            .iter()
            .any(|k| is_word(code, &toks[i - 3], k))
    {
        let name = code[toks[i - 2].start..toks[i - 2].end].to_string();
        let exported = i >= 4 && is_word(code, &toks[i - 4], "export");
        return Some((name, exported));
    }
    None
}

fn escape_js(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

// ---------------------------------------------------------------------------
// §10.1 render-mode inference.
// ---------------------------------------------------------------------------

/// The render mode inferred from a route's loader module, plus — when the
/// mode is `Dynamic` — the byte span of the read that made it dynamic.
///
/// The span lets `oxide routes` name the exact expression, which is the
/// difference between "this route is dynamic" and "this route is dynamic
/// *because* it reads `cookies` on line 7".
#[derive(Debug, Clone, PartialEq)]
pub struct InferredMode {
    /// `"static"` or `"dynamic"`.
    pub mode: &'static str,
    /// Byte span of the first request-specific read, when the mode is
    /// `Dynamic`. `None` for `Static`.
    pub trigger_span: Option<(usize, usize)>,
    /// Human-readable name of the trigger (`"cookies"`, `"request.headers"`,
    /// `"locals"`, `"searchParams"`).
    pub trigger: Option<&'static str>,
}

/// V2-F16: one request-specific read in a loader module — the unit the
/// `oxide why --route` report and the `.oxide/routes.reasons.json` side-car
/// are built from.
#[derive(Debug, Clone, PartialEq)]
pub struct DynamicReason {
    /// Which request-scoped read this is.
    pub kind: ReasonKind,
    /// Byte span of the read (`source.get(span.0..span.1)` always works).
    pub span: (usize, usize),
    /// 1-based line of `span.0`.
    pub line: u32,
    /// The trimmed source line containing `span.0`.
    pub snippet: String,
}

/// V2-F16: the kind of request-scoped read (Appendix V2.K code `OX-DYN`).
///
/// The four §10.1 kinds map 1:1 onto the trigger strings
/// [`infer_render_mode`] has always reported. `UncachedFetch` is the F16
/// addition and deliberately does **not** flip the render mode: a `fetch`
/// call reads no request state, so it only feeds the why report and the
/// side-car (the `compiler_tdd` §10.1 pins must keep passing).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ReasonKind {
    /// A `cookies` read (the bind or the accessor).
    CookieRead,
    /// A `request.headers` read (the bind or the accessor).
    HeaderRead,
    /// A `locals` read (the bind or the accessor).
    LocalsRead,
    /// A `searchParams` read (the bind or the accessor).
    SearchParamRead,
    /// A `fetch(` call outside any `'use cache'` function.
    UncachedFetch,
}

impl ReasonKind {
    /// The scanner vocabulary `oxide routes` already prints (the §10.1
    /// trigger names; `fetch` is the V2-F16 addition).
    #[must_use]
    pub fn trigger(self) -> &'static str {
        match self {
            Self::CookieRead => "cookies",
            Self::HeaderRead => "request.headers",
            Self::LocalsRead => "locals",
            Self::SearchParamRead => "searchParams",
            Self::UncachedFetch => "fetch",
        }
    }

    /// The diagnostic headline (the OX-DYN message).
    #[must_use]
    pub fn message(self) -> &'static str {
        match self {
            Self::CookieRead => "route is dynamic: cookies read",
            Self::HeaderRead => "route is dynamic: request headers read",
            Self::LocalsRead => "route is dynamic: locals read",
            Self::SearchParamRead => "route is dynamic: searchParams read",
            Self::UncachedFetch => "uncached fetch: the response cannot be cached",
        }
    }

    /// The fix help text (the miette `help` of the OX-DYN diagnostic, and
    /// the contract the deferred `--fix` codemods must verify against).
    #[must_use]
    pub fn help(self) -> &'static str {
        match self {
            Self::CookieRead => {
                "Move the cookie read into the loader and return it as data; \
                 components consume props, never the request."
            }
            Self::HeaderRead => {
                "Read the header in the loader (or set it via `setHeaders`) \
                 and return it as data; components consume props, never the request."
            }
            Self::LocalsRead => {
                "Return the locals value from the loader as data so the route \
                 can be cached without its middleware dependency."
            }
            Self::SearchParamRead => {
                "Declare a `searchSchema` so the listed keys become cache \
                 dimensions instead of a dynamic read."
            }
            Self::UncachedFetch => {
                "Wrap the fetch in a `'use cache'` function so the loader \
                 result is cacheable."
            }
        }
    }

    /// The why-chain cause attached under the headline.
    #[must_use]
    pub fn cause(self) -> &'static str {
        match self {
            Self::CookieRead => "`cookies` is request-scoped state (§10.1)",
            Self::HeaderRead => "`request.headers` is request-scoped state (§10.1)",
            Self::LocalsRead => "`locals` is middleware-owned request state (§10.1)",
            Self::SearchParamRead => "`searchParams` is request-scoped state (§10.1)",
            Self::UncachedFetch => {
                "the `fetch(` call is not inside a `'use cache'` function (§9.3)"
            }
        }
    }
}

/// The `OX-DYN` code (spec Appendix V2.K): "Route is dynamic; reasons
/// attached (F16)".
const OX_DYN: &str = "OX-DYN";

impl DynamicReason {
    /// Build the source-mapped `OX-DYN` diagnostic for this read: headline,
    /// location, rendered code frame, the why-chain cause and the fix help
    /// text (the miette help of the diagnostic).
    #[must_use]
    pub fn diagnostic(&self, path: &str, source: &str) -> Diagnostic {
        let prefix = source.get(..self.span.0).unwrap_or("");
        let col = prefix
            .rfind('\n')
            .map_or(self.span.0 + 1, |i| self.span.0 - i);
        let frame = oxide_fw_diagnostics::render_code_frame(source, self.line as usize, Some(col));
        Diagnostic::info(OX_DYN, self.kind.message())
            .at(oxide_fw_diagnostics::Location::file(
                path,
                self.line,
                u32::try_from(col).unwrap_or(u32::MAX),
            ))
            .with_code_frame(frame)
            .with_fix(self.kind.help())
            .with_cause(self.kind.cause())
    }

    /// The side-car row (`.oxide/routes.reasons.json` entry shape:
    /// `{ kind, line, snippet, span }`).
    #[must_use]
    pub fn row(&self) -> RouteReasonRow {
        RouteReasonRow {
            kind: self.kind,
            line: self.line,
            snippet: self.snippet.clone(),
            span: [self.span.0, self.span.1],
        }
    }
}

/// One row of the `.oxide/routes.reasons.json` side-car (V2-F16) — the
/// JSON map of route id to reasons that inspectors and the MCP
/// `routes.list` tool consume instead of re-running the AST pass.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RouteReasonRow {
    /// The read kind (camelCase in JSON: `cookieRead`, `uncachedFetch`, ...).
    pub kind: ReasonKind,
    /// 1-based line of the read.
    pub line: u32,
    /// The trimmed source line containing the read.
    pub snippet: String,
    /// Byte span `[start, end]` of the read.
    pub span: [usize; 2],
}

/// §V2-F16: every request-specific read in a loader module, in pre-order —
/// the collect-all sibling of [`infer_render_mode`]'s first-hit visitor.
///
/// A read counts when it is a *member access* on a request-scoped object,
/// when a loader-shaped function binds one of the request-scoped names out
/// of its params, or — the F16 addition — when a `fetch(` call sits outside
/// any `'use cache'` function ([`ReasonKind::UncachedFetch`]). A destructured
/// bind and its member use are two hits, so `({ cookies })` plus
/// `cookies.get(...)` reports two [`DynamicReason`]s.
///
/// A module that does not parse reports no reasons (the conservative mode
/// answer stays `dynamic` via [`infer_render_mode`]; there is no span to
/// point a diagnostic at).
#[must_use]
pub fn infer_reasons(source: &str, path: &str) -> Vec<DynamicReason> {
    let _span = tracing::debug_span!("infer_reasons", path = %path).entered();
    match collect_request_hits(source, path) {
        Some(hits) => hits.iter().map(|hit| hit.reason(source)).collect(),
        None => Vec::new(),
    }
}

/// One request-scoped read found by the AST walk, before line/snippet
/// enrichment.
struct Hit {
    span: (usize, usize),
    kind: ReasonKind,
}

impl Hit {
    /// Enrich the span with its 1-based line and trimmed source line.
    fn reason(&self, source: &str) -> DynamicReason {
        let start = self.span.0;
        let prefix = source.get(..start).unwrap_or("");
        let line = 1u32
            + prefix
                .bytes()
                .filter(|b| *b == b'\n')
                .count()
                .try_into()
                .unwrap_or(u32::MAX - 1);
        let line_start = prefix.rfind('\n').map_or(0, |i| i + 1);
        let rest = source.get(start..).unwrap_or("");
        let line_end = rest
            .find('\n')
            .map_or(source.len(), |i| start.saturating_add(i));
        let snippet = source
            .get(line_start..line_end)
            .unwrap_or("")
            .trim()
            .to_string();
        DynamicReason {
            kind: self.kind,
            span: self.span,
            line,
            snippet,
        }
    }
}

/// Parse the module and collect **every** request-scoped read in pre-order.
/// `None` when the module does not parse (the caller decides between the
/// conservative `dynamic` mode answer and the empty reasons list).
fn collect_request_hits(source: &str, path: &str) -> Option<Vec<Hit>> {
    let allocator = oxc_allocator::Allocator::default();
    // `.server.ts` files are TypeScript.
    let source_type = oxc_span::SourceType::ts();
    let parsed = oxc_parser::Parser::new(&allocator, source, source_type).parse();
    if !parsed.diagnostics.is_empty() {
        // The module does not parse. It is a build error the transform pass
        // will surface with a proper diagnostic; the inference's job is to
        // not crash, and the conservative answer is "dynamic". The path is
        // named here so the inference is observable in traces (it used to be
        // discarded with a bare `let _ = path`).
        tracing::debug!(
            path = %path,
            "loader module does not parse: mode falls back to dynamic, no reason spans exist"
        );
        return None;
    }
    let mut hits = Vec::new();
    for stmt in &parsed.program.body {
        walk_stmt_for_request_reads(stmt, source, false, &mut hits);
    }
    Some(hits)
}

/// §10.1: does this loader module read anything request-specific?
///
/// Runs a real Oxc parse and walks the AST — not a line scan. A read counts
/// when it is a *member access* on a request-scoped object, so a string
/// literal or a comment mentioning `cookies` does not flip the mode, and an
/// identifier named `cookies` that is a local variable is not a request
/// read. The four objects the spec names:
///
/// * `cookies` — the load event's cookie accessor
/// * `locals` — middleware-injected request state
/// * `request.headers` — request headers
/// * `searchParams` — the search-param accessor
///
/// An explicit `config.render` always wins; the caller checks that first and
/// only calls this when no mode was declared.
///
/// Falls back to the line-based scan when the module does not parse (a
/// `.server.ts` the parser rejects is a build error reported elsewhere, and
/// the conservative answer here is `dynamic`).
///
/// V2-F16: this stays the cheap first-hit entry point — its contract is
/// frozen (the §10.1 pins must not change). Every hit, including the
/// `UncachedFetch` kind that never flips the mode, comes from
/// [`infer_reasons`] instead.
#[must_use]
pub fn infer_render_mode(source: &str, path: &str) -> InferredMode {
    match collect_request_hits(source, path) {
        None => InferredMode {
            mode: "dynamic",
            trigger_span: None,
            trigger: None,
        },
        Some(hits) => match hits
            .iter()
            .find(|hit| hit.kind != ReasonKind::UncachedFetch)
        {
            // An UncachedFetch never flips the mode (it reads no request
            // state): the first-hit contract scans past it, exactly what
            // the pre-F16 visitor answered.
            Some(hit) => InferredMode {
                mode: "dynamic",
                trigger_span: Some(hit.span),
                trigger: Some(hit.kind.trigger()),
            },
            None => InferredMode {
                mode: "static",
                trigger_span: None,
                trigger: None,
            },
        },
    }
}

fn walk_stmt_for_request_reads(
    stmt: &oxc_ast::ast::Statement<'_>,
    source: &str,
    in_cache: bool,
    hits: &mut Vec<Hit>,
) {
    use oxc_ast::ast::Statement;
    match stmt {
        Statement::ExpressionStatement(es) => walk_expr(&es.expression, source, in_cache, hits),
        Statement::ReturnStatement(r) => {
            if let Some(arg) = &r.argument {
                walk_expr(arg, source, in_cache, hits);
            }
        }
        Statement::VariableDeclaration(v) => {
            for d in &v.declarations {
                if let Some(init) = &d.init {
                    walk_expr(init, source, in_cache, hits);
                }
            }
        }
        Statement::IfStatement(i) => {
            walk_expr(&i.test, source, in_cache, hits);
            walk_stmt_for_request_reads(&i.consequent, source, in_cache, hits);
            if let Some(alt) = &i.alternate {
                walk_stmt_for_request_reads(alt, source, in_cache, hits);
            }
        }
        Statement::BlockStatement(b) => {
            for s in &b.body {
                walk_stmt_for_request_reads(s, source, in_cache, hits);
            }
        }
        Statement::ExportDeclaration(e) => {
            walk_decl(&e.declaration, source, in_cache, hits);
        }
        Statement::ExportDefaultDeclaration(e) => {
            if let oxc_ast::ast::ExportDefaultDeclarationKind::FunctionDeclaration(f) =
                &e.declaration
                && let Some(body) = &f.body
            {
                let cached = in_cache || has_use_cache_directive(body);
                for s in &body.statements {
                    walk_stmt_for_request_reads(s, source, cached, hits);
                }
            }
        }
        Statement::FunctionDeclaration(f) => {
            walk_fn_like(&f.params, f.body.as_deref(), source, in_cache, hits);
        }
        Statement::TryStatement(t) => {
            for s in &t.block.body {
                walk_stmt_for_request_reads(s, source, in_cache, hits);
            }
            if let Some(h) = &t.handler {
                for s in &h.body.body {
                    walk_stmt_for_request_reads(s, source, in_cache, hits);
                }
            }
        }
        _ => {}
    }
}

fn walk_decl(d: &oxc_ast::ast::Declaration<'_>, source: &str, in_cache: bool, hits: &mut Vec<Hit>) {
    use oxc_ast::ast::Declaration;
    match d {
        Declaration::VariableDeclaration(v) => {
            for d in &v.declarations {
                if let Some(init) = &d.init {
                    walk_expr(init, source, in_cache, hits);
                }
            }
        }
        Declaration::FunctionDeclaration(f) => {
            walk_fn_like(&f.params, f.body.as_deref(), source, in_cache, hits);
        }
        _ => {}
    }
}

fn walk_expr(e: &oxc_ast::ast::Expression<'_>, source: &str, in_cache: bool, hits: &mut Vec<Hit>) {
    use oxc_ast::ast::Expression;
    match e {
        Expression::StaticMemberExpression(m) => {
            // The property name is the read. `cookies.get(...)` — the
            // `cookies` in `cookies.get` is a `StaticMemberExpression` on
            // object `cookies`, property `get`; but `cookies` itself is an
            // `Identifier` in the inner expression, not a member. So the
            // check is on the *object* being one of the four names, not on
            // the property. A member access whose object is `cookies` or
            // `locals` is a request read; `request.headers` is a member
            // whose object is `request` and property is `headers`.
            if let Expression::Identifier(obj) = &m.object {
                let n = obj.name.as_str();
                if let Some(kind) = request_scoped_kind(n) {
                    hits.push(Hit {
                        span: (m.span.start as usize, m.span.end as usize),
                        kind,
                    });
                }
            }
            if let Expression::Identifier(obj) = &m.object
                && obj.name.as_str() == "request"
                && m.property.name.as_str() == "headers"
            {
                hits.push(Hit {
                    span: (m.span.start as usize, m.span.end as usize),
                    kind: ReasonKind::HeaderRead,
                });
            }
            walk_expr(&m.object, source, in_cache, hits);
        }
        Expression::ComputedMemberExpression(m) => {
            if let Expression::Identifier(obj) = &m.object
                && let Some(kind) = request_scoped_kind(obj.name.as_str())
            {
                hits.push(Hit {
                    span: (m.span.start as usize, m.span.end as usize),
                    kind,
                });
            }
            walk_expr(&m.object, source, in_cache, hits);
            walk_expr(&m.expression, source, in_cache, hits);
        }
        Expression::CallExpression(call) => {
            walk_call_expression(call, source, in_cache, hits);
        }
        Expression::AwaitExpression(a) => walk_expr(&a.argument, source, in_cache, hits),
        Expression::ParenthesizedExpression(p) => walk_expr(&p.expression, source, in_cache, hits),
        Expression::AssignmentExpression(a) => walk_expr(&a.right, source, in_cache, hits),
        Expression::ConditionalExpression(c) => {
            walk_expr(&c.test, source, in_cache, hits);
            walk_expr(&c.consequent, source, in_cache, hits);
            walk_expr(&c.alternate, source, in_cache, hits);
        }
        Expression::BinaryExpression(b) => {
            walk_expr(&b.left, source, in_cache, hits);
            walk_expr(&b.right, source, in_cache, hits);
        }
        Expression::LogicalExpression(l) => {
            walk_expr(&l.left, source, in_cache, hits);
            walk_expr(&l.right, source, in_cache, hits);
        }
        Expression::ObjectExpression(o) => {
            for p in &o.properties {
                if let oxc_ast::ast::ObjectPropertyKind::ObjectProperty(op) = p {
                    walk_expr(&op.value, source, in_cache, hits);
                }
            }
        }
        Expression::ArrayExpression(arr) => {
            for el in &arr.elements {
                if let Some(e) = el.as_expression() {
                    walk_expr(e, source, in_cache, hits);
                }
            }
        }
        Expression::ArrowFunctionExpression(a) => {
            // `ArrowFunctionBody` is `FunctionBody | INHERIT(Expression)`;
            // the inherited variant's accessor is `as_expression`.
            match &a.body {
                oxc_ast::ast::ArrowFunctionBody::FunctionBody(b) => {
                    walk_fn_like(&a.params, Some(b), source, in_cache, hits);
                }
                other => {
                    // §10.1: destructured request state in params counts as
                    // a read: `load = async ({ cookies }) => ...` observes
                    // request state without a member expression on it.
                    check_fn_params_for_request_read(&a.params, source, hits);
                    if let Some(expr) = other.as_expression() {
                        walk_expr(expr, source, in_cache, hits);
                    }
                }
            }
        }
        Expression::FunctionExpression(f) => {
            walk_fn_like(&f.params, f.body.as_deref(), source, in_cache, hits);
        }
        _ => {}
    }
}

/// The shared shape of the function-like arms: the formal params are checked
/// for a request-event bind (§10.1), then the body is walked with the
/// `'use cache'` scope threaded through (a cached function's fetches are
/// not `UncachedFetch` reasons, V2-F16).
fn walk_fn_like(
    params: &oxc_ast::ast::FormalParameters<'_>,
    body: Option<&oxc_ast::ast::FunctionBody>,
    source: &str,
    in_cache: bool,
    hits: &mut Vec<Hit>,
) {
    check_fn_params_for_request_read(params, source, hits);
    if let Some(body) = body {
        let cached = in_cache || has_use_cache_directive(body);
        for s in &body.statements {
            walk_stmt_for_request_reads(s, source, cached, hits);
        }
    }
}

/// The call arm, extracted to keep [`walk_expr`] under the pedantic
/// function-length budget: a `fetch(` call outside any `'use cache'`
/// function is an `UncachedFetch` reason (V2-F16), recorded in pre-order
/// (before the arguments are walked) and never flipping the render mode.
fn walk_call_expression(
    call: &oxc_ast::ast::CallExpression<'_>,
    source: &str,
    in_cache: bool,
    hits: &mut Vec<Hit>,
) {
    use oxc_ast::ast::Argument;
    if !in_cache && is_fetch_callee(&call.callee) {
        hits.push(Hit {
            span: (call.span.start as usize, call.span.end as usize),
            kind: ReasonKind::UncachedFetch,
        });
    }
    walk_expr(&call.callee, source, in_cache, hits);
    for a in &call.arguments {
        match a {
            Argument::SpreadElement(s) => walk_expr(&s.argument, source, in_cache, hits),
            _ => {
                if let Some(e) = a.as_expression() {
                    walk_expr(e, source, in_cache, hits);
                }
            }
        }
    }
}

/// §10.1: the formal params of a loader-shaped function bind the request
/// event (`{ cookies }`, `{ locals }`, `{ searchParams }`, `request`).
/// A binding of one of those names is a request-specific read even when
/// the body never performs a member access on it
/// (`async ({ cookies }) => ({})` returns an empty object but is dynamic).
fn check_fn_params_for_request_read(
    params: &oxc_ast::ast::FormalParameters<'_>,
    source: &str,
    hits: &mut Vec<Hit>,
) {
    let (start, end) = (params.span.start as usize, params.span.end as usize);
    let text = source.get(start..end).unwrap_or("");
    for name in ["cookies", "locals", "searchParams", "request"] {
        if has_word(text, name) {
            let kind: ReasonKind = match name {
                "cookies" => ReasonKind::CookieRead,
                "locals" => ReasonKind::LocalsRead,
                "searchParams" => ReasonKind::SearchParamRead,
                _ => ReasonKind::HeaderRead,
            };
            hits.push(Hit {
                span: (start, end),
                kind,
            });
            return;
        }
    }
}

/// Word-boundary substring search without pulling in a regex.
fn has_word(haystack: &str, needle: &str) -> bool {
    let hb = haystack.as_bytes();
    let nb = needle.as_bytes();
    if nb.len() > hb.len() {
        return false;
    }
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'$';
    let mut i = 0;
    while i + nb.len() <= hb.len() {
        if &hb[i..i + nb.len()] == nb {
            let before = i == 0 || !is_word(hb[i - 1]);
            let after = i + nb.len() == hb.len() || !is_word(hb[i + nb.len()]);
            if before && after {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// The four request-scoped names the spec (§10.1) treats as dynamic.
fn request_scoped_kind(n: &str) -> Option<ReasonKind> {
    match n {
        "cookies" => Some(ReasonKind::CookieRead),
        "locals" => Some(ReasonKind::LocalsRead),
        "searchParams" => Some(ReasonKind::SearchParamRead),
        _ => None,
    }
}

/// V2-F16: is this call expression's callee a `fetch` call — the bare
/// identifier `fetch` or a member whose property is `fetch`
/// (`globalThis.fetch`, `window.fetch`)?
fn is_fetch_callee(callee: &oxc_ast::ast::Expression) -> bool {
    match callee {
        oxc_ast::ast::Expression::Identifier(id) => id.name.as_str() == "fetch",
        oxc_ast::ast::Expression::StaticMemberExpression(m) => m.property.name.as_str() == "fetch",
        _ => false,
    }
}

/// V2-F16: does this function body carry the `'use cache'` directive?
/// Mirrors the directive pass's check (`value.trim() == "use cache"`) at
/// the AST level, so a fetch inside a cached function is not reported.
fn has_use_cache_directive(body: &oxc_ast::ast::FunctionBody) -> bool {
    body.directives.iter().any(|d| {
        d.expression
            .value
            .as_str()
            .is_some_and(|s| s.trim() == "use cache")
    })
}

#[cfg(test)]
mod strip_server_fn_imports_tests {
    use super::*;

    #[test]
    fn multi_name_import_keeps_balanced_braces() {
        let input = "import { serverFn, defineLoad } from \"@oxide-fw/core/server\";\nexport const x = 1;\n";
        let out = strip_server_fn_imports(input);
        assert!(!out.contains("serverFn"), "serverFn still present:\n{out}");
        assert!(
            out.contains("{ defineLoad }"),
            "balanced braces missing (regression: original impl dropped closing brace):\n{out}"
        );
        assert!(
            out.contains("\"@oxide-fw/core/server\""),
            "specifier lost:\n{out}"
        );
        assert!(
            out.contains("export const x = 1"),
            "rest of file lost:\n{out}"
        );
    }

    #[test]
    fn aliased_server_fn_is_stripped() {
        let input = "import { serverFn as sf, defineLoad } from \"x\";\n";
        let out = strip_server_fn_imports(input);
        assert!(!out.contains("serverFn"), "serverFn still present:\n{out}");
        assert!(!out.contains(" as sf"), "alias still present:\n{out}");
        assert!(out.contains("defineLoad"), "defineLoad lost:\n{out}");
    }

    #[test]
    fn type_only_server_fn_is_stripped() {
        let input = "import type { serverFn, defineLoad } from \"x\";\n";
        let out = strip_server_fn_imports(input);
        assert!(!out.contains("serverFn"), "serverFn still present:\n{out}");
        assert!(out.contains("import type"), "type modifier lost:\n{out}");
        assert!(out.contains("defineLoad"), "defineLoad lost:\n{out}");
    }

    #[test]
    fn default_plus_named_drops_server_fn_default() {
        let input = "import serverFn, { foo } from \"x\";\n";
        let out = strip_server_fn_imports(input);
        assert!(!out.contains("serverFn"), "serverFn still present:\n{out}");
        assert!(out.contains("foo"), "foo lost:\n{out}");
    }

    #[test]
    fn multiline_import_is_handled() {
        let input = "import {\n  serverFn,\n  defineLoad\n} from \"x\";\n";
        let out = strip_server_fn_imports(input);
        assert!(!out.contains("serverFn"), "serverFn still present:\n{out}");
        assert!(out.contains("defineLoad"), "defineLoad lost:\n{out}");
    }

    #[test]
    fn only_server_fn_import_line_is_removed() {
        let input = "import { serverFn } from \"x\";\nexport const y = 2;\n";
        let out = strip_server_fn_imports(input);
        assert!(!out.contains("serverFn"), "serverFn still present:\n{out}");
        assert!(!out.contains("import"), "import line not removed:\n{out}");
        assert!(out.contains("export const y = 2"), "rest lost:\n{out}");
    }

    #[test]
    fn unrelated_import_is_untouched() {
        let input = "import { foo, bar } from \"x\";\n";
        let out = strip_server_fn_imports(input);
        assert_eq!(out, input);
    }

    #[test]
    fn side_effect_import_untouched() {
        let input = "import \"x\";\n";
        let out = strip_server_fn_imports(input);
        assert_eq!(out, input);
    }

    #[test]
    fn dynamic_import_untouched() {
        let input = "const m = import(\"x\");\n";
        let out = strip_server_fn_imports(input);
        assert_eq!(out, input);
    }

    #[test]
    fn compound_identifier_does_not_match() {
        let input = "import { myServerFn, serverFnHelper } from \"x\";\n";
        let out = strip_server_fn_imports(input);
        assert_eq!(
            out, input,
            "compound identifiers should not match bare serverFn"
        );
    }
}
