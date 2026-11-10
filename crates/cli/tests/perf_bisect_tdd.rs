//! TDD pins for the V2-F18 performance-bisect substrate
//! (`oxide_fw_cli::perf_bisect`, spec v0.2 §V2-F18).
//!
//! The search is a pure function so it is testable without git: the spec's
//! `bisect` signature takes a `FnMut(&str) -> f64` measure, and the driver
//! wraps a [`Probe`] (the injectable worktree-build seam) into exactly that.
//! The git plumbing (rev-list parse, worktree path scheme, shared
//! `OXIDE_CACHE_DIR`) is pinned as pure functions too — no process spawns.
//!
//! Written FIRST (red): these pins failed on unresolved-symbol errors
//! before the `perf_bisect` module existed. The worktree/build probe that
//! shells out to a real `oxide build` is the only untested part (it needs a
//! toolchain and a binary; the CLI command wires it, the acceptance bound
//! is enforced on the counting probe below).

use oxide_fw_cli::perf_bisect::{
    BISECT_DIRNAME, BisectError, CACHE_DIR_VAR, Metric, Probe, Verdict, bisect, cache_dir_from,
    find_culprit, median_of_runs, parse_rev_list_oldest_first, parse_threshold_pct, probe_budget,
    rev_list_args, shared_cache_dir, worktree_path,
};

/// `n` synthetic commits, oldest first: `0000`, `0001`, …
fn history(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("{i:04}")).collect()
}

/// A measure that steps from `base` to `spike` at (and after) index `at`.
/// HRTB spelling: the closure captures only `Copy` values, so the `&str`
/// argument is quantified over every lifetime instead of borrowing the
/// return-position environment.
fn step_measure(at: usize, base: f64, spike: f64) -> impl for<'a> FnMut(&'a str) -> f64 {
    move |rev: &str| {
        let idx: usize = rev.parse().unwrap_or(0);
        if idx >= at { spike } else { base }
    }
}

/// A [`Probe`] that never leaves the process: the "measurement" is a pure
/// step function, and the probe counts logical calls for the acceptance
/// bound (§V2-F18: found in at most `ceil(log2(n))+1` builds).
struct CountingProbe {
    calls: usize,
    at: usize,
    base: f64,
    spike: f64,
}

impl CountingProbe {
    fn step(at: usize) -> Self {
        Self {
            calls: 0,
            at,
            base: 100.0,
            spike: 300.0,
        }
    }
}

impl Probe for CountingProbe {
    fn measure(&mut self, rev: &str, runs: usize) -> Result<f64, BisectError> {
        self.calls += 1;
        assert_eq!(runs, 3, "the driver forwards the --runs value");
        let idx: usize = rev.parse().unwrap_or(0);
        Ok(if idx >= self.at {
            self.spike
        } else {
            self.base
        })
    }
}

/// A [`Probe`] whose samples carry deterministic noise: every third raw
/// sample spikes ×10. With `runs = 3` each logical probe sees exactly one
/// spike, so `median_of_runs` recovers the true metric every time — the
/// spec's "noisy fake measures (median of three)" pin.
struct NoisyProbe {
    samples: usize,
    at: usize,
}

impl Probe for NoisyProbe {
    fn measure(&mut self, rev: &str, runs: usize) -> Result<f64, BisectError> {
        let idx: usize = rev.parse().unwrap_or(0);
        let truth = if idx >= self.at { 300.0 } else { 100.0 };
        let mut raw: Vec<f64> = Vec::with_capacity(runs);
        for _ in 0..runs {
            self.samples += 1;
            let noisy = if self.samples.is_multiple_of(3) {
                truth * 10.0
            } else {
                truth
            };
            raw.push(noisy);
        }
        Ok(median_of_runs(&mut raw).expect("non-empty samples"))
    }
}

/// A [`Probe`] that fails immediately — the driver must propagate the
/// error instead of panicking or reporting a bogus culprit.
struct FailingProbe;

impl Probe for FailingProbe {
    fn measure(&mut self, _rev: &str, _runs: usize) -> Result<f64, BisectError> {
        Err(BisectError::BuildFailed {
            rev: "deadbeef".into(),
            detail: "seeded failure".into(),
        })
    }
}

// ---------------------------------------------------------------------------
// The spec-verbatim search
// ---------------------------------------------------------------------------

#[test]
fn first_regressor_is_found_monotone() {
    let commits = history(16);
    let at = 7;
    let found = bisect(&commits, step_measure(at, 100.0, 300.0), 100.0, 10.0);
    assert_eq!(found, Some(at));
    assert_eq!(commits[found.expect("some")], "0007");
}

#[test]
fn no_regression_returns_none() {
    let commits = history(16);
    let found = bisect(&commits, step_measure(16, 100.0, 300.0), 100.0, 10.0);
    assert_eq!(found, None);
}

