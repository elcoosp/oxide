//! The V2-F18 performance-bisect substrate (spec v0.2 §V2-F18).
//!
//! `oxide perf bisect --good <rev> --bad <rev> --metric build-ms` walks the
//! first-parent range between two revisions, builds each candidate in a
//! `git worktree` under `.oxide/bisect/` — all sharing one `OXIDE_CACHE_DIR`
//! so rebuilds are incremental — and reports the first commit whose metric
//! exceeds the known-good baseline by more than the threshold.
//!
//! The layout is the F16/F12 substrate pattern: the search itself is a pure
//! function testable without git ([`bisect`] is the spec-verbatim body), the
//! worktree/build side sits behind the injectable [`Probe`] seam so the TDD
//! pins run without a toolchain ([`BuildProbe`] is the only process-spawning
//! impl and it is exercised only by the real command), and the git plumbing
//! is pinned as pure string functions ([`rev_list_args`],
//! [`parse_rev_list_oldest_first`], [`worktree_path`]).
//!
//! Errors are diagnostics: every variant of [`BisectError`] carries a stable
//! `PERF-*` code and a fix line ([`BisectError::diagnostic`]) so the CLI
//! surface renders the same code/cause/fix shape as the rest of the tool
//! (architecture invariant 3 — the miette-style help text).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use thiserror::Error;

use oxide_fw_diagnostics::{Diagnostic, OxideError};

/// Where the per-candidate worktrees live, relative to the project root
/// (spec §V2-F18: "a `git worktree` under `.oxide/bisect/`").
pub const BISECT_DIRNAME: &str = ".oxide/bisect";

/// The environment variable every candidate build shares so rebuilds are
/// incremental (spec Appendix V2 env table, the F18 row).
pub const CACHE_DIR_VAR: &str = "OXIDE_CACHE_DIR";

/// How many leading characters of a revision name a worktree directory.
const WORKTREE_REV_CHARS: usize = 12;

// ---------------------------------------------------------------------------
// Metrics
// ---------------------------------------------------------------------------

/// The metric a bisect minimizes (spec §V2-F18: `build-ms | client-js-bytes
/// | ttfb-ms`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Metric {
    /// Wall-clock milliseconds of `oxide build --root <worktree>`.
    BuildMs,
    /// Total bytes of the `dist/client/**/*.js` bundle output.
    ClientJsBytes,
    /// Milliseconds to first response byte for `GET /` on the built server.
    TtfbMs,
}

impl Metric {
    /// Every metric in CLI spelling order.
    pub const ALL: [Metric; 3] = [Metric::BuildMs, Metric::ClientJsBytes, Metric::TtfbMs];

    /// The kebab-case spelling the CLI accepts and prints.
    pub fn label(self) -> &'static str {
        match self {
            Metric::BuildMs => "build-ms",
            Metric::ClientJsBytes => "client-js-bytes",
            Metric::TtfbMs => "ttfb-ms",
        }
    }

    /// Parse a CLI metric spelling.
    pub fn parse(spec: &str) -> Result<Metric, BisectError> {
        Metric::ALL
            .into_iter()
            .find(|m| m.label() == spec)
            .ok_or_else(|| BisectError::UnknownMetric {
                name: spec.to_string(),
            })
    }
}

// ---------------------------------------------------------------------------
// Errors (each variant is a diagnostic with a stable code and a fix line)
// ---------------------------------------------------------------------------

