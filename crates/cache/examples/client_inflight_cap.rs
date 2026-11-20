//! The client's inflight bound (V2-F68), walked: the fan-out's last
//! unbounded resource learns its cap.
//!
//! The §11.2 client opens one fresh socket per exchange and degrades
//! transport failures through the file's arms — but nothing bounded HOW
//! MANY exchanges ran at once: a render fanning out over a hundred keys
//! opened a hundred concurrent sockets against the same plane.
//! `with_inflight_cap` bounds the client's concurrency: the permit is
//! acquired BEFORE the socket opens and INSIDE the deadline window, so a
//! saturated cap degrades within the F60 clock — a loud `CACHE-HTTP`
//! deadline error, never a hang — and the F63 probes ride the same gate.
//! The default is the pre-F68 unbounded shape: the operator opts in, the
//! choice is greppable.
//!
//! Run with: `cargo run -p oxide-fw-cache --example client_inflight_cap`

use oxide_fw_cache::{
    CacheHandler, Entry, HttpCacheHandler, HttpCacheService, PlanePeer, Policy, Result,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// A backing handler whose `get` and `set` count their arrivals, then
/// park on a gate the walkthrough holds shut — the deterministic slow
/// plane the rendezvous rides (the parking IS the demo).
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
        self.arrivals.fetch_add(1, Ordering::SeqCst);
        let _held = self.gate.lock().await;
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

#[tokio::main]
async fn main() {
    // 1. The default: the pre-F68 shape, unbounded — no gate, no permit.
    let bare = HttpCacheHandler::new("http://127.0.0.1:1");
    println!(
        "1. a fresh client rides the pre-F68 shape: inflight_cap() = {:?}",
        bare.inflight_cap()
    );

    // The plane: a real socket shell over the gate handler (the SHELL is
    // unbounded here — the bound under demo is the CLIENT's).
    let arrivals = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new(tokio::sync::Mutex::<()>::new(()));
    let handler = Arc::new(GateHandler {
        arrivals: Arc::clone(&arrivals),
        gate: Arc::clone(&gate),
    });
    let svc = HttpCacheService::new(handler as Arc<dyn CacheHandler>);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        svc.serve(listener).await.expect("the accept loop serves");
    });

    // 2. The bound: cap = 1, deadline 250 ms — the queue rides the F60
    //    clock, so the demo's parked exchange degrades inside the
    //    deadline instead of hanging the walkthrough.
    let client = Arc::new(
        HttpCacheHandler::new(format!("http://{addr}"))
            .with_inflight_cap(1)
            .with_timeout(Duration::from_millis(250)),
    );
    println!(
        "2. the operator caps the client: inflight_cap() = {:?} (deadline 250 ms)",
        client.inflight_cap()
    );

    // 3. The rendezvous: the plane's gate shuts, exchange A parks
    //    server-side; B queues client-side (the permit is A's).
    let guard = gate.lock().await;
    let parked_client = Arc::clone(&client);
    let parked_get = tokio::spawn(async move { parked_client.get("route:/a").await });
    for _ in 0..200 {
        if arrivals.load(Ordering::SeqCst) == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    let queued_client = Arc::clone(&client);
    let queued_set = tokio::spawn(async move {
        queued_client
            .set(
                "route:/b",
                Entry {
                    body: b"<html>b</html>".to_vec(),
                    status: 200,
                    headers: vec![],
                    tags: vec![],
                    policy: Policy::default(),
                    stored_at: 0,
                    etag: String::new(),
                },
            )
            .await
    });
    println!(
        "3. A parked server-side (arrivals = {}); B queued client-side — \
         the permit is A's",
        arrivals.load(Ordering::SeqCst)
    );

    // 4. The degrade: A's deadline fires while parked (a miss, the §11.2
    //    contract), B's fires while queued (a LOUD set `Err` naming the
    //    deadline) — both inside the F60 clock, never a hang.
    let a_out = tokio::time::timeout(Duration::from_secs(2), parked_get)
        .await
        .expect("A degrades inside its deadline")
        .expect("A's task joins");
    println!(
        "4. A degraded to a miss (Ok(None) = {} — the parked exchange outlived \
         its deadline; the local tier keeps serving)",
        matches!(a_out, Ok(None))
    );
    let b_out = tokio::time::timeout(Duration::from_secs(2), queued_set)
        .await
        .expect("B degrades inside its deadline")
        .expect("B's task joins");
    match b_out {
        Err(e) => println!("4. B degraded LOUD: [{}] {}", e.0.code, e.0.message),
        Ok(()) => println!("4. B stored (the deadline did not fire)"),
    }

    // 5. The drain: the plane's gate opens, healthy traffic flows — and
    //    the probes ride the same gate (the provider-less plane answers
    //    the F40 404, LOUD).
    drop(guard);
    let post_gate = client
        .get("route:/c")
        .await
        .expect("the gate opened — the exchange answers")
        .expect("the entry hits");
    println!(
        "5. after the gate opens, a fresh exchange answers: {} bytes",
        post_gate.body.len()
    );
    match client.peer_health().await {
        Err(e) => println!(
            "   the probe rides the same gate and answers loud: [{}] {}",
            e.0.code, e.0.message
        ),
        Ok(_) => println!("   the probe answered (a provider-attached plane)"),
    }
    println!(
        "6. the client's bounds table: TIME = with_timeout, MEMORY = with_read_limit, \
        NAMES = plane_name, COUNT = with_inflight_cap"
    );
}