#[test]
fn empty_history_is_none_not_an_error() {
    // The spec body's `checked_sub(1)?` maps an empty range to `None`.
    let found = bisect(&[], step_measure(0, 100.0, 300.0), 100.0, 10.0);
    assert_eq!(found, None);
}

#[test]
fn single_commit_history() {
    let commits = history(1);
    assert_eq!(
        bisect(&commits, step_measure(0, 100.0, 300.0), 100.0, 10.0),
        Some(0)
    );
    assert_eq!(
        bisect(&commits, step_measure(1, 100.0, 300.0), 100.0, 10.0),
        None
    );
}

#[test]
fn exact_limit_is_not_a_regression() {
    // The scan is inclusive (`>=` semantics live in the threshold): a metric
    // exactly at `good * (1 + threshold)` does not count as a regression.
    let commits = history(4);
    let at_limit = |_rev: &str| 110.0_f64;
    assert_eq!(bisect(&commits, at_limit, 100.0, 10.0), None);
    let just_over = |_rev: &str| 110.01_f64;
    assert_eq!(bisect(&commits, just_over, 100.0, 10.0), Some(0));
}

// ---------------------------------------------------------------------------
// The threshold grammar
// ---------------------------------------------------------------------------

#[test]
fn threshold_grammar() {
    assert_eq!(parse_threshold_pct("10").expect("bare number"), 10.0);
    assert_eq!(parse_threshold_pct("10%").expect("percent suffix"), 10.0);
    assert_eq!(parse_threshold_pct("0.5").expect("fractional"), 0.5);
    assert!(parse_threshold_pct("-1").is_err());
    assert!(parse_threshold_pct("abc").is_err());
    assert!(parse_threshold_pct("NaN").is_err());
    assert!(parse_threshold_pct("inf").is_err());
    assert!(parse_threshold_pct("").is_err());
}

// ---------------------------------------------------------------------------
// The median and the runs seam
// ---------------------------------------------------------------------------

#[test]
fn median_of_runs_odd_and_even() {
    assert_eq!(median_of_runs(&mut [3.0, 1.0, 2.0]), Some(2.0));
    assert_eq!(median_of_runs(&mut [4.0, 1.0, 3.0, 2.0]), Some(2.5));
    assert_eq!(median_of_runs(&mut [42.0]), Some(42.0));
    assert_eq!(median_of_runs(&mut []), None);
}

#[test]
fn median_of_three_beats_the_noisy_measure() {
    // runs = 3: every logical probe samples exactly one ×10 spike, so the
    // median is the truth and the culprit is found deterministically.
    let commits = history(16);
    let mut probe = NoisyProbe { samples: 0, at: 7 };
    let verdict = find_culprit(&commits, &mut probe, 100.0, 10.0, 3)
        .expect("no probe error")
        .expect("regression exists");
    assert_eq!(verdict.index, 7);
    assert_eq!(verdict.rev, "0007");
    assert_eq!(verdict.metric, 300.0);
    // The limit is `good * (1 + threshold)` — compare at f64-representable
    // precision (`100.0 * 1.1` is not exactly `110.0`).
    assert!((verdict.limit - 110.0).abs() < 1e-9);
    // Every series entry is the recovered truth, spike filtered.
    assert!(
        verdict
            .series
            .iter()
            .all(|(_, m)| (*m - 100.0).abs() < 1.0 || (*m - 300.0).abs() < 1.0)
    );
}

// ---------------------------------------------------------------------------
// The acceptance bound (§V2-F18: at most ceil(log2(n))+1 builds)
// ---------------------------------------------------------------------------

#[test]
fn probe_budget_is_log2_plus_one() {
    assert_eq!(probe_budget(0), 0);
    assert_eq!(probe_budget(1), 1);
    assert_eq!(probe_budget(2), 2);
    assert_eq!(probe_budget(3), 3);
    assert_eq!(probe_budget(16), 5);
    assert_eq!(probe_budget(1024), 11);
}

#[test]
fn driver_respects_the_acceptance_bound() {
    for (n, at) in [(16usize, 7usize), (1024, 500), (1024, 1023)] {
        let commits = history(n);
        let mut probe = CountingProbe::step(at);
        let verdict = find_culprit(&commits, &mut probe, 100.0, 10.0, 3)
            .expect("no probe error")
            .expect("regression exists");
        assert_eq!(verdict.index, at, "culprit index for n={n}");
        assert_eq!(verdict.probes, probe.calls);
        assert!(
            probe.calls <= probe_budget(n),
            "n={n}: {} probes exceed the ceil(log2(n))+1 bound {}",
            probe.calls,
            probe_budget(n)
        );
    }
}

#[test]
fn driver_never_probes_a_clean_history() {
    let commits = history(16);
    let mut probe = CountingProbe::step(16); // no regression anywhere
    assert_eq!(
        find_culprit(&commits, &mut probe, 100.0, 10.0, 3).expect("no error"),
        None
    );
    // One probe only: the bad end proves there is nothing to find.
    assert_eq!(probe.calls, 1);
}