/// Everything that can refuse a `perf bisect` run. Each variant renders as
/// an `OxideError(Diagnostic)` at the CLI boundary with a stable `PERF-*`
/// code and a fix line — the miette-style help text.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum BisectError {
    /// `git` is not on `PATH`.
    #[error("git is not available on PATH")]
    GitMissing,
    /// A git invocation failed (rev-list, worktree add/remove, rev-parse).
    #[error("`git {context}` failed: {detail}")]
    GitFailure {
        /// The git subcommand phrase that failed.
        context: String,
        /// The stderr tail git produced.
        detail: String,
    },
    /// The good..bad range listed no commits.
    #[error("the range `{good}..{bad}` lists no commits")]
    EmptyRange {
        /// The known-good revision argument.
        good: String,
        /// The known-bad revision argument.
        bad: String,
    },
    /// The metric name is none of the three spec spellings.
    #[error("`{name}` is not a metric — expected build-ms, client-js-bytes or ttfb-ms")]
    UnknownMetric {
        /// The rejected metric spelling.
        name: String,
    },
    /// The threshold is not a non-negative finite percentage.
    #[error("`{spec}` is not a threshold — expected a non-negative percentage like `10%` or `10`")]
    BadThreshold {
        /// The rejected threshold spelling.
        spec: String,
    },
    /// `--runs` was zero; the median of zero samples is undefined.
    #[error("--runs must be at least 1 (got 0)")]
    NoRuns,
    /// A candidate build or measurement failed inside its worktree.
    #[error("measuring `{rev}` failed: {detail}")]
    BuildFailed {
        /// The revision being measured.
        rev: String,
        /// The build or serve error detail.
        detail: String,
    },
}

impl BisectError {
    /// The stable diagnostic code for this error.
    pub fn code(&self) -> &'static str {
        match self {
            BisectError::GitMissing => "PERF-GIT",
            BisectError::GitFailure { .. } => "PERF-GIT",
            BisectError::EmptyRange { .. } => "PERF-RANGE",
            BisectError::UnknownMetric { .. } => "PERF-METRIC",
            BisectError::BadThreshold { .. } => "PERF-THRESHOLD",
            BisectError::NoRuns => "PERF-RUNS",
            BisectError::BuildFailed { .. } => "PERF-BUILD",
        }
    }

    /// The miette-style help text: what to do about this error.
    pub fn fix(&self) -> String {
        match self {
            BisectError::GitMissing => "install git (https://git-scm.com) — the bisect \
                 drives `git rev-list` and `git worktree`"
                .to_string(),
            BisectError::GitFailure { .. } => "run the same `git` command by hand to see \
                 the full failure; the bisect never mutates refs (worktrees are detached)"
                .to_string(),
            BisectError::EmptyRange { good, bad } => format!(
                "pass an ancestor as --good and a descendant as --bad so the range is \
                 non-empty (`git log --oneline {good}..{bad}` lists the candidates)"
            ),
            BisectError::UnknownMetric { .. } => "pick one of: build-ms, client-js-bytes, \
                 ttfb-ms"
                .to_string(),
            BisectError::BadThreshold { .. } => "pass a non-negative percentage, e.g. \
                 `--threshold 10%`; bare numbers mean percent too"
                .to_string(),
            BisectError::NoRuns => "pass `--runs 3` (the default) or any value >= 1; the \
                 median of the runs is what the search compares"
                .to_string(),
            BisectError::BuildFailed { .. } => "confirm the good revision builds cleanly \
                 first (`oxide build`); a candidate that fails to build cannot be measured"
                .to_string(),
        }
    }

    /// The full diagnostic for the CLI boundary: code, message and fix.
    pub fn diagnostic(&self) -> OxideError {
        OxideError(Box::new(
            Diagnostic::error(self.code(), self.to_string()).with_fix(self.fix()),
        ))
    }
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

/// The spec-verbatim search (docs/spec-v0.2-draft.md §V2-F18). `commits` is
/// ordered oldest (known good) to newest (known bad); returns the index of
/// the first commit whose metric exceeds `good * (1 + threshold)`.
///
/// The body is copied from the spec unchanged so the TDD pins test the
/// exact algorithm the work package names.
pub fn bisect<F: FnMut(&str) -> f64>(
    commits: &[String],
    mut measure: F,
    good: f64,
    threshold_pct: f64,
) -> Option<usize> {
    let limit = good * (1.0 + threshold_pct / 100.0);
    let (mut lo, mut hi) = (0usize, commits.len().checked_sub(1)?);
    if measure(&commits[hi]) <= limit {
        return None;
    }
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if measure(&commits[mid]) > limit {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    Some(lo)
}

/// The median of `samples` (odd count: the middle; even count: the mean of
/// the two middle values). `None` on an empty slice. Samples must be finite
/// — the measure contract maps failures to errors, never to NaN.
pub fn median_of_runs(samples: &mut [f64]) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    samples.sort_by(f64::total_cmp);
    let mid = samples.len() / 2;
    if samples.len() % 2 == 1 {
        Some(samples[mid])
    } else {
        Some((samples[mid - 1] + samples[mid]) / 2.0)
    }
}

