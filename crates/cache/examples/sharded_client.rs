//! Consumer walkthrough for the V2-F69 sharded multi-plane client: one
//! node, many planes, keys routed by consistent hashing.
//!
//! Until F69 a node pointed ONE `HttpCacheHandler` at ONE plane — the
//! plane's tier was the node's whole remote capacity, fenced by the
//! bounds arc (F67 concurrency, F64 shape, F60 time) but never
//! SPREAD. [`oxide_fw_cache::ShardedCacheHandler`] is the composition:
//! entry verbs route a key to exactly ONE shard (rendezvous hashing
//! over blake3, the crate's own digest), tag verbs fan out over EVERY
//! shard (a tag's key set is plane-local, so it is scattered), and the
//! whole composite is one `Arc<dyn CacheHandler>` — it composes behind
//! a plane service, inside a `TieredCache`'s remote slot, or under a
//! probe like any handler.
//!
//! Shows the story end to end:
//!
//! 1. Four planes boot on loopback (the F53 service over in-memory
//!    tiers — the `cache.handler = { type: 'http' }` nodes of the story).
//! 2. The composite routes: the `shard_of` table shows the key → shard
//!    map, deterministic across rebuilds.
//! 3. A set through the composite lands on its routed plane ALONE — the
//!    direct client on that plane sees the entry, the other planes 404.
//! 4. A get rides the same route and finds the entry.
//! 5. The tag fan-out: keys under one tag on every plane, the
//!    composite's `tag_keys` gathers all four lists, `invalidate_tag`
//!    crosses every plane, the summed count reports.
//! 6. Minimal disruption: growing the shard set moves only the keys
//!    whose rendezvous winner changed.
//! 7. An empty shard set refuses loudly (`CAC-SHARD`) — never a silent
//!    dropper.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-cache --example sharded_client
//! ```

use oxide_fw_cache::{
    CacheHandler, Entry, HttpCacheHandler, HttpCacheService, Policy, TieredCache,
};
use std::sync::Arc;
use std::time::Duration;

/// A fixture entry with a finite TTL born NOW (an entry born expired is
/// a correct miss the seams did not cause — the recorded fixture class).
fn fixture_entry(body: &[u8], tags: &[&str], etag: &str) -> Entry {
    Entry {
        body: body.to_vec(),
        status: 200,
        headers: vec![],
        tags: tags.iter().map(|t| (*t).to_string()).collect(),
        policy: Policy {
            ttl: Duration::from_secs(3600),
            swr: None,
            stale_if_error: None,
            soft_tags: true,
        },
        stored_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX),
        etag: etag.into(),
    }
}

/// The direct clients over the booted planes, and the plane addresses.
fn direct_clients(addrs: &[std::net::SocketAddr]) -> Vec<HttpCacheHandler> {
    addrs
        .iter()
        .map(|a| HttpCacheHandler::new(format!("http://{a}")))
        .collect()
}

/// The routed-plane story: a set lands on ONE plane alone, the get
/// rides the same route.
async fn routing_story(client: &oxide_fw_cache::ShardedCacheHandler, direct: &[HttpCacheHandler]) {
    let key = "route:/sharded-entry";
    let routed = client.shard_of(key);
    client
        .set(
            key,
            fixture_entry(b"<html>one shard only</html>", &[], "sharded-v1"),
        )
        .await
        .expect("the routed set stores");
    let seen = direct[routed]
        .get(key)
        .await
        .expect("the routed plane answers")
        .expect("the routed plane holds the entry");
    let elsewhere = direct[(routed + 1) % direct.len()]
        .get(key)
        .await
        .expect("the plane answers");
    println!(
        "== the set landed on plane {routed} alone: the routed plane sees {:?}, a non-routed plane sees {elsewhere:?}",
        String::from_utf8_lossy(&seen.body),
    );
    let got = client
        .get(key)
        .await
        .expect("the composite answers")
        .expect("the entry rides its route");
    println!(
        "== the composite's get rides the route: plane {routed} served {:?}",
        String::from_utf8_lossy(&got.body)
    );
}

/// The tag fan-out story: every plane holds one key under the tag (set
/// through its own direct client), the composite gathers all the lists
/// and the invalidation crosses every plane.
async fn tag_story(client: &oxide_fw_cache::ShardedCacheHandler, direct: &[HttpCacheHandler]) {
    for (i, h) in direct.iter().enumerate() {
        h.set(
            &format!("route:/tagged/{i}"),
            fixture_entry(
                format!("<html>tagged {i}</html>").as_bytes(),
                &["session"],
                "",
            ),
        )
        .await
        .expect("the direct set stores");
    }
    let mut keys = client.tag_keys("session").await.expect("the fan gathers");
    keys.sort();
    println!("== the tag fan-out gathers all four planes: {keys:?}");
    let count = client
        .invalidate_tag("session", true)
        .await
        .expect("the fan invalidates");
    println!("== the invalidation crossed every plane: count={count} (the SUM)");
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    rt.block_on(async {
        // 1. Four planes on loopback; the tiers are plain in-memory
        //    stores behind the F53 shell.
        let mut addrs = Vec::new();
        for _ in 0..4 {
            let svc = HttpCacheService::new(TieredCache::new_memory() as Arc<dyn CacheHandler>);
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move { svc.serve(listener).await.expect("the accept loop serves") });
            addrs.push(addr);
        }
        let clients: Vec<Arc<dyn CacheHandler>> = direct_clients(&addrs)
            .into_iter()
            .map(|h| Arc::new(h) as Arc<dyn CacheHandler>)
            .collect();

        // 2. The composite routes; the routing is deterministic, so two
        //    rebuilds agree on every key (the reshard-safe contract).
        let client =
            oxide_fw_cache::ShardedCacheHandler::new(clients).expect("four shards compose");
        println!("== the shard_of table ==");
        for key in [
            "route:/",
            "route:/blog/[slug]",
            "route:/dash",
            "L1:data:home",
            "route:/api/users",
        ] {
            println!("  {key:<24} -> plane {}", client.shard_of(key));
        }

        // 3+4. The routed-plane story.
        let direct = direct_clients(&addrs);
        routing_story(&client, &direct).await;

        // 5. The tag fan-out story.
        tag_story(&client, &direct).await;

        // 6. Minimal disruption: growing 4 → 5 planes moves only the
        //    keys whose winner changed (the consistent-hash promise).
        let mut grown: Vec<Arc<dyn CacheHandler>> = direct_clients(&addrs)
            .into_iter()
            .map(|h| Arc::new(h) as Arc<dyn CacheHandler>)
            .collect();
        grown.push(Arc::new(HttpCacheHandler::new(format!("http://{}", addrs[0]))));
        let grown = oxide_fw_cache::ShardedCacheHandler::new(grown).expect("five shards compose");
        let moved = (0..200)
            .filter(|i| {
                grown.shard_of(&format!("route:/page/{i}"))
                    != client.shard_of(&format!("route:/page/{i}"))
            })
            .count();
        println!(
            "== growing 4 -> 5 planes moved {moved} of 200 keys (a minority — the reshard-safe contract)"
        );

        // 7. The loud refusal: an empty shard set is a caller bug — the
        //    minted `CAC-SHARD` code names it, never a silent dropper.
        match oxide_fw_cache::ShardedCacheHandler::new(Vec::new()) {
            Ok(_) => println!("== UNREACHABLE: the empty set composes"),
            Err(e) => println!("== the empty set refuses: [{}] {}", e.0.code, e.0.message),
        }
    });

    Ok(())
}
