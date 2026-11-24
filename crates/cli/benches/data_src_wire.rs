//! §V2-F17 bench: the dev precompile pass's stamp cost — the
//! `data-ox-src` wire (`PrecompilePlan::data_ox_src`) compiles the STAMPED
//! source, so the pass's added work is the engine's per-source stamp
//! (`inject_data_ox_src`) on top of the discovery + payload shape the
//! unstamped pass already pays. The two groups isolate that delta over a
//! mixed `.vue`/`.svelte`/`.tsx` fixture set against the scripted pool.
//! Runs in test mode (`cargo test -p oxide-fw-cli --features bench-util
//! --benches -- --test` or `cargo bench -p oxide-fw-cli --features
//! bench-util`).
//!
//! The bench is gated behind `bench-util` (the V2-F18 pattern: plain
//! `cargo test` never pays the criterion build).

use criterion::{Criterion, criterion_group, criterion_main};
use oxide_fw_cli::precompile::{PrecompilePlan, precompile_entries};
use oxide_fw_compiler::native::Backend;
use oxide_fw_jshost::{CallInput, CallStream, EntryRef, Pool, PoolConfig, ScriptedPool};
use std::collections::BTreeMap;
use std::hint::black_box as bb;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// The mixed fixture set: one `.vue`, one `.svelte`, one Solid `.tsx` —
/// the three kinds the pool compiles (the engine's template scan owns the
/// first two, the JSX visitor the third).
fn fixture_entries(root: &std::path::Path) -> BTreeMap<String, PathBuf> {
    let vue = root.join("src/routes/index/page.vue");
    std::fs::create_dir_all(vue.parent().unwrap()).expect("mkdir vue");
    std::fs::write(&vue, "<template><main><p>hi</p></main></template>").expect("write vue");
    let svelte = root.join("src/routes/other/page.svelte");
    std::fs::create_dir_all(svelte.parent().unwrap()).expect("mkdir svelte");
    std::fs::write(
        &svelte,
        "<script>\nlet n = 0;\n</script>\n<section><span>{n}</span></section>\n",
    )
    .expect("write svelte");
    let tsx = root.join("src/routes/island.tsx");
    std::fs::write(
        &tsx,
        "export default function Island() {\n  return <main>hi</main>;\n}\n",
    )
    .expect("write tsx");
    let mut entries = BTreeMap::new();
    entries.insert("routes/index".to_string(), vue);
    entries.insert("routes/other".to_string(), svelte);
    entries.insert("routes/island".to_string(), tsx);
    entries
}

/// The scripted pool answers `{"js": "pool js"}` — the bench measures the
/// CLI-side pass (discovery, stamp, payload), never a real worker compile.
fn scripted_pool() -> Arc<dyn Pool> {
    Arc::new(ScriptedPool::new(
        Arc::new(|_entry: EntryRef, _req: CallInput| {
            Ok(CallStream::Complete(bytes::Bytes::from(
                serde_json::json!({ "js": "pool js" }).to_string(),
            )))
        }),
        PoolConfig::default(),
    ))
}

fn bench_data_src_wire(c: &mut Criterion) {
    let dir = tempfile::tempdir().expect("tempdir");
    let entries = fixture_entries(dir.path());
    let pool = scripted_pool();
    let seen: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
    let _ = seen; // the scripted pool above discards payloads; the stamp's
    // cost is paid BEFORE the call either way.
    let src_root = dir.path().join("src");
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");

    let mut group = c.benchmark_group("data_src_wire");
    group.bench_function("dev_pass_stamped", |b| {
        b.iter(|| {
            let out = rt
                .block_on(precompile_entries(PrecompilePlan {
                    pool: Some(&pool),
                    entries: &entries,
                    svelte_generate: "dom",
                    framework: Some("solid"),
                    cache: None,
                    tool_version: "0.0.0-test",
                    native: None,
                    backend: Backend::Auto,
                    data_ox_src: Some(&src_root),
                }))
                .expect("the dev pass compiles");
            bb(out.len());
        });
    });
    group.bench_function("dev_pass_unstamped", |b| {
        b.iter(|| {
            let out = rt
                .block_on(precompile_entries(PrecompilePlan {
                    pool: Some(&pool),
                    entries: &entries,
                    svelte_generate: "dom",
                    framework: Some("solid"),
                    cache: None,
                    tool_version: "0.0.0-test",
                    native: None,
                    backend: Backend::Auto,
                    data_ox_src: None,
                }))
                .expect("the dev pass compiles");
            bb(out.len());
        });
    });
    group.finish();
}

criterion_group!(benches, bench_data_src_wire);
criterion_main!(benches);
