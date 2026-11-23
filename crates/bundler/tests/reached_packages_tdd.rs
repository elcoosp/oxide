//! TDD pins for the V2-F20 "packages that reached a bundle" module→package
//! map (`oxide_fw_bundler`, spec §V2-F20). The substrate's own
//! `crates/cli/src/attest.rs:1039` comment names the deferral verbatim:
//!
//! > "The 'packages that reached a bundle' manifest filter is the
//! > substrate's documented deferral — every locked package is listed."
//!
//! And `docs/deviations.md` delta 5 (the F20 record) names the missing
//! seam the same way:
//!
//! > "Intersecting `pnpm-lock.yaml` with the client manifest's source list
//! > needs the bundler's module-to-package map on the attest path; until
//! > that lands, the SBOM over-reports (every locked package, not only the
//! > ones that reached the bundle)."
//!
//! [`packages_reached`] IS that map: a pure classification of a route's
//! source modules into the npm packages that own them (the
//! `node_modules/<pkg>` segment of each resolved path, scoped packages
//! folded to `@scope/pkg`). [`ClientManifest::reached_packages`] is the
//! one-read aggregate over every route — the value the attest path
//! intersects the lockfile with. Written FIRST (red): these pins fail on
//! unresolved-symbol errors before the seam exists.

use std::collections::BTreeSet;
use std::path::PathBuf;

use oxide_fw_bundler::{ClientManifest, RouteBundle, packages_reached};

// ---------------------------------------------------------------------------
// packages_reached — the module→package classification
// ---------------------------------------------------------------------------

/// The first-party prefix every fixture shares — a module NOT under
/// `node_modules` contributes nothing (it is the app's own source).
const APP_SRC: &str = "/proj/src/app.tsx";

#[test]
fn packages_reached_classifies_a_bare_node_modules_package() {
    // A resolved React entry under node_modules → the package is `react`.
    let modules = [PathBuf::from("/proj/node_modules/react/index.js")];
    let reached = packages_reached(&modules);
    assert_eq!(reached, BTreeSet::from(["react".to_string()]));
}

#[test]
fn packages_reached_classifies_a_scoped_package() {
    // A scoped package folds the `@scope` prefix and the package name into
    // one `@scope/pkg` identifier (the pnpm-lock spelling the attest
    // intersects with).
    let modules = [PathBuf::from("/proj/node_modules/@scope/pkg/sub.js")];
    let reached = packages_reached(&modules);
    assert_eq!(reached, BTreeSet::from(["@scope/pkg".to_string()]));
}

#[test]
fn packages_reached_skips_first_party_modules() {
    // An app-source module is NOT under node_modules — it contributes no
    // package. The reached set is empty (the route reached no npm package).
    let modules = [PathBuf::from(APP_SRC)];
    let reached = packages_reached(&modules);
    assert!(reached.is_empty(), "first-party modules reach no package");
}

#[test]
fn packages_reached_dedups_one_package_across_many_modules() {
    // Two modules from the same package resolve to ONE reached entry — the
    // set is the package set, not the module set.
    let modules = [
        PathBuf::from("/proj/node_modules/react/index.js"),
        PathBuf::from("/proj/node_modules/react/jsx-runtime.js"),
    ];
    let reached = packages_reached(&modules);
    assert_eq!(reached, BTreeSet::from(["react".to_string()]));
}

#[test]
fn packages_reached_collects_distinct_packages() {
    // A route that imports react AND vue reaches both — the set carries
    // every distinct package the route's modules resolved under.
    let modules = [
        PathBuf::from("/proj/node_modules/react/index.js"),
        PathBuf::from("/proj/node_modules/vue/index.js"),
        PathBuf::from(APP_SRC),
    ];
    let reached = packages_reached(&modules);
    assert_eq!(
        reached,
        BTreeSet::from(["react".to_string(), "vue".to_string()])
    );
}

#[test]
fn packages_reached_handles_a_nested_node_modules() {
    // A pnpm layout nests the real package under
    // `node_modules/.pnpm/<pkg>@<ver>/node_modules/<pkg>` — the LAST
    // `node_modules/<pkg>` segment is the package (the `.pnpm` directory
    // is the virtual store, never a package itself).
    let modules = [PathBuf::from(
        "/proj/node_modules/.pnpm/react@18.2.0/node_modules/react/index.js",
    )];
    let reached = packages_reached(&modules);
    assert_eq!(reached, BTreeSet::from(["react".to_string()]));
}

#[test]
fn packages_reached_empty_input_is_empty() {
    let reached = packages_reached(&[]);
    assert!(reached.is_empty());
}

#[test]
fn packages_reached_skips_a_bare_node_modules_with_no_package_after() {
    // `node_modules` as the final component is a directory scan artifact,
    // not a resolved module — there is no package after it, so the module
    // contributes nothing (never fabricates a package name).
    let modules = [PathBuf::from("/proj/node_modules")];
    let reached = packages_reached(&modules);
    assert!(
        reached.is_empty(),
        "a trailing node_modules reaches no package"
    );
}

// ---------------------------------------------------------------------------
// ClientManifest::reached_packages — the one-read aggregate over routes
// ---------------------------------------------------------------------------

fn route(name: &str, modules: &[&str]) -> RouteBundle {
    RouteBundle {
        entry_chunk: format!("{name}.js"),
        modules: modules.iter().map(PathBuf::from).collect(),
        ..RouteBundle::default()
    }
}

fn manifest(routes: &[(&str, &[&str])]) -> ClientManifest {
    ClientManifest {
        routes: routes
            .iter()
            .map(|(name, mods)| ((*name).to_string(), route(name, mods)))
            .collect(),
        ..ClientManifest::default()
    }
}

#[test]
fn reached_packages_unions_every_routes_modules() {
    // Two routes, each reaching a distinct package — the manifest's
    // reached set is the union.
    let m = manifest(&[
        ("home", &["/proj/node_modules/react/index.js"]),
        ("about", &["/proj/node_modules/vue/index.js"]),
    ]);
    assert_eq!(
        m.reached_packages(),
        BTreeSet::from(["react".to_string(), "vue".to_string()])
    );
}

#[test]
fn reached_packages_dedups_across_routes() {
    // Two routes both reach react — the manifest's reached set lists
    // react once (the set, not the multiset).
    let m = manifest(&[
        ("home", &["/proj/node_modules/react/index.js"]),
        ("about", &["/proj/node_modules/react/jsx-runtime.js"]),
    ]);
    assert_eq!(m.reached_packages(), BTreeSet::from(["react".to_string()]));
}

#[test]
fn reached_packages_empty_when_no_route_reached_a_package() {
    // A manifest of first-party-only routes reaches no package — the set
    // is empty, and the attest falls back to listing every locked package
    // (the safe over-reporting default).
    let m = manifest(&[("home", &[APP_SRC])]);
    assert!(m.reached_packages().is_empty());
}

#[test]
fn reached_packages_empty_manifest_is_empty() {
    let m = ClientManifest::default();
    assert!(m.reached_packages().is_empty());
}
