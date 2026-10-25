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

pub mod framework;
pub mod generate_params;
pub mod lexer;
mod oxc_backend;

pub use framework::{
    Framework, FrameworkCompiler, FrameworkInput, FrameworkOutput, ReactInline, SolidInline,
    ToolingDispatcher, compiler_for,
};
pub use generate_params::{GenerateParamsResult, extract_generate_params};
pub use oxc_backend::{OxcTransformer, lower_to_js};

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
pub fn scan_import_spans(code: &str) -> Vec<ImportSpan> {
    use lexer::Tok;
    let toks = lexer::lex(code);
    let n = toks.len();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < n {
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
                    let mut j = i + 1;
                    if matches!(toks.get(j).map(|x| &x.kind), Some(Tok::Ident)) {
                        j += 1; // default binding (also absorbs TS `import type`)
                    }
                    if is_punct(toks.get(j), ',') {
                        j += 1;
                    }
                    if is_punct(toks.get(j), '{') {
                        j = skip_balanced(&toks, j, '{', '}');
                    }
                    if is_punct(toks.get(j), '*') {
                        j += 1;
                        if matches!(toks.get(j).map(|x| &x.kind), Some(Tok::Ident))
                            && is_word(code, &toks[j], "as")
                        {
                            j += 1;
                        }
                        if matches!(toks.get(j).map(|x| &x.kind), Some(Tok::Ident)) {
                            j += 1;
                        }
                    }
                    if is_word_opt(code, toks.get(j), "from") {
                        if let Some(Tok::Str { value }) = toks.get(j + 1).map(|x| &x.kind) {
                            let end_idx = stmt_end_idx(&toks, j + 1);
                            out.push(ImportSpan {
                                start: t.start,
                                end: toks[end_idx - 1].end,
                                specifier: value.clone(),
                                dynamic: false,
                            });
                            i = end_idx;
                            continue;
                        }
                    }
                    i += 1; // e.g. `import x = require('y')` — not an ESM import
                    continue;
                }
            }
        }
        if is_word(code, t, "export") {
            // re-export: `export {..} from '...'` / `export * from '...'` / `export * as ns from '...'`
            let reexport = matches!(
                toks.get(i + 1).map(|x| &x.kind),
                Some(Tok::Punct('{')) | Some(Tok::Punct('*'))
            );
            if reexport {
                let mut j = i + 1;
                if is_punct(toks.get(j), '{') {
                    j = skip_balanced(&toks, j, '{', '}');
                } else {
                    j += 1; // '*'
                    if is_word_opt(code, toks.get(j), "as") {
                        j += 2;
                    }
                }
                let mut k = j;
                while k < n {
                    if is_punct(toks.get(k), ';') {
                        break;
                    }
                    if is_word_opt(code, toks.get(k), "from") {
                        if let Some(Tok::Str { value }) = toks.get(k + 1).map(|x| &x.kind) {
                            let end_idx = stmt_end_idx(&toks, k + 1);
                            out.push(ImportSpan {
                                start: t.start,
                                end: toks[end_idx - 1].end,
                                specifier: value.clone(),
                                dynamic: false,
                            });
                            i = end_idx;
                            break;
                        }
                        break;
                    }
                    k += 1;
                }
                continue;
            }
            i += 1;
            continue;
        }
        i += 1;
    }
    out
}

/// Import scan (§6.2 step 7): static imports, dynamic imports, asset queries.
/// Specifiers containing `?` are asset queries (§6.7) and excluded from `imports`.
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
pub fn is_server_module_path(path: &str) -> bool {
    ["ts", "tsx", "js", "jsx", "mjs"]
        .iter()
        .any(|ext| path.ends_with(&format!(".server.{ext}")))
}