// ---------------------------------------------------------------------------
// The driver's error path and the verdict shape
// ---------------------------------------------------------------------------

#[test]
fn driver_propagates_probe_errors() {
    let commits = history(4);
    let mut probe = FailingProbe;
    let out = find_culprit(&commits, &mut probe, 100.0, 10.0, 3);
    match out {
        Err(BisectError::BuildFailed { rev, detail }) => {
            assert_eq!(rev, "deadbeef");
            assert_eq!(detail, "seeded failure");
        }
        other => panic!("expected BuildFailed, got {other:?}"),
    }
}

#[test]
fn verdict_carries_the_probe_series() {
    // n=16, step at 7: probes are 15 (bad end), then mids 7, 3, 5, 6.
    let commits = history(16);
    let mut probe = CountingProbe::step(7);
    let verdict = find_culprit(&commits, &mut probe, 100.0, 10.0, 3)
        .expect("no probe error")
        .expect("regression exists");
    let seq: Vec<&str> = verdict.series.iter().map(|(rev, _)| rev.as_str()).collect();
    assert_eq!(seq, vec!["0015", "0007", "0003", "0005", "0006"]);
    // The expected limit is built from the same expression the engine uses,
    // so the f64 bits match exactly.
    let limit = 100.0_f64 * (1.0 + 10.0 / 100.0);
    assert_eq!(
        verdict,
        Verdict {
            index: 7,
            rev: "0007".into(),
            metric: 300.0,
            limit,
            probes: 5,
            runs: 3,
            series: vec![
                ("0015".into(), 300.0),
                ("0007".into(), 300.0),
                ("0003".into(), 100.0),
                ("0005".into(), 100.0),
                ("0006".into(), 100.0),
            ],
        }
    );
}

// ---------------------------------------------------------------------------
// The git plumbing, pinned as pure functions
// ---------------------------------------------------------------------------

#[test]
fn rev_list_parse_is_oldest_first() {
    // `git rev-list` prints newest-first; the engine wants oldest-first.
    let out = "c0ffee3\ndeadbee\n\n0badcafe\r\n";
    assert_eq!(
        parse_rev_list_oldest_first(out),
        vec!["0badcafe", "deadbee", "c0ffee3"]
    );
    assert!(parse_rev_list_oldest_first("").is_empty());
}

#[test]
fn rev_list_args_shape() {
    assert_eq!(
        rev_list_args("v0.6.0", "HEAD"),
        vec!["rev-list", "--first-parent", "v0.6.0..HEAD"]
    );
}

#[test]
fn worktree_paths_are_stable_and_truncated() {
    let root = std::path::Path::new("/proj");
    assert_eq!(
        worktree_path(root, 7, "0123456789abcdef"),
        std::path::PathBuf::from("/proj/.oxide/bisect/0007-0123456789ab")
    );
    // A short rev passes through untruncated.
    assert_eq!(
        worktree_path(root, 0, "abc"),
        std::path::PathBuf::from("/proj/.oxide/bisect/0000-abc")
    );
}

#[test]
fn seam_names_are_pinned() {
    assert_eq!(BISECT_DIRNAME, ".oxide/bisect");
    assert_eq!(CACHE_DIR_VAR, "OXIDE_CACHE_DIR");
    assert_eq!(
        shared_cache_dir(std::path::Path::new("/proj")),
        std::path::PathBuf::from("/proj/.oxide/cache")
    );
}

#[test]
fn cache_dir_respects_the_override() {
    // The pure decision: an override wins, the historical default stays.
    let root = std::path::Path::new("/proj");
    assert_eq!(
        cache_dir_from(Some("/shared/cas".into()), root),
        std::path::PathBuf::from("/shared/cas")
    );
    assert_eq!(
        cache_dir_from(None, root),
        std::path::PathBuf::from("/proj/.oxide/cache")
    );
    // An empty override is no override (an unset env var reads as empty
    // through some shells' exports).
    assert_eq!(
        cache_dir_from(Some("".into()), root),
        std::path::PathBuf::from("/proj/.oxide/cache")
    );
}

#[test]
fn metric_parse_and_labels() {
    assert_eq!(Metric::parse("build-ms"), Ok(Metric::BuildMs));
    assert_eq!(Metric::parse("client-js-bytes"), Ok(Metric::ClientJsBytes));
    assert_eq!(Metric::parse("ttfb-ms"), Ok(Metric::TtfbMs));
    let err = Metric::parse("cpu-ms").expect_err("unknown metric");
    match err {
        BisectError::UnknownMetric { name } => assert_eq!(name, "cpu-ms"),
        other => panic!("expected UnknownMetric, got {other:?}"),
    }
    assert_eq!(Metric::BuildMs.label(), "build-ms");
    assert_eq!(Metric::ClientJsBytes.label(), "client-js-bytes");
    assert_eq!(Metric::TtfbMs.label(), "ttfb-ms");
}
