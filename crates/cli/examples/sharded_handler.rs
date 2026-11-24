//! V2-F69 config/CLI half walkthrough: the `cache.handler = { type:
//! 'sharded', shards: [...] }` config array the boot resolves into the
//! F69 composite — the resolver's mapping (each shard builds through the
//! same arms the single handlers ride), the composite's rendezvous
//! routing table, and the loud empty refusal (`CAC-SHARD`, the boot
//! stays local-only).
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-cli --no-default-features --example sharded_handler
//! ```

use std::sync::Arc;

use oxide_fw_cache::{CacheHandler, HttpCacheHandler, ShardedCacheHandler, TieredCache};
use oxide_fw_cli::remote_cache_handler;
use oxide_fw_config::CacheHandlerConfig;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        // The config shape the §11.2 two-node story speaks: the planes
        // this node fronts, each element speaking the same shape a
        // top-level handler does.
        let cfg = CacheHandlerConfig::Sharded {
            shards: vec![
                CacheHandlerConfig::Http {
                    url: "https://plane-a.internal:3100".into(),
                },
                CacheHandlerConfig::Http {
                    url: "https://plane-b.internal:3100".into(),
                },
            ],
        };

        // The boot wire: the resolver composes the F69 composite (the
        // erased tier the worker then sees). Construction is lazy — no
        // wire bytes leave until an entry verb routes.
        let handler = remote_cache_handler(Some(&cfg))
            .await
            .expect("two http shards compose");
        println!(
            "cache.handler=sharded: composed {} (the erased tier the worker sees)",
            handler.handler_kind()
        );

        // The composite's own surface: the rendezvous routing table over
        // sample keys — growth moves only the keys whose winner changed.
        let composite = ShardedCacheHandler::new(vec![
            Arc::new(HttpCacheHandler::new("https://plane-a.internal:3100"))
                as Arc<dyn CacheHandler>,
            Arc::new(HttpCacheHandler::new("https://plane-b.internal:3100"))
                as Arc<dyn CacheHandler>,
            Arc::new(HttpCacheHandler::new("https://plane-c.internal:3100"))
                as Arc<dyn CacheHandler>,
        ])
        .expect("three planes compose");
        println!("shards: {}", composite.shards());
        for key in ["index.html", "blog/slug-a", "blog/slug-b", "products/42"] {
            println!("  shard_of({key}) = {}", composite.shard_of(key));
        }

        // The loud refusal: an empty set is a config mistake, not a
        // silent local-only fallback at the composite's level — the
        // resolver surfaces it as `CAC-SHARD` and keeps the local tier.
        match ShardedCacheHandler::new(Vec::<Arc<dyn CacheHandler>>::new()) {
            Ok(_) => unreachable!("an empty set refuses"),
            Err(e) => println!(
                "warn [{}]: {} — the boot stays local-only (§11.2)",
                e.0.code, e.0.message
            ),
        }
        let _ = TieredCache::new(0); // the local tier the boot keeps
    });
    Ok(())
}
