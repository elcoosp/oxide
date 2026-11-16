//! Moved verbatim from lib.rs by scripts/split_cli.py (V2-A01).

use crate::*;

/// SPEC-21 / §16.6 / §V2-F14: emit the PWA worker + its registration over
/// the bundler's emitter (`oxide_fw_bundler::pwa`). The call-site gate is
/// the emitter's documented contract: the build produced a client manifest
/// (upstream guarantees it — the build bails when the bundler wrote none)
/// and `offline.enabled` + `sw.enabled` both hold. `offline.enabled =
/// false` "installs no service worker" (the `OfflineConfig` docs — this is
/// the default posture, so the skip is silent); `sw.enabled = false`
/// disables emission even with offline on (the `SwConfig` docs — loud, a
/// configured-but-refused feature is worth a line).
///
/// The prerendered-shell walk is the call site's [`ShellSource`]
/// implementation: the emitter stays pure over the client manifest.
pub(crate) fn emit_pwa_worker(
    cfg: &oxide_fw_config::OxideConfig,
    dist: &Path,
    client_manifest_snapshot: &serde_json::Value,
    w: Writer<'_>,
) -> anyhow::Result<()> {
    if !cfg.offline.enabled {
        return Ok(());
    }
    if !cfg.sw.enabled {
        outln!(w, "pwa: sw disabled by config (sw.enabled = false)");
        return Ok(());
    }
    // The snapshot came from the manifest the bundler wrote, so a shape
    // mismatch here is a bundler/CLI contract bug — loud, not a silent skip.
    let manifest: oxide_fw_bundler::ClientManifest =
        serde_json::from_value(client_manifest_snapshot.clone())
            .map_err(|e| anyhow::anyhow!("pwa: client manifest does not deserialize: {e}"))?;
    let shells = PrerenderedShells::collect(&dist.join("prerendered"))?;
    let plan = oxide_fw_bundler::pwa::plan_precache(&manifest, &cfg.sw, &cfg.offline, &shells);
    oxide_fw_bundler::pwa::emit_pwa(dist, &plan)
        .map_err(|e| anyhow::anyhow!("write pwa emission: {e}"))?;
    outln!(
        w,
        "pwa: sw.js + pwa-register.js emitted (cache ox-{}, fallback {}, {} precached)",
        plan.build_id,
        plan.offline_fallback,
        plan.precache.len()
    );
    Ok(())
}

impl oxide_fw_bundler::pwa::ShellSource for PrerenderedShells {
    fn shell_urls(&self) -> std::collections::BTreeSet<String> {
        self.urls.clone()
    }
}

pub(crate) fn emit_web_manifest(
    cfg: &oxide_fw_config::OxideConfig,
    dist: &Path,
    w: Writer<'_>,
) -> anyhow::Result<()> {
    let Some(m) = &cfg.manifest else {
        return Ok(());
    };
    let mut json = serde_json::Map::new();

    // A browser needs at least one of name / short_name.
    let name = m.name.clone().or_else(|| m.short_name.clone());
    let short = m.short_name.clone().or_else(|| m.name.clone());
    if let Some(n) = name {
        json.insert("name".into(), serde_json::Value::String(n));
    }
    if let Some(s) = short {
        json.insert("short_name".into(), serde_json::Value::String(s));
    }
    json.insert(
        "start_url".into(),
        serde_json::Value::String(m.start_url.clone().unwrap_or_else(|| "/".into())),
    );
    if let Some(d) = &m.display {
        json.insert("display".into(), serde_json::Value::String(d.clone()));
    }
    if let Some(c) = &m.background_color {
        json.insert(
            "background_color".into(),
            serde_json::Value::String(c.clone()),
        );
    }
    if let Some(c) = &m.theme_color {
        json.insert("theme_color".into(), serde_json::Value::String(c.clone()));
    }
    let icons: Vec<serde_json::Value> = m
        .icons
        .iter()
        .map(|i| {
            let mut obj = serde_json::Map::new();
            obj.insert("src".into(), serde_json::Value::String(i.src.clone()));
            obj.insert("sizes".into(), serde_json::Value::String(i.sizes.clone()));
            let ty = i.r#type.clone().or_else(|| {
                i.src
                    .rsplit_once('.')
                    .map(|(_, ext)| format!("image/{ext}"))
            });
            if let Some(t) = ty {
                obj.insert("type".into(), serde_json::Value::String(t));
            }
            if let Some(p) = &i.purpose {
                obj.insert("purpose".into(), serde_json::Value::String(p.clone()));
            }
            serde_json::Value::Object(obj)
        })
        .collect();
    if !icons.is_empty() {
        json.insert("icons".into(), serde_json::Value::Array(icons));
    }

    let body = serde_json::to_string_pretty(&serde_json::Value::Object(json))
        .map_err(|e| anyhow::anyhow!("serialize manifest: {e}"))?;
    std::fs::write(dist.join("manifest.webmanifest"), format!("{body}\n"))
        .map_err(|e| anyhow::anyhow!("write manifest.webmanifest: {e}"))?;
    outln!(w, "manifest: manifest.webmanifest emitted");
    Ok(())
}
