//! The cache plane's HTTP-service walkthrough (F53, SPEC-12 / §11.2): the
//! protocol's server half — one node hosts the shared store, the other
//! nodes point `cache.handler = { type: 'http', url }` at it.
//!
//! Until F53 the protocol existed only as its client (`HttpCacheHandler`)
//! and a doc block saying "one node runs the cache service" — nothing
//! did. This walkthrough boots the real service (`HttpCacheService`)
//! over an in-memory `TieredCache` on a loopback port, then drives the
//! protocol's whole story through two REAL clients standing in for two
//! build nodes:
//!
//! 1. node A stores an entry tagged `shared` — sealed on the wire;
//! 2. node B reads it (the same `OXIDE_CACHE_MAC_KEY` rule: in-process,
//!    the key is shared; across processes, the operator sets the env);
//! 3. node B lists the tag's keys and hard-invalidates the tag — the
//!    delete crosses nodes through the shared backing;
//! 4. node A's next read misses: the multi-node plane is coherent.
//!
//! A forged PUT (plain JSON, no MAC) is refused with a 400 on the way —
//! the trust boundary holds where the payload lands, not just on read.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-cache --example cache_plane
//! ```

use std::sync::Arc;
use std::time::Duration;

use oxide_fw_cache::{
    CacheHandler as _, Entry, HttpCacheHandler, HttpCacheService, PlaneRequest, Policy, TieredCache,
};

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn entry(body: &[u8], tags: &[&str]) -> Entry {
    Entry {
        body: body.to_vec(),
        status: 200,
        headers: vec![("content-type".into(), "text/html".into())],
        tags: tags.iter().copied().map(str::to_string).collect(),
        policy: Policy {
            ttl: Duration::from_secs(60),
            swr: None,
            stale_if_error: None,
            soft_tags: true,
        },
        stored_at: now_ms(),
        etag: "\"v1\"".into(),
    }
}

#[tokio::main]
async fn main() {
    // The shared backing: one node's tier (memory here; a `with_disk`
    // tier is the same shape with a durable L1).
    let tier = TieredCache::new_memory();

    // Host the plane on a loopback port the operator would front with
    // their TLS/proxy layer.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local addr");
    let backing: Arc<dyn oxide_fw_cache::CacheHandler> = tier.clone();
    println!(
        "the cache plane hosts a `{}` backing store at {addr}",
        backing.handler_kind()
    );
    let service = HttpCacheService::new(backing);
    tokio::spawn(async move {
        service
            .serve(listener)
            .await
            .expect("the accept loop serves");
    });

    // Two nodes, one plane.
    let base = format!("http://{addr}");
    let node_a = HttpCacheHandler::new(base.clone());
    let node_b = HttpCacheHandler::new(base.clone());

    // 1. Node A stores; the client seals the envelope before it leaves.
    node_a
        .set(
            "post/1",
            entry(b"<html>node A rendered this</html>", &["shared"]),
        )
        .await
        .expect("node A sets");
    println!("node A stored post/1 (tag `shared`)");

    // 2. Node B reads it — the plane is shared, no Redis/S3 involved.
    let got = node_b
        .get("post/1")
        .await
        .expect("get")
        .expect("node B hits");
    println!(
        "node B reads post/1: {} bytes, etag {}",
        got.body.len(),
        got.etag
    );

    // 3. A forged PUT (plain JSON, no MAC) is refused at the boundary —
    //    driven through the pure core (a service over the same backing;
    //    the socket shell answers the same, bytes parse into the same
    //    PlaneRequest).
    let forged = serde_json::to_vec(&entry(b"forged", &[])).expect("serializes");
    let core = HttpCacheService::new(tier.clone() as Arc<dyn oxide_fw_cache::CacheHandler>);
    let resp = core
        .respond(&PlaneRequest::put("/entry/post/1-forged", forged))
        .await;
    println!(
        "forged PUT answered {} (refused, never stored)",
        resp.status
    );

    // 4. The tag listing and the invalidation cross nodes.
    let keys = node_b.tag_keys("shared").await.expect("tag keys");
    println!("node B lists tag `shared`: {keys:?}");
    let count = node_b
        .invalidate_tag("shared", false)
        .await
        .expect("invalidate");
    println!("node B hard-invalidated tag `shared`: {count} entries deleted");

    // 5. Node A's next read misses: the plane is coherent across nodes.
    let after = node_a.get("post/1").await.expect("get");
    println!(
        "node A re-reads post/1 after the invalidation: {}",
        after.is_none()
    );
}
