//! The workspace-links walkthrough (V2-E12): watch a monorepo app's linked
//! package resolve to its REAL tree and the watcher roots extend into it —
//! the seam that makes "edit `packages/ui/src`, the app in `apps/web` hot
//! updates" a substrate contract instead of a cwd accident.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-dev --example workspace_links
//! ```

use oxide_fw_dev::Watcher;
use oxide_fw_dev::workspace::{ManifestWorkspaceLinks, WorkspaceLinks, real_path};
use std::path::Path;

fn main() {
    // 1. Build the spec's fixture workspace on disk: `apps/web` (the app)
    //    depends on `@acme/ui`; `packages/ui` is the workspace package.
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

    // 2. The pnpm link: `node_modules/@acme/ui` -> `packages/ui`. Resolution
    //    must report the REAL tree, never the link.
    #[cfg(unix)]
    {
        let dir = app.join("node_modules/@acme");
        std::fs::create_dir_all(&dir).expect("node_modules scope");
        std::os::unix::fs::symlink(&pkg, dir.join("ui")).expect("symlink");
    }
    #[cfg(not(unix))]
    {
        // No symlink privileges: a plain directory whose `main` reaches the
        // real tree through a relative specifier.
        let dir = app.join("node_modules/@acme/ui");
        std::fs::create_dir_all(&dir).expect("node_modules");
        let ups = "../..".to_string();
        let main = format!("{ups}/../../../packages/ui/src/index.ts");
        std::fs::write(
            dir.join("package.json"),
            format!(r#"{{"name":"@acme/ui","main":"{main}"}}"#),
        )
        .expect("linked manifest");
    }

    // 3. Discovery: the app's own dependency manifest drives it — the
    //    workspace's other packages (nobody imports them here) are not
    //    watched, and a published package that shares a name cannot sneak
    //    in (the candidate set is the globs').
    let links = ManifestWorkspaceLinks.links(&app).expect("discovery");
    println!("discovered {} linked package(s):", links.len());
    for link in &links {
        println!(
            "  {} -> real_root={} watch_root={}",
            link.name,
            link.real_root.display(),
            link.watch_root.display()
        );
    }
    assert_eq!(links.len(), 1, "the fixture links exactly @acme/ui");
    let real = links[0].real_root.display().to_string();
    assert!(
        !real.contains("node_modules"),
        "the real root is the package tree, not the link: {real}"
    );

    // 4. The package-edge bridge: a module path that arrived through the
    //    link canonicalizes onto the same real tree the watcher reports —
    //    graph nodes and watcher events agree, so the boundary planner
    //    walks across the edge.
    let through_link = app.join("node_modules/@acme/ui/src/index.ts");
    if let Some(canonical) = real_path(&through_link) {
        println!(
            "real_path bridge: {} -> {}",
            through_link.display(),
            canonical.display()
        );
        assert_eq!(
            canonical.parent().expect("entry has a parent"),
            links[0]
                .real_root
                .canonicalize()
                .expect("exists")
                .join("src"),
            "the module path canonicalizes into the real tree"
        );
    }

    // 5. Wire-up: the watcher roots gain the linked package's source dir.
    let watcher = Watcher::with_workspace_links(vec![app.clone()], &ManifestWorkspaceLinks, &app)
        .expect("watcher");
    println!("watcher roots:");
    for root_dir in &watcher.roots {
        println!("  {}", root_dir.display());
    }
    assert!(watcher.roots.contains(&app));
    assert!(watcher.roots.contains(&pkg.join("src")));

    // 6. The injectable seam: a fake WorkspaceLinks drives the same wire-up
    //    (the pattern the record/replay fakes use) — here a policy that
    //    watches nothing extra.
    struct NoLinks;
    impl WorkspaceLinks for NoLinks {
        fn links(
            &self,
            _app_root: &Path,
        ) -> oxide_fw_dev::Result<Vec<oxide_fw_dev::LinkedPackage>> {
            Ok(Vec::new())
        }
    }
    let plain = Watcher::with_workspace_links(vec![app.clone()], &NoLinks, &app)
        .expect("watcher over the fake");
    assert_eq!(plain.roots, vec![app.clone()]);
    println!(
        "fake-link watcher keeps the app-only roots: {:?}",
        plain.roots
    );

    // The tempdir cleans itself up.
}
