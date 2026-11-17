//! V2-F02: the auto-islands build adoption — the `scan_interactivity`
//! call-site (the engine landed in
//! `oxide_fw_compiler::interactivity`; this is the build half the row
//! named: scan → side-car → registry merge).
//!
//! When `framework.autoIslands` is on, the build scans the client tree —
//! the route components under `routes/` (client files only: `*.server.*`
//! and `.d.ts` are excluded) plus every registered island — classifies
//! each file through `scan_interactivity` and its transitive
//! same-project import fixpoint, writes the `.oxide/island-manifest.json`
//! side-car and hands `generate_island_boot_entry` the merged registry:
//! the files the scanner found interactive join the hydration set, an
//! explicit `islands` registration always wins, and a pure-classified
//! registered island is skipped (the engine's `hydrates` decision). With
//! the gate off (the default — the spec defers by default) the registry
//! is returned unchanged and nothing is written.
//!
//! The dev-warn half (the row's last thin half, landed here): the dev
//! server scans the SAME set at boot ([`dev_island_manifest`], read-only
//! — no side-car, no registry merge) and carries the manifest in
//! [`crate::esm_dev::EsmDevState`]; the ESM transform pipeline checks
//! each served module's `on*`-props usage surface against it and logs the
//! pure-classified mismatches (`AST-ISL-002`). The scan-set rule is this
//! file's ([`scanned_sources`]), so dev and build can never disagree
//! about what was scanned.

use crate::*;

/// The framework string the §V2-F01 inference order answers, mapped onto
/// the scan dialect. `.tsx` files that declare Solid (pragma or project)
/// scan as Solid; everything else follows the extension.
fn scan_framework_of(
    src: &str,
    explicit: Option<&str>,
    project: &str,
    source: &str,
) -> oxide_fw_compiler::Framework {
    let first_line = source.lines().next().unwrap_or("");
    let named = oxide_fw_config::islands::island_framework(src, explicit, project, first_line);
    match named.as_str() {
        "react" => oxide_fw_compiler::Framework::React,
        "vue" => oxide_fw_compiler::Framework::Vue,
        "svelte" => oxide_fw_compiler::Framework::Svelte,
        "solid" => oxide_fw_compiler::Framework::Solid,
        _ => {
            let ext = std::path::Path::new(src)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            oxide_fw_compiler::Framework::from_extension(ext)
        }
    }
}

/// A client-source file the scan reads: `.ts(x)` / `.js(x)` / `.vue` /
/// `.svelte`, never a `.server.*` module (those run in the server bundle)
/// or a `.d.ts` (declarations carry no component).
fn is_client_source(path: &str) -> bool {
    let name = std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    if name.ends_with(".d.ts") || name.contains(".server.") {
        return false;
    }
    matches!(
        std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or(""),
        "ts" | "tsx" | "js" | "jsx" | "vue" | "svelte"
    )
}

/// Collect the scan set: every client source under the routes dir plus
/// every registered island source that exists. Keys are the
/// project-root-relative paths the manifest names (forward slashes).
fn walk_client_sources(dir: &std::path::Path, rel: &str, files: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        let child_rel = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        let p = e.path();
        if p.is_dir() {
            walk_client_sources(&p, &child_rel, files);
        } else if is_client_source(&child_rel) {
            files.push(p);
        }
    }
}

fn scan_set(
    rootp: &std::path::Path,
    routes_dir: &str,
    islands: &std::collections::BTreeMap<String, oxide_fw_config::IslandSpec>,
) -> Vec<std::path::PathBuf> {
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    walk_client_sources(&rootp.join(routes_dir), "", &mut files);
    for spec in islands.values() {
        let p = rootp.join(spec.src());
        if p.is_file() && !files.contains(&p) {
            files.push(p);
        }
    }
    files
}

