//! V2-A02/V2-A06/V2-B01: criterion benchmarks for the dispatch substrate
//! (`crates/dispatch`).
//!
//! Four groups:
//! - `url/*`: the prelude's per-request URL helpers (`path_of`,
//!   `normalize_path`, `data_route_of`, `parse_form_urlencoded`,
//!   `action_from_query`) on representative inputs. These set the baseline
//!   the phase-3 closure adoption is compared against.
//! - `ctx_table/*`: `RouteContextTable` build and `get` — the boot-time
//!   dispatch answers `stage_match` and the build-time prerender walk
//!   consume through this crate's dependency surface (V2-B01; the
//!   serving-side and build-side integrations are landed, see
//!   `docs/deviations.md`).
//! - `baseline_scan/*`: the OLD per-request O(entries) walks the request
//!   closure and the prerender loop paid per route (spec perf #1 / #10) —
//!   the baseline `ctx_table/get` replaces. The walks copy the pre-B01
//!   scan shapes verbatim; the per-layout `is_file()` stat is NOT included
//!   (no filesystem in a bench — the stat made the old path strictly
//!   slower).
//! - `loads/*`: the §9.2 parallel load graph (`run_load_graph`, V2-A06) —
//!   orchestration overhead on a zero-delay runner, plus the wall-clock
//!   win of independent loads versus a fully parent-chained chain at the
//!   same node count.
//!
//! Run: `cargo bench -p oxide-fw-dispatch --features bench-util`

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_dispatch::loads::{LoadNode, run_load_graph};
use oxide_fw_dispatch::url::{
    action_from_query, data_route_of, normalize_path, parse_form_urlencoded, path_of,
};
use oxide_fw_router::RouteContextTable;
use oxide_fw_router::testing::synthetic_entries;
use oxide_fw_router::{RouteEntry, RouteId, RouteKind};
use oxide_fw_server::{LoaderOutput, LoaderRunInput, LoaderRunner};
use std::hint::black_box;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Page path + encoded data route + fragment: one representative URL that
/// exercises every `?`/`#` branch of the helpers.
const URL: &str = "/blog/a/posts?route=%2Fblog%2Fa&id=42#frag";

/// A §9.3 form body: two plain pairs, one `+`-encoded, one percent-encoded,
/// one bare key (the malformed-pair branch).
const FORM_BODY: &[u8] = b"name=elmo&value=42&tag=caf%C3%A9&flag";

fn bench_url_helpers(c: &mut Criterion) {
    let mut g = c.benchmark_group("url");
    g.sample_size(40);
    g.measurement_time(Duration::from_secs(4));
    g.bench_function("path_of", |b| {
        b.iter(|| path_of(black_box(URL)));
    });
    g.bench_function("normalize_path", |b| {
        b.iter(|| normalize_path(black_box("/blog/a/posts/")));
    });
    g.bench_function("data_route_of", |b| {
        b.iter(|| data_route_of(black_box("GET"), black_box("/__data"), black_box(URL)));
    });
    g.bench_function("action_from_query", |b| {
        b.iter(|| action_from_query(black_box("/comment?/setcookie#frag")));
    });
    g.bench_function("parse_form_urlencoded", |b| {
        b.iter(|| parse_form_urlencoded(black_box(FORM_BODY)));
    });
    g.finish();
}

/// Route id of synthetic entry 7 (`routes/s7/p7/page`): a static page, a
/// guaranteed hit for every n >= 8.
const CTX_HIT_ID: &str = "routes/s7/p7/page";

/// A mixed synthetic set for the scan baseline: pages plus layouts,
/// middleware, error and loading modules at fixed strides — the shapes the
/// old per-request scans hunted for (a page-only set makes every walk
/// degenerate to an empty answer).
fn mixed_entries(n: usize) -> Vec<RouteEntry> {
    (0..n)
        .map(|i| {
            let (kind, suffix) = match i % 5 {
                0 => (RouteKind::Layout, "layout"),
                1 => (RouteKind::Middleware, "middleware"),
                2 => (RouteKind::Error, "error"),
                3 => (RouteKind::Loading, "loading"),
                _ => (RouteKind::Page, "page"),
            };
            let dir = format!("d{}", i % 64);
            RouteEntry {
                id: RouteId(format!("routes/{dir}/{suffix}")),
                kind,
                segments: Vec::new(),
                file: PathBuf::from(format!("{dir}/{suffix}.tsx")),
                render_mode: None,
                tags: Vec::new(),
                has_server_module: i % 2 == 0,
                needs_parent: false,
                live: None,
                priority: None,
                slot: None,
            }
        })
        .collect()
}

