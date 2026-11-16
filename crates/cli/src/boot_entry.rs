//! §10.3 / §V2-F01: the island boot entry — the CLI seam over the
//! compiler-side boot planner (`oxide_fw_compiler::islands`). The
//! emission itself (the legacy single-framework shape, byte-preserving,
//! and the mixed registry shape) lives in the planner; the CLI owns the
//! registry resolution (the config crate's inference order over the
//! merged registry) and the two artifact writes the build and dev paths
//! share: `.oxide/island-boot.mjs`'s source and the
//! `.oxide/islands.json` tooling payload.

use crate::*;

/// §V2-F01: resolve a registry entry set through the config crate's
/// inference order — explicit `framework` → file extension → first-line
/// pragma → project framework. The file read is best-effort: an
/// unreadable island file contributes an empty pragma line, so the
/// inference still answers (the project framework). This is the ONE
/// definition site; the planner (`plan_island_boot`) consumes the
/// resolved set and never touches the filesystem.
pub(crate) fn resolve_islands(
    root: &Path,
    framework: &str,
    islands: &BTreeMap<String, oxide_fw_config::IslandSpec>,
) -> BTreeMap<String, oxide_fw_compiler::islands::ResolvedIsland> {
    let mut resolved = BTreeMap::new();
    for (name, spec) in islands {
        let src = spec.src();
        let first_line = std::fs::read_to_string(root.join(src))
            .map(|b| b.lines().next().unwrap_or("").to_string())
            .unwrap_or_default();
        resolved.insert(
            name.clone(),
            oxide_fw_compiler::islands::ResolvedIsland {
                src: src.to_string(),
                framework: oxide_fw_config::island_framework(
                    src,
                    spec.framework(),
                    framework,
                    &first_line,
                ),
            },
        );
    }
    resolved
}

/// §10.3: generate the synthetic boot module for a project with islands.
///
/// The entry is planned by `oxide_fw_compiler::islands::plan_island_boot`
/// (§V2-F01): a single-framework registry keeps the LEGACY shape
/// byte-identically (the static adapter import, the static island
/// imports + the `modules` map, `installBootRuntime()`,
/// `window.__oxideRegisterAdapter(clientAdapter, ...)`, the §8.4 loading
/// map and the `bootWithRouter` tail), and a mixed registry emits the
/// registry shape — one lazy adapter import per framework present and an
/// `islands` map carrying `{ fw, load }` per entry
/// (`installBootRuntime({ adapters, islands })`). The mixed entry is what
/// the build's client bundle bundles: the lazy adapter imports pull the
/// rolldown `advancedChunks` per-framework vendor chunks the plan's
/// `vendor_groups` derived, and the client's mixed registry hydrates each
/// island through its own adapter (the foreign island's SSR HTML the
/// worker's `island:render` op produced). Bundled as a single client
/// chunk and injected into every SSR'd page as
/// `<script type="module" src="/<boot-url>">`.
pub(crate) fn generate_island_boot_entry(
    root: &Path,
    framework: &str,
    islands: &BTreeMap<String, oxide_fw_config::IslandSpec>,
    // §8.4 loading boundaries: URL prefix (`/`, `/blog`) → on-disk
    // `loading.tsx` path. Emitted as static imports + a `loading` map passed
    // to `bootWithRouter`.
    loading: &BTreeMap<String, PathBuf>,
    // §16.6 / §V2-F14: whether the build emitted a service worker — the
    // emitter's `offline.enabled` + `sw.enabled` gate. When true, the boot
    // entry loads `/pwa-register.js` so the worker registers.
    pwa: bool,
) -> String {
    oxide_fw_compiler::islands::plan_island_boot(
        framework,
        &resolve_islands(root, framework, islands),
    )
    .boot_entry_source(root, loading, pwa)
}

/// §V2-F01: the `.oxide/islands.json` tooling payload — every registry
/// entry resolved through the same inference order the boot entry used,
/// serialized by the compiler-side boot planner. The file read is
/// best-effort: an unreadable island file contributes an empty pragma
/// line, so the inference still answers (the project framework).
pub(crate) fn islands_json_source(
    root: &Path,
    framework: &str,
    islands: &BTreeMap<String, oxide_fw_config::IslandSpec>,
) -> String {
    oxide_fw_compiler::islands::plan_island_boot(
        framework,
        &resolve_islands(root, framework, islands),
    )
    .islands_json()
}

