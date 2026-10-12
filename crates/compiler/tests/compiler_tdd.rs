//! TDD suite for oxide-compiler (spec §6.2–6.4, §9.3–9.4, §19.4).

use oxide_compiler::{
    guard_server_only, inject_hmr_boundary, scan_import_spans, scan_imports, server_fn_id,
    transform_server_fn_stubs, transform_use_cache, DirectiveKind, Env, ScannerTransformer,
    TransformInput, Transformer,
};
use oxide_diagnostics::Diagnostic;
use std::collections::BTreeMap;

// ---------------------------------------------------------------- lexer / scan

#[test]
fn lexer_ignores_imports_in_strings_and_templates() {
    let code = r#"const s = "import x from 'y'";
const t = `import z from 'w'`;
const u = 'import q from "w"';
"#;
    assert_eq!(scan_imports(code), Default::default());
}

#[test]
fn lexer_ignores_imports_in_comments() {
    let code = "// import x from 'y'\n/* import q from 'z' */\nconst a = 1;\n";
    assert_eq!(scan_imports(code), Default::default());
}

#[test]
fn lexer_regex_literals_do_not_confuse_the_scanner() {
    // A regex containing quotes and slashes must not open a bogus string state.
    let code = "const re = /a\"b\\/c/;\nimport x from './x';\nconst d = a / b;\n";
    let scan = scan_imports(code);
    assert_eq!(scan.imports, vec!["./x".to_string()]);
}

#[test]
fn scan_all_static_import_forms() {
    let code = r#"import './polyfill';
import react from 'react';
import { a, b as c } from './lib';
import * as ns from './ns';
export { helper } from './helper';
export * from './wild';
export * as alias from './alias';
"#;
    let scan = scan_imports(code);
    for spec in [
        "./polyfill",
        "react",
        "./lib",
        "./ns",
        "./helper",
        "./wild",
        "./alias",
    ] {
        assert!(scan.imports.iter().any(|s| s == spec), "missing {spec}");
    }
    assert!(scan.dynamic_imports.is_empty());
}

#[test]
fn scan_dynamic_imports() {
    let code = "const a = import('./a');\nload(import(\"./b\"));\nawait import('./c?raw');\n";
    let scan = scan_imports(code);
    // `./c?raw` carries a query → asset query, not a dynamic module import
    assert_eq!(scan.dynamic_imports, vec!["./a", "./b"]);
    assert!(scan.imports.is_empty());
    assert_eq!(scan.asset_queries, vec!["./c?raw".to_string()]);
}

#[test]
fn scan_asset_queries_land_separately() {
    let code = "import hero from './hero.png?w=640&format=avif';\nimport css from './app.css';\n";
    let scan = scan_imports(code);
    assert_eq!(
        scan.asset_queries,
        vec!["./hero.png?w=640&format=avif".to_string()]
    );
    assert_eq!(scan.imports, vec!["./app.css".to_string()]);
}

#[test]
fn scan_template_interpolation_nesting() {
    // The lexer must re-enter code mode inside ${}, with nested quotes and templates.
    let code = r#"const t = `outer ${ "import x from 'y'" } mid ${ `inner ${1} end` } tail`;
import real from './real';
"#;
    let scan = scan_imports(code);
    assert_eq!(scan.imports, vec!["./real".to_string()]);
}

#[test]
fn scan_import_spans_cover_statements() {
    let code = "import x from './x';\nconst y = 1;\n";
    let spans = scan_import_spans(code);
    assert_eq!(spans.len(), 1);
    assert_eq!(&code[spans[0].start..spans[0].end], "import x from './x';");
}

// ------------------------------------------------------------- server_fn_id

