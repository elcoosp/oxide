//! TDD suite for the PWA worker emitter (the V2-F14 deferred half + the
//! §16.6 `sw:` key adoption): the precache plan, the service-worker source
//! and the `dist/sw.js` emission.
//!
//! The spec's worker contract, pinned here:
//!
//! - caches are **versioned by build id** (a new build swaps caches; the
//!   activate step deletes the stale ones);
//! - the precache breadth follows the `offline.precache` docs verbatim:
//!   `routes` = the prerendered route shells (never client chunks),
//!   `all` = every hashed asset the client manifest carries — the shell
//!   rides both, because the navigation fallback needs it offline;
//! - navigation falls back to the resolved `offlineFallback` route
//!   (`sw.offlineFallback` override → `/offline` when the build produced
//!   it → `/`, the config comment's exact order);
//! - `/__data` responses serve stale-while-revalidate;
//! - with `offline.outbox` on, the worker fans an `OXIDE_OUTBOX_FLUSH`
//!   message out to its clients on activate — the coordination point the
//!   client outbox listener (`packages/core/src/client/outbox.ts`)
//!   answers.

use oxide_fw_bundler::pwa::{PrecachePlan, ShellSource, emit_pwa, plan_precache, sw_source};
use oxide_fw_bundler::{ClientManifest, RouteBundle};
use oxide_fw_config::{OfflineConfig, OfflinePrecache, SwConfig};
use std::collections::{BTreeMap, BTreeSet};

/// A three-route manifest in the bundler's own spellings (the
/// `head_producer` example's shape): two static routes sharing a chunk
/// and one with its own bundle, plus a CSS asset.
fn manifest_fixture() -> ClientManifest {
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
    ClientManifest {
        build_id: "b17e5f0a4c9d2e8f".to_string(),
        routes,
        assets,
        integrity: BTreeMap::new(),
    }
}

/// The prerendered shells the CLI's walk would report: `/` plus the two
/// static pages. The dynamic route has no prerendered shell.
fn shells_fixture() -> BTreeSet<String> {
    BTreeSet::from([
        "/".to_string(),
        "/about".to_string(),
        "/offline".to_string(),
    ])
}

/// The `ShellSource` seam held behind `&dyn`/`Box<dyn>` — the exact shape
/// the CLI prerender walk implements and the tests inject.
struct FixedShells(BTreeSet<String>);

impl ShellSource for FixedShells {
    fn shell_urls(&self) -> BTreeSet<String> {
        self.0.clone()
    }
}

fn offline(precache: OfflinePrecache) -> OfflineConfig {
    OfflineConfig {
        enabled: true,
        precache,
        outbox: true,
    }
}

fn sw() -> SwConfig {
    SwConfig::default()
}

#[test]
fn shell_source_is_object_safe_over_a_fake() {
    // The CLI injection shape: the caller owns the walk, the emitter sees
    // the trait object.
    let boxed: Box<dyn ShellSource> = Box::new(FixedShells(shells_fixture()));
    assert!(boxed.shell_urls().contains("/about"));
}

#[test]
fn plan_routes_breadth_precaches_the_prerendered_route_shells() {
    let plan = plan_precache(
        &manifest_fixture(),
        &sw(),
        &offline(OfflinePrecache::Routes),
        &FixedShells(shells_fixture()),
    );
    // `routes` = the cached route shell + the prerendered routes — the
    // shell URLs themselves, byte-sorted, deduped.
    let expected = ["/", "/about", "/offline"]
        .iter()
        .map(|s| (*s).to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        plan.precache, expected,
        "routes breadth carries exactly the shell URLs"
    );
}

#[test]
fn plan_routes_mode_never_carries_client_chunks() {
    let plan = plan_precache(
        &manifest_fixture(),
        &sw(),
        &offline(OfflinePrecache::Routes),
        &FixedShells(shells_fixture()),
    );
    assert!(
        plan.precache.iter().all(|u| !u.starts_with("/client/")),
        "routes breadth is the navigation surface, not the JS graph"
    );
}