/// The scan set with frameworks resolved and sources read: the
/// `(rel-path, (framework, source))` map the engine consumes (the module
/// never touches the filesystem itself). The build side-car and the dev
/// manifest scan the SAME map — one rule, two consumers.
pub(crate) fn scanned_sources(
    rootp: &std::path::Path,
    cfg: &oxide_fw_config::OxideConfig,
) -> std::collections::BTreeMap<String, (oxide_fw_compiler::Framework, String)> {
    let files = scan_set(rootp, &cfg.routes.dir, &cfg.framework.islands);
    let project_framework = cfg
        .framework
        .adapter
        .clone()
        .or_else(|| cfg.framework.name.clone())
        .unwrap_or_else(|| "react".to_string());
    let mut scanned: std::collections::BTreeMap<String, (oxide_fw_compiler::Framework, String)> =
        std::collections::BTreeMap::new();
    for p in &files {
        let Ok(source) = std::fs::read_to_string(p) else {
            continue;
        };
        let rel = p
            .strip_prefix(rootp)
            .unwrap_or(p)
            .to_string_lossy()
            .replace('\\', "/");
        let explicit = cfg
            .framework
            .islands
            .values()
            .find(|s| s.src() == rel)
            .and_then(|s| s.framework());
        let fw = scan_framework_of(&rel, explicit, &project_framework, &source);
        scanned.insert(rel, (fw, source));
    }
    scanned
}

/// The dev-boot island manifest (the F02 dev-warn half): the same scan
/// the build runs, READ-ONLY — no side-car write, no registry merge; the
/// classification stays the hydration decision's, the warn only reads it.
/// `None` with the gate off (the default): no auto classification
/// exists, so there is nothing to warn about.
pub(crate) fn dev_island_manifest(
    rootp: &std::path::Path,
    cfg: &oxide_fw_config::OxideConfig,
) -> Option<oxide_fw_compiler::IslandManifest> {
    if !cfg.framework.auto_islands {
        return None;
    }
    let scanned = scanned_sources(rootp, cfg);
    Some(oxide_fw_compiler::IslandManifest::build(&scanned))
}

