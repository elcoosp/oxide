//! TDD pins for the V2-E12 workspace-links substrate (spec §V2-E12).
//!
//! Written FIRST (red): failed to compile before `dev::workspace` existed —
//! the E04/E07 pattern. These pins are the spec-verbatim contract of
//! `oxide_fw_dev::workspace`:
//!
//! - a linked workspace package is discovered from the app's own dependency
//!   manifest (not "every sibling dir"), resolved with `oxc_resolver`'s
//!   symlink-resolving defaults (the spec's `preserveSymlinks: false`) so the
//!   reported root is the REAL package tree (`packages/ui`), never the
//!   `node_modules` link;
//! - the watcher roots gain exactly the linked packages' source dirs, so an
//!   edit to `../packages/ui/src` is watched in dev;
//! - a non-workspace app, a workspace package the app does not depend on, and
//!   a candidate outside the configured globs produce NO links;
//! - a malformed workspace manifest is a LOUD `DEV-WS-001` error naming the
//!   manifest and the fix, not a silent empty set (the house convention);
//! - `real_path` is the package-edge bridge the importers-graph harvest uses:
//!   a module path that arrived through a `node_modules` link canonicalizes
//!   onto the real tree, so graph nodes and watcher events agree.

use oxide_fw_dev::Watcher;
use oxide_fw_dev::workspace::{LinkedPackage, ManifestWorkspaceLinks, WorkspaceLinks, real_path};
use std::path::{Path, PathBuf};

/// Build the fixture workspace the spec names: `apps/web` (the app) and
/// `packages/ui` (a linked package). `linked` controls whether the app's
/// manifest declares the dependency (the discovery input).
fn fixture_workspace() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let ws = tempfile::tempdir().expect("tempdir");
    let root = ws.path().to_path_buf();
    let app = root.join("apps/web");
    let pkg = root.join("packages/ui");
    std::fs::create_dir_all(app.join("src")).expect("app src");
    std::fs::create_dir_all(pkg.join("src")).expect("pkg src");
    std::fs::write(
        root.join("pnpm-workspace.yaml"),
        "packages:\n  - 'packages/*'\n",
    )
    .expect("workspace manifest");
    std::fs::write(
        app.join("package.json"),
        r#"{"name":"web","dependencies":{"@acme/ui":"workspace:*"}}"#,
    )
    .expect("app manifest");
    std::fs::write(
        pkg.join("package.json"),
        r#"{"name":"@acme/ui","version":"0.1.0","main":"./src/index.ts"}"#,
    )
    .expect("package manifest");
    std::fs::write(pkg.join("src/index.ts"), "export const ui = 'ui';\n").expect("entry");
    (ws, app, pkg)
}

/// Materialize `node_modules/@acme/ui` as a symlink to the real package
/// (the pnpm layout). Windows needs privileges for symlinks, so the pins
/// that exercise link-through use the plain-directory layout the npm
/// fallback produces — `real_path` itself is pinned under `cfg(unix)`.
#[cfg(unix)]
fn link_node_modules(app: &Path, pkg: &Path, scope: &str, name: &str) {
    let dir = app.join("node_modules").join(scope);
    std::fs::create_dir_all(&dir).expect("node_modules scope");
    std::os::unix::fs::symlink(pkg, dir.join(name)).expect("symlink");
}