#[test]
fn plan_all_breadth_adds_every_hashed_manifest_asset() {
    let plan = plan_precache(
        &manifest_fixture(),
        &sw(),
        &offline(OfflinePrecache::All),
        &FixedShells(shells_fixture()),
    );
    // Every hashed asset the client manifest carries, spelled the URL the
    // head producer emits: `/client/<file>`.
    for file in [
        "index-a1b2c3d4.js",
        "about-9z8y7x6w.js",
        "blog-0a1b2c3d.js",
        "shared-e5f6a7b8.js",
        "index-c9d0e1f2.css",
        "blog-4e5f6a7b.css",
        "global-1a2b3c4d.css",
    ] {
        let url = format!("/client/{file}");
        assert!(plan.precache.contains(&url), "all breadth carries {url}");
    }
    // ...and the shells ride too (the navigation fallback needs them).
    assert!(plan.precache.contains(&"/offline".to_string()));
}

#[test]
fn plan_dedupes_and_sorts_the_precache_list() {
    // The shared chunk appears under two routes — exactly one precache
    // entry; the list stays byte-sorted for a stable diff.
    let plan = plan_precache(
        &manifest_fixture(),
        &sw(),
        &offline(OfflinePrecache::All),
        &FixedShells(shells_fixture()),
    );
    let shared = "/client/shared-e5f6a7b8.js";
    assert_eq!(
        plan.precache.iter().filter(|u| *u == shared).count(),
        1,
        "a chunk shared by two routes precaches once"
    );
    let mut sorted = plan.precache.clone();
    sorted.sort();
    assert_eq!(plan.precache, sorted, "the precache list is byte-sorted");
}

#[test]
fn cache_name_is_versioned_by_the_build_id() {
    let plan = plan_precache(
        &manifest_fixture(),
        &sw(),
        &offline(OfflinePrecache::Routes),
        &FixedShells(shells_fixture()),
    );
    assert_eq!(plan.cache_name, "ox-b17e5f0a4c9d2e8f");
    let mut other = manifest_fixture();
    other.build_id = "c29f6a1b5d0e3f90".to_string();
    let plan2 = plan_precache(
        &other,
        &sw(),
        &offline(OfflinePrecache::Routes),
        &FixedShells(shells_fixture()),
    );
    assert_ne!(
        plan.cache_name, plan2.cache_name,
        "a new build swaps caches"
    );
}

#[test]
fn offline_fallback_resolution_follows_the_config_comment_contract() {
    // 1. `/offline` when the build produced that route.
    let plan = plan_precache(
        &manifest_fixture(),
        &sw(),
        &offline(OfflinePrecache::Routes),
        &FixedShells(shells_fixture()),
    );
    assert_eq!(plan.offline_fallback, "/offline");
    // 2. otherwise `/` (the precached shell).
    let no_offline: BTreeSet<String> = BTreeSet::from(["/".to_string(), "/about".to_string()]);
    let plan = plan_precache(
        &manifest_fixture(),
        &sw(),
        &offline(OfflinePrecache::Routes),
        &FixedShells(no_offline),
    );
    assert_eq!(plan.offline_fallback, "/");
    // 3. `sw.offlineFallback` overrides both.
    let sw_cfg = SwConfig {
        offline_fallback: Some("/about".to_string()),
        ..SwConfig::default()
    };
    let plan = plan_precache(
        &manifest_fixture(),
        &sw_cfg,
        &offline(OfflinePrecache::Routes),
        &FixedShells(shells_fixture()),
    );
    assert_eq!(plan.offline_fallback, "/about");
}

#[test]
fn sw_source_is_deterministic_and_versioned() {
    let plan = plan_precache(
        &manifest_fixture(),
        &sw(),
        &offline(OfflinePrecache::Routes),
        &FixedShells(shells_fixture()),
    );
    let a = sw_source(&plan);
    let b = sw_source(&plan);
    assert_eq!(a, b, "the same plan emits byte-identical source");
    assert!(
        a.contains("ox-b17e5f0a4c9d2e8f"),
        "the cache const names the build id"
    );
}