/// The registry the boot entry hydrates: the config's islands merged with
/// the auto-discovered interactive files (the §V2-F02 adoption). When the
/// gate is off the config registry comes back unchanged; when it is on
/// the scan runs, the side-car lands at `.oxide/island-manifest.json`,
/// and every file the scanner found interactive joins the registry under
/// a derived name unless it was already registered.
pub(crate) fn islands_for_boot(
    cfg: &oxide_fw_config::OxideConfig,
    tree: &oxide_fw_router::RouteTree,
    rootp: &std::path::Path,
    w: Writer<'_>,
) -> anyhow::Result<std::collections::BTreeMap<String, oxide_fw_config::IslandSpec>> {
    let mut merged = cfg.framework.islands.clone();
    if !cfg.framework.auto_islands {
        return Ok(merged);
    }
    let _ = tree;
    let scanned = scanned_sources(rootp, cfg);
    let manifest = oxide_fw_compiler::IslandManifest::build(&scanned);
    let sidecar = rootp.join(".oxide").join("island-manifest.json");
    std::fs::create_dir_all(rootp.join(".oxide"))?;
    std::fs::write(&sidecar, manifest.to_json())?;
    // The registry merge: explicit wins (a registered src is never
    // re-added); the engine's `hydrates` decision decides every file the
    // scan classified (pure classification is the only skip; missing data
    // hydrates).
    let mut auto = 0usize;
    for (rel, inter) in manifest.iter() {
        if merged.values().any(|s| s.src() == rel) {
            continue;
        }
        if !oxide_fw_compiler::hydrates(false, Some(inter)) {
            continue;
        }
        let stem = std::path::Path::new(rel)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("island")
            .to_string();
        let mut name = stem.clone();
        let mut n = 1u32;
        while merged.contains_key(&name) {
            name = format!("{stem}_{n}");
            n += 1;
        }
        merged.insert(
            name,
            oxide_fw_config::IslandSpec::Object {
                src: rel.clone(),
                framework: None,
            },
        );
        auto += 1;
    }
    outln!(
        w,
        "auto-islands: {auto} auto-discovered of {} scanned (manifest .oxide/island-manifest.json)",
        scanned.len()
    );
    Ok(merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with(auto: bool, islands: &[(&str, &str)]) -> oxide_fw_config::OxideConfig {
        let mut c = oxide_fw_config::OxideConfig::default();
        c.framework.auto_islands = auto;
        for (name, src) in islands {
            c.framework.islands.insert(
                (*name).to_string(),
                oxide_fw_config::IslandSpec::Plain((*src).to_string()),
            );
        }
        c
    }

    /// A gate-off build changes nothing — the registry is the config's.
    #[test]
    fn gate_off_returns_the_config_registry() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = cfg_with(false, &[("Counter", "src/islands/counter.tsx")]);
        let tree = oxide_fw_router::RouteTree::new();
        let mut buf: Vec<u8> = Vec::new();
        let merged = islands_for_boot(&cfg, &tree, dir.path(), &mut buf).unwrap();
        assert_eq!(merged.len(), 1);
        assert!(!dir.path().join(".oxide/island-manifest.json").exists());
    }

    /// A gate-on build scans, writes the side-car and adds the interactive
    /// files; an explicit registration wins (never re-added).
    #[test]
    fn gate_on_scans_writes_the_sidecar_and_merges() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("routes")).unwrap();
        std::fs::create_dir_all(root.join("src/islands")).unwrap();
        // An interactive route component (useState) and a pure one.
        std::fs::write(
            root.join("routes/counter.tsx"),
            "export default function C() { const [n, setN] = useState(0); return <button onClick={() => setN(n + 1)}>{n}</button>; }",
        )
        .unwrap();
        std::fs::write(
            root.join("routes/banner.tsx"),
            "export default function B() { return <p>hi</p>; }",
        )
        .unwrap();
        // A registered island — interactive, but explicitly registered
        // (its src never re-joins under a derived name).
        std::fs::write(
            root.join("src/islands/ticker.tsx"),
            "export default function T() { useEffect(() => {}, []); return <p/>; }",
        )
        .unwrap();
        let mut cfg = cfg_with(true, &[("Ticker", "src/islands/ticker.tsx")]);
        cfg.routes.dir = "routes".to_string();
        let tree = oxide_fw_router::RouteTree::new();
        let mut buf: Vec<u8> = Vec::new();
        let merged = islands_for_boot(&cfg, &tree, root, &mut buf).unwrap();
        // The side-car exists and classifies both route files.
        let manifest = std::fs::read_to_string(root.join(".oxide/island-manifest.json")).unwrap();
        assert!(manifest.contains("\"routes/counter.tsx\""));
        assert!(manifest.contains("\"routes/banner.tsx\""));
        let parsed =
            oxide_fw_compiler::IslandManifest::from_json(&manifest).expect("the side-car parses");
        assert!(parsed.get("routes/counter.tsx").unwrap().is_interactive());
        assert!(!parsed.get("routes/banner.tsx").unwrap().is_interactive());
        // The merged registry: Ticker (explicit) + the interactive route
        // component; the pure banner is skipped.
        assert_eq!(merged.len(), 2, "explicit + one auto: {merged:?}");
        assert!(
            merged.values().any(|s| s.src() == "routes/counter.tsx"),
            "the interactive route component joined: {merged:?}"
        );
        assert!(
            !merged.values().any(|s| s.src() == "routes/banner.tsx"),
            "the pure component is skipped: {merged:?}"
        );
    }

    /// Server modules and `.d.ts` never enter the scan set.
    #[test]
    fn server_modules_and_declarations_are_not_scanned() {
        assert!(!is_client_source("routes/index/page.server.ts"));
        assert!(!is_client_source("types/routes.d.ts"));
        assert!(is_client_source("routes/index/page.tsx"));
        assert!(is_client_source("routes/blog/Banner.vue"));
    }

    /// The dev-warn manifest (the F02 dev-warn half): gate off is `None`
    /// and nothing is written — the dev posture is read-only.
    #[test]
    fn dev_manifest_gate_off_is_none_and_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = cfg_with(false, &[("Counter", "src/islands/counter.tsx")]);
        let manifest = dev_island_manifest(dir.path(), &cfg);
        assert!(manifest.is_none(), "{manifest:?}");
        assert!(!dir.path().join(".oxide/island-manifest.json").exists());
    }

    /// Gate on: the manifest classifies the same set the build scans,
    /// read-only — no side-car lands (the build half owns that write).
    #[test]
    fn dev_manifest_gate_on_classifies_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("routes")).unwrap();
        std::fs::write(
            root.join("routes/counter.tsx"),
            "export default function C() { const [n, setN] = useState(0); return <button onClick={() => setN(n + 1)}>{n}</button>; }",
        )
        .unwrap();
        std::fs::write(
            root.join("routes/banner.tsx"),
            "export default function B() { return <p>hi</p>; }",
        )
        .unwrap();
        let mut cfg = cfg_with(true, &[]);
        cfg.routes.dir = "routes".to_string();
        let manifest = dev_island_manifest(root, &cfg).expect("the gate is on");
        assert!(manifest.get("routes/counter.tsx").unwrap().is_interactive());
        assert!(!manifest.get("routes/banner.tsx").unwrap().is_interactive());
        assert!(!root.join(".oxide/island-manifest.json").exists());
    }
}
