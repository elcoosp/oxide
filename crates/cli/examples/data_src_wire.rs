//! §V2-F17 consumer example: a V8-free walkthrough of the `data-ox-src`
//! compile-path wire — the milestone that connects the compiler engine's
//! DOM→source stamp (`inject_data_ox_src`) to the two DEV compile paths the
//! pool compiles through, so the overlay's cost data can map a rendered DOM
//! element back to its component.
//!
//! The wire has two halves, both exercised here against the scripted pool
//! (no Node, no pnpm, no V8):
//!
//! 1. **The dev SSR pass** — `PrecompilePlan::data_ox_src: Some(<root>/src)`
//!    (the `dev_component_sources` shim's shape): `precompile_entries`
//!    stamps every `.vue`/`.svelte`/Solid-`.tsx` job's source BEFORE the
//!    compile, so the pool's output — and the DOM it renders — carries
//!    `data-ox-src="src/<rel>:1"`, the cost endpoint's `?src=` spelling.
//! 2. **The dev client pass** — the ESM dev server's pool-compile arm
//!    stamps the same way (the engine call the site makes), shown here by
//!    driving `inject_data_ox_src` directly the way the site does.
//!
//! The production shape (`data_ox_src: None`) is printed alongside: the
//! build pass never stamps, and the stamped/unstamped CAS keys stay
//! separate.
//!
//! ```text
//! cargo run -p oxide-fw-cli --no-default-features --example data_src_wire
//! ```

use oxide_fw_cli::precompile::{PrecompilePlan, precompile_entries};
use oxide_fw_compiler::inject_data_ox_src;
use oxide_fw_compiler::native::Backend;
use oxide_fw_jshost::{CallInput, CallStream, EntryRef, Pool, PoolConfig, ScriptedPool};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// The mixed fixture set: one `.vue`, one `.svelte`, one Solid `.tsx` —
/// the three kinds the pool compiles in dev.
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

/// The scripted pool prints the source each compile request carries, so
/// the walkthrough shows the stamped bytes the real worker would compile.
fn printing_pool(seen: Arc<Mutex<Vec<serde_json::Value>>>) -> Arc<dyn Pool> {
    Arc::new(ScriptedPool::new(
        Arc::new(move |_entry: EntryRef, req: CallInput| {
            seen.lock().unwrap().push(req.payload.clone());
            Ok(CallStream::Complete(bytes::Bytes::from(
                serde_json::json!({ "js": "pool js" }).to_string(),
            )))
        }),
        PoolConfig::default(),
    ))
}

fn sources_of(seen: &[serde_json::Value]) -> Vec<String> {
    seen.iter()
        .filter_map(|p| p.get("source").and_then(|v| v.as_str()))
        .map(str::to_string)
        .collect()
}

fn main() {
    let dir = tempfile::tempdir().expect("tempdir");
    let entries = fixture_entries(dir.path());
    let src_root = dir.path().join("src");
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");

    println!("== §V2-F17: the dev SSR pass compiles the STAMPED source ==");
    let seen: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
    let pool = printing_pool(Arc::clone(&seen));
    let stamped = rt
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
    for (path, js) in &stamped {
        println!("  compiled {} -> {}", path.display(), js);
    }
    for src in sources_of(&seen.lock().unwrap()) {
        let marker = if src.contains("data-ox-src") {
            "STAMPED"
        } else {
            "unstamped"
        };
        println!("  [{}] {}", marker, src.replace('\n', "\\n"));
    }

    println!();
    println!("== the production shape (data_ox_src: None) never stamps ==");
    let seen: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
    let pool = printing_pool(Arc::clone(&seen));
    rt.block_on(precompile_entries(PrecompilePlan {
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
    .expect("the production pass compiles");
    for src in sources_of(&seen.lock().unwrap()) {
        let marker = if src.contains("data-ox-src") {
            "STAMPED"
        } else {
            "unstamped"
        };
        println!("  [{}] {}", marker, src.replace('\n', "\\n"));
    }

    println!();
    println!("== the dev client pass stamps the same way (the ESM arm) ==");
    // The ESM dev server's pool-compile arm drives the engine directly
    // with the `?src=` spelling before handing the payload to the pool —
    // the same call shape the SSR pass rides through `stamped_source`.
    let vue = entries.get("routes/index").expect("vue entry");
    let src = std::fs::read_to_string(vue).expect("read vue");
    let stamped_client = inject_data_ox_src(&src, "vue", "src/routes/index/page.vue")
        .expect("the engine never errors")
        .expect("the template carries a stampable element");
    println!("  [STAMPED] {}", stamped_client.replace('\n', "\\n"));
    // Idempotent: re-stamping an already-stamped source is a no-op (the
    // HMR re-serve reads fresh bytes per request and stamps again).
    let restamped = inject_data_ox_src(&stamped_client, "vue", "src/routes/index/page.vue")
        .expect("the engine never errors");
    println!("  re-stamp is a no-op: {}", restamped.is_none());
}
