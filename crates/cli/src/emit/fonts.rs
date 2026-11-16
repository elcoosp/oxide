//! Moved verbatim from lib.rs by scripts/split_cli.py (V2-A01).

use crate::*;

/// §16.3: write `manifest.webmanifest` when the config declares a
/// `manifest` block. The emitted JSON is the standard web app manifest
/// shape — the fields a PWA needs to be installable.
///
/// `start_url` defaults to `/`. `name` defaults to `shortName` (and vice
/// versa), because a manifest with neither is rejected by some browsers.
/// An icon `type` is inferred from the extension when omitted.
/// §16.2: bundle every configured font family, emit the face files under
/// `dist/fonts/`, and write a `dist/fonts.css` carrying the `@font-face`
/// rules (including the metric-override fallback faces that eliminate
/// font-swap layout shift). A family whose files cannot be read is reported
/// and skipped; fonts are never load-bearing for a build.
pub(crate) fn emit_fonts(
    cfg: &oxide_fw_config::OxideConfig,
    root: &Path,
    dist: &Path,
    w: Writer<'_>,
) -> anyhow::Result<()> {
    use oxide_fw_assets::{DefaultFontBackend, FontBackend as _, FontSpec};

    if cfg.fonts.families.is_empty() {
        return Ok(());
    }
    let fonts_dir = dist.join("fonts");
    std::fs::create_dir_all(&fonts_dir)
        .map_err(|e| anyhow::anyhow!("create {}: {e}", fonts_dir.display()))?;

    let mut all_css = String::new();
    let mut emitted = 0usize;
    // V2-C08 (steps 1-2): the glyph-usage strategy harvests the text the
    // build actually renders — the prerendered pages' body text plus the
    // content-collection field text — into one char set every family
    // subsets to (the spec's single-set-per-family simplification). A
    // build that did not opt into `used` skips the harvest entirely.
    let used_chars = if cfg.fonts.subset.iter().any(|s| s == "used") {
        harvest_used_chars(cfg, root, dist)
    } else {
        std::collections::BTreeSet::new()
    };
    for fam in &cfg.fonts.families {
        let local_paths: Vec<std::path::PathBuf> = fam
            .local_paths
            .iter()
            .map(|p| {
                let pb = std::path::PathBuf::from(p);
                if pb.is_absolute() { pb } else { root.join(pb) }
            })
            .collect();
        let spec = FontSpec {
            family: fam.family.clone(),
            provider: fam.provider.clone(),
            weights: fam.weights.clone(),
            styles: fam.styles.clone(),
            subset: cfg.fonts.subset.clone(),
            // The same harvested set feeds EVERY family (the spec's
            // single-set-per-family simplification), so each iteration
            // clones it — the set is moved into the spec.
            used_chars: used_chars.clone(),
            local_paths,
        };
        match DefaultFontBackend.bundle(&spec) {
            Ok(bundle) => {
                for (name, bytes) in &bundle.files {
                    std::fs::write(fonts_dir.join(name), bytes)
                        .map_err(|e| anyhow::anyhow!("write font {name}: {e}"))?;
                    emitted += 1;
                }
                all_css.push_str(&bundle.css);
                all_css.push('\n');
            }
            Err(e) => {
                eprintln!(
                    "warn: font family '{}' skipped: {} {}",
                    fam.family, e.0.code, e.0.message
                );
            }
        }
    }
    if emitted == 0 && all_css.is_empty() {
        return Ok(());
    }
    std::fs::write(dist.join("fonts.css"), &all_css)
        .map_err(|e| anyhow::anyhow!("write fonts.css: {e}"))?;
    outln!(w, "fonts: {emitted} file(s), dist/fonts.css");
    Ok(())
}

/// V2-C08 (steps 1-2): the glyph-usage harvest — the prerendered pages'
/// body text (every `*.html` under `dist`, through the seam's `lol_html`
/// `text!`-over-body walk) unioned with the content-collection field text
/// (entry bodies + frontmatter strings). One set for every family (the
/// spec's honest simplification); a collection the scanner refuses is
/// skipped with a warning — fonts are never load-bearing for a build.
pub(crate) fn harvest_used_chars(
    cfg: &oxide_fw_config::OxideConfig,
    root: &Path,
    dist: &Path,
) -> std::collections::BTreeSet<char> {
    use oxide_fw_assets::harvest_text_codepoints;
    use oxide_fw_assets::{CollectionConfig, SchemaKind, harvest_html_codepoints};

    let mut pages: Vec<String> = Vec::new();
    collect_html_sources(dist, &mut pages, 0);
    let mut set = harvest_html_codepoints(pages.iter().map(String::as_str));

    for coll in &cfg.content.collections {
        let coll_cfg = CollectionConfig {
            name: coll.name.clone(),
            dir: root.join(&coll.dir),
            schema: SchemaKind::Unchecked,
            indexed_fields: coll.indexed_fields.clone(),
        };
        match oxide_fw_assets::scan_collection(&coll_cfg) {
            Ok(scanned) => {
                let sources = scanned
                    .entries
                    .iter()
                    .map(|e| e.body_markdown.as_str())
                    .chain(scanned.entries.iter().flat_map(|e| {
                        e.frontmatter.values().filter_map(serde_json::Value::as_str)
                    }));
                set.extend(harvest_text_codepoints(sources));
            }
            Err(e) => {
                tracing::warn!(
                    code = %e.0.code,
                    msg = %e.0.message,
                    collection = %coll.name,
                    "fonts: the collection glyph harvest was skipped"
                );
            }
        }
    }
    set
}

/// Every `*.html` file under `dir`, depth-capped (the prerendered pages
/// and any emitted HTML shells). Reads are best-effort: a file that fails
/// to read contributes nothing to the harvest. Sorted so the harvest is
/// deterministic regardless of directory iteration order.
pub(crate) fn collect_html_sources(dir: &Path, out: &mut Vec<String>, depth: usize) {
    if depth > 8 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<std::path::PathBuf> =
        entries.filter_map(Result::ok).map(|e| e.path()).collect();
    paths.sort();
    for p in paths {
        if p.is_dir() {
            collect_html_sources(&p, out, depth + 1);
        } else if p.extension().and_then(|e| e.to_str()) == Some("html")
            && let Ok(src) = std::fs::read_to_string(&p)
        {
            out.push(src);
        }
    }
}