/// The acceptance bound (§V2-F18: the culprit is found in at most
/// `ceil(log2(n)) + 1` builds — one probe of the bad end plus the binary
/// search). `0` for an empty history (no probe runs at all).
pub fn probe_budget(n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    // ceil(log2(n)): floor(log2(n)) is the highest set bit's index; a power
    // of two rounds itself up to itself instead of one below.
    let log2 = usize::BITS - n.leading_zeros() - u32::from(n.is_power_of_two());
    1 + log2 as usize
}

/// Parse the `--threshold` spelling: a non-negative finite percentage, with
/// or without the `%` suffix (`"10"`, `"10%"`, `"0.5"`).
pub fn parse_threshold_pct(spec: &str) -> Result<f64, BisectError> {
    let trimmed = spec.trim();
    let bare = trimmed.strip_suffix('%').unwrap_or(trimmed);
    let value: f64 = bare
        .parse()
        .map_err(|_| BisectError::BadThreshold {
            spec: spec.to_string(),
        })?;
    if !value.is_finite() || value < 0.0 {
        return Err(BisectError::BadThreshold {
            spec: spec.to_string(),
        });
    }
    Ok(value)
}

/// The exact `git` invocation that lists the candidate commits, newest
/// first (spec §V2-F18: `git rev-list --first-parent <good>..<bad>`).
pub fn rev_list_args(good: &str, bad: &str) -> Vec<String> {
    vec![
        "rev-list".to_string(),
        "--first-parent".to_string(),
        format!("{good}..{bad}"),
    ]
}

/// Parse `git rev-list` stdout (newest-first, one revision per line) into
/// the engine's oldest-first ordering. Blank lines are skipped; each line
/// is trimmed (git owns the revision grammar — no validation here).
pub fn parse_rev_list_oldest_first(stdout: &str) -> Vec<String> {
    let mut revs: Vec<String> = stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    revs.reverse();
    revs
}

/// The worktree path for probe `index` of `rev`, under
/// `<project_root>/.oxide/bisect/`: `0007-0123456789ab` (zero-padded probe
/// number, revision truncated to 12 characters).
pub fn worktree_path(project_root: &Path, index: usize, rev: &str) -> PathBuf {
    let short: String = rev.chars().take(WORKTREE_REV_CHARS).collect();
    project_root
        .join(BISECT_DIRNAME)
        .join(format!("{index:04}-{short}"))
}

/// The cache directory every candidate worktree shares: the project's own
/// `.oxide/cache`, so a candidate's rebuild hits the parent checkout's
/// fingerprints instead of starting cold (spec §V2-F18).
pub fn shared_cache_dir(project_root: &Path) -> PathBuf {
    project_root.join(".oxide/cache")
}

/// The build path's cache root (§V2-F18): an `OXIDE_CACHE_DIR` override
/// wins when set — the bisect points every candidate worktree at the same
/// directory — and the historical `<root>/.oxide/cache` is the default.
pub fn cache_dir_for(project_root: &Path) -> PathBuf {
    cache_dir_from(std::env::var_os(CACHE_DIR_VAR), project_root)
}

/// The pure decision behind [`cache_dir_for`], split out so the override
/// branch is testable without mutating process state.
pub fn cache_dir_from(override_dir: Option<std::ffi::OsString>, project_root: &Path) -> PathBuf {
    match override_dir {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => project_root.join(".oxide/cache"),
    }
}

// ---------------------------------------------------------------------------
// The injectable measure seam
// ---------------------------------------------------------------------------

