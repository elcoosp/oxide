//! Consumer example for the PWA worker emitter (§16.6 + §V2-F14): the
//! precache plan, the deterministic service-worker source and the
//! `dist/sw.js` emission — the deferred half of the offline-first module.
//!
//! Shows the three pieces the feature composes:
//!
//! 1. [`oxide_fw_bundler::pwa::ShellSource`] — the seam: the prerendered
//!    route shells this build produced (URL paths a navigation requests).
//!    The CLI's prerender walk implements it at the build call site; here
//!    a fixed set stands in for the walk.
//! 2. [`oxide_fw_bundler::pwa::plan_precache`] — the pure plan over the
//!    client manifest + the typed `sw:`/`offline:` config keys: the
//!    build-id-versioned cache name, the deduped byte-sorted precache
//!    list (the `offline.precache` breadth's own docs, implemented
//!    verbatim) and the resolved offline fallback.
//! 3. [`oxide_fw_bundler::pwa::emit_pwa`] — the writes to the dist root
//!    (`/sw.js`, the scope the navigation fallback needs, plus
//!    `/pwa-register.js`, the registration snippet the boot entry loads).
//!    The call-site gate (`offline.enabled && sw.enabled` over a produced
//!    manifest) is the CLI build's one-liner; the module itself is total.
//!
//! Run it:
//!
//! ```text
//! cargo run -p oxide-fw-bundler --example pwa_worker
//! ```

use oxide_fw_bundler::pwa::{ShellSource, emit_pwa, plan_precache, pwa_register_source, sw_source};
use oxide_fw_bundler::{ClientManifest, RouteBundle};
use oxide_fw_config::{OfflineConfig, OfflinePrecache, SwConfig};
use std::collections::{BTreeMap, BTreeSet};

/// The prerendered shells the CLI's walk would report for this build.
struct Prerendered(BTreeSet<String>);

impl ShellSource for Prerendered {
    fn shell_urls(&self) -> BTreeSet<String> {
        self.0.clone()
    }
}

fn main() {
    // A three-route manifest in the bundler's own spellings: two static
    // pages sharing a chunk, one with its own bundle, plus a global CSS
    // asset. `integrity` rides it (E04) — the precached chunks are the
    // files the manifest digests.
    let mut routes = BTreeMap::new();
    routes.insert(
        "routes/index/page".to_string(),
        RouteBundle {
            entry_chunk: "index-a1b2c3d4.js".to_string(),
            preload: vec!["shared-e5f6a7b8.js".to_string()],
            css: vec!["index-c9d0e1f2.css".to_string()],
            js_bytes: 120,
            modules: Vec::new(),
            layout_chain: Vec::new(),
        },
    );
    routes.insert(
        "routes/about/page".to_string(),
        RouteBundle {
            entry_chunk: "about-9z8y7x6w.js".to_string(),
            preload: vec!["shared-e5f6a7b8.js".to_string()],
            css: Vec::new(),
            js_bytes: 80,
            modules: Vec::new(),
            layout_chain: Vec::new(),
        },
    );
    routes.insert(
        "routes/blog/_slug_/page".to_string(),
        RouteBundle {
            entry_chunk: "blog-0a1b2c3d.js".to_string(),
            preload: Vec::new(),
            css: vec!["blog-4e5f6a7b.css".to_string()],
            js_bytes: 200,
            modules: Vec::new(),
            layout_chain: Vec::new(),
        },
    );
    let mut assets = BTreeMap::new();
    assets.insert(
        "src/styles/global.css".to_string(),
        "global-1a2b3c4d.css".to_string(),
    );
    let manifest = ClientManifest {
        build_id: "b17e5f0a4c9d2e8f".to_string(),
        routes,
        assets,
        integrity: BTreeMap::new(),
    };

    // The prerendered shells: `/`, `/about` and the `/offline` fallback
    // page this build produced. The dynamic blog route has none.
    let shells = Prerendered(BTreeSet::from([
        "/".to_string(),
        "/about".to_string(),
        "/offline".to_string(),
    ]));

    // The typed config keys the CLI forwards: the offline module on with
    // the `all` precache breadth and the outbox, the sw section at its
    // defaults (so the fallback resolves `/offline` — the build produced
    // that route).
    let offline = OfflineConfig {
        enabled: true,
        precache: OfflinePrecache::All,
        outbox: true,
    };
    let sw = SwConfig::default();

    // 1. Plan: shells + every hashed asset, sorted and deduped.
    let plan = plan_precache(&manifest, &sw, &offline, &shells);
    println!("build id:         {}", plan.build_id);
    println!("cache name:       {}", plan.cache_name);
    println!("offline fallback: {}", plan.offline_fallback);
    println!("outbox fan-out:   {}", plan.outbox);
    println!("precache ({} URLs):", plan.precache.len());
    for url in &plan.precache {
        println!("  {url}");
    }

    // 2. Emit: the worker source is deterministic — the same plan emits
    //    byte-identical bytes on every build (the reproducibility
    //    contract), so a rebuild swaps only when the build id changes.
    //    The registration snippet rides the same emission: the boot entry
    //    loads `/pwa-register.js`, so a worker without it could never
    //    register.
    let dir = tempfile::tempdir().expect("tempdir");
    emit_pwa(dir.path(), &plan).expect("emit sw.js");
    let written = std::fs::read_to_string(dir.path().join("sw.js")).expect("read sw.js");
    debug_assert_eq!(written, sw_source(&plan));
    println!(
        "\n--- emitted {} ({} bytes) ---",
        dir.path().join("sw.js").display(),
        written.len()
    );
    println!("{written}");
    let reg = std::fs::read_to_string(dir.path().join("pwa-register.js")).expect("read register");
    debug_assert_eq!(reg, pwa_register_source());
    println!(
        "--- emitted {} ({} bytes) ---",
        dir.path().join("pwa-register.js").display(),
        reg.len()
    );
    println!("{reg}");
}
