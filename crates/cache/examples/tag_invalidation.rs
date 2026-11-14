//! The tiered-cache + tag-invalidation walkthrough (§11.2/§11.3, the
//! V2-B05/B06 seams): route entries live in the memory and disk tiers
//! under canonical `KeyStr` keys, every entry carries its tags, the tag
//! index answers "which keys?" in O(1), and one content-deploy invalidates
//! the whole tag set across both tiers.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-cache --example tag_invalidation
//! ```

use std::time::Duration;

use oxide_fw_cache::{Entry, Hit, Policy, TieredCache, key_of};
use tempfile::tempdir;

fn main() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        // 1. The tiered cache: L0 memory (small cap so the walkthrough
        //    shows the tiers) + L1 disk over a tempdir (§11.2).
        let dir = tempdir().expect("tempdir");
        let cache = TieredCache::new(64 * 1024)
            .with_disk(dir.path().to_path_buf())
            .expect("disk tier opens");

        // 2. Two entries under one content tag (§11.3): the canonical key
        //    is the B05 `KeyStr` — layer + route + params + vary + build id.
        let policy = Policy {
            ttl: Duration::from_secs(60),
            swr: Some(Duration::from_secs(30)),
            ..Policy::default()
        };
        for route in ["/", "/blog/hello"] {
            let key = key_of("L0", route, "", "", "build-a");
            let entry = Entry {
                body: format!("<h1>{route}</h1>").into_bytes(),
                status: 200,
                headers: vec![("content-type".into(), "text/html".into())],
                tags: vec!["content:posts".into()],
                policy: policy.clone(),
                // The freshness windows measure against wall-clock now.
                stored_at: u64::try_from(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .expect("clock")
                        .as_millis(),
                )
                .unwrap_or(0),
                etag: format!("\"{route}\""),
            };
            cache.set(key.as_str(), entry).await.expect("set");
        }

        // 3. Hits come back classified (Fresh / Stale) with their tags
        //    attached, from whichever tier holds them.
        let hit = cache
            .get(key_of("L0", "/", "", "", "build-a").as_str())
            .await
            .expect("get")
            .expect("the entry is fresh");
        let entry = match &hit {
            Hit::Fresh(e) | Hit::Stale(e) => e.clone(),
        };
        println!("hit status={} tags={:?}", entry.status, entry.tags);

        // 4. The tag index (P-18b: O(1) insert, sorted deterministic read).
        let keys = cache.tag_keys("content:posts").await.expect("tag index");
        println!("tag content:posts indexes {} key(s)", keys.len());
        assert_eq!(keys.len(), 2);

        // 5. One content deploy invalidates the tag across both tiers.
        //    Soft (stale-while-regenerate, §11.3) marks the entries and
        //    KEEPS the index so the stale-then-regenerate path can find
        //    them; a hard invalidation (soft = false) clears both.
        let invalidated = cache
            .invalidate_tag("content:posts", true)
            .await
            .expect("invalidate");
        println!("soft-invalidated {invalidated} key(s)");
        let soft_keys = cache.tag_keys("content:posts").await.expect("tag index");
        println!("after soft: {} key(s) still indexed", soft_keys.len());
        assert_eq!(soft_keys.len(), 2, "soft keeps the index for SWR");
        let hard = cache
            .invalidate_tag("content:posts", false)
            .await
            .expect("invalidate");
        let hard_keys = cache.tag_keys("content:posts").await.expect("tag index");
        println!(
            "hard-invalidated {hard} key(s); index now {}",
            hard_keys.len()
        );
        assert!(hard_keys.is_empty(), "hard invalidation clears the index");

        // 6. A different build id is a different key — the deploy boundary
        //    the skew pool (§V2-F10) rides on.
        let stale_build = cache
            .get(key_of("L0", "/", "", "", "build-old").as_str())
            .await
            .expect("get");
        println!("old-build key hit: {}", stale_build.is_some());

        println!("tiered-cache walkthrough complete");
    });
}
