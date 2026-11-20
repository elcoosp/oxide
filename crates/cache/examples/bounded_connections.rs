//! The shell's concurrency bound (V2-F67), walked: the accept loop's
//! last unbounded resource learns its cap.
//!
//! The F53 shell bounds each exchange's shape (`HEADER_CAP`) and — since
//! F64 — its body (`ExchangeLimits`); the loop itself spawned one task
//! per accepted connection with NO cap. `with_max_connections` bounds
//! HOW MANY exchanges run at once: the excess connections wait (the
//! socket is accepted, the bytes unread) until a running exchange frees
//! its permit — a wait, never a refusal, the peer sees a slow answer and
//! the exchange runs untouched once a permit frees. The default is the
//! pre-F67 unbounded shape: the operator opts in, the choice is
//! greppable.
//!
//! Run with: `cargo run -p oxide-fw-cache --example bounded_connections`

use oxide_fw_cache::{CacheHandler, Entry, HttpCacheService, Policy, Result, TieredCache};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// A backing handler whose `get` counts its arrival, then parks on a
/// gate the walkthrough holds shut — the deterministic slow plane the
/// rendezvous rides (the parking IS the demo; no real peer needed).
struct GateHandler {
    arrivals: Arc<AtomicUsize>,
    gate: Arc<tokio::sync::Mutex<()>>,
}

#[async_trait::async_trait]
impl CacheHandler for GateHandler {
    async fn get(&self, key: &str) -> Result<Option<Entry>> {
        self.arrivals.fetch_add(1, Ordering::SeqCst);
        let _held = self.gate.lock().await;
        Ok(Some(Self::entry(key)))
    }
    async fn set(&self, _key: &str, _entry: Entry) -> Result<()> {
        Ok(())
    }
    async fn delete(&self, _key: &str) -> Result<()> {
        Ok(())
    }
    async fn tag_keys(&self, _tag: &str) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
    async fn invalidate_tag(&self, _tag: &str, _soft: bool) -> Result<usize> {
        Ok(0)
    }
    async fn publish_invalidation(&self, _tag: &str, _soft: bool) -> Result<()> {
        Ok(())
    }
}

impl GateHandler {
    fn entry(key: &str) -> Entry {
        Entry {
            body: format!("<html>{key}</html>").into_bytes(),
            status: 200,
            headers: vec![],
            tags: vec![],
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
            etag: String::new(),
        }
    }
}

/// One raw GET exchange, the whole answer out (the shell closes after
/// one exchange — `Connection: close`).
async fn raw_get(addr: std::net::SocketAddr, key: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut sock = tokio::net::TcpStream::connect(addr).await.expect("connect");
    sock.write_all(
        format!("GET /entry/{key} HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n").as_bytes(),
    )
    .await
    .expect("write");
    let mut buf = Vec::new();
    sock.read_to_end(&mut buf).await.expect("read");
    String::from_utf8_lossy(&buf).into_owned()
}

#[tokio::main]
async fn main() {
    // 1. The default: the pre-F67 shape, unbounded — no gate, no permit,
    //    byte-identical for every existing deployment.
    let fresh = HttpCacheService::new(TieredCache::new_memory() as Arc<dyn CacheHandler>);
    println!(
        "1. a fresh service rides the pre-F67 shape: max_connections() = {:?}",
        fresh.max_connections()
    );

    // 2. The bound: cap = 1 — at most one exchange runs at once.
    let arrivals = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new(tokio::sync::Mutex::<()>::new(()));
    let handler = Arc::new(GateHandler {
        arrivals: Arc::clone(&arrivals),
        gate: Arc::clone(&gate),
    });
    let svc = HttpCacheService::new(handler as Arc<dyn CacheHandler>).with_max_connections(1);
    println!(
        "2. the operator caps the shell: max_connections() = {:?}",
        svc.max_connections()
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = l.local_addr().expect("addr");
    tokio::spawn(async move {
        svc.serve(l).await.expect("the accept loop serves");
    });

    // 3. The rendezvous: the gate shuts, conn A runs and parks; conn B
    //    is accepted but its exchange must WAIT (the permit is A's).
    let guard = gate.lock().await;
    let a = tokio::spawn(raw_get(addr, "route:/a"));
    for _ in 0..200 {
        if arrivals.load(Ordering::SeqCst) == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    let b = tokio::spawn(raw_get(addr, "route:/b"));
    let b_ran = tokio::time::timeout(Duration::from_millis(50), async {
        while arrivals.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await;
    println!(
        "3. A parked (arrivals = {}); B accepted under the cap — its exchange ran \
         while A holds the permit? {}",
        arrivals.load(Ordering::SeqCst),
        if b_ran.is_ok() {
            "yes (broken bound!)"
        } else {
            "no — it waits"
        }
    );

    // 4. The drain: the gate opens, A answers and frees its permit, B's
    //    queued exchange runs and answers — a wait, never a refusal.
    drop(guard);
    let a_resp = tokio::time::timeout(Duration::from_secs(2), a)
        .await
        .expect("A completes once the gate opens")
        .expect("A's exchange task joins");
    let b_resp = tokio::time::timeout(Duration::from_secs(2), b)
        .await
        .expect("B drains once the permit frees")
        .expect("B's exchange task joins");
    println!(
        "4. A answered {} · B (the queued exchange) answered {}",
        first_line(&a_resp),
        first_line(&b_resp)
    );
    println!(
        "   final handler arrivals: {} — B ran exactly once, after the permit freed",
        arrivals.load(Ordering::SeqCst)
    );

    // 5. The moral: the F60 deadline bounds an exchange's TIME, the
    //    F64/F65 bounds bound its MEMORY, the F67 bound bounds HOW MANY
    //    run at once — the pure core never touches the gate, the bound
    //    is the shell's concern.
    println!(
        "5. the shell's bounds table: TIME = with_timeout, MEMORY = with_limits \
        / with_read_limit, COUNT = with_max_connections"
    );
}

fn first_line(resp: &str) -> &str {
    resp.lines().next().unwrap_or("")
}
