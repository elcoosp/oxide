// §6.6 / D5: native Tailwind v4 integration.
//
// When the config declares `plugins: [{ engine: "tailwind", ... }]`, this
// module precomputes the full Tailwind CSS for the project's entry stylesheet
// and hands it to `build_css_transform_hook` — which substitutes it in when
// the bundler reaches that file. Everything else (other CSS files) still goes
// through Lightning CSS as before.
//
// The actual Tailwind work runs in `@oxide-fw/tailwind/runtime.mjs` via the
// tooling pool (`ToolingModules::tailwind_scan` / `tailwind_build`), which
// uses Tailwind's own Rust scanner (`@tailwindcss/oxide`) and its own JS
// compiler (`tailwindcss`). We do not reimplement Tailwind.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use oxide_fw_assets::{CssBackend as _, CssInput, CssKind, LightningCssBackend};
use oxide_fw_config::OxideConfig;
use oxide_fw_diagnostics::{Diagnostic, OxideError};
use oxide_fw_jshost::{Pool, ToolingModules};

const SOURCE_EXTENSIONS: [&str; 7] = ["ts", "tsx", "js", "jsx", "vue", "svelte", "html"];

/// The result of precomputing Tailwind for a project: which entry stylesheet
/// was processed, and its fully compiled CSS (Tailwind output already run
/// through Lightning CSS for the target browsers).
pub struct TailwindOutput {
    pub entry: PathBuf,
    pub css: String,
}

fn bld_err(msg: impl Into<String>) -> OxideError {
    OxideError(Diagnostic::error("BLD-TAILWIND", msg))
}

/// Scan project sources, run Tailwind's compiler, run Lightning CSS, and
/// return the final CSS keyed to the entry stylesheet path.
///
/// Returns `Ok(None)` when:
///   - the config doesn't declare the `@oxide-fw/tailwind` plugin, or
///   - the plugin is declared but no CSS file under `src/styles/` contains
///     `@import "tailwindcss"` (a warning is logged).
///
/// Errors only on genuine failures (I/O, Tailwind scanner/compiler crash,
/// Lightning CSS parse error) — never on absence.
pub async fn precompute(
    pool: Arc<dyn Pool>,
    cfg: &OxideConfig,
    root: &Path,
) -> Result<Option<TailwindOutput>, OxideError> {
    let has_tailwind = cfg.plugins.iter().any(|p| {
        p.get("engine").and_then(|e| e.as_str()) == Some("tailwind")
            || p.get("name").and_then(|n| n.as_str()) == Some("@oxide-fw/tailwind")
    });
    if !has_tailwind {
        return Ok(None);
    }

    let entry = match find_entry_css(root) {
        Ok(Some(p)) => p,
        Ok(None) => {
            eprintln!(
                "oxide: tailwind plugin active but no stylesheet under src/styles/ contains \
                 `@import \"tailwindcss\"` — skipping Tailwind"
            );
            return Ok(None);
        }
        Err(e) => return Err(bld_err(format!("walk src/styles: {e}"))),
    };

    let src_dir = root.join("src");
    let mut files: Vec<serde_json::Value> = Vec::new();
    collect_sources(&src_dir, &mut files)
        .map_err(|e| bld_err(format!("walk src: {e}")))?;
    if files.is_empty() {
        eprintln!(
            "oxide: tailwind plugin active but no source files found under {} — skipping",
            src_dir.display()
        );
        return Ok(None);
    }

    let tooling = ToolingModules::new(pool);
    let candidates = tooling.tailwind_scan(files).await?;

    let css_src = std::fs::read_to_string(&entry)
        .map_err(|e| bld_err(format!("read {}: {e}", entry.display())))?;
    let base = entry
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let tailwind_css = tooling.tailwind_build(&css_src, &base, &candidates).await?;

    let input = CssInput {
        code: tailwind_css,
        file: Some(entry.to_string_lossy().to_string()),
        kind: CssKind::Global,
        minify: false,
        targets: vec![cfg.build.target.clone()],
    };
    let out = LightningCssBackend
        .transform(&input)
        .map_err(|e| bld_err(format!("lightningcss: {}", e.0.message)))?;

    Ok(Some(TailwindOutput {
        entry,
        css: out.code,
    }))
}

fn find_entry_css(root: &Path) -> std::io::Result<Option<PathBuf>> {
    let styles = root.join("src").join("styles");
    if !styles.is_dir() {
        return Ok(None);
    }
    for entry in std::fs::read_dir(&styles)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("css") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if text.contains("@import \"tailwindcss\"") || text.contains("@import 'tailwindcss'") {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

fn collect_sources(dir: &Path, out: &mut Vec<serde_json::Value>) -> std::io::Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_sources(&path, out)?;
            continue;
        }
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        if !SOURCE_EXTENSIONS.contains(&ext) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        out.push(serde_json::json!({
            "content": text,
            "extension": ext,
        }));
    }
    Ok(())
}