/// One logical probe of the metric. Implementations return the MEDIAN of
/// `runs` raw measurements for `rev` (spec §V2-F18: "measures the median of
/// `--runs`"); the median itself is [`median_of_runs`], so a fake can be a
/// pure function while the real probe shells out to builds.
pub trait Probe {
    /// The median of `runs` measurements of `rev`, or an error if the
    /// candidate cannot be built or measured.
    ///
    /// # Errors
    /// The impl's own failure shape (for [`BuildProbe`], a [`BisectError`]
    /// with the failing revision and the tool output tail).
    fn measure(&mut self, rev: &str, runs: usize) -> Result<f64, BisectError>;
}

/// What a completed bisect reports: the culprit, its metric against the
/// limit, the probe accounting, and every measured (revision, median) pair
/// in probe order — the "metric series" the command prints.
#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    /// Index into the oldest-first candidate list.
    pub index: usize,
    /// The culprit revision (the first commit past the threshold).
    pub rev: String,
    /// The culprit's median metric.
    pub metric: f64,
    /// The regression limit the metric exceeded (`good * (1 + threshold)`).
    pub limit: f64,
    /// Logical probes spent (at most [`probe_budget`]).
    pub probes: usize,
    /// Raw measurements per logical probe.
    pub runs: usize,
    /// Every measured (revision, median) pair, in probe order.
    pub series: Vec<(String, f64)>,
}

/// The baseline measurement paired with the search outcome — what the
/// command's blocking job returns so the baseline prints beside the series.
pub type BisectOutcome = (f64, Result<Option<Verdict>, BisectError>);

/// Drive [`bisect`] over a [`Probe`]: the counting/median glue between the
/// spec-verbatim search and the injectable measure, plus the verdict
/// assembly. Probe errors propagate (no panic, no bogus culprit).
///
/// # Errors
/// [`BisectError`] when a probe fails, or [`BisectError::NoRuns`] for a
/// zero-run request.
pub fn find_culprit<P: Probe>(
    commits: &[String],
    probe: &mut P,
    good: f64,
    threshold_pct: f64,
    runs: usize,
) -> Result<Option<Verdict>, BisectError> {
    if runs == 0 {
        return Err(BisectError::NoRuns);
    }
    let limit = good * (1.0 + threshold_pct / 100.0);
    let mut series: Vec<(String, f64)> = Vec::new();
    let mut failed: Option<BisectError> = None;
    let mut measure = |rev: &str| -> f64 {
        match probe.measure(rev, runs) {
            Ok(m) => {
                series.push((rev.to_string(), m));
                m
            }
            Err(e) => {
                if failed.is_none() {
                    failed = Some(e);
                }
                f64::INFINITY
            }
        }
    };
    let index = bisect(commits, &mut measure, good, threshold_pct);
    // `measure` was consumed by `bisect`, so its borrows end here.
    if let Some(e) = failed {
        return Err(e);
    }
    let Some(index) = index else {
        return Ok(None);
    };
    let metric = series
        .iter()
        .find(|(rev, _)| rev == &commits[index])
        .map_or(limit, |(_, m)| *m);
    Ok(Some(Verdict {
        index,
        rev: commits[index].clone(),
        metric,
        limit,
        probes: series.len(),
        runs,
        series,
    }))
}

// ---------------------------------------------------------------------------
// The real probe: git worktree + the current binary
// ---------------------------------------------------------------------------

/// Measures candidates by checking each revision out into a detached
/// `git worktree` under `.oxide/bisect/` and running the current binary's
/// `build`/`start` inside it, with `OXIDE_CACHE_DIR` pointed at the parent
/// checkout's cache so rebuilds are incremental.
///
/// The binary under test is the app at the candidate revision; the binary
/// DOING the measuring is the running `oxide` (the operator's tool) — the
/// standard bisect arrangement.
#[derive(Debug)]
pub struct BuildProbe {
    /// The project checkout the worktrees branch from.
    pub root: PathBuf,
    /// The metric to measure.
    pub metric: Metric,
    probe_counter: usize,
}

impl BuildProbe {
    /// A probe rooted at the project checkout.
    pub fn new(root: &Path, metric: Metric) -> Self {
        Self {
            root: root.to_path_buf(),
            metric,
            probe_counter: 0,
        }
    }

