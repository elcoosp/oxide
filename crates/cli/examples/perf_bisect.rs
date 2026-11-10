//! Consumer example for the V2-F18 performance-bisect engine: the pure
//! search a contributor drives (and the tests pin) when hunting the commit
//! that regressed a metric.
//!
//! Shows the three pieces an integration (or an agent) composes:
//!
//! 1. [`oxide_fw_cli::perf_bisect::bisect`] — the spec-verbatim search
//!    (docs/spec-v0.2-draft.md §V2-F18): oldest-first commits, a measure,
//!    the good baseline and the threshold; `None` when the bad end does not
//!    regress.
//! 2. [`oxide_fw_cli::perf_bisect::find_culprit`] — the driver over the
//!    injectable [`Probe`](oxide_fw_cli::perf_bisect::Probe) seam with the
//!    median-of-`runs` contract and the acceptance-bound probe accounting
//!    (`ceil(log2(n)) + 1` builds).
//! 3. The metric series a verdict carries — what `oxide perf bisect`
//!    prints, and what a caller logs when the search succeeds.
//!
//! The real command wires the same driver to
//! [`BuildProbe`](oxide_fw_cli::perf_bisect::BuildProbe) (git worktree +
//! the current binary + a shared `OXIDE_CACHE_DIR`); this example fakes the
//! probe so it runs in milliseconds with no git and no toolchain:
//!
//! ```text
//! cargo run -p oxide-fw-cli --example perf_bisect
//! ```

use oxide_fw_cli::perf_bisect::{BisectError, Probe, Verdict, find_culprit, probe_budget};

/// A synthetic history: nine commits, oldest first.
fn history() -> Vec<String> {
    [
        "boot", "a11y", "css", "hmr", "island", "ssr", "cache", "og", "edge",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

/// A fake measure: the build gets slower from `ssr` onward (index 5), as
/// if that commit introduced an O(n²) pass.
struct FixtureProbe {
    step: usize,
}

impl Probe for FixtureProbe {
    fn measure(&mut self, rev: &str, _runs: usize) -> Result<f64, BisectError> {
        let idx = history().iter().position(|r| r == rev).unwrap_or(0);
        Ok(if idx >= self.step { 4_100.0 } else { 3_000.0 })
    }
}

fn main() {
    let commits = history();
    let step = 5; // the `ssr` commit is the seeded culprit

    // The acceptance bound the spec names: at most ceil(log2(n)) + 1 builds.
    println!(
        "history of {} commits, acceptance bound: at most {} candidate builds",
        commits.len(),
        probe_budget(commits.len()),
    );

    let mut probe = FixtureProbe { step };
    let verdict: Verdict = find_culprit(&commits, &mut probe, 3_000.0, 10.0, 3)
        .expect("a fake probe cannot fail")
        .expect("the fixture regression exists");

    println!();
    println!("probe series (median of 3 runs, limit 3300.0 ms):");
    for (rev, metric) in &verdict.series {
        let marker = if *metric > verdict.limit { ">" } else { " " };
        println!("  {marker} {rev:<8} {metric:>8.1} ms");
    }
    println!();
    println!(
        "culprit: {} (index {} of 0..{}, {} probes <= bound {})",
        verdict.rev,
        verdict.index,
        commits.len(),
        verdict.probes,
        probe_budget(commits.len()),
    );

    // The no-regression shape: a clean bad end returns None after one probe.
    let mut clean = FixtureProbe { step: usize::MAX };
    let none =
        find_culprit(&commits, &mut clean, 3_000.0, 10.0, 3).expect("a fake probe cannot fail");
    println!("clean history: {none:?} (one probe of the bad end settles it)");
}
