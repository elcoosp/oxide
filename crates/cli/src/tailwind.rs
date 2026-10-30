// Section 6.6 / D5: native Tailwind v4 integration.
use std::path::{Path, PathBuf};
use std::sync::Arc;

use oxide_fw_assets::{CssBackend as _, CssInput, CssKind, LightningCssBackend};
use oxide_fw_config::OxideConfig;
use oxide_fw_diagnostics::{Diagnostic, OxideError};
use oxide_fw_jshost::{Pool, ToolingModules};

const SOURCE_EXTENSIONS: [&str; 7] = ["ts", "tsx", "js", "jsx", "vue", "svelte", "html"];

pub struct TailwindOutput {
    pub entry: PathBuf,
    pub css: String,
}

fn bld_err(msg: impl Into<String>) -> OxideError {
    OxideError(Box::new(Diagnostic::error("BLD-TAILWIND", msg)))
}

/// True when the config declares the @oxide-fw/tailwind plugin.
///
/// The static config extractor parses `plugins: [tailwind()]` into
/// `{ name: "tailwind" }` — same convention as `framework: react()` becoming
/// `{ name: "react" }`. The runtime object produced by `tailwind()` is
/// `{ name: "@oxide-fw/tailwind", engine: "tailwind" }`. Match all three forms.
pub fn has_tailwind(cfg: &OxideConfig) -> bool {
    cfg.plugins.iter().any(|p| {
        p.get("engine").and_then(|e| e.as_str()) == Some("tailwind")
            || p.get("name").and_then(|n| n.as_str()) == Some("@oxide-fw/tailwind")
            || p.get("name").and_then(|n| n.as_str()) == Some("tailwind")
    })
}

/// True for source files that could contain tailwind class names.
/// Excludes .server.ts/.server.tsx (server-only, no client classes),
/// oxide.config.* (the config file itself), and .css/.scss/.sass
/// (those go through HmrBehavior::StyleSwap).
pub fn is_source_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".server.ts") || lower.ends_with(".server.tsx") {
        return false;
    }
    if lower.contains("oxide.config") {
        return false;
    }
    if lower.ends_with(".css") || lower.ends_with(".scss") || lower.ends_with(".sass") {
        return false;
    }
    [".ts", ".tsx", ".js", ".jsx", ".vue", ".svelte", ".html"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

async fn compute_css(
    pool: Arc<dyn Pool>,
    cfg: &OxideConfig,
    root: &Path,
) -> Result<Option<(PathBuf, String)>, OxideError> {
    let entry = match find_entry_css(root) {
        Ok(Some(p)) => p,
        Ok(None) => {
            eprintln!(
                "oxide: tailwind plugin active but no stylesheet under src/styles/ contains `@import \"tailwindcss\"` — skipping Tailwind"
            );
            return Ok(None);
        }
        Err(e) => return Err(bld_err(format!("walk src/styles: {e}"))),
    };

    let src_dir = root.join("src");
    // [wr104] Pass source globs so Scanner.scan() walks src/ directly.
    let sources = vec![serde_json::json!({
        "base": src_dir.to_string_lossy(),
        "pattern": "**/*.{ts,tsx,js,jsx,vue,svelte,html}",
        "negated": false
    })];

    let tooling = ToolingModules::new(pool);
    let candidates = tooling.tailwind_scan(sources, Vec::new()).await?;
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

    Ok(Some((entry, out.code)))
}

/// [wr104] Fallback: if src/styles/*.css has `@import "tailwindcss"`,
/// Tailwind is active. Catches `plugins: [tailwind()]` when the static
/// config extractor fails on zero-arg factory calls.
fn css_has_tailwind_import(root: &Path) -> bool {
    // Walk src/styles recursively (future-proof) and src/styles itself.
    fn scan_dir(dir: &Path) -> bool {
        let Ok(entries) = std::fs::read_dir(dir) else { return false };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if scan_dir(&path) { return true; }
                continue;
            }
            if !path.extension().map(|e| e == "css").unwrap_or(false) { continue; }
            let Ok(content) = std::fs::read_to_string(&path) else { continue };
            if content.contains("@import \"tailwindcss\"")
                || content.contains("@import 'tailwindcss'")
            {
                return true;
            }
        }
        false
    }
    scan_dir(&root.join("src/styles"))
}

pub async fn precompute(
    pool: Arc<dyn Pool>,
    cfg: &OxideConfig,
    root: &Path,
) -> Result<Option<TailwindOutput>, OxideError> {
    if !has_tailwind(cfg) && !css_has_tailwind_import(root) {
        return Ok(None);
    }
    match compute_css(pool, cfg, root).await? {
        Some((entry, css)) => Ok(Some(TailwindOutput { entry, css })),
        None => Ok(None),
    }
}

/// Dev-time HMR: rebuild CSS and overwrite the existing dist/client/tailwind-*.css.
/// Returns (url, css) when the CSS changed, None when no-op.
pub async fn rebuild_for_dev(
    pool: Arc<dyn Pool>,
    cfg: &OxideConfig,
    root: &Path,
) -> Result<Option<(String, String)>, OxideError> {
    if !has_tailwind(cfg) {
        return Ok(None);
    }

    let dist = root.join("dist").join("client");
    let existing = std::fs::read_dir(&dist).ok().and_then(|rd| {
        rd.filter_map(Result::ok).map(|e| e.path()).find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("tailwind-") && n.ends_with(".css"))
                .unwrap_or(false)
        })
    });
    let Some(existing_path) = existing else {
        return Ok(None);
    };

    let Some((_entry, new_css)) = compute_css(pool, cfg, root).await? else {
        return Ok(None);
    };

    let current = std::fs::read_to_string(&existing_path).unwrap_or_default();
    if current == new_css {
        return Ok(None);
    }

    std::fs::write(&existing_path, &new_css)
        .map_err(|e| bld_err(format!("write {}: {e}", existing_path.display())))?;

    let basename = existing_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("tailwind.css");
    Ok(Some((format!("/client/{basename}"), new_css)))
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
