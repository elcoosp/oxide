//! Walkthrough for the V2-F20 module→package map
//! (`oxide_fw_bundler::packages_reached` /
//! `ClientManifest::reached_packages`, spec §V2-F20). The substrate's
//! attestation (`oxide build --attest`) lists every locked npm package in
//! the SBOM until this map lands; the deviations record names the gap
//! verbatim — "Intersecting `pnpm-lock.yaml` with the client manifest's
//! source list needs the bundler's module-to-package map on the attest
//! path." This example walks the map end to end:
//!
//! 1. A `ClientManifest` three routes built — `home` reaches `react`,
//!    `about` reaches `vue` and `axios`, `contact` is first-party-only.
//! 2. `ClientManifest::reached_packages()` folds every route's modules
//!    into the package set the bundle reached — `{ axios, react, vue }`.
//!    `contact`'s first-party modules contribute nothing.
//! 3. A mock lockfile (the `pnpm-lock.yaml` shape the attest parses) lists
//!    five packages; the intersection the attest's `filter_reached` performs
//!    keeps the three the bundle reached and drops the two it did not.
//!
//! Run: `cargo run -p oxide-fw-bundler --example reached_packages`.

use std::collections::BTreeSet;
use std::path::PathBuf;

use oxide_fw_bundler::{ClientManifest, RouteBundle, packages_reached};

/// One route's source modules — the resolved paths the bundler records in
/// `RouteBundle.modules` (PERF-09: the data `oxide why` and the attest
/// both read).
fn route(entry: &str, modules: &[&str]) -> (String, RouteBundle) {
    (
        entry.to_string(),
        RouteBundle {
            entry_chunk: format!("{entry}.js"),
            modules: modules.iter().map(PathBuf::from).collect(),
            ..RouteBundle::default()
        },
    )
}

/// A mock `pnpm-lock.yaml` package row — the `LockPackage { name, version }`
/// shape `crates/cli/src/attest.rs` parses.
#[derive(Clone)]
struct LockPackage {
    name: &'static str,
    version: &'static str,
}

fn main() {
    // 1. A three-route client manifest. `home` and `about` reach npm
    //    packages under `node_modules`; `contact` is first-party-only.
    let manifest = ClientManifest {
        routes: [
            route(
                "home",
                &[
                    "/proj/src/home.tsx",
                    "/proj/node_modules/react/index.js",
                    "/proj/node_modules/react/jsx-runtime.js",
                ],
            ),
            route(
                "about",
                &[
                    "/proj/src/about.tsx",
                    "/proj/node_modules/vue/index.js",
                    "/proj/node_modules/axios/index.js",
                ],
            ),
            route("contact", &["/proj/src/contact.tsx"]),
        ]
        .into_iter()
        .collect(),
        ..ClientManifest::default()
    };

    // 2. The manifest's reached-packages read — one fold over every route.
    //    `react` (reached through two modules) lists once; `contact`'s
    //    first-party module contributes nothing.
    let reached: BTreeSet<String> = manifest.reached_packages();
    println!("reached packages (the manifest's one-read fold):");
    for pkg in &reached {
        println!("  - {pkg}");
    }
    assert_eq!(
        reached,
        ["axios", "react", "vue"]
            .iter()
            .map(|s| (*s).to_string())
            .collect(),
        "react (two modules) dedups; contact's first-party module reaches nothing"
    );

    // 3. The per-route view `packages_reached` returns on its own — the
    //    function the attest could call per-route if it ever needed the
    //    split (it does not; `reached_packages` is the one-read aggregate).
    let home_reached = packages_reached(&manifest.routes["home"].modules);
    println!("\nhome route's own reached set: {:?}", home_reached);

    // 4. The intersection the attest's `filter_reached` performs: a mock
    //    lockfile lists five packages; only the three the bundle reached
    //    land in the SBOM. (The real filter lives in `oxide_fw_cli::attest`;
    //    this sketch shows the contract the bundler's map upholds.)
    let lockfile = [
        LockPackage {
            name: "react",
            version: "19.2.0",
        },
        LockPackage {
            name: "vue",
            version: "3.4.0",
        },
        LockPackage {
            name: "axios",
            version: "1.7.0",
        },
        LockPackage {
            name: "lodash",
            version: "4.17.21",
        },
        LockPackage {
            name: "tinybench",
            version: "2.9.0",
        },
    ];
    let reached_in_lock: Vec<&LockPackage> = lockfile
        .iter()
        .filter(|p| reached.contains(p.name))
        .collect();
    println!("\nthe SBOM's npm components (the lockfile intersected with reached):");
    for p in &reached_in_lock {
        println!("  - pkg:npm/{}@{}", p.name, p.version);
    }
    assert_eq!(reached_in_lock.len(), 3, "lodash and tinybench drop out");
    println!("\nlodash and tinybench dropped (the bundle never reached them).");
}
