//! Consumer example for the V2-F42 hot-key top window's boot flag: the
//! `--cache-hot-key-top` spelling a contributor (or an operator without
//! shell access to the process env) drives the §20 scrape's window with.
//!
//! Shows the four pieces the boot composes:
//!
//! 1. [`oxide_fw_cli::observability::cache_hot_key_top`] — the resolver
//!    the boot calls ONCE at the `set_cache_plane` wire: the
//!    `--cache-hot-key-top` flag's override slot wins, the
//!    `OXIDE_CACHE_PLANE_HOT_KEY_TOP` env var keeps its shell spelling,
//!    and with neither set the F40 default window (10) answers.
//! 2. [`oxide_fw_cli::observability::cache_hot_key_top_from`] — the pure
//!    parse core: the falsy spellings (`""`/`0`/`false`/`off`) keep the
//!    default, a parsed value rides verbatim, garbage degrades
//!    loudly-but-honestly to the default.
//! 3. [`oxide_fw_cli::observability::set_cache_hot_key_top_override`] —
//!    the override slot `apply_startup_env` writes pre-runtime (the
//!    flag's landing point); a host embedding the CLI drives the same
//!    slot directly.
//! 4. The ledger render under the resolved window — the composition the
//!    boot's `cache_plane_scrape` rides: the exporter's families ahead,
//!    the ledger's labeled `oxide_cache_plane_hot_key_total` family over
//!    the boot's window.
//! 5. (V2-F46) The FLAGS walk `Cli::parse_from` + `apply_startup_env` —
//!    the parse-side thread the binary entry point rides, both flags at
//!    once (`--cache-hot-key-top` and `--cache-plane-coverage`).
//! 6. (V2-F46) The coverage arm: the ledger's F44 coverage exposition —
//!    the unattributed score's counter, the occupancy and bound gauges —
//!    composed beside the labeled family under the
//!    `cache.coverageMetrics` opt-in.
//! 7. (V2-F47) The health arm: the tier's F32 posture exposition —
//!    `TieredCache::health()` → `render_prometheus()`'s nine
//!    `oxide_cache_plane_*` gauge families — composed after the labeled
//!    and coverage families under the `OXIDE_CACHE_PLANE_HEALTH`
//!    opt-in, over a tier with real resident entries (the posture reads
//!    the live stack, never a zeroed stand-in).
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-cli --example hot_key_top
//! ```

use clap::Parser as _;
use oxide_fw_cache::{
    CacheEvent, CacheObserver as _, CacheOp, HotKeyObserver, Layer, MetricsCacheObserver,
};
use oxide_fw_cli::observability::{
    cache_hot_key_top, cache_hot_key_top_from, cache_plane_coverage,
    cache_plane_coverage_with_config, cache_plane_health_from, set_cache_hot_key_top_override,
};

