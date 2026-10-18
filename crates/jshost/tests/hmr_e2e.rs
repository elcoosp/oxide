//! End-to-end test: worker-level HMR cache invalidation (spec §13.3).
//!
//! Proves the mechanism the dev server relies on: import a module, edit the
//! file, and confirm that:
//!   - without invalidate, Node returns the cached module (no change visible)
//!   - after invalidate, the next import re-evaluates the edited file
//!
//! Requires Node on PATH; `#[ignore]` by default because it spawns a worker.
//! Run explicitly:
//!     cargo test -p oxide-jshost --test hmr_e2e -- --ignored

use oxide_jshost::{FrameworkHost, JsHost, LoaderHost, NodeHost, Pool, PoolConfig};
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

fn e2e_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/.tmp-e2e")
}

/// A monotonic counter combined with pid + nanos. Prevents the race where
/// parallel tests call SystemTime::now() within the same microsecond and
/// produce an identical nonce.
static NONCE_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn unique_nonce() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = NONCE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    format!("{pid}-{nanos}-{seq}", pid = std::process::id())
}

struct TempModule(PathBuf);

impl TempModule {
    fn new_path() -> Self {
        let dir = e2e_dir();
        fs::create_dir_all(&dir).expect("mkdir .tmp-e2e");
        Self(dir.join(format!("hmr-{}.mjs", unique_nonce())))
    }
    fn write(&self, source: &str) {
        fs::write(&self.0, source).expect("write module");
    }
    fn path_str(&self) -> String {
        self.0.to_string_lossy().to_string()
    }
}

impl Drop for TempModule {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// A loader module source with a distinctive data marker so the test can
/// prove which revision ran.
fn loader_source(marker: &str) -> String {
    format!("export const load = async () => ({{ marker: '{marker}' }});\n")
}

#[tokio::test]
#[ignore]
async fn invalidate_forces_module_reload() {
    let m = TempModule::new_path();
    m.write(&loader_source("v1"));

    // One shared pool: both hosts talk to the same worker process, so the
    // moduleGeneration counter that `FrameworkHost::invalidate()` bumps is
    // the same one that `LoaderHost::run()` observes when importing.
    let host = NodeHost::new(NodeHost::worker_script_default());
    let pool: Arc<dyn Pool> = host
        .tooling_pool(PoolConfig::default())
        .expect("spawn node worker");

    let loader = LoaderHost::new(Arc::clone(&pool));
    let framework = FrameworkHost::new(Arc::clone(&pool));

    // 1. First import: module evaluated, marker "v1".
    let out1 = loader
        .run(&m.path_str(), json!({ "route": "r", "url": "/" }))
        .await
        .expect("first load");
    assert_eq!(out1["data"]["marker"], "v1");

    // 2. Rewrite the file. Without invalidation, Node's module cache holds
    //    the old URL, so the change MUST NOT be visible yet.
    m.write(&loader_source("v2"));
    let out2 = loader
        .run(&m.path_str(), json!({ "route": "r", "url": "/" }))
        .await
        .expect("second load before invalidate");
    assert_eq!(
        out2["data"]["marker"], "v1",
        "before invalidate: cached module should still report v1"
    );

    // 3. Invalidate: the worker bumps its generation, every subsequent
    //    import is tagged with ?v=<new-gen>, Node treats it as fresh.
    let generation = framework.invalidate().await.expect("invalidate");
    assert!(
        generation >= 1,
        "generation should be >= 1, got {generation}"
    );

    // 4. Third import: new URL, fresh module, marker "v2".
    let out3 = loader
        .run(&m.path_str(), json!({ "route": "r", "url": "/" }))
        .await
        .expect("third load after invalidate");
    assert_eq!(
        out3["data"]["marker"], "v2",
        "after invalidate: reload should pick up v2"
    );
}

#[tokio::test]
#[ignore]
async fn invalidate_is_monotonic_across_calls() {
    let host = NodeHost::new(NodeHost::worker_script_default());
    let pool: Arc<dyn Pool> = host
        .tooling_pool(PoolConfig::default())
        .expect("spawn node worker");
    let framework = FrameworkHost::new(pool);

    let g1 = framework.invalidate().await.unwrap();
    let g2 = framework.invalidate().await.unwrap();
    let g3 = framework.invalidate().await.unwrap();
    assert!(g2 > g1, "g2={g2} should exceed g1={g1}");
    assert!(g3 > g2, "g3={g3} should exceed g2={g2}");
}