/// Non-unix (and unix) fallback: `node_modules/@scope/name` as a real
/// directory holding a package.json whose main file is the REAL package's
/// entry, reached through a relative specifier — the plain-path shape the
/// resolver must serve without symlink privileges (the entry still lands
/// in the package's real tree, so `package_root_of` reports it).
fn copy_node_modules(app: &Path, pkg: &Path, scope: &str, name: &str) {
    let dir = app.join("node_modules").join(scope).join(name);
    std::fs::create_dir_all(&dir).expect("node_modules scope");
    let entry = pkg.join("src/index.ts");
    let main = rel_path(&dir, &entry);
    std::fs::write(
        dir.join("package.json"),
        format!(r#"{{"name":"{scope}/{name}","version":"0.1.0","main":"{main}"}}"#),
    )
    .expect("linked manifest");
}

/// A minimal `pathdiff`-style relative path between two absolute paths
/// (test-only: the fixtures need a `main` that survives tempdir paths).
fn rel_path(from_dir: &Path, target: &Path) -> String {
    let from: Vec<_> = from_dir.components().collect();
    let to: Vec<_> = target.components().collect();
    let common = from
        .iter()
        .zip(to.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let mut parts: Vec<String> = from[common..].iter().map(|_| "..".to_string()).collect();
    parts.extend(
        to[common..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    parts.join("/")
}

fn write_pkg_json(path: &Path, json: &str) {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).expect("dir");
    }
    std::fs::write(path, json).expect("manifest");
}

#[test]
fn links_resolve_the_real_package_root_not_the_node_modules_link() {
    let (_ws, app, pkg) = fixture_workspace();
    #[cfg(unix)]
    link_node_modules(&app, &pkg, "@acme", "ui");
    #[cfg(not(unix))]
    copy_node_modules(&app, &pkg, "@acme", "ui");

    let links = ManifestWorkspaceLinks.links(&app).expect("links");
    assert_eq!(
        links.len(),
        1,
        "exactly the app's linked package: {links:?}"
    );
    let link = &links[0];
    assert_eq!(link.name, "@acme/ui");
    let real = link.real_root.canonicalize().expect("real root exists");
    assert_eq!(real, pkg.canonicalize().expect("fixture exists"));
    let rendered = link.real_root.display().to_string();
    assert!(
        !rendered.contains("node_modules"),
        "the real root must not be the node_modules link: {rendered}"
    );
    assert_eq!(
        link.watch_root,
        pkg.join("src"),
        "the watch root is the package's source tree"
    );
}

#[test]
fn watcher_roots_gain_the_linked_packages_source_dirs() {
    let (_ws, app, pkg) = fixture_workspace();
    #[cfg(unix)]
    link_node_modules(&app, &pkg, "@acme", "ui");
    #[cfg(not(unix))]
    copy_node_modules(&app, &pkg, "@acme", "ui");

    let watcher = Watcher::with_workspace_links(vec![app.clone()], &ManifestWorkspaceLinks, &app)
        .expect("watcher");
    assert!(
        watcher.roots.contains(&app),
        "the app root stays watched: {:?}",
        watcher.roots
    );
    assert!(
        watcher.roots.contains(&pkg.join("src")),
        "the linked package's source dir is watched: {:?}",
        watcher.roots
    );
    assert_eq!(
        watcher.roots.len(),
        2,
        "no duplicate roots: {:?}",
        watcher.roots
    );
}

#[test]
fn a_non_workspace_app_yields_no_links() {
    let ws = tempfile::tempdir().expect("tempdir");
    let app = ws.path().join("solo");
    std::fs::create_dir_all(&app).expect("app");
    write_pkg_json(
        &app.join("package.json"),
        r#"{"name":"solo","dependencies":{"lodash":"^4"}}"#,
    );
    let links = ManifestWorkspaceLinks.links(&app).expect("links");
    assert!(links.is_empty(), "no workspace manifests above: {links:?}");
    let watcher = Watcher::with_workspace_links(vec![app.clone()], &ManifestWorkspaceLinks, &app)
        .expect("watcher");
    assert_eq!(watcher.roots, vec![app.clone()]);
}

#[test]
fn a_workspace_package_the_app_does_not_depend_on_is_not_a_link() {
    let (ws, app, pkg) = fixture_workspace();
    #[cfg(unix)]
    link_node_modules(&app, &pkg, "@acme", "ui");
    #[cfg(not(unix))]
    copy_node_modules(&app, &pkg, "@acme", "ui");
    // A second package the app never imports.
    let other = ws.path().join("packages/other");
    std::fs::create_dir_all(&other).expect("other pkg");
    write_pkg_json(
        &other.join("package.json"),
        r#"{"name":"@acme/other","version":"0.1.0"}"#,
    );
    let links = ManifestWorkspaceLinks.links(&app).expect("links");
    assert_eq!(links.len(), 1, "only the declared dependency: {links:?}");
    assert_eq!(links[0].name, "@acme/ui");
}

#[test]
fn a_candidate_outside_the_workspace_globs_is_not_a_link() {
    let (ws, app, pkg) = fixture_workspace();
    #[cfg(unix)]
    link_node_modules(&app, &pkg, "@acme", "ui");
    #[cfg(not(unix))]
    copy_node_modules(&app, &pkg, "@acme", "ui");
    // Inside the tree but outside `packages/*` — and resolvable, so the
    // pin isolates the candidate-set rule (a same-named published install
    // must not smuggle a non-workspace package into the links).
    let tools = ws.path().join("tools/toolkit");
    std::fs::create_dir_all(tools.join("src")).expect("tools pkg");
    write_pkg_json(
        &tools.join("package.json"),
        r#"{"name":"@acme/toolkit","version":"0.1.0","main":"./src/index.ts"}"#,
    );
    std::fs::write(tools.join("src/index.ts"), "export {};\n").expect("entry");
    copy_node_modules(&app, &tools, "@acme", "toolkit");
    write_pkg_json(
        &app.join("package.json"),
        r#"{"name":"web","dependencies":{"@acme/ui":"workspace:*","@acme/toolkit":"workspace:*"}}"#,
    );
    let links = ManifestWorkspaceLinks.links(&app).expect("links");
    assert_eq!(
        links.len(),
        1,
        "glob-lite owns the candidate set: {links:?}"
    );
    assert_eq!(links[0].name, "@acme/ui");
}

#[test]
fn an_uninstalled_workspace_dependency_is_skipped_loudly_not_fatal() {
    let (_ws, app, _pkg) = fixture_workspace();
    // No node_modules at all: the dependency cannot resolve.
    let links = ManifestWorkspaceLinks.links(&app).expect("links");
    assert!(links.is_empty(), "not installed yet: {links:?}");
}

#[test]
fn a_malformed_workspace_manifest_is_a_loud_dev_ws_001() {
    let ws = tempfile::tempdir().expect("tempdir");
    let app = ws.path().join("apps/web");
    std::fs::create_dir_all(&app).expect("app");
    write_pkg_json(&app.join("package.json"), r#"{"name":"web"}"#);
    std::fs::write(
        ws.path().join("pnpm-workspace.yaml"),
        "packages: 'packages/*'\n",
    )
    .expect("manifest");
    let err = ManifestWorkspaceLinks
        .links(&app)
        .expect_err("a scalar `packages:` is malformed");
    let msg = err.0.message.clone();
    assert!(msg.contains("DEV-WS-001") || err.0.code == "DEV-WS-001");
    assert!(
        msg.contains("pnpm-workspace.yaml"),
        "the message names the manifest: {msg}"
    );
}

#[test]
fn links_are_sorted_by_name_for_deterministic_roots() {
    let ws = tempfile::tempdir().expect("tempdir");
    let root = ws.path();
    let app = root.join("apps/web");
    std::fs::create_dir_all(&app).expect("app");
    for (dir, name) in [
        ("packages/zeta", "@acme/zeta"),
        ("packages/alpha", "@acme/alpha"),
    ] {
        let pkg = root.join(dir);
        std::fs::create_dir_all(&pkg).expect("pkg");
        write_pkg_json(
            &pkg.join("package.json"),
            &format!(r#"{{"name":"{name}","main":"./src/index.ts"}}"#),
        );
        std::fs::create_dir_all(pkg.join("src")).expect("src");
        std::fs::write(pkg.join("src/index.ts"), "export {};\n").expect("entry");
        // The plain-directory node_modules layout resolution serves.
        copy_node_modules(&app, &pkg, "@acme", name.rsplit('/').next().expect("leaf"));
    }
    std::fs::write(
        root.join("pnpm-workspace.yaml"),
        "packages:\n  - 'packages/*'\n",
    )
    .expect("manifest");
    write_pkg_json(
        &app.join("package.json"),
        r#"{"name":"web","dependencies":{"@acme/zeta":"workspace:*","@acme/alpha":"workspace:*"}}"#,
    );

    let links = ManifestWorkspaceLinks.links(&app).expect("links");
    let names: Vec<&str> = links.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["@acme/alpha", "@acme/zeta"],
        "sorted: {names:?}"
    );
}

#[test]
#[cfg(unix)]
fn real_path_canonicalizes_through_the_node_modules_link() {
    let (_ws, app, pkg) = fixture_workspace();
    link_node_modules(&app, &pkg, "@acme", "ui");
    let link = app.join("node_modules/@acme/ui");
    let real = real_path(&link).expect("link resolves");
    assert_eq!(real, pkg.canonicalize().expect("fixture exists"));
    assert!(real_path(&app.join("node_modules/@acme/missing")).is_none());
}

#[test]
#[cfg(unix)]
fn the_package_json_workspaces_shape_discovers_too() {
    // package.json `workspaces` (npm/yarn classic) instead of the pnpm file.
    let ws = tempfile::tempdir().expect("tempdir");
    let root = ws.path();
    let app = root.join("apps/web");
    let pkg = root.join("packages/ui");
    std::fs::create_dir_all(&app).expect("app");
    std::fs::create_dir_all(pkg.join("src")).expect("pkg src");
    write_pkg_json(
        &root.join("package.json"),
        r#"{"name":"mono","workspaces":["packages/*"]}"#,
    );
    write_pkg_json(
        &app.join("package.json"),
        r#"{"name":"web","dependencies":{"@acme/ui":"workspace:*"}}"#,
    );
    write_pkg_json(
        &pkg.join("package.json"),
        r#"{"name":"@acme/ui","main":"./src/index.ts"}"#,
    );
    std::fs::write(pkg.join("src/index.ts"), "export const ui = 'ui';\n").expect("entry");
    copy_node_modules(&app, &pkg, "@acme", "ui");
    let links = ManifestWorkspaceLinks.links(&app).expect("links");
    assert_eq!(
        links.len(),
        1,
        "the package.json shape discovers too: {links:?}"
    );
    assert_eq!(links[0].name, "@acme/ui");
}

#[test]
fn linked_package_is_cloneable_and_comparable_for_the_wiring() {
    let link = LinkedPackage {
        name: "@acme/ui".into(),
        real_root: PathBuf::from("/ws/packages/ui"),
        watch_root: PathBuf::from("/ws/packages/ui/src"),
    };
    let clone = link.clone();
    assert_eq!(link, clone);
}