/// §V2-F01: write the `.oxide/islands.json` tooling payload for the
/// merged registry (`oxide why` / the F17 overlays read it). The build
/// path calls this beside its boot-entry write (the spec's item 2: the
/// payload exists "at build for tooling"); the dev path writes the same
/// payload through `islands_json_source` directly.
pub(crate) fn write_islands_json(
    root: &Path,
    framework: &str,
    cfg: &oxide_fw_config::OxideConfig,
    tree: &oxide_fw_router::RouteTree,
    w: Writer<'_>,
) -> anyhow::Result<()> {
    let islands = islands_for_boot(cfg, tree, root, w)?;
    let dir = root.join(".oxide");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join("islands.json"),
        islands_json_source(root, framework, &islands),
    )?;
    Ok(())
}

/// §6.3: the JSX runtime for a framework. React uses Oxc's default
/// (`react/jsx-runtime`); Solid 2.0's runtime lives in `@solidjs/web`,
/// whose `jsx-runtime` subpath exports `jsx`/`jsxs`/`Fragment`. Vue and
/// Svelte entries arrive precompiled from the tooling pool, so their
/// `.vue`/`.svelte` files never reach the TS/JSX lowering pass — but a
/// stray `.tsx` in a Vue project still needs the default.
///
/// This mirrors the `jsxImportSource` field the adapter packages declare
/// in their manifests. When the config carries an explicit framework
/// descriptor with a jsxImportSource, prefer that; today the CLI resolves
/// it from the framework name.
pub(crate) fn jsx_import_source_for(framework: &str) -> Option<String> {
    // BUG-13: honour the adapter's `jsxImportSource` (contract §7,
    // `packages/core/src/types.ts`). React uses the Oxc default runtime
    // (`react/jsx-runtime`), which is what `None` means. Solid's renderer
    // lives in `@solidjs/web`, not the React factory.
    match framework {
        "solid" => Some("@solidjs/web".to_string()),
        _ => None,
    }
}

/// Write one prerendered HTML file. When `page_renderer` is attached (§7),
/// the shell comes from the framework adapter; otherwise the neutral-IR
/// placeholder. Either way, `oxide_fw_render::Injector` wraps the shell with
/// the CSP nonce + hydration bootstrap (§10.2 step 4).
/// §10.3: inject the boot script into an SSR'd shell. Placed right before
/// `</body>` so the parser sees the DOM the island scanner will walk.
/// Nonce-carrying if the shell already uses one; the boot chunk carries
/// only static imports and a `registerAdapter` call, so it is a module
/// script and inherits the document's script policy.
pub(crate) fn inject_island_boot_script(
    shell: &str,
    boot_url: &str,
    nonce: &str,
    integrity: Option<&str>,
) -> String {
    // §17.1: the CSP uses `strict-dynamic`, which requires a nonce on every
    // script tag — `'self'` alone is not enough. The prerender pass has the
    // build-time nonce (derived from build_id) in scope; the live server
    // path derives a per-request nonce and stamps it the same way.
    // Normalize the leading slash: the bundler path passes
    // `client/__boot__.js` (no leading slash) and the native-ESM dev
    // path passes `/@boot/boot.mjs` (already leading-slashed). A blind
    // format!("/{boot_url}") produced `//@boot/boot.mjs`, which the
    // browser parses as a protocol-relative URL (`//user:pass@host/path`)
    // → `http://boot/boot.mjs` → ERR_NAME_NOT_RESOLVED.
    let tag = boot_script_tag(boot_url, nonce, integrity);
    if let Some(idx) = shell.rfind("</body>") {
        let mut out = String::with_capacity(shell.len() + tag.len() + 1);
        out.push_str(&shell[..idx]);
        out.push_str(&tag);
        out.push_str(&shell[idx..]);
        out
    } else {
        format!("{shell}\n{tag}")
    }
}