#[test]
fn server_fn_id_is_24_hex_and_stable() {
    let a = server_fn_id("cart.server.ts", "addToCart", "secret1");
    let b = server_fn_id("cart.server.ts", "addToCart", "secret1");
    assert_eq!(a, b);
    assert_eq!(a.len(), 24);
    assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn server_fn_id_varies_by_secret_and_export() {
    let a = server_fn_id("cart.server.ts", "addToCart", "secret1");
    let b = server_fn_id("cart.server.ts", "addToCart", "secret2");
    let c = server_fn_id("cart.server.ts", "checkout", "secret1");
    assert_ne!(a, b);
    assert_ne!(a, c);
}

// ---------------------------------------------------------- 'use cache' pass

#[test]
fn use_cache_named_function_hoists_tags_and_life() {
    let code = r#"import { db } from './db.server';

async function getPosts() {
  'use cache';
  cacheTag('posts');
  cacheLife({ revalidate: 60 });
  return db.getPosts();
}
"#;
    let (out, directives) = transform_use_cache(code, "routes/page.server.ts").unwrap();
    assert!(!out.contains("'use cache'"));
    assert!(out.contains("__oxide_cache("));
    assert!(out.contains("\"routes/page.server.ts\", \"getPosts\""));
    assert!(out.contains("tags: ['posts']") || out.contains("tags: [\'posts\']"));
    assert!(out.contains("revalidate: 60"));
    // original body statement preserved
    assert!(out.contains("return db.getPosts();"));
    // helper call is an expression statement
    assert!(out.contains("const getPosts = __oxide_cache("));
    // removed statements are gone
    assert!(!out.contains("cacheTag("));
    assert!(!out.contains("cacheLife("));
    assert_eq!(directives.len(), 1);
    assert_eq!(directives[0].kind, DirectiveKind::UseCache);
    assert_eq!(directives[0].name.as_deref(), Some("getPosts"));
    let span = &code[directives[0].start..directives[0].end];
    assert!(span.contains("async function getPosts"));
}

#[test]
fn use_cache_exported_arrow_function() {
    let code = r#"export const bar = async () => {
  'use cache';
  return fetch('/x');
};
"#;
    let (out, directives) = transform_use_cache(code, "m.ts").unwrap();
    assert!(out.contains("export const bar = __oxide_cache("));
    assert!(out.contains("\"m.ts\", \"bar\""));
    assert!(!out.contains("'use cache'"));
    assert!(out.contains("return fetch('/x');"));
    assert_eq!(directives[0].name.as_deref(), Some("bar"));
}

#[test]
fn use_cache_zero_tags_life_options_empty() {
    let code = "async function foo() {\n  'use cache';\n  return 1;\n}\n";
    let (out, _) = transform_use_cache(code, "a.server.ts").unwrap();
    assert!(out.contains("\"a.server.ts\", \"foo\", {}"));
    assert!(out.contains("return 1;"));
}

#[test]
fn use_cache_many_tags_and_multiple_functions() {
    let code = r#"async function a() {
  'use cache';
  cacheTag('x');
  cacheTag("y");
  return 1;
}

async function b() {
  'use cache';
  cacheLife({ revalidate: 10 });
  cacheLife({ swr: 30 });
  return 2;
}
"#;
    let (out, directives) = transform_use_cache(code, "m.server.ts").unwrap();
    assert!(out.contains("tags: ['x', \"y\"]"));
    assert!(out.contains("life: { revalidate: 10, swr: 30 }"));
    assert_eq!(directives.len(), 2);
    assert!(!out.contains("cacheTag("));
    assert!(!out.contains("cacheLife("));
}

#[test]
fn use_cache_export_decl_form() {
    let code = "export async function loader() {\n  'use cache';\n  return db.q();\n}\n";
    let (out, _) = transform_use_cache(code, "p.server.ts").unwrap();
    assert!(out.contains("export const loader = __oxide_cache(async function loader()"));
    assert!(out.contains("return db.q();"));
    assert!(!out.contains("'use cache'"));
}

#[test]
fn use_cache_unmarked_functions_untouched() {
    let code = "async function plain() {\n  const s = 'use cache';\n  return s;\n}\n";
    let (out, directives) = transform_use_cache(code, "m.ts").unwrap();
    // string is not the first *statement*, so nothing is rewritten
    assert_eq!(out, code);
    assert!(directives.is_empty());
}

// ------------------------------------------------------ server-fn stub pass

const SERVER_FN_SRC: &str = r#"import { serverFn } from 'oxide/server';

export const addToCart = serverFn(async (productId: string) => {
  return db.add(productId);
});

export const checkout = serverFn(async function () {
  return db.checkout();
});
"#;

#[test]
fn stubs_client_emit_fetch_stub_per_export() {
    let (out, stubs) =
        transform_server_fn_stubs(SERVER_FN_SRC, "cart.server.ts", Env::Client, "sec").unwrap();
    assert_eq!(stubs.len(), 2);
    assert_eq!(stubs[0].export_name, "addToCart");
    assert_eq!(
        stubs[0].id,
        server_fn_id("cart.server.ts", "addToCart", "sec")
    );
    // exactly one export per fn, replaced with the POST stub
    assert_eq!(out.matches("export const addToCart").count(), 1);
    assert_eq!(out.matches("export const checkout").count(), 1);
    assert!(!out.contains("serverFn("));
    for stub in &stubs {
        let needle = format!("fetch(\"/_oxide/fn/{}\"", stub.id);
        assert!(out.contains(&needle), "missing {needle}");
    }
    assert!(out.contains("__oxide_encode(args)"));
    assert!(out.contains("__oxide_decode(await r.arrayBuffer())"));
    assert!(out.contains("\"content-type\": \"application/oxide-fn\""));
}