    /// `git worktree add --detach <path> <rev>`; returns the worktree path.
    fn worktree_add(&mut self, rev: &str) -> Result<PathBuf, BisectError> {
        let path = worktree_path(&self.root, self.probe_counter, rev);
        self.probe_counter += 1;
        let out = git(&self.root, ["worktree", "add", "--detach"])
            .arg(&path)
            .arg(rev)
            .output()
            .map_err(|e| Self::git_spawn_error("worktree add", &e))?;
        if !out.status.success() {
            return Err(BisectError::GitFailure {
                context: "worktree add".to_string(),
                detail: stderr_tail(&out.stderr),
            });
        }
        Ok(path)
    }

    /// `git worktree remove --force <path>`, best effort — a leftover
    /// worktree under `.oxide/bisect/` is cleaned by the command's final
    /// sweep and never affects correctness.
    fn worktree_remove(&self, path: &Path) {
        let _ = git(&self.root, ["worktree", "remove", "--force"])
            .arg(path)
            .output();
    }

    fn build_once(&self, worktree: &Path, rev: &str) -> Result<(), BisectError> {
        let exe = current_oxide()?;
        let started = Instant::now();
        let out = Command::new(&exe)
            .arg("build")
            .arg("--root")
            .arg(worktree)
            .env(CACHE_DIR_VAR, shared_cache_dir(&self.root))
            .output()
            .map_err(|e| BisectError::BuildFailed {
                rev: rev.to_string(),
                detail: e.to_string(),
            })?;
        tracing::debug!(
            rev = %rev,
            build_ms = started.elapsed().as_millis() as u64,
            "perf bisect candidate build"
        );
        if !out.status.success() {
            return Err(BisectError::BuildFailed {
                rev: rev.to_string(),
                detail: stderr_tail(&out.stderr),
            });
        }
        Ok(())
    }

    fn measure_build_ms(
        &self,
        worktree: &Path,
        rev: &str,
        runs: usize,
    ) -> Result<f64, BisectError> {
        let mut samples = Vec::with_capacity(runs);
        for _ in 0..runs {
            let started = Instant::now();
            self.build_once(worktree, rev)?;
            samples.push(started.elapsed().as_millis() as f64);
        }
        Ok(median_of_runs(&mut samples).unwrap_or(f64::INFINITY))
    }

    fn measure_client_js_bytes(
        &self,
        worktree: &Path,
        rev: &str,
        runs: usize,
    ) -> Result<f64, BisectError> {
        // The metric is deterministic per build; one build is measured and
        // each run re-reads the output tree.
        self.build_once(worktree, rev)?;
        let dist = worktree.join("dist").join("client");
        let mut samples = Vec::with_capacity(runs);
        for _ in 0..runs {
            let mut total = 0_u64;
            sum_js_bytes(&dist, &mut total);
            samples.push(total as f64);
        }
        Ok(median_of_runs(&mut samples).unwrap_or(f64::INFINITY))
    }

    fn measure_ttfb_ms(&self, worktree: &Path, rev: &str, runs: usize) -> Result<f64, BisectError> {
        self.build_once(worktree, rev)?;
        let exe = current_oxide()?;
        let mut samples = Vec::with_capacity(runs);
        for _ in 0..runs {
            let port = free_port().ok_or_else(|| BisectError::BuildFailed {
                rev: rev.to_string(),
                detail: "no free local port for the ttfb-ms probe".to_string(),
            })?;
            let mut child = Command::new(&exe)
                .args(["start", "--root"])
                .arg(worktree)
                .arg("--port")
                .arg(port.to_string())
                .arg("--no-warmup")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .map_err(|e| BisectError::BuildFailed {
                    rev: rev.to_string(),
                    detail: format!("oxide start did not spawn: {e}"),
                })?;
            let measured = (|| -> Result<f64, BisectError> {
                wait_for_port(port, rev)?;
                first_byte_ms(port)
            })();
            let _ = child.kill();
            let _ = child.wait();
            samples.push(measured?);
        }
        Ok(median_of_runs(&mut samples).unwrap_or(f64::INFINITY))
    }
}

