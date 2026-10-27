//! §8.1 regression: bbae744 (V8 SSR lazy-entry resolver parity).
//!
//! The bug: `resolve_ssr_entry` prefers `dist/server/ssr/lazy.js`, whose
//! routes map holds `() => import('...')` loader arrows. The Node worker
//! normalizes these (awaits them) before handing the resolver to the
//! adapter. The V8 dispatcher did not: `componentResolver`'s guard
//!
//!     if (!ns || typeof ns !== 'object') return ns;
//!
//! returned the raw loader arrow as the component. React 19 treats a
//! function component that returns a thenable as an **async component**;
//! it awaited the arrow, got the route chunk's ESM namespace, and then
//! threw when trying to render that namespace as a child of the layout:
//!
//!     OX-500: Objects are not valid as a React child (found: [object Module])
//!
//! Fix (`bbae744`): pre-resolve the routes this render needs before
//! constructing the resolver. This test locks that fix; if a future
//! loader refactor regresses the pre-resolve, the assertion below catches
//! it — not the CI smoke, which requires a full fixture build.
//!
//! Ignored by default: boots a V8 isolate (~1s, leaks on shutdown for the
//! same reason `snapshot_build`'s test is ignored). Run:
//!
//!     cargo test -p oxide-fw-jshost --test v8_lazy_entry_tdd -- --ignored
//!
//! No Node required — all route modules are plain ESM.

use oxide_fw_jshost::{FrameworkHost, JsHost, Pool, PoolConfig, V8Host};
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/")
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

/// Unique tempdir base under `packages/react/.tmp-e2e/` — same pattern as
/// `framework_host_e2e.rs`. The location is deliberate: route modules
/// `import { createElement } from 'react'`, and resolution walks up from
/// this dir to find the workspace's `node_modules`. A copy in `/tmp` would
/// fail to resolve `react`.
fn e2e_base() -> PathBuf {
    workspace_root().join("packages/react/.tmp-e2e")
}

static NONCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn unique_nonce() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = NONCE.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    format!("{pid}-{nanos}-{seq}", pid = std::process::id())
}

struct Cleanup(Vec<PathBuf>);
impl Drop for Cleanup {
    fn drop(&mut self) {
        for p in &self.0 {
            let _ = fs::remove_file(p);
        }
        // Best-effort: remove files, leave the .tmp-e2e dir itself
        // (shared with other tests, cleaned by future runs).
    }
}

/// A V8 pool with size 1 (matches `v8_dispatch_tdd.rs`'s convention: one
/// isolate for one test, no cross-test sharing).
fn pool_v8() -> std::sync::Arc<dyn Pool> {
    V8Host::new()
        .render_pool(PoolConfig {
            size: 1,
            ..Default::default()
        })
        .expect("spawn V8 render pool")
}

/// The fixture: a route module that renders `<h1>Home</h1>`, wrapped in
/// the lazy-entry shape the bug hit.
struct Fixture {
    config: PathBuf,
    entry: PathBuf, // lazy-shaped: routes[id] = () => import(...)
    _cleanup: Cleanup,
}

fn build_fixture(nonce: &str) -> Fixture {
    let base = e2e_base();
    fs::create_dir_all(&base).expect("mkdir .tmp-e2e");

    // Route module — real React component.
    let route_file = base.join(format!("v8lazy-route-{nonce}.mjs"));
    fs::write(
        &route_file,
        "import { createElement } from 'react'\n\
         const Home = () => createElement('h1', null, 'Home')\n\
         export default Home\n",
    )
    .expect("write route module");

    // Lazy entry: `routes[id] = () => import(...)` — this is the exact
    // shape that `dist/server/ssr/lazy.js` exports and that the V8
    // `componentResolver` used to hand to React un-normalized.
    let entry_file = base.join(format!("v8lazy-entry-{nonce}.mjs"));
    fs::write(
        &entry_file,
        format!(
            "export const routes = {{\n\
             \x20 'routes/index': () => import({route:?}),\n\
             }}\n",
            route = route_file.to_string_lossy()
        ),
    )
    .expect("write lazy entry");

    // Config that uses the real React adapter without a componentFor —
    // components must come from `input.components` via the resolver.
    let config_file = base.join(format!("v8lazy-config-{nonce}.mjs"));
    fs::write(
        &config_file,
        "import { createRealAdapter } from '../src/real-adapter.ts'\n\
         export default { framework: createRealAdapter({ ssr: 'real' }) }\n",
    )
    .expect("write config");

    Fixture {
        config: config_file.clone(),
        entry: entry_file.clone(),
        _cleanup: Cleanup(vec![config_file, entry_file, route_file]),
    }
}

