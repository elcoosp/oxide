//! TDD suite for oxide-fw-bundler (spec §6.4): resolution reuse, route-based code
//! splitting, shared-chunk heuristic, content-hashed output, client/server
//! manifests, the §6.4 server-only guard, cycles, externals, minify/sourcemap.

use oxide_fw_bundler::{
    BundleEnv, BundleOptions, BundleOutput, Bundler, Chunk, ChunkBundler, ChunkKind,
    ClientManifest, ServerManifest,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

// ------------------------------------------------------------------ fixtures

fn write(root: &Path, rel: &str, content: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

const CARD_V1: &str = "\
import \"./card.css\";
// MARKER-COMMENT-XYZ: stripped by minify
export function Card(label: string) {
  return { tag: \"div\", label };
}
";

/// The §6.4 fixture tree from the task brief.
fn fixture(root: &Path) {
    write(
        root,
        "src/routes/index/page.tsx",
        "\
import { Card } from \"../../components/Card.tsx\";
import { session } from \"./x.server.ts\";
export const meta = { title: \"Home\" };
export default function Page() {
  return Card(\"home:\" + session());
}
",
    );
    write(
        root,
        "src/routes/about/page.tsx",
        "\
import { Card } from \"../../components/Card.tsx\";
export const meta = { title: \"About\" };
export default function About() {
  return Card(\"about\");
}
",
    );
    write(root, "src/components/Card.tsx", CARD_V1);
    write(
        root,
        "src/components/card.css",
        ".card { color: rebeccapurple; }\n",
    );
    write(
        root,
        "src/routes/index/x.server.ts",
        "\
export function session() {
  return \"sess-1\";
}
",
    );
    write(
        root,
        "src/routes/index/page.server.ts",
        "\
import { db } from \"../../db.server.ts\";
export const loader = () => db.query(\"index\");
",
    );
    write(
        root,
        "src/db.server.ts",
        "\
export const db = { query: (q: string) => [\"row:\" + q] };
",
    );
    write(
        root,
        "src/routes/api/route.ts",
        "\
export const GET = () => ({ ok: true });
",
    );
    write(
        root,
        "node_modules/pkg/package.json",
        "{ \"name\": \"pkg\", \"version\": \"1.0.0\", \"exports\": \
         { \".\": { \"import\": \"./esm.js\", \"default\": \"./cjs.js\" } } }\n",
    );
    write(
        root,
        "node_modules/pkg/esm.js",
        "export const pkg = \"esm\";\n",
    );
    write(
        root,
        "node_modules/pkg/cjs.js",
        "module.exports = { pkg: \"cjs\" };\n",
    );
}

const CLIENT_ENTRIES: &[(&str, &str)] = &[
    ("index", "src/routes/index/page.tsx"),
    ("about", "src/routes/about/page.tsx"),
];
const SSR_ENTRIES: &[(&str, &str)] = &[
    ("index", "src/routes/index/page.server.ts"),
    ("api", "src/routes/api/route.ts"),
];

fn opts(root: &Path, env: BundleEnv, entries: &[(&str, &str)]) -> BundleOptions {
    BundleOptions {
        env,
        entries: entries
            .iter()
            .map(|(r, p)| (r.to_string(), root.join(p)))
            .collect(),
        out_dir: root.join("dist"),
        minify: false,
        sourcemap: false,
        externals: Vec::new(),
        define: BTreeMap::new(),
        tsconfig_paths: BTreeMap::new(),
        build_secret: "test-secret".to_string(),
        precompiled: BTreeMap::new(),
        jsx_import_source: None,
        transform_hook: None,
        resolve_hook: None,
        load_hook: None,
        render_chunk_hook: None,
        generate_bundle_hook: None,
        transform_html_hook: None,
        css_transform: None,
        inline_dynamic_imports: false,
        resolve_conditions: None,
        virtual_modules: Default::default(),
    }
}

/// Happy-path bundle: asserts no diagnostics leaked.
fn bundle(root: &Path, env: BundleEnv, entries: &[(&str, &str)]) -> BundleOutput {
    let out = ChunkBundler::new(root)
        .bundle(&opts(root, env, entries))
        .unwrap();
    assert!(
        out.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        out.diagnostics
    );
    out
}

fn chunk_named<'a>(out: &'a BundleOutput, name: &str) -> &'a Chunk {
    out.chunks.iter().find(|c| c.name == name).unwrap()
}

fn shared_chunk(out: &BundleOutput) -> &Chunk {
    out.chunks
        .iter()
        .find(|c| c.kind == ChunkKind::Shared)
        .unwrap()
}

fn body(root: &Path, file: &str) -> String {
    std::fs::read_to_string(root.join("dist").join(file)).unwrap()
}

// ------------------------------------------------------- §6.4 server guard

#[test]
fn client_bundle_excludes_server_only_modules_entirely() {
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    let out = bundle(dir.path(), BundleEnv::Client, CLIENT_ENTRIES);
    assert!(
        !out.modules
            .iter()
            .any(|m| m.server_only || m.resolved.to_string_lossy().contains(".server.")),
        "server modules leaked into the client graph"
    );
    assert!(!out.chunks.iter().any(|c| {
        c.modules
            .iter()
            .any(|p| p.to_string_lossy().contains(".server."))
    }));
    // the importer survives, but its `*.server` import statement is stripped
    assert!(
        out.modules
            .iter()
            .any(|m| m.resolved.ends_with("routes/index/page.tsx"))
    );
    let src = body(dir.path(), &chunk_named(&out, "index").file);
    assert!(
        !src.contains("x.server"),
        "client output leaked a server import:\n{src}"
    );
}

#[test]
fn ssr_bundle_includes_server_modules_and_writes_adapter() {
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    let out = bundle(dir.path(), BundleEnv::Ssr, SSR_ENTRIES);
    assert!(out.client_manifest.is_none());
    let sm = out.server_manifest.as_ref().unwrap();
    assert_eq!(sm.adapter_entry, "ssr/index.js");
    for p in ["page.server.ts", "db.server.ts"] {
        assert!(
            out.modules
                .iter()
                .any(|m| m.resolved.to_string_lossy().ends_with(p)),
            "ssr bundle is missing {p}"
        );
    }
    let adapter = std::fs::read_to_string(dir.path().join("dist/ssr/index.js")).unwrap();
    assert!(adapter.contains("export const routes"));
    for route in ["index", "api"] {
        let c = chunk_named(&out, route);
        assert_eq!(sm.routes.get(route).unwrap().module, c.file);
        // adapter lives in dist/ssr, chunks in dist — the path must climb one level
        assert!(
            adapter.contains(&format!("\"../{}\"", c.file)),
            "adapter: {adapter}"
        );
    }
}

// ------------------------------------------------------- code splitting

#[test]
fn module_used_by_two_entries_lands_in_shared_chunk() {
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    let out = bundle(dir.path(), BundleEnv::Client, CLIENT_ENTRIES);
    let shared = shared_chunk(&out);
    assert_eq!(shared.name, "shared-0");
    assert!(
        shared
            .modules
            .iter()
            .any(|p| p.ends_with("components/Card.tsx"))
    );
    // exactly one shared chunk: the css rides with Card's chunk, not its own
    assert_eq!(
        out.chunks
            .iter()
            .filter(|c| c.kind == ChunkKind::Shared)
            .count(),
        1
    );
    assert_eq!(shared.css.len(), 1);
    assert!(shared.css[0].ends_with(".css"));
    // shared chunks re-export their scanned binding names for cross-chunk imports
    let sbody = body(dir.path(), &shared.file);
    assert!(
        sbody.contains("export { Card }"),
        "shared chunk body:\n{sbody}"
    );
}

#[test]
fn single_entry_bundle_has_no_shared_chunk() {
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    let out = bundle(
        dir.path(),
        BundleEnv::Client,
        &[("index", "src/routes/index/page.tsx")],
    );
    assert!(!out.chunks.iter().any(|c| c.kind == ChunkKind::Shared));
    let entry = chunk_named(&out, "index");
    assert!(
        entry
            .modules
            .iter()
            .any(|p| p.ends_with("components/Card.tsx"))
    );
}

#[test]
fn entry_chunk_imports_its_shared_chunk_file() {
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    let out = bundle(dir.path(), BundleEnv::Client, CLIENT_ENTRIES);
    let shared = shared_chunk(&out);
    let entry = chunk_named(&out, "index");
    assert_eq!(entry.imports, vec![shared.file.clone()]);
    let src = body(dir.path(), &entry.file);
    assert!(
        src.contains(&format!("import \"./{}\"", shared.file)),
        "entry body:\n{src}"
    );
    // cross-chunk binding rewrite: `from "./shared-0-<hash>.js"`
    assert!(
        src.contains(&format!("from \"./{}\"", shared.file)),
        "entry body:\n{src}"
    );
}

#[test]
fn chunks_and_css_files_are_written_non_empty_with_hashed_names() {
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    let out = bundle(dir.path(), BundleEnv::Client, CLIENT_ENTRIES);
    for c in &out.chunks {
        assert!(!dir.path().join("dist").join(&c.file).is_dir());
        let js = std::fs::read(dir.path().join("dist").join(&c.file)).unwrap();
        assert!(!js.is_empty(), "chunk {} is empty", c.file);
        for f in &c.css {
            assert!(
                !std::fs::read(dir.path().join("dist").join(f))
                    .unwrap()
                    .is_empty()
            );
        }
    }
    let entry = chunk_named(&out, "index");
    assert_eq!(entry.file.split('-').next().unwrap(), "index");
    assert!(entry.file.ends_with(".js"));
    assert_eq!(entry.hash.len(), 12);
    assert!(entry.hash.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn js_bytes_equals_sum_of_reachable_chunk_files() {
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    let out = bundle(dir.path(), BundleEnv::Client, CLIENT_ENTRIES);
    let man = out.client_manifest.as_ref().unwrap();
    let by_file: BTreeMap<&str, &Chunk> = out.chunks.iter().map(|c| (c.file.as_str(), c)).collect();
    for (route, rb) in &man.routes {
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut stack = vec![rb.entry_chunk.clone()];
        let mut total = 0u64;
        while let Some(f) = stack.pop() {
            if !seen.insert(f.clone()) {
                continue;
            }
            total += std::fs::metadata(dir.path().join("dist").join(&f))
                .unwrap()
                .len();
            if let Some(c) = by_file.get(f.as_str()) {
                stack.extend(c.imports.iter().cloned());
            }
        }
        assert!(total > 0);
        assert_eq!(
            rb.js_bytes, total,
            "route {route}: js_bytes must be the reachable bytes"
        );
    }
}

// ------------------------------------------------------- content hashing

#[test]
fn chunk_hashes_and_build_id_stable_across_runs() {
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    let out1 = bundle(dir.path(), BundleEnv::Client, CLIENT_ENTRIES);
    let out2 = bundle(dir.path(), BundleEnv::Client, CLIENT_ENTRIES);
    let files1: Vec<&str> = out1.chunks.iter().map(|c| c.file.as_str()).collect();
    let files2: Vec<&str> = out2.chunks.iter().map(|c| c.file.as_str()).collect();
    assert_eq!(files1, files2);
    assert_eq!(
        out1.client_manifest.as_ref().unwrap().build_id,
        out2.client_manifest.as_ref().unwrap().build_id
    );
}

#[test]
fn hash_changes_after_editing_shared_module_source() {
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    let out1 = bundle(dir.path(), BundleEnv::Client, CLIENT_ENTRIES);
    let shared1 = shared_chunk(&out1).file.clone();
    let id1 = out1.client_manifest.as_ref().unwrap().build_id.clone();
    write(
        dir.path(),
        "src/components/Card.tsx",
        "export function Card(label: string) {\n  return { tag: \"div\", label, v2: true };\n}\n",
    );
    let out2 = bundle(dir.path(), BundleEnv::Client, CLIENT_ENTRIES);
    let shared2 = shared_chunk(&out2).file.clone();
    let id2 = out2.client_manifest.as_ref().unwrap().build_id.clone();
    assert_ne!(
        shared1, shared2,
        "shared chunk file must be re-hashed after an edit"
    );
    assert_ne!(id1, id2, "build_id must change when any chunk hash changes");
}

#[test]
fn content_hash_unit_stability_and_sensitivity() {
    let a = ChunkBundler::content_hash(&[b"alpha", b"beta"]);
    assert_eq!(a, ChunkBundler::content_hash(&[b"alpha", b"beta"]));
    assert_ne!(a, ChunkBundler::content_hash(&[b"beta", b"alpha"]));
    assert_ne!(a, ChunkBundler::content_hash(&[b"alphabeta"]));
    assert_eq!(a.len(), 12);
    assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
}

// ------------------------------------------------------- manifests

#[test]
fn manifests_persisted_and_roundtrip_to_returned_values() {
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    let out_c = bundle(dir.path(), BundleEnv::Client, CLIENT_ENTRIES);
    let disk_c: ClientManifest = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("dist/client.manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(disk_c, out_c.client_manifest.unwrap());
    assert!(!dir.path().join("dist/server.manifest.json").exists());

    let dir2 = tempfile::tempdir().unwrap();
    fixture(dir2.path());
    let out_s = bundle(dir2.path(), BundleEnv::Ssr, SSR_ENTRIES);
    let disk_s: ServerManifest = serde_json::from_str(
        &std::fs::read_to_string(dir2.path().join("dist/server.manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(disk_s, out_s.server_manifest.unwrap());
    assert!(!dir2.path().join("dist/client.manifest.json").exists());
}

#[test]
fn build_id_is_16_hex_chars() {
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    let out = bundle(dir.path(), BundleEnv::Client, CLIENT_ENTRIES);
    let id = &out.client_manifest.unwrap().build_id;
    assert_eq!(id.len(), 16);
    assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn css_module_is_emitted_mapped_and_preloaded() {
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    let out = bundle(dir.path(), BundleEnv::Client, CLIENT_ENTRIES);
    let man = out.client_manifest.as_ref().unwrap();
    let shared = shared_chunk(&out);
    let css_file = &shared.css[0];
    assert!(dir.path().join("dist").join(css_file).is_file());
    assert_eq!(
        man.assets
            .get("src/components/card.css")
            .or_else(|| man.assets.get(r"src\components\card.css"))
            .unwrap(),
        css_file
    );
    let rb = man.routes.get("index").unwrap();
    assert!(
        rb.css.contains(css_file),
        "route css must include shared chunk css"
    );
    // the JS body no longer imports the css directly — it rides the chunk record
    let sbody = body(dir.path(), &shared.file);
    assert!(!sbody.contains("card.css"), "shared chunk body:\n{sbody}");
}

// ------------------------------------------------------- failure tolerance

#[test]
fn import_cycle_completes_with_bld_cycle_diagnostic() {
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    write(
        dir.path(),
        "src/cyc/a.ts",
        "import { b } from \"./b.ts\";\nexport const a = () => \"a\" + b();\n",
    );
    write(
        dir.path(),
        "src/cyc/b.ts",
        "import { a } from \"./a.ts\";\nexport const b = () => \"b\";\n",
    );
    let out = ChunkBundler::new(dir.path())
        .bundle(&opts(
            dir.path(),
            BundleEnv::Client,
            &[("cyc", "src/cyc/a.ts")],
        ))
        .unwrap();
    assert!(
        out.diagnostics
            .iter()
            .any(|d| d.code == "BLD-CYCLE" && d.message.contains("->"))
    );
    // all modules still placed (lexicographic fallback order), output non-empty
    let entry = chunk_named(&out, "cyc");
    assert!(entry.modules.iter().any(|p| p.ends_with("a.ts")));
    assert!(entry.modules.iter().any(|p| p.ends_with("b.ts")));
    assert!(!body(dir.path(), &entry.file).is_empty());
}

#[test]
fn unresolved_bare_import_yields_bld_resolve_but_completes() {
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    write(
        dir.path(),
        "src/solo.ts",
        "import { thing } from \"missing-pkg\";\nexport const solo = thing;\n",
    );
    let out = ChunkBundler::new(dir.path())
        .bundle(&opts(
            dir.path(),
            BundleEnv::Client,
            &[("solo", "src/solo.ts")],
        ))
        .unwrap();
    assert!(
        out.diagnostics
            .iter()
            .any(|d| d.code == "BLD-RESOLVE" && d.message.contains("missing-pkg"))
    );
    assert!(out.modules.iter().any(|m| m.resolved.ends_with("solo.ts")));
    let entry = chunk_named(&out, "solo");
    assert!(!body(dir.path(), &entry.file).is_empty());
}

#[test]
fn external_bare_import_is_silent_and_not_bundled() {
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    write(
        dir.path(),
        "src/solo.ts",
        "import { thing } from \"missing-pkg\";\nexport const solo = thing;\n",
    );
    let mut o = opts(dir.path(), BundleEnv::Client, &[("solo", "src/solo.ts")]);
    o.externals = vec!["missing-pkg".to_string()];
    let out = ChunkBundler::new(dir.path()).bundle(&o).unwrap();
    assert!(
        out.diagnostics.is_empty(),
        "externals must be silent: {:?}",
        out.diagnostics
    );
    assert!(
        !out.modules
            .iter()
            .any(|m| m.resolved.to_string_lossy().contains("missing"))
    );
    let src = body(dir.path(), &chunk_named(&out, "solo").file);
    assert!(
        src.contains("from \"missing-pkg\""),
        "external import preserved:\n{src}"
    );
}

#[test]
fn missing_entry_module_is_a_hard_error() {
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    let e = ChunkBundler::new(dir.path())
        .bundle(&opts(
            dir.path(),
            BundleEnv::Client,
            &[("ghost", "src/does-not-exist.ts")],
        ))
        .unwrap_err();
    assert_eq!(e.0.code, "BLD-RESOLVE");
}

// ------------------------------------------------------- minify + sourcemap

#[test]
fn sourcemap_flag_emits_map_files_and_comment() {
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    let mut o = opts(dir.path(), BundleEnv::Client, CLIENT_ENTRIES);
    o.sourcemap = true;
    let out = ChunkBundler::new(dir.path()).bundle(&o).unwrap();
    for c in &out.chunks {
        assert!(
            dir.path()
                .join("dist")
                .join(format!("{}.map", c.file))
                .is_file()
        );
        let src = body(dir.path(), &c.file);
        assert!(src.contains(&format!("//# sourceMappingURL={}.map", c.file)));
    }
}

#[test]
fn minify_strips_comments_from_chunk_output() {
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    let mut o = opts(dir.path(), BundleEnv::Client, CLIENT_ENTRIES);
    o.minify = true;
    let out = ChunkBundler::new(dir.path()).bundle(&o).unwrap();
    let src = body(dir.path(), &chunk_named(&out, "index").file);
    assert!(!src.contains("MARKER-COMMENT-XYZ"), "minified body:\n{src}");
    assert!(src.contains("function Page"));
}

// ------------------------------------------------------- resolver reuse

#[test]
fn resolve_helper_relative_tsx_and_package_exports() {
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    let b = ChunkBundler::new(dir.path());
    let from_page = dir.path().join("src/routes/index/page.tsx");
    assert_eq!(
        b.resolve(
            "../../components/Card.tsx",
            &from_page,
            BundleEnv::Client,
            &BTreeMap::new()
        )
        .unwrap(),
        dir.path().join("src/components/Card.tsx")
    );
    let from_api = dir.path().join("src/routes/api/route.ts");
    assert_eq!(
        b.resolve("pkg", &from_api, BundleEnv::Client, &BTreeMap::new())
            .unwrap(),
        dir.path().join("node_modules/pkg/esm.js")
    );
    let e = b
        .resolve(
            "no-such-pkg",
            &from_api,
            BundleEnv::Client,
            &BTreeMap::new(),
        )
        .unwrap_err();
    assert_eq!(e.0.code, "BLD-RESOLVE");
    assert!(e.0.fix.is_some());
}

// ------------------------------------------------------- §6.6 css transform

#[test]
fn css_transform_hook_rewrites_emitted_stylesheet() {
    // §6.6: the bundler hands every emitted stylesheet to `css_transform`
    // before writing it. The CLI wraps Lightning CSS in this hook, so a
    // project with nesting / unprefixed properties gets the lowered form.
    // This test uses a marker-appending hook to prove the call reaches
    // the write site and its return value is what lands on disk.
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    let mut o = opts(dir.path(), BundleEnv::Client, CLIENT_ENTRIES);
    o.css_transform = Some(oxide_fw_bundler::CssTransformHook::new(
        |path: &str, source: &str, is_module: bool, minify: bool| {
            assert!(
                path.ends_with("card.css"),
                "hook saw unexpected path: {path}"
            );
            assert!(!is_module, "card.css must not be treated as a CSS module");
            let _ = minify;
            let mut out = source.to_string();
            out.push_str("\n/* TRANSFORMED-BY-HOOK */\n");
            Ok((out, BTreeMap::new()))
        },
    ));
    let out = ChunkBundler::new(dir.path()).bundle(&o).unwrap();
    let css_file = out
        .chunks
        .iter()
        .flat_map(|c| c.css.iter())
        .next()
        .expect("a css file was emitted");
    let css = std::fs::read_to_string(dir.path().join("dist").join(css_file)).unwrap();
    assert!(
        css.contains("/* TRANSFORMED-BY-HOOK */"),
        "hook output must reach disk, got:\n{css}"
    );
    assert!(
        css.contains("rebeccapurple"),
        "original css must still be present, got:\n{css}"
    );
}

// ---- SPEC-15.2: resolve_hook + load_hook --------------------------------

#[test]
fn resolve_hook_can_alias_an_import() {
    use oxide_fw_bundler::{LoadHook, ResolveHook};
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Entry imports `./from-alias` (which does not exist on disk);
    // the resolve hook maps it to `./actual.ts`.
    write(root, "src/actual.ts", "export const x = 42;\n");
    write(
        root,
        "src/entry.ts",
        "import { x } from './from-alias';\nexport const y = x + 1;\n",
    );
    let mut opts = opts(root, BundleEnv::Client, &[("routes/index", "src/entry.ts")]);
    let actual = root.join("src/actual.ts");
    let mut resolve_calls = 0usize;
    opts.resolve_hook = Some(ResolveHook::new({
        let actual = actual.clone();
        move |spec: &str, _from: &str| {
            if spec == "./from-alias" {
                Ok(Some(actual.clone()))
            } else {
                Ok(None)
            }
        }
    }));
    let _ = &mut resolve_calls;
    // Sanity: no load hook needed
    opts.load_hook = None::<LoadHook>;
    let b = ChunkBundler::new(root);
    let out = b.bundle(&opts).expect("bundle ok");
    // The bundle wrote its chunks; read them back from disk and look for
    // the aliased source's content (42 from actual.ts).
    let joined: String = out
        .chunks
        .iter()
        .map(|c| std::fs::read_to_string(opts.out_dir.join(&c.file)).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        joined.contains("42"),
        "aliased source landed in the bundle:\n{joined}"
    );
}

#[test]
fn load_hook_overrides_an_existing_file() {
    use oxide_fw_bundler::LoadHook;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // The target file EXISTS on disk with one content; the load hook
    // overrides it with another. This proves the hook is consulted before
    // disk when both are present — the common case for a plugin that
    // synthesizes a replacement for a real file.
    write(root, "src/message.ts", "export const msg = 'from-disk';\n");
    write(
        root,
        "src/entry.ts",
        "import { msg } from './message';\nexport const s = msg;\n",
    );
    let mut opts = opts(root, BundleEnv::Client, &[("routes/index", "src/entry.ts")]);
    opts.load_hook = Some(LoadHook::new(move |id: &str| {
        if id.ends_with("message.ts") {
            Ok(Some("export const msg = 'from-plugin';\n".to_string()))
        } else {
            Ok(None)
        }
    }));
    let b = ChunkBundler::new(root);
    let out = b.bundle(&opts).expect("bundle ok");
    // `c.file` is relative to `out_dir` (e.g. `routes-index-abc.js`), so
    // join against it. `out_dir` is the tempdir's `dist/` (see `opts()`).
    let joined: String = out
        .chunks
        .iter()
        .map(|c| std::fs::read_to_string(opts.out_dir.join(&c.file)).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        joined.contains("from-plugin"),
        "plugin content replaced the on-disk content:\n{joined}"
    );
    assert!(
        !joined.contains("from-disk"),
        "on-disk content must be gone once the hook fires:\n{joined}"
    );
}

#[test]
fn load_hook_returning_none_falls_through_to_disk() {
    use oxide_fw_bundler::LoadHook;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(root, "src/message.ts", "export const msg = 'from-disk';\n");
    write(
        root,
        "src/entry.ts",
        "import { msg } from './message';\nexport const s = msg;\n",
    );
    let mut opts = opts(root, BundleEnv::Client, &[("routes/index", "src/entry.ts")]);
    // Hook returns None for every id — the bundler must read from disk.
    opts.load_hook = Some(LoadHook::new(|_id: &str| Ok(None)));
    let b = ChunkBundler::new(root);
    let out = b.bundle(&opts).expect("bundle ok");
    let joined: String = out
        .chunks
        .iter()
        .map(|c| std::fs::read_to_string(opts.out_dir.join(&c.file)).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        joined.contains("from-disk"),
        "None → fall through to disk:\n{joined}"
    );
}