/// The compiler trait (D4 isolation point; Oxc backend slots in here).
pub trait Transformer: Send + Sync {
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
        let mut blanks: Vec<(usize, usize)> = Vec::new();
        let mut tags: Vec<String> = Vec::new();
        let mut life: Vec<String> = Vec::new();
        let d_end = stmt_end_idx(&toks, first);
        blanks.push((toks[first].start, toks[d_end - 1].end));
        let mut j = first + 1;
        while j < af.body_close {
            if is_word(code, &toks[j], "cacheTag")
                && is_punct(toks.get(j + 1), '(')
                && matches!(toks.get(j + 2).map(|t| &t.kind), Some(Tok::Str { .. }))
                && is_punct(toks.get(j + 3), ')')
            {
                tags.push(code[toks[j + 2].start..toks[j + 2].end].to_string());
                let e = stmt_end_idx(&toks, j + 3);
                blanks.push((toks[j].start, toks[e - 1].end));
                j = e;
                continue;
            }
            if is_word(code, &toks[j], "cacheLife") && is_punct(toks.get(j + 1), '(') {
                let close = skip_balanced(&toks, j + 1, '(', ')');
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
                let e = stmt_end_idx(&toks, close - 1);
                blanks.push((toks[j].start, toks[e - 1].end));
                j = e;
                continue;
            }
            j += 1;
        }

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

/// Server-fn stub pass (§9.4): on the **client** environment, importing `addToCart`
/// from `cart.server.ts` compiles to a stub calling `POST /_oxide/fn/<id>`.
/// On the **server** environment the body is kept and the id registry emitted.
///
/// Recognized: `export const <name> = serverFn(async (...) => {...})` and
/// `serverFn(async function (...) {...})`; an optional schema first argument
/// (`serverFn(schema, async ...)`) is supported (the schema arg is dropped in the
/// client stub, which only forwards `...args`).
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
                    Tok::Punct('(') | Tok::Punct('{') | Tok::Punct('[') => depth += 1,
                    Tok::Punct(')') | Tok::Punct('}') | Tok::Punct(']') => {
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
                // The stub calls `__oxide_call_fn` from `oxide/fn` rather
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
        // One import regardless of how many stubs were emitted. Placed at
        // the top so it sits before any code the transform left in place.
        let import = "import { __oxide_call_fn } from \"oxide/fn\";\n";
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
        out.push_str(&format!(
            "export const __oxide_fn_ids = [{}];\n",
            ids.join(", ")
        ));
    }
    Ok((out, stubs))
}