// ---------------------------------------------------------------------------
// §8.1 — the regression test
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "boots a V8 isolate; run with --ignored"]
async fn lazy_entry_renders_page_through_v8_resolver() {
    let fixture = build_fixture(&unique_nonce());

    let pool = pool_v8();
    let fw = FrameworkHost::new(pool);

    let input = json!({
        "route": "routes/index",
        "url": "/",
        "params": {},
        "data": {},
        "head": {
            "title": "V8 lazy",
            "meta": [], "link": [], "style": [], "script": []
        },
        "components": { "entry": fixture.entry.to_string_lossy() },
    });

    let html = fw
        .render(&fixture.config.to_string_lossy(), input)
        .await
        .expect("framework:render on lazy entry");

    // The bug produced the error page (`Something went wrong` /
    // `[object Module]`) even though the dispatch succeeded. Success =
    // the real component rendered.
    assert!(
        html.contains("<h1>Home</h1>"),
        "V8 must pre-resolve the lazy loader and render the real page \
         (bbae744 regression); got:\n{html}"
    );

    // Negative assertions: catch the exact regression modes the bug had.
    assert!(
        !html.contains("[object Module]"),
        "lazy loader must NOT reach React as a component; got:\n{html}"
    );
    assert!(
        !html.contains("Something went wrong"),
        "no error-page fallback; got:\n{html}"
    );
    assert!(!html.contains("OX-500"), "no OX-500 marker; got:\n{html}");
}

// ---------------------------------------------------------------------------
// Eager parity (locks the OXIDE_SSR_EAGER path — the pre-bbae744 baseline)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "boots a V8 isolate; run with --ignored"]
async fn eager_entry_still_works() {
    // Same page, but as an *eager* entry: `routes[id] = { default: Page }`
    // — the shape `dist/server/ssr/index.js` emits. This shape never had
    // the bug (the resolver's `ns.default ?? ns` chain unwraps it), so
    // this test guards against a future fix breaking the working path.
    let base = e2e_base();
    fs::create_dir_all(&base).expect("mkdir .tmp-e2e");
    let nonce = unique_nonce();

    let route_file = base.join(format!("v8eager-route-{nonce}.mjs"));
    fs::write(
        &route_file,
        "import { createElement } from 'react'\n\
         const Home = () => createElement('h1', null, 'Home')\n\
         export default Home\n",
    )
    .expect("write route");

    let entry_file = base.join(format!("v8eager-entry-{nonce}.mjs"));
    fs::write(
        &entry_file,
        format!(
            "import * as route_ns from {route:?}\n\
             export const routes = {{ 'routes/index': route_ns }}\n",
            route = route_file.to_string_lossy()
        ),
    )
    .expect("write eager entry");

    let config_file = base.join(format!("v8eager-config-{nonce}.mjs"));
    fs::write(
        &config_file,
        "import { createRealAdapter } from '../src/real-adapter.ts'\n\
         export default { framework: createRealAdapter({ ssr: 'real' }) }\n",
    )
    .expect("write config");

    let _cleanup = Cleanup(vec![config_file.clone(), entry_file.clone(), route_file]);

    let pool = pool_v8();
    let fw = FrameworkHost::new(pool);

    let input = json!({
        "route": "routes/index",
        "url": "/",
        "params": {},
        "data": {},
        "head": { "title": "V8 eager", "meta": [], "link": [], "style": [], "script": [] },
        "components": { "entry": entry_file.to_string_lossy() },
    });

    let html = fw
        .render(&config_file.to_string_lossy(), input)
        .await
        .expect("framework:render on eager entry");

    assert!(
        html.contains("<h1>Home</h1>"),
        "eager entry path must remain functional; got:\n{html}"
    );
    assert!(
        !html.contains("[object Module]"),
        "no module leak; got:\n{html}"
    );
}

// ---------------------------------------------------------------------------
// Resolver miss: an entry that omits the wanted id is a diagnostic, not a panic
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "boots a V8 isolate; run with --ignored"]
async fn resolver_miss_surfaces_ox_500_not_panic() {
    let base = e2e_base();
    fs::create_dir_all(&base).expect("mkdir .tmp-e2e");
    let nonce = unique_nonce();

    // Entry that exports routes for *another* id — the wanted id is absent.
    let entry_file = base.join(format!("v8miss-entry-{nonce}.mjs"));
    fs::write(&entry_file, "export const routes = {}\n").expect("write empty entry");

    let config_file = base.join(format!("v8miss-config-{nonce}.mjs"));
    fs::write(
        &config_file,
        "import { createRealAdapter } from '../src/real-adapter.ts'\n\
         export default { framework: createRealAdapter({ ssr: 'real' }) }\n",
    )
    .expect("write config");

    let _cleanup = Cleanup(vec![config_file.clone(), entry_file.clone()]);

    let pool = pool_v8();
    let fw = FrameworkHost::new(pool);

    let input = json!({
        "route": "routes/index",
        "url": "/",
        "params": {},
        "data": {},
        "head": { "title": "miss", "meta": [], "link": [], "style": [], "script": [] },
        "components": { "entry": entry_file.to_string_lossy() },
    });

    let html = fw
        .render(&config_file.to_string_lossy(), input)
        .await
        .expect("framework:render should not error at dispatch level");

    // `strategy: 'real'` makes a missing component a diagnostic (OX-500
    // with a "no component resolved" message), not a silent blank and
    // not a crash. The pipeline promotes the marker to a 500 status.
    assert!(
        html.contains("OX-500"),
        "missing component is a diagnostic, not a panic; got:\n{html}"
    );
    assert!(
        html.contains("no component resolved") || html.contains("no component resolved for route"),
        "the diagnostic must name the missing route; got:\n{html}"
    );
}
