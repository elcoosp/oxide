//! V2-F11: `oxide deploy` — the CLI orchestration over the landed
//! delta-deploy engine (`oxide_fw_adapters::delta`) and the deploy targets
//! (`dir:` via `DirStore`, `s3://`/`r2://` via the object-store
//! `DeployStore` behind the `deploy-s3` feature — the release builds'
//! counterpart of the `search` feature gate).
//!
//! The three operations the spec names map one-to-one onto the engine:
//! `oxide deploy <target>` runs [`deploy_dir`] (the §V2-F11 order of
//! operations: immutable chunks first, the release copy, the ledger, the
//! pointer swap LAST), `--prune` runs [`prune`] (the skew-window age-off;
//! the release the pointer names is never pruned — the rollback-safety
//! rule) and `--rollback` runs [`rollback`] (the one small pointer write).

use crate::*;

/// The deploy instant: epoch millis of "now". The engine takes it as a
/// parameter (the pins freeze it); the CLI hands the wall clock.
fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Build the `DeployStore` for a target string: `dir:/path` constructs the
/// local store; everything else is an object-store URL (`s3://bucket/prefix`,
/// `r2://...` — the S3 API) behind the `deploy-s3` feature. A release build
/// without the feature refuses the target loudly instead of silently
/// deploying to nowhere.
fn build_store(target: &str) -> anyhow::Result<std::sync::Arc<dyn oxide_fw_adapters::DeployStore>> {
    if let Some(path) = target.strip_prefix("dir:") {
        let root = std::path::PathBuf::from(path);
        if root.starts_with("~") {
            anyhow::bail!(
                "DEP-TARGET: expand the `~` in `{target}` before passing it (the CLI does not read your shell)"
            );
        }
        return Ok(std::sync::Arc::new(oxide_fw_adapters::DirStore::new(root)));
    }
    #[cfg(feature = "deploy-s3")]
    {
        let store = oxide_fw_adapters::deploy_target_from_url(target)?;
        return Ok(std::sync::Arc::new(store));
    }
    #[cfg(not(feature = "deploy-s3"))]
    {
        let _ = target;
        anyhow::bail!(
            "DEP-TARGET: object-store targets (`s3://`, `r2://`) need the `deploy-s3` \
             feature — rebuild with `--features deploy-s3`; `dir:/path` targets work \
             in every build"
        );
    }
}

/// `oxide deploy <target>` — deploy the last build's `dist/` (V2-F11).
///
/// The build id comes from the stamped build (`dist/build-id`, the A04
/// substrate): deploying an unstamped dist is refused — the pointer must
/// never name a guess.
pub(crate) fn cmd_deploy(
    target: &str,
    root: &str,
    prune_flag: bool,
    rollback_flag: bool,
    w: Writer<'_>,
) -> anyhow::Result<i32> {
    if prune_flag && rollback_flag {
        anyhow::bail!("usage: --prune and --rollback are separate operations");
    }
    let rootp = PathBuf::from(root);
    let dist = rootp.join("dist");

    let store = build_store(target)?;

    if prune_flag {
        // The skew window rides the config (`deploy.skewWindow`, the §14
        // duration grammar; the store's marks age off after it).
        let cfg = crate::config_load::load_config_strict(&rootp, "production")?;
        let window = std::time::Duration::from_secs(
            oxide_fw_config::parse_duration_secs(&cfg.deploy.skew_window)
                .unwrap_or(oxide_fw_adapters::DEFAULT_SKEW_WINDOW.as_secs()),
        );
        let report = oxide_fw_adapters::prune(
            &store,
            &oxide_fw_adapters::PruneOptions {
                now_ms: now_millis(),
                window,
            },
        )?;
        outln!(
            w,
            "prune ok — {} deleted, {} kept",
            report.deleted.len(),
            report.kept.len()
        );
        for p in &report.deleted {
            outln!(w, "  deleted {p}");
        }
        for p in &report.kept {
            outln!(w, "  kept    {p}");
        }
        return Ok(0);
    }

    if rollback_flag {
        let restored = oxide_fw_adapters::rollback(&store)?;
        outln!(w, "rollback ok — `current` now names {restored}");
        return Ok(0);
    }

    let Some(build_id) = oxide_fw_cache::read_build_id(&dist) else {
        anyhow::bail!(
            "DEP-BUILDID: {} has no stamped build id — run `oxide build` first \
             (the pointer must never name a guess)",
            dist.display()
        );
    };
    let plan = oxide_fw_adapters::deploy_dir(
        &dist,
        &store,
        &oxide_fw_adapters::DeployOptions {
            build_id,
            now_ms: now_millis(),
        },
    )?;
    outln!(
        w,
        "deploy ok — {} upload(s), {} keep, {} pending delete, {} bytes up",
        plan.upload.len(),
        plan.keep.len(),
        plan.delete_later.len(),
        plan.bytes_up
    );
    for p in &plan.upload {
        outln!(w, "  upload  {p}");
    }
    if plan.keep.len() <= 10 {
        for p in &plan.keep {
            outln!(w, "  keep    {p}");
        }
    }
    Ok(0)
}
