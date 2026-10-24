//! End-to-end: the framework config cache is keyed on the **disk**
//! generation (WP-r9-5, spec §13.3).
//!
//! Why this is its own binary, not a case in `hmr_e2e.rs`: the scenario
//! needs `OXIDE_ROOT` pointed at a throwaway tree so the worker reads a
//! generation file this test controls. `OXIDE_ROOT` is process-global and
//! is read by the worker at spawn, so a test that sets it can leak into any
//! other test spawning a worker in the same process. Isolating it to its
//! own binary makes the hazard structural rather than a discipline.
//!
//! Requires Node on PATH; `#[ignore]` by default because it spawns a worker.
//! Run explicitly:
//!     cargo test -p oxide-jshost --test config_cache_e2e -- --ignored

use oxide_fw_jshost::{FrameworkHost, JsHost, NodeHost, Pool, PoolConfig};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

fn tmp_base() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/.tmp-config-cache");
    fs::create_dir_all(&d).expect("mkdir tmp base");
    d
}

/// `OXIDE_ROOT` is process-global and inherited by the worker at spawn.
/// This guard restores the previous value on drop, so a panicking assertion
/// cannot leave it pointing at a directory this test later deletes.
struct OxideRootGuard(Option<std::ffi::OsString>);

impl OxideRootGuard {
    fn set(path: &std::path::Path) -> Self {
        let prev = std::env::var_os("OXIDE_ROOT");
        unsafe { std::env::set_var("OXIDE_ROOT", path) };
        Self(prev)
    }
}

impl Drop for OxideRootGuard {
    fn drop(&mut self) {
        unsafe {
            match self.0.take() {
                Some(v) => std::env::set_var("OXIDE_ROOT", v),
                None => std::env::remove_var("OXIDE_ROOT"),
            }
        }
    }
}

struct Fixture {
    root: PathBuf,
    config: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tmp_base().join(format!(
            "root-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(root.join(".oxide")).expect("mkdir .oxide");
        let config = root.join("oxide.config.mjs");
        Self { root, config }
    }

    fn set_generation(&self, g: u64) {
        fs::write(self.root.join(".oxide/hmr-generation"), g.to_string()).expect("write gen");
    }

    fn write_config(&self, framework_name: &str) {
        fs::write(
            &self.config,
            format!(
                "export default {{ framework: {{ name: '{framework_name}', \
                 extensions: ['.tsx'], capabilities: {{}} }} }};\n"
            ),
        )
        .expect("write config");
    }

    fn config_str(&self) -> String {
        self.config.to_string_lossy().to_string()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// WP-r9-5: `loadFrameworkConfig` keys its cache on
/// `${path}:gen${currentGeneration()}`, and `currentGeneration()` reads
/// `.oxide/hmr-generation` fresh.
///
/// That is the fix for the pool-wide staleness bug: `framework:invalidate`
/// bumps an in-memory counter on exactly one worker, so with N workers the
/// other N-1 kept the stale config. A disk read means every worker sees
/// every edit.
///
/// This test never calls `invalidate()`, so the in-memory counter never
/// moves — only the file does. A cache keyed on the in-memory counter
/// returns the stale descriptor at step 3 and fails the assertion.
#[tokio::test]
#[ignore]
async fn config_cache_is_keyed_on_the_disk_generation() {
    let fx = Fixture::new();
    fx.set_generation(1);
    fx.write_config("v1");

    // Must be set before the worker spawns — the worker computes its
    // generation-file path once, at module load.
    let _root = OxideRootGuard::set(&fx.root);

    let host = NodeHost::new(NodeHost::worker_script_default());
    let pool: Arc<dyn Pool> = host
        .tooling_pool(PoolConfig::default())
        .expect("spawn node worker");
    let framework = FrameworkHost::new(Arc::clone(&pool));
    let path = fx.config_str();

    // 1. gen 1, config v1 -> v1, cached under `<path>:gen1`.
    let d1 = framework.load_config(&path).await.expect("first load");
    assert_eq!(d1.name, "v1", "initial config");

    // 2. Rewrite to v2, leave the generation alone: same key, cached, so
    //    the worker must still report v1.
    fx.write_config("v2");
    let d2 = framework
        .load_config(&path)
        .await
        .expect("second load, same generation");
    assert_eq!(
        d2.name, "v1",
        "same disk generation must hit the cache, not re-import"
    );

    // 3. Bump the generation on disk only. No `invalidate()` — the
    //    in-memory counter is untouched. The key becomes `gen2`, so the
    //    next load imports fresh and sees v2.
    fx.set_generation(2);
    let d3 = framework
        .load_config(&path)
        .await
        .expect("third load after a disk generation bump");
    assert_eq!(
        d3.name, "v2",
        "a disk generation bump must invalidate the config cache"
    );
}