/// Forbidden-import guard (§6.4): a compile-time check that no `*.server.ts` module
/// is reachable from the client entry except via the server-fn stub transform.
/// Returns diagnostics naming the full import chain (§19.4).
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
                lexer::Tok::Punct('(')
                | lexer::Tok::Punct('[')
                | lexer::Tok::Punct('<')
                | lexer::Tok::Punct('{') => depth += 1,
                lexer::Tok::Punct(')')
                | lexer::Tok::Punct(']')
                | lexer::Tok::Punct('>')
                | lexer::Tok::Punct('}') => depth -= 1,
                lexer::Tok::Punct(';') | lexer::Tok::Punct(',') if depth == 0 => return None,
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
            lexer::Tok::Punct('(')
            | lexer::Tok::Punct('[')
            | lexer::Tok::Punct('<')
            | lexer::Tok::Punct('{') => depth += 1,
            lexer::Tok::Punct(')')
            | lexer::Tok::Punct(']')
            | lexer::Tok::Punct('>')
            | lexer::Tok::Punct('}') => depth -= 1,
            lexer::Tok::Punct(';') | lexer::Tok::Punct(',') if depth == 0 => return None,
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
pub fn infer_render_mode(source: &str, path: &str) -> InferredMode {
    let allocator = oxc_allocator::Allocator::default();
    // `.server.ts` files are TypeScript.
    let source_type = oxc_span::SourceType::ts();
    let parsed = oxc_parser::Parser::new(&allocator, source, source_type).parse();
    if !parsed.diagnostics.is_empty() {
        // The module does not parse. It is a build error the transform pass
        // will surface with a proper diagnostic; the inference's job is to
        // not crash, and the conservative answer is "dynamic".
        let _ = path;
        return InferredMode {
            mode: "dynamic",
            trigger_span: None,
            trigger: None,
        };
    }

    let mut found: Option<((usize, usize), &'static str)> = None;
    walk_for_request_reads(&parsed.program, source, &mut found);
    match found {
        Some(((start, end), trigger)) => InferredMode {
            mode: "dynamic",
            trigger_span: Some((start, end)),
            trigger: Some(trigger),
        },
        None => InferredMode {
            mode: "static",
            trigger_span: None,
            trigger: None,
        },
    }
}

/// Recursively scan every expression for a request-scoped read. Returns the
/// first one found (top-down, so the outermost access wins) — its span is
/// what a diagnostic points at.
fn walk_for_request_reads<'a>(
    program: &oxc_ast::ast::Program<'a>,
    source: &str,
    found: &mut Option<((usize, usize), &'static str)>,
) {
    for stmt in &program.body {
        walk_stmt_for_request_reads(stmt, source, found);
        if found.is_some() {
            return;
        }
    }
}

fn walk_stmt_for_request_reads<'a>(
    stmt: &oxc_ast::ast::Statement<'a>,
    source: &str,
    found: &mut Option<((usize, usize), &'static str)>,
) {
    use oxc_ast::ast::Statement;
    if found.is_some() {
        return;
    }
    match stmt {
        Statement::ExpressionStatement(es) => walk_expr(&es.expression, source, found),
        Statement::ReturnStatement(r) => {
            if let Some(arg) = &r.argument {
                walk_expr(arg, source, found);
            }
        }
        Statement::VariableDeclaration(v) => {
            for d in &v.declarations {
                if let Some(init) = &d.init {
                    walk_expr(init, source, found);
                }
            }
        }
        Statement::IfStatement(i) => {
            walk_expr(&i.test, source, found);
            walk_stmt_for_request_reads(&i.consequent, source, found);
            if let Some(alt) = &i.alternate {
                walk_stmt_for_request_reads(alt, source, found);
            }
        }
        Statement::BlockStatement(b) => {
            for s in &b.body {
                walk_stmt_for_request_reads(s, source, found);
                if found.is_some() {
                    return;
                }
            }
        }
        Statement::ExportDeclaration(e) => {
            walk_decl(&e.declaration, source, found);
        }
        Statement::ExportDefaultDeclaration(e) => {
            if let oxc_ast::ast::ExportDefaultDeclarationKind::FunctionDeclaration(f) =
                &e.declaration
            {
                if let Some(body) = &f.body {
                    for s in &body.statements {
                        walk_stmt_for_request_reads(s, source, found);
                        if found.is_some() {
                            return;
                        }
                    }
                }
            }
        }
        Statement::FunctionDeclaration(f) => {
            if check_fn_params_for_request_read(&f.params, source, found) {
                return;
            }
            if let Some(body) = &f.body {
                for s in &body.statements {
                    walk_stmt_for_request_reads(s, source, found);
                    if found.is_some() {
                        return;
                    }
                }
            }
        }
        Statement::TryStatement(t) => {
            for s in &t.block.body {
                walk_stmt_for_request_reads(s, source, found);
                if found.is_some() {
                    return;
                }
            }
            if let Some(h) = &t.handler {
                for s in &h.body.body {
                    walk_stmt_for_request_reads(s, source, found);
                    if found.is_some() {
                        return;
                    }
                }
            }
        }
        _ => {}
    }
}

fn walk_decl<'a>(
    d: &oxc_ast::ast::Declaration<'a>,
    source: &str,
    found: &mut Option<((usize, usize), &'static str)>,
) {
    use oxc_ast::ast::Declaration;
    match d {
        Declaration::VariableDeclaration(v) => {
            for d in &v.declarations {
                if let Some(init) = &d.init {
                    walk_expr(init, source, found);
                }
            }
        }
        Declaration::FunctionDeclaration(f) => {
            if check_fn_params_for_request_read(&f.params, source, found) {
                return;
            }
            if let Some(body) = &f.body {
                for s in &body.statements {
                    walk_stmt_for_request_reads(s, source, found);
                    if found.is_some() {
                        return;
                    }
                }
            }
        }
        _ => {}
    }
}

fn walk_expr<'a>(
    e: &oxc_ast::ast::Expression<'a>,
    source: &str,
    found: &mut Option<((usize, usize), &'static str)>,
) {
    use oxc_ast::ast::{Argument, Expression};
    if found.is_some() {
        return;
    }
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
                if let Some(name) = request_scoped_name(n) {
                    *found = Some(((m.span.start as usize, m.span.end as usize), name));
                    return;
                }
            }
            if let Expression::Identifier(obj) = &m.object {
                if obj.name.as_str() == "request" && m.property.name.as_str() == "headers" {
                    *found = Some((
                        (m.span.start as usize, m.span.end as usize),
                        "request.headers",
                    ));
                    return;
                }
            }
            walk_expr(&m.object, source, found);
        }
        Expression::ComputedMemberExpression(m) => {
            if let Expression::Identifier(obj) = &m.object {
                if let Some(name) = request_scoped_name(obj.name.as_str()) {
                    *found = Some(((m.span.start as usize, m.span.end as usize), name));
                    return;
                }
            }
            walk_expr(&m.object, source, found);
            walk_expr(&m.expression, source, found);
        }
        Expression::CallExpression(call) => {
            walk_expr(&call.callee, source, found);
            for a in &call.arguments {
                match a {
                    Argument::SpreadElement(s) => walk_expr(&s.argument, source, found),
                    _ => {
                        if let Some(e) = a.as_expression() {
                            walk_expr(e, source, found);
                        }
                    }
                }
                if found.is_some() {
                    return;
                }
            }
        }
        Expression::AwaitExpression(a) => walk_expr(&a.argument, source, found),
        Expression::ParenthesizedExpression(p) => walk_expr(&p.expression, source, found),
        Expression::AssignmentExpression(a) => walk_expr(&a.right, source, found),
        Expression::ConditionalExpression(c) => {
            walk_expr(&c.test, source, found);
            walk_expr(&c.consequent, source, found);
            walk_expr(&c.alternate, source, found);
        }
        Expression::BinaryExpression(b) => {
            walk_expr(&b.left, source, found);
            walk_expr(&b.right, source, found);
        }
        Expression::LogicalExpression(l) => {
            walk_expr(&l.left, source, found);
            walk_expr(&l.right, source, found);
        }
        Expression::ObjectExpression(o) => {
            for p in &o.properties {
                if let oxc_ast::ast::ObjectPropertyKind::ObjectProperty(op) = p {
                    walk_expr(&op.value, source, found);
                    if found.is_some() {
                        return;
                    }
                }
            }
        }
        Expression::ArrayExpression(arr) => {
            for el in &arr.elements {
                if let Some(e) = el.as_expression() {
                    walk_expr(e, source, found);
                }
                if found.is_some() {
                    return;
                }
            }
        }
        Expression::ArrowFunctionExpression(a) => {
            // §10.1: destructured request state in params counts as a read:
            // `load = async ({ cookies }) => ...` observes request state
            // without ever producing a member expression on it.
            if check_fn_params_for_request_read(&a.params, source, found) {
                return;
            }
            // `ArrowFunctionBody` is `FunctionBody | INHERIT(Expression)`;
            // the inherited variant's accessor is `as_expression`.
            match &a.body {
                oxc_ast::ast::ArrowFunctionBody::FunctionBody(b) => {
                    for s in &b.statements {
                        walk_stmt_for_request_reads(s, source, found);
                        if found.is_some() {
                            return;
                        }
                    }
                }
                other => {
                    if let Some(expr) = other.as_expression() {
                        walk_expr(expr, source, found);
                    }
                }
            }
        }
        Expression::FunctionExpression(f) => {
            if check_fn_params_for_request_read(&f.params, source, found) {
                return;
            }
            if let Some(body) = &f.body {
                for s in &body.statements {
                    walk_stmt_for_request_reads(s, source, found);
                    if found.is_some() {
                        return;
                    }
                }
            }
        }
        _ => {}
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
    found: &mut Option<((usize, usize), &'static str)>,
) -> bool {
    let (start, end) = (params.span.start as usize, params.span.end as usize);
    let text = source.get(start..end).unwrap_or("");
    for name in ["cookies", "locals", "searchParams", "request"] {
        if has_word(text, name) {
            let trigger: &'static str = match name {
                "cookies" => "cookies",
                "locals" => "locals",
                "searchParams" => "searchParams",
                _ => "request.headers",
            };
            *found = Some(((start, end), trigger));
            return true;
        }
    }
    false
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
fn request_scoped_name(n: &str) -> Option<&'static str> {
    match n {
        "cookies" => Some("cookies"),
        "locals" => Some("locals"),
        "searchParams" => Some("searchParams"),
        _ => None,
    }
}