#[test]
fn stubs_ssr_keep_bodies_and_register_ids() {
    let (out, stubs) =
        transform_server_fn_stubs(SERVER_FN_SRC, "cart.server.ts", Env::Ssr, "sec").unwrap();
    assert_eq!(stubs.len(), 2);
    // bodies kept
    assert!(out.contains("return db.add(productId);"));
    assert!(out.contains("return db.checkout();"));
    for stub in &stubs {
        assert!(out.contains(&format!(
            "__oxide_register_fn(\"{}\", {});",
            stub.id, stub.export_name
        )));
    }
    let ids: Vec<&str> = stubs.iter().map(|s| s.id.as_str()).collect();
    assert!(out.contains(&format!(
        "export const __oxide_fn_ids = [\"{}\", \"{}\"];",
        ids[0], ids[1]
    )));
}

#[test]
fn stubs_schema_first_arg_supported() {
    let code =
        "export const create = serverFn(schema, async (input) => {\n  return db.insert(input);\n});\n";
    let (out, stubs) = transform_server_fn_stubs(code, "x.server.ts", Env::Client, "s").unwrap();
    assert_eq!(stubs.len(), 1);
    assert_eq!(stubs[0].export_name, "create");
    assert!(out.contains("fetch(\"/_oxide/fn/"));
    assert!(out.contains("__oxide_encode(args)"));
    assert_eq!(out.matches("export const create").count(), 1);
}

// ------------------------------------------------------------------- guard

fn diag_code(d: &Diagnostic) -> &str {
    &d.code
}

#[test]
fn guard_clean_graph_has_no_diagnostics() {
    let mut graph = BTreeMap::new();
    graph.insert("entry.tsx".to_string(), vec!["a.ts".to_string()]);
    graph.insert("a.ts".to_string(), vec![]);
    let mut servers = BTreeMap::new();
    servers.insert("db.server.ts".to_string(), true);
    assert!(guard_server_only(&graph, &servers).is_empty());
}

#[test]
fn guard_reports_full_import_chain() {
    let mut graph = BTreeMap::new();
    graph.insert("entry.tsx".to_string(), vec!["a.ts".to_string()]);
    graph.insert("a.ts".to_string(), vec!["db.server.ts".to_string()]);
    let mut servers = BTreeMap::new();
    servers.insert("db.server.ts".to_string(), true);
    let diags = guard_server_only(&graph, &servers);
    // Literal semantics (task definition): every non-server graph key is a client
    // entry, so `a.ts` and `entry.tsx` each report the violation → 2 diagnostics.
    assert_eq!(diags.len(), 2);
    assert!(diags.iter().all(|d| diag_code(d) == "CMP-GUARD"));
    assert!(diags
        .iter()
        .any(|d| d.message.contains("entry.tsx -> a.ts -> db.server.ts")));
    assert!(diags
        .iter()
        .any(|d| d.message.contains("a.ts -> db.server.ts")));
    assert!(diags[0].fix.is_some());
}

#[test]
fn guard_multiple_violations_multiple_diagnostics() {
    let mut graph = BTreeMap::new();
    graph.insert(
        "entry.tsx".to_string(),
        vec!["a.ts".to_string(), "b.ts".to_string()],
    );
    graph.insert("a.ts".to_string(), vec!["db.server.ts".to_string()]);
    graph.insert("b.ts".to_string(), vec!["auth.server.ts".to_string()]);
    let mut servers = BTreeMap::new();
    servers.insert("db.server.ts".to_string(), true);
    servers.insert("auth.server.ts".to_string(), true);
    let diags = guard_server_only(&graph, &servers);
    // entries = a.ts, b.ts, entry.tsx → 2 + 2 = 4 diagnostics, all chains intact
    assert_eq!(diags.len(), 4);
    assert!(diags
        .iter()
        .any(|d| d.message.contains("entry.tsx -> a.ts -> db.server.ts")));
    assert!(diags
        .iter()
        .any(|d| d.message.contains("entry.tsx -> b.ts -> auth.server.ts")));
    assert!(diags.iter().any(|d| d.message.contains("db.server.ts")));
    assert!(diags.iter().any(|d| d.message.contains("auth.server.ts")));
}

// -------------------------------------------------------------------- HMR