fn main() {
    // 1. The boot's default: no flag, no env — the F40 window answers.
    println!("default window (no flag, no env): {}", cache_hot_key_top());

    // 2. The parse core: the spellings the resolver answers, without
    //    touching the process env.
    for spelling in [Some("50"), Some("off"), Some("ten"), None] {
        println!(
            "cache_hot_key_top_from({spelling:?}) = {}",
            cache_hot_key_top_from(spelling)
        );
    }

    // 3. The flag's landing point: the override slot the boot writes
    //    pre-runtime from `--cache-hot-key-top` (the flag wins over the
    //    env var; the env spelling keeps the shell user's form).
    set_cache_hot_key_top_override(Some(50));
    let resolved = cache_hot_key_top();
    println!("with --cache-hot-key-top 50:      {resolved}");
    set_cache_hot_key_top_override(None);
    println!("after the flag is absent again:   {}", cache_hot_key_top());

    // 4. The composition the boot rides: the ledger renders its labeled
    //    family over the resolved window, the exporter's families ride
    //    ahead (here one exporter, three attributed keys, the window 1 —
    //    the narrowest honest scrape).
    let ledger = HotKeyObserver::bounded(2048);
    for (key, weight) in [
        ("/routes/index", 7),
        ("/routes/docs", 3),
        ("/routes/api", 1),
    ] {
        for _ in 0..weight {
            ledger.on_event(&CacheEvent {
                key,
                op: CacheOp::Hit {
                    layer: Layer::L0,
                    stale: false,
                    bytes: 128,
                },
            });
        }
    }
    let exporter = MetricsCacheObserver::new();
    let mut scrape = exporter.render_prometheus();
    scrape.push_str(&ledger.render_prometheus(resolved.min(1)));
    let start = scrape
        .find("# HELP oxide_cache_plane_hot_key_total")
        .expect("the hot-key family renders");
    println!(
        "--- the hot-key family under window {} ---",
        resolved.min(1)
    );
    println!("{}", &scrape[start..]);

    // 5. The V2-F46 polish tail: the FLAGS walk `Cli::parse_from` — the
    //    parse-side shape the F42 pins hold, here so a contributor sees
    //    the whole thread: the flag parses, `apply_startup_env` writes
    //    the override slots pre-runtime, and the resolvers answer from
    //    them. (Sections 1-3 drove the resolvers directly; this walks
    //    the command line the way the binary entry point does.)
    let cli = oxide_fw_cli::Cli::parse_from([
        "oxide",
        "start",
        "--cache-hot-key-top",
        "50",
        "--cache-plane-coverage",
        "--cache-plane-health",
    ]);
    oxide_fw_cli::apply_startup_env(&cli);
    println!(
        "parse_from(start --cache-hot-key-top 50 --cache-plane-coverage \
         --cache-plane-health): top={}, coverage={}, health={}",
        cache_hot_key_top(),
        cache_plane_coverage(),
        oxide_fw_cli::observability::cache_plane_health()
    );
    // The hygiene the pins keep: the slots clear so the process-wide
    // state never outlives the walk.
    oxide_fw_cli::observability::set_cache_hot_key_top_override(None);
    oxide_fw_cli::observability::set_cache_plane_coverage_override(None);
    oxide_fw_cli::observability::set_cache_plane_health_override(None);

    // 6. The V2-F46 coverage arm: the boot's composition with the
    //    opt-in ON — the ledger's F44 coverage exposition (the
    //    unattributed score's counter, the occupancy and bound gauges)
    //    renders beside the labeled window family. The default shape
    //    (opt-in off) is everything above this section's tail.
    let coverage = cache_plane_coverage_with_config(Some(true));
    let mut full_scrape = exporter.render_prometheus();
    full_scrape.push_str(&ledger.render_prometheus(resolved.min(1)));
    if coverage {
        full_scrape.push_str(&ledger.render_coverage_prometheus());
    }
    let cov = full_scrape
        .find("# HELP oxide_cache_plane_hot_key_unattributed_total")
        .expect("the coverage exposition renders under the opt-in");
    println!("--- the coverage arm (cache.coverageMetrics = true) ---");
    println!("{}", &full_scrape[cov..]);

    // 7. The V2-F47 health arm: the tier's F32 posture exposition. The
    //    parse core first (the same spellings family the coverage knob
    //    speaks), then the composition the boot rides — a small tier
    //    with real resident entries so the posture gauges render the
    //    live stack (`memory_entries` counts the sets below), followed
    //    by the exporter's families ahead of it (the fixed order:
    //    exporter, labeled window, coverage, posture LAST).
    for spelling in [Some("on"), Some("off"), Some("sure"), None] {
        println!(
            "cache_plane_health_from({spelling:?}) = {}",
            cache_plane_health_from(spelling)
        );
    }
    let health = oxide_fw_cli::observability::cache_plane_health_with_config(Some(true));
    let tier = oxide_fw_cache::TieredCache::new(4 * 1024 * 1024);
    let rt = tokio::runtime::Runtime::new().expect("the example's tokio runtime");
    rt.block_on(async {
        for key in ["/routes/index", "/routes/docs", "/routes/api"] {
            tier.set(
                key,
                oxide_fw_cache::Entry {
                    body: b"<html>hello</html>".to_vec(),
                    status: 200,
                    headers: vec![("content-type".into(), "text/html".into())],
                    tags: vec!["demo".into()],
                    policy: oxide_fw_cache::Policy {
                        ttl: std::time::Duration::from_secs(60),
                        swr: None,
                        stale_if_error: None,
                        soft_tags: true,
                    },
                    stored_at: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .expect("the wall clock runs")
                        .as_millis() as u64,
                    etag: "\"v1\"".into(),
                },
            )
            .await
            .expect("the tier takes the write");
        }
    });
    if health {
        let posture = tier.health();
        println!("--- the health arm (OXIDE_CACHE_PLANE_HEALTH = on) ---");
        println!(
            "memory_entries={} disk_attached={} remote_attached={}",
            posture.memory_entries, posture.disk_attached, posture.remote_attached
        );
        println!("{}", posture.render_prometheus());
    }
}
