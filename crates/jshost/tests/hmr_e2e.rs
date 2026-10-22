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

/// A content change is visible on the **next** load, without an explicit
/// `invalidate()`.
///
/// This test previously asserted the opposite — that the change stayed
/// invisible until `invalidate()` bumped the generation. That was true when
/// the worker imported the source path directly and relied on the
/// `?v=<generation>` tag to defeat Node's URL-keyed module cache. B-13
/// replaced that with a content-hashed temp file
/// (`.{base}.{hash}.oxide.ts`), so a changed file produces a different URL
/// and Node imports it fresh on its own; the generation tag is no longer
/// what makes an edit visible.
///
/// The generation is still bumped and still monotonic — that is covered by
/// `invalidate_is_monotonic_across_calls` — so this test asserts the
/// property that actually matters today: no stale read after a rewrite.
#[tokio::test]
#[ignore]
async fn content_change_is_visible_on_the_next_load() {
    let m = TempModule::new_path();
    m.write(&loader_source("v1"));

    let host = NodeHost::new(NodeHost::worker_script_default());
    let pool: Arc<dyn Pool> = host
        .tooling_pool(PoolConfig::default())
        .expect("spawn node worker");

    let loader = LoaderHost::new(Arc::clone(&pool));

    // 1. First import: module evaluated, marker "v1".
    let out1 = loader
        .run(&m.path_str(), json!({ "route": "r", "url": "/" }))
        .await
        .expect("first load");
    assert_eq!(out1["data"]["marker"], "v1");

    // 2. Rewrite the file and load again — the content hash changed, so the
    //    worker compiles to a new temp URL and Node evaluates it. No
    //    invalidation step is required for correctness.
    m.write(&loader_source("v2"));
    let out2 = loader
        .run(&m.path_str(), json!({ "route": "r", "url": "/" }))
        .await
        .expect("second load after rewrite");
    assert_eq!(
        out2["data"]["marker"], "v2",
        "a rewritten module must not be served from a stale compile"
    );

    // 3. A third identical load is still v2 — the cache is keyed on content,
    //    not on call count, so it stays consistent.
    let out3 = loader
        .run(&m.path_str(), json!({ "route": "r", "url": "/" }))
        .await
        .expect("third load, unchanged");
    assert_eq!(out3["data"]["marker"], "v2");
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