#[test]
fn hmr_css_self_accepts() {
    let out = inject_hmr_boundary("body { color: red }", "src/app.css", "css").unwrap();
    assert!(out.contains("if (import.meta.hot) { import.meta.hot.accept(); }"));
}

#[test]
fn hmr_component_refreshes() {
    let out = inject_hmr_boundary("export default () => null", "src/page.tsx", "tsx").unwrap();
    assert!(out.contains("import.meta.hot.accept((mod) => { __oxide_refresh(mod); });"));
}

#[test]
fn hmr_server_module_swaps() {
    let out = inject_hmr_boundary("export const x = 1", "src/data.server.ts", "ts").unwrap();
    assert!(out.contains("__oxide_server_swap(\"src/data.server.ts\")"));
}

#[test]
fn hmr_is_idempotent() {
    let code = "export default () => null;";
    let once = inject_hmr_boundary(code, "src/page.tsx", "tsx").unwrap();
    let twice = inject_hmr_boundary(&once, "src/page.tsx", "tsx").unwrap();
    assert_eq!(once, twice);
    // code already using import.meta.hot is left alone
    let manual = "if (import.meta.hot) { import.meta.hot.accept(); }";
    assert_eq!(
        inject_hmr_boundary(manual, "src/a.tsx", "tsx").unwrap(),
        manual
    );
}

// ---------------------------------------------------- ScannerTransformer e2e

const PAGE_SERVER: &str = r#"import { db } from './db.server';
import { serverFn } from 'oxide/server';

export const getPosts = serverFn(async () => {
  return db.query('SELECT * FROM posts');
});

export async function cachedCount() {
  'use cache';
  cacheTag('posts');
  return db.count();
}
"#;

#[test]
fn transformer_page_server_ssr_end_to_end() {
    let input = TransformInput {
        path: "routes/page.server.ts".into(),
        code: PAGE_SERVER.to_string(),
        env: Env::Ssr,
        lang: "ts",
        build_secret: "s3cr3t".into(),
        dev: false,
    };
    let out = ScannerTransformer.transform(&input).unwrap();
    // scan
    assert!(out.scan.imports.iter().any(|s| s == "./db.server"));
    assert!(out.scan.imports.iter().any(|s| s == "oxide/server"));
    // cache directive rewritten (ssr keeps bodies)
    assert_eq!(out.directives.len(), 1);
    assert_eq!(out.directives[0].name.as_deref(), Some("cachedCount"));
    assert!(out.code.contains("__oxide_cache("));
    assert!(!out.code.contains("'use cache'"));
    assert!(out.code.contains("return db.count();"));
    // server fns registered, ids stable
    assert_eq!(out.server_fns.len(), 1);
    assert_eq!(out.server_fns[0].export_name, "getPosts");
    assert_eq!(
        out.server_fns[0].id,
        server_fn_id("routes/page.server.ts", "getPosts", "s3cr3t")
    );
    assert!(out.code.contains("__oxide_register_fn("));
    assert!(out.code.contains("__oxide_fn_ids"));
    // dev boundary not injected into server modules
    assert!(!out.code.contains("import.meta.hot"));
    assert!(out.diagnostics.is_empty());
}

#[test]
fn transformer_server_module_client_env_stubs() {
    let input = TransformInput {
        path: "routes/page.server.ts".into(),
        code: PAGE_SERVER.to_string(),
        env: Env::Client,
        lang: "ts",
        build_secret: "s3cr3t".into(),
        dev: false,
    };
    let out = ScannerTransformer.transform(&input).unwrap();
    // client env: fetch stubs, no 'use cache' handling (ssr only), no server swap
    assert!(out.code.contains("fetch(\"/_oxide/fn/"));
    assert!(!out.code.contains("__oxide_register_fn("));
    assert_eq!(out.server_fns.len(), 1);
}

#[test]
fn transformer_client_page_dev_injects_refresh() {
    let code = "import { Button } from './Button';\nimport { addToCart } from './cart.server';\n\nexport default function Page() {\n  return Button;\n}\n";
    let input = TransformInput {
        path: "routes/page.tsx".into(),
        code: code.to_string(),
        env: Env::Client,
        lang: "tsx",
        build_secret: "s3cr3t".into(),
        dev: true,
    };
    let out = ScannerTransformer.transform(&input).unwrap();
    assert!(out.scan.imports.contains(&"./Button".to_string()));
    assert!(out.scan.imports.contains(&"./cart.server".to_string()));
    assert!(out.directives.is_empty());
    assert!(out.server_fns.is_empty());
    assert!(out.code.contains("__oxide_refresh(mod);"));
    assert!(out.code.starts_with("import { Button }"));
}
