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
/// implementation: the emitter stays pure over the client manifest. The
/// emit site also owns the fallback answerability audit
/// ([`fallback_answerable_by_build`]) — the dist-grounded half of the
/// navigation contract's precache guarantee the pure plan cannot speak
/// for.
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
    // The navigation contract's audit arm: the plan guarantees the
    // fallback rides the install fetch; only the dist can say whether
    // the fetch answers. Unanswerable → loud, naming the key and the
    // value (the runtime-answer escape is in the line, so an SSR-route
    // fallback reads as a notice, not an error).
    if !fallback_answerable_by_build(dist, &shells, &plan.offline_fallback) {
        outln!(
            w,
            "pwa: offlineFallback '{}' is not a prerendered shell or dist artifact — the SW install fetch must be answered at runtime (an SSR route or a server rewrite) or the offline fallback fails",
            plan.offline_fallback
        );
    }
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

/// Whether THIS BUILD can answer the resolved offline-fallback URL — the
/// emit-site half of the navigation contract's precache guarantee (the
/// r9-K3 hazard's audit arm). The pure plan sees only the manifest and
/// the shells, so it can guarantee the fallback rides the install fetch
/// but not that the fetch SUCCEEDS; the emit site is where the dist
/// exists, so the answerability check lives here: the URL is answerable
/// when it is a prerendered shell ([`PrerenderedShells`], the navigation
/// URL spelling the walk reports) or a dist artifact the same URL spells
/// (`<path>.html`, `<path>/index.html`, or a plain file — the `public/`
/// copy convention).
///
/// A miss is a LOUD warn at the call site, never a refusal: the URL may
/// legitimately be answered at runtime by an SSR route or a server
/// rewrite the build cannot see — the build's job is to say so, the
/// operator's to fix the typo (`cache.addAll` rejects on any failed
/// fetch, so an unanswerable fallback kills the whole install and the
/// offline module dies silently without this line).
fn fallback_answerable_by_build(dist: &Path, shells: &PrerenderedShells, fallback: &str) -> bool {
    if shells.urls.contains(fallback) {
        return true;
    }
    let Some(rel) = fallback.strip_prefix('/') else {
        return false;
    };
    if rel.is_empty() {
        return dist.join("index.html").is_file();
    }
    dist.join(format!("{rel}.html")).is_file()
        || dist.join(rel).join("index.html").is_file()
        || dist.join(rel).is_file()
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