/// The OLD per-request scans (spec perf #1 / #10): the O(entries) walks the
/// request closure and the prerender loop paid per route — the layout chain
/// (outermost first, `format!("{base}/")` per candidate), the nearest
/// error boundary, the middleware chain and the not-found id. Copied from
/// the pre-B01 scan shapes so the comparison stays honest.
fn baseline_scan(
    entries: &[RouteEntry],
    id: &str,
) -> (Vec<String>, Option<String>, Vec<PathBuf>, Option<String>) {
    let layout_ids: Vec<String> = {
        let mut chain: Vec<(String, usize)> = Vec::new();
        for candidate in entries {
            if !matches!(candidate.kind, RouteKind::Layout) {
                continue;
            }
            let layout_base = candidate
                .id
                .0
                .strip_suffix("/layout")
                .unwrap_or(&candidate.id.0);
            let wraps = layout_base == "routes"
                || layout_base.is_empty()
                || id.starts_with(&format!("{layout_base}/"));
            if wraps {
                chain.push((candidate.id.0.clone(), layout_base.len()));
            }
        }
        chain.sort_by_key(|(_, len)| *len);
        chain.into_iter().map(|(id_, _)| id_).collect()
    };
    let error_route: Option<String> = {
        let mut best: Option<(String, usize)> = None;
        for candidate in entries {
            if !matches!(candidate.kind, RouteKind::Error) {
                continue;
            }
            let base = candidate
                .id
                .0
                .strip_suffix("/error")
                .unwrap_or(&candidate.id.0);
            let wraps = base == "routes" || base.is_empty() || id.starts_with(&format!("{base}/"));
            if !wraps {
                continue;
            }
            let len = base.len();
            match &best {
                Some((_, prev)) if *prev >= len => {}
                _ => best = Some((candidate.id.0.clone(), len)),
            }
        }
        best.map(|(id_, _)| id_)
    };
    let middleware: Vec<PathBuf> = {
        let mut chain: Vec<(PathBuf, usize)> = Vec::new();
        for candidate in entries {
            if !matches!(candidate.kind, RouteKind::Middleware) {
                continue;
            }
            let base = candidate
                .id
                .0
                .strip_suffix("/middleware")
                .unwrap_or(&candidate.id.0);
            let wraps = base == "routes" || base.is_empty() || id.starts_with(&format!("{base}/"));
            if wraps {
                chain.push((candidate.file.clone(), base.len()));
            }
        }
        chain.sort_by_key(|(_, len)| *len);
        chain.into_iter().map(|(f, _)| f).collect()
    };
    let not_found: Option<String> = entries
        .iter()
        .find(|e| matches!(e.kind, RouteKind::NotFound))
        .map(|e| e.id.0.clone());
    (layout_ids, error_route, middleware, not_found)
}

fn bench_baseline_scan(c: &mut Criterion) {
    let mut g = c.benchmark_group("baseline_scan");
    g.sample_size(30);
    g.measurement_time(Duration::from_secs(4));
    for n in [100, 1000] {
        g.bench_with_input(BenchmarkId::new("walks", n), &n, |b, &n| {
            let entries = mixed_entries(n);
            let id = entries
                .iter()
                .find(|e| matches!(e.kind, RouteKind::Page))
                .expect("the mixed set carries pages")
                .id
                .0
                .clone();
            b.iter(|| {
                let answers = baseline_scan(black_box(&entries), black_box(&id));
                black_box(answers)
            });
        });
    }
    g.finish();
}

