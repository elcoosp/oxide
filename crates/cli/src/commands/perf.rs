//! Moved verbatim from lib.rs by scripts/split_cli.py (V2-A01).

use crate::*;

/// `oxide perf bisect` (§V2-F18): find the first commit in the first-parent
/// `good..bad` range whose metric regresses past the threshold. Candidates
/// build in git worktrees under `.oxide/bisect/`, all sharing the project's
/// cache via `OXIDE_CACHE_DIR`; the median of `--runs` measurements is what
/// the spec-verbatim search compares. Probe failures surface as `PERF-*`
/// diagnostics with a fix line.
pub(crate) async fn cmd_perf_bisect(
    root: &str,
    good: &str,
    bad: &str,
    metric: perf_bisect::Metric,
    threshold: &str,
    runs: usize,
    w: Writer<'_>,
) -> anyhow::Result<i32> {
    let _span = tracing::info_span!(
        "cli_perf_bisect",
        good = %good,
        bad = %bad,
        metric = metric.label()
    )
    .entered();
    let rootp = PathBuf::from(root);
    let threshold_pct = perf_bisect::parse_threshold_pct(threshold)
        .map_err(|e| anyhow::Error::new(e.diagnostic()))?;

    let out = std::process::Command::new("git")
        .args(perf_bisect::rev_list_args(good, bad))
        .current_dir(&rootp)
        .output()
        .map_err(|e| {
            anyhow::Error::new(
                perf_bisect::BisectError::git_spawn_error("rev-list", &e).diagnostic(),
            )
        })?;
    if !out.status.success() {
        return Err(anyhow::Error::new(
            perf_bisect::BisectError::GitFailure {
                context: "rev-list".to_string(),
                detail: String::from_utf8_lossy(&out.stderr).trim().to_string(),
            }
            .diagnostic(),
        ));
    }
    let revs = perf_bisect::parse_rev_list_oldest_first(&String::from_utf8_lossy(&out.stdout));
    if revs.is_empty() {
        return Err(anyhow::Error::new(
            perf_bisect::BisectError::EmptyRange {
                good: good.to_string(),
                bad: bad.to_string(),
            }
            .diagnostic(),
        ));
    }

    outln!(
        w,
        "bisect {good}..{bad}: {} candidate(s), metric {}, threshold {threshold}, \
         median of {runs} run(s)",
        revs.len(),
        metric.label()
    );
    let mut probe = perf_bisect::BuildProbe::new(&rootp, metric);
    // `measure` is a trait method (`Probe`) — the trait must be in scope at
    // this call site, not only at the impl.
    use perf_bisect::Probe as _;
    let good_owned = good.to_string();
    // `revs` rides the blocking closure by move (find_culprit walks it) AND
    // is printed after the join (`revs.len()` in the verdict) — the closure
    // gets its own clone, the report keeps the original.
    let revs_for_probe = revs.clone();
    let job = move || -> Result<perf_bisect::BisectOutcome, perf_bisect::BisectError> {
        let baseline = probe.measure(&good_owned, runs)?;
        let inner =
            perf_bisect::find_culprit(&revs_for_probe, &mut probe, baseline, threshold_pct, runs);
        Ok((baseline, inner))
    };
    let outcome = tokio::task::spawn_blocking(job)
        .await
        .map_err(|e| anyhow::anyhow!("the bisect worker panicked: {e}"))?;
    // The worktrees are best-effort swept whatever the outcome.
    let _ = std::fs::remove_dir_all(rootp.join(perf_bisect::BISECT_DIRNAME));
    let (baseline, inner) = outcome.map_err(|e| anyhow::Error::new(e.diagnostic()))?;
    outln!(w, "baseline {good}: {baseline:.1}");
    let verdict = inner.map_err(|e| anyhow::Error::new(e.diagnostic()))?;
    match verdict {
        None => outln!(
            w,
            "no regression: the bad end measures within the +{threshold} limit"
        ),
        Some(v) => {
            outln!(w, "metric series (median of {runs}, limit {:.1}):", v.limit);
            for (rev, m) in &v.series {
                outln!(w, "  {rev:<16} {m:>12.1}");
            }
            outln!(
                w,
                "culprit: {} ({} of {} candidates; {:.1} > limit {:.1}, {} probes)",
                v.rev,
                v.index,
                revs.len(),
                v.metric,
                v.limit,
                v.probes
            );
        }
    }
    Ok(0)
}

/// The clean-tree gate: `git status --porcelain` non-empty means dirty.
/// Not a git repo → `false` (nothing to consult, nothing to protect).
pub(crate) fn git_tree_dirty(root: &std::path::Path) -> bool {
    let out = std::process::Command::new("git")
        .args(["-C", &root.to_string_lossy(), "status", "--porcelain"])
        .output();
    match out {
        Ok(o) if o.status.success() => !o.stdout.is_empty(),
        _ => false,
    }
}