impl Probe for BuildProbe {
    fn measure(&mut self, rev: &str, runs: usize) -> Result<f64, BisectError> {
        if runs == 0 {
            return Err(BisectError::NoRuns);
        }
        let worktree = self.worktree_add(rev)?;
        let outcome = match self.metric {
            Metric::BuildMs => self.measure_build_ms(&worktree, rev, runs),
            Metric::ClientJsBytes => self.measure_client_js_bytes(&worktree, rev, runs),
            Metric::TtfbMs => self.measure_ttfb_ms(&worktree, rev, runs),
        };
        self.worktree_remove(&worktree);
        outcome
    }
}

// ---------------------------------------------------------------------------
// Process helpers (std only; the command layer owns no async here)
// ---------------------------------------------------------------------------

fn git(dir: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(dir).args(args);
    cmd
}

impl BisectError {
    /// Classify a `std::process` spawn failure: a missing `git` binary is
    /// [`BisectError::GitMissing`], anything else is a
    /// [`BisectError::GitFailure`] for `context`.
    pub fn git_spawn_error(context: &str, e: &std::io::Error) -> BisectError {
        if e.kind() == std::io::ErrorKind::NotFound {
            BisectError::GitMissing
        } else {
            BisectError::GitFailure {
                context: context.to_string(),
                detail: e.to_string(),
            }
        }
    }
}

fn current_oxide() -> Result<PathBuf, BisectError> {
    std::env::current_exe().map_err(|e| BisectError::BuildFailed {
        rev: "<current>".to_string(),
        detail: format!("cannot resolve the running oxide binary: {e}"),
    })
}

fn stderr_tail(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim();
    const TAIL: usize = 400;
    if text.chars().count() <= TAIL {
        text.to_string()
    } else {
        let tail: String = text.chars().skip(text.chars().count() - TAIL).collect();
        format!("…{tail}")
    }
}

/// Sum the byte length of every `.js` file under `dir` (recursive).
fn sum_js_bytes(dir: &Path, total: &mut u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            sum_js_bytes(&path, total);
        } else if path.extension().is_some_and(|ext| ext == "js") {
            if let Ok(meta) = entry.metadata() {
                *total += meta.len();
            }
        }
    }
}

/// Reserve an ephemeral port by binding and immediately releasing it (the
/// tiny TOCTOU window is accepted: the server bind that follows either
/// succeeds or fails the probe with a loud error).
fn free_port() -> Option<u16> {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .ok()
        .and_then(|l| l.local_addr().ok())
        .map(|a| a.port())
}

/// Poll-connect until the bisect server accepts (bounded: ~15 s).
fn wait_for_port(port: u16, rev: &str) -> Result<(), BisectError> {
    for _ in 0..150 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Err(BisectError::BuildFailed {
        rev: rev.to_string(),
        detail: format!("the bisect server never accepted on port {port}"),
    })
}

/// `GET /` and time connect-to-first-byte in milliseconds.
fn first_byte_ms(port: u16) -> Result<f64, BisectError> {
    use std::io::Read as _;
    use std::io::Write as _;
    let started = Instant::now();
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).map_err(|e| {
        BisectError::BuildFailed {
            rev: "<serve>".to_string(),
            detail: format!("connect failed: {e}"),
        }
    })?;
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .map_err(|e| BisectError::BuildFailed {
            rev: "<serve>".to_string(),
            detail: e.to_string(),
        })?;
    write!(
        stream,
        "GET / HTTP/1.1\r\nhost: oxide-bisect\r\nconnection: close\r\n\r\n"
    )
    .map_err(|e| BisectError::BuildFailed {
        rev: "<serve>".to_string(),
        detail: format!("request write failed: {e}"),
    })?;
    let mut first = [0_u8; 1];
    stream.read_exact(&mut first).map_err(|e| {
        BisectError::BuildFailed {
            rev: "<serve>".to_string(),
            detail: format!("no response byte: {e}"),
        }
    })?;
    Ok(started.elapsed().as_millis() as f64)
}
