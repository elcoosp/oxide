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
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-cli --example hot_key_top
//! ```

use oxide_fw_cache::{
    CacheEvent, CacheObserver as _, CacheOp, HotKeyObserver, Layer, MetricsCacheObserver,
};
use oxide_fw_cli::observability::{
    cache_hot_key_top, cache_hot_key_top_from, set_cache_hot_key_top_override,
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
}
