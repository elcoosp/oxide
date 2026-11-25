//! §10.4 / P0-4: the build-time PPR shell seed — the half the runtime
//! shell cache never had.
//!
//! The runtime half (B-27, `crate::prerender`) caches a shell when the
//! FIRST request streams through the render stage (`ppr_tap_stream`), so
//! every deploy re-charges the warm-up miss to the first visitor. §10.4's
//! contract — "the static shell up to each boundary is cached in L0, and
//! only the holes render per request and stream into the cached shell" —
//! names a build-time half the tap cannot supply. The prerender walk
//! (`phase_prerender`) already renders every static page at build; a page
//! whose body carries `<template data-ox-b>` boundaries yields its shell
//! through the same `oxide_fw_render::ppr::split_shell` the runtime tap
//! runs, under the SAME canonical L0 key the render stage computes
//! ([`crate::l0_cache::L0Cache::key_for`]).
//!
//! This module is the ONE definition site of the seed contract:
//!
//! - [`ppr_shell_seed_rows`] turns the walk's rendered rows into shell rows
//!   (boundary-free pages and non-default locales contribute nothing — the
//!   §16.4/O2 rule the L0 seed's own doc records: the L0 vary is
//!   header-only today, so locale variants collapse onto the
//!   default-locale key);
//! - [`ppr_shell_seed_manifest`] encodes the persisted artifact
//!   (`dist/server/ppr-shells.json`, the `dist/server/l0-seed.json`
//!   pattern: a `build_id` header plus the entries);
//! - [`parse_ppr_shell_seed`] is the boot's read side — same-build rows,
//!   a stale-build skip (the normal post-redeploy artifact), or a corrupt
//!   manifest the boot warns about under the CACHE-SEED posture.
//!
//! `phase_ppr_seed` writes the manifest; the serve boot seeds the
//! [`crate::PprShellStore`] from it. A missing manifest (dev, or a build
//! with no boundary-carrying pages) stays a silent no-op — the pre-M1
//! shape.

use crate::build::BuildCtx;

/// One rendered page row the shell collector consumes — exactly the facts
/// the prerender walk holds per target.
#[derive(Debug, Clone)]
pub struct PprSeedSource {
    /// The canonical route id (the walk's `canonical_route_id`, the same
    /// identity the render stage's `l0_route` carries).
    pub route_id: String,
    /// The prerendered URL path (`/about`), the `path:` tag's spelling.
    pub url_path: String,
    /// The target's params in the route stage's `l0_params` spelling
    /// (sorted `k=v` pairs joined by `&`; empty when paramless).
    pub params: String,
    /// The SENTINELED prerender body (`splice_nonce` already swapped the
    /// build nonce for [`NONCE_SENTINEL`] — the same post-processing the
    /// runtime tap applies before it stores a shell).
    pub sentineled_html: String,
    /// The target's locale variant (`""` for the un-prefixed default).
    pub locale: String,
}

/// One persisted shell row: the canonical L0 key, the URL it serves, the
/// sentineled shell HTML and the invalidation tags (the runtime tap's
/// `(route, path:<path>)` pair).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PprShellSeedRow {
    /// [`crate::l0_cache::L0Cache::key_for`] over (route, params, vary,
    /// build) — the exact key the render stage's shell lookup computes.
    pub key: String,
    /// The prerendered URL path the row serves.
    pub url: String,
    /// The shell HTML with `<ox-hole>` placeholders, nonce still sentinel.
    pub shell: String,
    /// The runtime tap's tag pair, so `revalidate`/path invalidation
    /// reaches seeded rows the same way it reaches tapped ones.
    pub tags: Vec<String>,
}

/// The route stage's `l0_params` shape (sorted `k=v` pairs joined by `&`)
/// for a params map — the spelling [`PprSeedSource`] carries.
///
/// The walk's targets hold `BTreeMap`s (already key-sorted), so iteration
/// order matches the route stage's own sorted join; the key-fidelity pin
/// (`ppr_seed_boot_store_serves_the_first_request_without_a_tap`) holds
/// the two spellings equal end to end.
#[must_use]
pub fn l0_params_string(params: &std::collections::BTreeMap<String, String>) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}