#[test]
fn sw_source_pins_the_spec_fetch_contract() {
    let plan = plan_precache(
        &manifest_fixture(),
        &sw(),
        &offline(OfflinePrecache::Routes),
        &FixedShells(shells_fixture()),
    );
    let src = sw_source(&plan);
    assert!(src.contains("'/__data'"), "the data-plane arm exists");
    assert!(
        src.contains("stale-while-revalidate"),
        "data responses are SWR"
    );
    assert!(
        src.contains("'/client/'"),
        "hashed client assets are cache-first"
    );
    assert!(
        src.contains("'/offline'"),
        "the navigation fallback is the resolved route"
    );
    assert!(
        src.contains("caches.delete"),
        "activate deletes the stale build's cache"
    );
    assert!(
        src.contains("addAll(PRECACHE)"),
        "install precaches the plan"
    );
}

#[test]
fn sw_source_precaches_exactly_the_plan_urls() {
    let plan = plan_precache(
        &manifest_fixture(),
        &sw(),
        &offline(OfflinePrecache::All),
        &FixedShells(shells_fixture()),
    );
    let src = sw_source(&plan);
    for url in &plan.precache {
        assert!(
            src.contains(&format!("'{url}',")),
            "the PRECACHE array carries {url}"
        );
    }
}

#[test]
fn sw_source_omits_the_outbox_fanout_when_offline_outbox_is_off() {
    let mut off = offline(OfflinePrecache::Routes);
    off.outbox = false;
    let plan = plan_precache(
        &manifest_fixture(),
        &sw(),
        &off,
        &FixedShells(shells_fixture()),
    );
    assert!(!plan.outbox);
    assert!(
        !sw_source(&plan).contains("OXIDE_OUTBOX_FLUSH"),
        "outbox off: the worker does not fan the flush message"
    );
    let plan_on = plan_precache(
        &manifest_fixture(),
        &sw(),
        &offline(OfflinePrecache::Routes),
        &FixedShells(shells_fixture()),
    );
    assert!(plan_on.outbox);
    assert!(
        sw_source(&plan_on).contains("OXIDE_OUTBOX_FLUSH"),
        "outbox on: activate fans the flush message to the clients"
    );
}

#[test]
fn empty_manifest_all_mode_still_carries_the_shell() {
    let empty = ClientManifest {
        build_id: "b17e5f0a4c9d2e8f".to_string(),
        routes: BTreeMap::new(),
        assets: BTreeMap::new(),
        integrity: BTreeMap::new(),
    };
    let plan = plan_precache(
        &empty,
        &sw(),
        &offline(OfflinePrecache::All),
        &FixedShells(shells_fixture()),
    );
    let expected = ["/", "/about", "/offline"]
        .iter()
        .map(|s| (*s).to_string())
        .collect::<Vec<_>>();
    assert_eq!(plan.precache, expected);
}

#[test]
fn emit_pwa_writes_the_worker_source_to_the_dist_root() {
    let dir = tempfile::tempdir().unwrap();
    let plan = PrecachePlan {
        build_id: "b17e5f0a4c9d2e8f".to_string(),
        cache_name: "ox-b17e5f0a4c9d2e8f".to_string(),
        precache: vec!["/".to_string()],
        offline_fallback: "/".to_string(),
        outbox: true,
    };
    emit_pwa(dir.path(), &plan).unwrap();
    let worker = dir.path().join("sw.js");
    let written = std::fs::read_to_string(&worker).unwrap();
    assert_eq!(
        written,
        sw_source(&plan),
        "the emitted bytes are the planned source"
    );
}

#[test]
fn emit_pwa_overwrites_a_stale_worker_from_a_previous_build() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("sw.js"), "// stale ox-oldbuild").unwrap();
    let plan = PrecachePlan {
        build_id: "b17e5f0a4c9d2e8f".to_string(),
        cache_name: "ox-b17e5f0a4c9d2e8f".to_string(),
        precache: vec!["/".to_string()],
        offline_fallback: "/".to_string(),
        outbox: false,
    };
    emit_pwa(dir.path(), &plan).unwrap();
    let written = std::fs::read_to_string(dir.path().join("sw.js")).unwrap();
    assert!(written.contains("ox-b17e5f0a4c9d2e8f"));
    assert!(
        !written.contains("ox-oldbuild"),
        "the previous build's worker is replaced"
    );
}
