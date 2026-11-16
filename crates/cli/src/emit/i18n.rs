//! Moved verbatim from lib.rs by scripts/split_cli.py (V2-A01).

use crate::*;

/// §16.4: stamp the project's locales into `dist/client/client.manifest.json`.
///
/// The client fetches `/client/messages/<locale>.json` on every page load to
/// warm the catalog. A project with no i18n has no such file, so that fetch
/// is a guaranteed 404 — which Chromium reports as a console error and which
/// breaks any test asserting a clean console.
///
/// The bundler does not know about i18n (it is framework- and feature-
/// agnostic), so the CLI — which owns the config — stamps the answer into the
/// manifest it already post-processes. The client then skips the fetch when
/// the list is empty.
pub(crate) fn stamp_i18n_locales(root: &Path, cfg: &oxide_fw_config::OxideConfig) {
    let manifest_path = root.join("dist/client/client.manifest.json");
    let Ok(raw) = std::fs::read_to_string(&manifest_path) else {
        return;
    };
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return;
    };
    let Some(obj) = v.as_object_mut() else { return };
    // The signal is "were catalogs emitted", not "what does config say":
    // `i18n.locales` defaults to `["en"]` even for a project that defines no
    // messages, so a config-driven check would claim catalogs exist and the
    // client would 404 on every load. `dist/client/messages/` is the artifact
    // that decides it.
    let emitted = root.join("dist/client/messages");
    let has_catalogs = std::fs::read_dir(&emitted)
        .map(|rd| {
            rd.flatten()
                .any(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
        })
        .unwrap_or(false);
    if has_catalogs {
        obj.insert(
            "i18n_locales".to_string(),
            serde_json::json!(cfg.i18n.locales),
        );
    } else {
        obj.remove("i18n_locales");
    }
    if let Ok(out) = serde_json::to_string_pretty(&v) {
        let _ = std::fs::write(&manifest_path, out + "\n");
    }
}

/// Extract the message keys a route's source names via `t('key')` or
/// `t("key")`. A conservative string scan: a route that builds a key
/// dynamically (`t(`nav.${x}`)`) names no static key and therefore ships no
/// subset — it falls back to the full catalog, which is correct.
pub(crate) fn scan_message_keys(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for quote in ['\'', '"'] {
        let pat = format!("t({quote}");
        let mut rest = src;
        while let Some(i) = rest.find(&pat) {
            let after = &rest[i + pat.len()..];
            if let Some(end) = after.find(quote) {
                let key = &after[..end];
                if !key.is_empty()
                    && key
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == '.' || c == '_' || c == '-')
                {
                    out.push(key.to_string());
                }
                rest = &after[end + 1..];
            } else {
                break;
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Find the 1-based `(line, column)` of the first occurrence of `needle` in
/// `src`. Returns `None` when the literal is not found as-is (a multi-line
/// string, or an escaped form) — in which case the diagnostic carries no
/// frame rather than pointing at the wrong line.
pub(crate) fn find_literal_line(src: &str, needle: &str) -> Option<(usize, usize)> {
    if needle.is_empty() {
        return None;
    }
    let mut offset = 0usize;
    for (i, line) in src.lines().enumerate() {
        if let Some(byte_col) = line.find(needle) {
            // Column is a character count within the line.
            let col = line[..byte_col].chars().count() + 1;
            return Some((i + 1, col));
        }
        offset += line.len() + 1;
        let _ = offset;
    }
    None
}