fn bench_ctx_table(c: &mut Criterion) {
    let mut g = c.benchmark_group("ctx_table");
    g.sample_size(30);
    g.measurement_time(Duration::from_secs(4));
    g.bench_function("build/1000", |b| {
        b.iter_batched(
            || synthetic_entries(1000),
            |entries| RouteContextTable::build_with_default_pattern(black_box(&entries)),
            criterion::BatchSize::SmallInput,
        );
    });
    for n in [100, 1000] {
        g.bench_with_input(BenchmarkId::new("get", n), &n, |b, &n| {
            let entries = synthetic_entries(n);
            let table = RouteContextTable::build_with_default_pattern(&entries);
            b.iter(|| {
                // One hit and one miss per iteration: the lookup the
                // `stage_match` integration will pay per request.
                let hit = table.get(black_box(CTX_HIT_ID)).is_some();
                let miss = table.get(black_box("routes/zzz/nothing/page")).is_none();
                hit && miss
            });
        });
    }
    g.finish();
}

/// A fake loader runner whose per-load cost is one `tokio::sleep` — the
/// stand-in for the real Node-worker round trip the CLI pays per load.
struct SleepRunner {
    delay: Duration,
}

#[async_trait::async_trait]
impl LoaderRunner for SleepRunner {
    fn name(&self) -> &'static str {
        "sleep"
    }

    async fn run(&self, input: LoaderRunInput) -> oxide_fw_server::Result<LoaderOutput> {
        tokio::time::sleep(self.delay).await;
        Ok(LoaderOutput::ok(serde_json::json!({ "node": input.route })))
    }
}

/// `n` nodes for the graph: all independent, or a fully parent-chained
/// chain (`needs_parent` on every node after the head).
fn graph_nodes(n: usize, chained: bool) -> Vec<LoadNode> {
    (0..n)
        .map(|i| LoadNode {
            input: LoaderRunInput::new(format!("/n{i}.server.ts"), format!("routes/n{i}"), "/"),
            needs_parent: chained && i > 0,
        })
        .collect()
}

/// V2-A06: the §9.2 load graph. `overhead/*` isolates the graph's own
/// scheduling cost (zero-delay runner — the price a fully-instant loader
/// chain pays for the substrate); `parallel_N` vs `chain_N` shows the
/// wall-clock shape the wiring buys: N independent one-unit loads collapse
/// to roughly one unit, the chained chain pays N.
fn bench_load_graph(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("bench tokio runtime");
    let mut g = c.benchmark_group("loads");
    g.sample_size(30);
    g.measurement_time(Duration::from_secs(4));

    let instant: Arc<dyn LoaderRunner> = Arc::new(SleepRunner {
        delay: Duration::ZERO,
    });
    for n in [8, 16] {
        g.bench_with_input(BenchmarkId::new("overhead", n), &n, |b, &n| {
            b.iter(|| {
                rt.block_on(async {
                    let result = run_load_graph(Arc::clone(&instant), graph_nodes(n, false)).await;
                    black_box(match result {
                        oxide_fw_dispatch::loads::LoadResult::Data(v) => v.len(),
                        _ => 0,
                    })
                })
            });
        });
    }

    let sleeper: Arc<dyn LoaderRunner> = Arc::new(SleepRunner {
        delay: Duration::from_millis(1),
    });
    // Independent: eight 1 ms loads overlap (≈ one delay + overhead).
    g.bench_function("parallel_8", |b| {
        b.iter(|| {
            rt.block_on(async {
                let result = run_load_graph(Arc::clone(&sleeper), graph_nodes(8, false)).await;
                black_box(match result {
                    oxide_fw_dispatch::loads::LoadResult::Data(v) => v.len(),
                    _ => 0,
                })
            })
        });
    });
    // The chain the sequential loop runs today: eight serialized loads.
    g.bench_function("chain_8", |b| {
        b.iter(|| {
            rt.block_on(async {
                let result = run_load_graph(Arc::clone(&sleeper), graph_nodes(8, true)).await;
                black_box(match result {
                    oxide_fw_dispatch::loads::LoadResult::Data(v) => v.len(),
                    _ => 0,
                })
            })
        });
    });

    g.finish();
}

criterion_group!(
    benches,
    bench_url_helpers,
    bench_ctx_table,
    bench_baseline_scan,
    bench_load_graph
);
criterion_main!(benches);
