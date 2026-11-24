//! V2-E09: criterion bench for the edge KV cache handler (§V2-E09's "KV
//! cache handler for the edge" — the A10 family's member over the
//! `env.OXIDE_KV` shape).
//!
//! The wiring-relevant question is the per-op cost the handler adds over
//! the store's own round-trip: the codec seal/encode, the tag index
//! read-modify-write (one extra read + one write per tagged set), and the
//! invalidation sweep. Cases:
//!
//! - `kv/{set_tagged,get}`: a tagged set (the entry blob + the index RMW)
//!   and the decode path's `get` against the in-memory `MemKv` fake —
//!   the floor the real store's latency rides on.
//! - `kv/invalidate_4`: the sweep over a 4-key tag (read the index, N
//!   deletes, drop the index), re-seeded per iteration.
//! - `kv/cap_refusal`: a set whose tag index sits at the cap — the
//!   best-effort stop's cost (the entry still writes).
//!
//! Run: `cargo bench -p oxide-fw-cache --features bench-util --bench kv`

use criterion::{Criterion, criterion_group, criterion_main};
use oxide_fw_cache::{CacheHandler, Entry, KvCacheHandler, KvStore, Policy};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// The in-memory `KvStore` fake — the worker binding's shape (flat keys,
/// opaque bytes; the bench needs no expiry).
#[derive(Debug, Default)]
struct MemKv {
    map: Mutex<BTreeMap<String, Vec<u8>>>,
}

#[async_trait::async_trait]
impl KvStore for MemKv {
    async fn get(&self, key: &str) -> oxide_fw_cache::Result<Option<Vec<u8>>> {
        Ok(self.map.lock().unwrap().get(key).cloned())
    }
    async fn put(
        &self,
        key: &str,
        value: Vec<u8>,
        _ttl: Option<std::time::Duration>,
    ) -> oxide_fw_cache::Result<()> {
        self.map.lock().unwrap().insert(key.to_string(), value);
        Ok(())
    }
    async fn delete(&self, key: &str) -> oxide_fw_cache::Result<()> {
        self.map.lock().unwrap().remove(key);
        Ok(())
    }
}

fn entry(tag: &str) -> Entry {
    Entry {
        body: vec![0u8; 512],
        status: 200,
        headers: vec![("content-type".into(), "text/html".into())],
        tags: if tag.is_empty() {
            Vec::new()
        } else {
            vec![tag.to_string()]
        },
        policy: Policy::default(),
        stored_at: 0,
        etag: "e".into(),
    }
}

fn bench_kv(c: &mut Criterion) {
    // One runtime for the whole bench (the client_keepalive shape): the
    // handler's cost is the seal/codec + the index RMW, not the executor.
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("bench runtime");
    let kv = Arc::new(MemKv::default());
    let h = Arc::new(KvCacheHandler::new(kv.clone()));
    rt.block_on(h.set("route:/about", entry("route"))).unwrap();

    let mut group = c.benchmark_group("kv");
    group.bench_function("set_tagged", |b| {
        b.iter(|| rt.block_on(h.set("route:/about", entry("route"))));
    });
    group.bench_function("get", |b| {
        b.iter(|| rt.block_on(h.get("route:/about")));
    });
    group.bench_function("invalidate_4", |b| {
        b.iter(|| {
            for i in 0..4 {
                rt.block_on(h.set(&format!("route:{i}"), entry("route")))
                    .unwrap();
            }
            rt.block_on(h.invalidate_tag("route", true))
        });
    });
    group.bench_function("cap_refusal", |b| {
        let capped = KvCacheHandler::new(kv.clone()).with_tag_cap(1);
        rt.block_on(capped.set("route:0", entry("route"))).unwrap();
        b.iter(|| rt.block_on(capped.set("route:9", entry("route"))));
    });
    group.finish();
}

criterion_group!(benches, bench_kv);
criterion_main!(benches);