/// Split the walk's rendered rows into shell rows (§10.4's build-time
/// half).
///
/// A row becomes a shell row only when its body carries boundaries (a
/// boundary-free page has no shell to cache — the full page IS the static
/// answer). A malformed boundary marker (the renderer emitted something
/// [`oxide_fw_render::ppr::split_shell`] refuses) is skipped with a loud
/// warn naming the route and URL — the F65-session loud convention, never
/// a silent drop. Non-default locales contribute nothing (the §16.4/O2
/// seed rule above).
#[must_use]
pub fn ppr_shell_seed_rows(
    sources: &[PprSeedSource],
    build_id: &str,
    vary: &str,
    default_locale: &str,
) -> Vec<PprShellSeedRow> {
    let mut rows = Vec::new();
    for source in sources {
        let is_default = source.locale.is_empty() || source.locale == default_locale;
        if !is_default {
            continue;
        }
        let shell = match oxide_fw_render::ppr::split_shell(&source.sentineled_html) {
            Ok(shell) if !shell.hole_ids.is_empty() => shell,
            Ok(_) => continue,
            Err(e) => {
                tracing::warn!(
                    code = "RND-PPR",
                    route = %source.route_id,
                    url = %source.url_path,
                    error = %e,
                    "ppr seed: the prerendered body carries a malformed boundary; no shell row was built"
                );
                continue;
            }
        };
        rows.push(PprShellSeedRow {
            key: crate::l0_cache::L0Cache::key_for(
                &source.route_id,
                &source.params,
                vary,
                build_id,
            ),
            url: source.url_path.clone(),
            shell: shell.html,
            tags: vec![source.route_id.clone(), format!("path:{}", source.url_path)],
        });
    }
    rows
}

/// Encode the persisted manifest (`dist/server/ppr-shells.json`) — the
/// `dist/server/l0-seed.json` shape: a `build_id` header plus the entries,
/// so the boot can refuse another build's artifact before inserting it.
#[must_use]
pub fn ppr_shell_seed_manifest(rows: &[PprShellSeedRow], build_id: &str) -> Vec<u8> {
    let entries: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            serde_json::json!({
                "key": row.key,
                "url": row.url,
                "shell": row.shell,
                "tags": row.tags,
            })
        })
        .collect();
    serde_json::to_vec_pretty(&serde_json::json!({
        "build_id": build_id,
        "entries": entries,
    }))
    .unwrap_or_default()
}

/// The boot's read verdict over a present manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PprShellSeed {
    /// Rows for THIS build — the boot inserts each into its shell store.
    Rows(Vec<PprShellSeedRow>),
    /// A manifest from a DIFFERENT build (the normal post-redeploy
    /// artifact, the L0 seed's `same_build` rule) — skipped silently.
    StaleBuild,
    /// A present-but-unreadable manifest — the boot warns under the
    /// CACHE-SEED posture instead of silently skipping the warm-up.
    Corrupt(String),
}

/// Parse a present manifest for a boot with `build_id`.
#[must_use]
pub fn parse_ppr_shell_seed(bytes: &[u8], build_id: &str) -> PprShellSeed {
    let value: serde_json::Value = match serde_json::from_slice(bytes) {
        Ok(v) => v,
        Err(e) => return PprShellSeed::Corrupt(e.to_string()),
    };
    if value.get("build_id").and_then(|b| b.as_str()) != Some(build_id) {
        return PprShellSeed::StaleBuild;
    }
    let Some(entries) = value.get("entries").and_then(|e| e.as_array()) else {
        return PprShellSeed::Corrupt("the manifest carries no entries array".to_string());
    };
    let rows = entries
        .iter()
        .filter_map(|e| {
            Some(PprShellSeedRow {
                key: e.get("key")?.as_str()?.to_string(),
                url: e.get("url")?.as_str()?.to_string(),
                shell: e.get("shell")?.as_str()?.to_string(),
                tags: e
                    .get("tags")?
                    .as_array()?
                    .iter()
                    .map(|t| t.as_str().unwrap_or_default().to_string())
                    .collect(),
            })
        })
        .collect();
    PprShellSeed::Rows(rows)
}

/// §10.4 / P0-4: write the build's shell-seed manifest — the build-time
/// half of the PPR shell cache. Runs after [`phase_seed_l0`], over the rows
/// [`phase_prerender`] collected while it walked (zero rows when
/// `render.ppr` is off, or no prerendered page carried boundaries: no
/// manifest, the pre-M1 shape).
pub(crate) fn phase_ppr_seed(cx: &mut BuildCtx<'_>) -> anyhow::Result<()> {
    let dist = &cx.dist;
    let w = &mut *cx.w;
    let rows = cx
        .ppr_shell_rows
        .take()
        .expect("phase_prerender sets ppr_shell_rows");
    if rows.is_empty() {
        return Ok(());
    }
    let build_id = cx
        .build_id
        .as_ref()
        .expect("phase_bundle_server sets build_id");
    let bytes = ppr_shell_seed_manifest(&rows, build_id);
    let path = dist.join("server/ppr-shells.json");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // The seed_l0 posture: a vanished write is the silent warm-up miss
    // nobody could diagnose — warn with the path and the error.
    if bytes.is_empty() {
        tracing::warn!(
            path = %path.display(),
            "ppr shell manifest serialize failed; first requests will tap instead of hitting"
        );
    } else if let Err(e) = std::fs::write(&path, &bytes) {
        tracing::warn!(
            path = %path.display(),
            error = %e,
            "ppr shell manifest write failed; first requests will tap instead of hitting"
        );
    } else {
        outln!(
            w,
            "ppr shells: {} shell(s) -> dist/server/ppr-shells.json",
            rows.len()
        );
    }
    Ok(())
}
