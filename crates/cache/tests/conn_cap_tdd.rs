//! TDD pins for the shell's concurrency bound (V2-F67): the accept loop
//! learns the bound every other shell resource already had.
//!
//! The F53 socket shell bounds each exchange's SHAPE (`HEADER_CAP`, the
//! hostile-header refusal) and, since F64, its BODY (`ExchangeLimits`,
//! the 413-before-the-drain refusal) — but the loop itself spawned one
//! task per accepted connection with NO cap: a thousand concurrent
//! connections ran a thousand concurrent exchanges (a thousand handler
//! pulls, a thousand buffered exchanges) against the same backing tier.
//! The F60 deadline bounds an exchange's TIME and the F64/F65 bounds
//! bound its MEMORY; nothing bounded HOW MANY ran at once. The bound is
//! the shell's last unbounded resource: `with_max_connections` gates the
//! accept loop on a permit per concurrent exchange — the excess
//! connections wait (the socket already accepted, the bytes unread) for
//! a permit to free, so the plane's concurrency is the operator's knob
//! instead of the clients'. The default stays the pre-F67 unbounded
//! shape (the F60 posture — the operator opts in, the choice is
//! greppable), the pure `respond` core never touches the gate (the F64
//! doctrine: the bounds are the shell's concern), and no legitimate
//! answer changes under a generous cap.
//!
//! Written FIRST (red): the pins fail with E0599 over the missing
//! `with_max_connections`/`max_connections`.

use oxide_fw_cache::{CacheHandler, Entry, HttpCacheService, PlaneRequest, Result, TieredCache};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// A backing handler whose `get` counts its arrival, then parks on a
/// mutex the test holds shut — the deterministic "slow plane" the
/// rendezvous pins ride (no sleeps; the parking IS the fixture).
struct GateHandler {
    arrivals: Arc<AtomicUsize>,
    gate: Arc<tokio::sync::Mutex<()>>,
}

#[async_trait::async_trait]
impl CacheHandler for GateHandler {
    async fn get(&self, key: &str) -> Result<Option<Entry>> {
        self.arrivals.fetch_add(1, Ordering::SeqCst);
        // The gate: locked by the test while the exchange must park,
        // released when the rendezvous proceeds. Locks queue FIFO-ish;
        // every parked get wakes when the guard drops.
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
            policy: oxide_fw_cache::Policy {
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

/// One raw exchange: request bytes in, the whole answer out (the shell
/// closes after one exchange — `Connection: close`). The request buffer
/// moves in: the helper is spawned, so borrows cannot ride the task.
async fn raw_exchange(addr: std::net::SocketAddr, req: Vec<u8>) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut sock = tokio::net::TcpStream::connect(addr).await.unwrap();
    sock.write_all(&req).await.unwrap();
    let mut buf = Vec::new();
    sock.read_to_end(&mut buf).await.unwrap();
    String::from_utf8_lossy(&buf).into_owned()
}

/// A plane over the gate handler, the caller picks the cap.
async fn gated_plane(
    max: Option<usize>,
) -> (
    std::net::SocketAddr,
    Arc<tokio::sync::Mutex<()>>,
    Arc<AtomicUsize>,
) {
    let arrivals = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new(tokio::sync::Mutex::<()>::new(()));
    let handler = Arc::new(GateHandler {
        arrivals: Arc::clone(&arrivals),
        gate: Arc::clone(&gate),
    });
    let mut svc = HttpCacheService::new(handler as Arc<dyn CacheHandler>);
    if let Some(n) = max {
        svc = svc.with_max_connections(n);
    }
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        svc.serve(l).await.expect("the accept loop serves");
    });
    (addr, gate, arrivals)
}

fn get_req(key: &str) -> String {
    format!("GET /entry/{key} HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n")
}

/// The default is the pre-F67 shape — unbounded, no gate materialized —
/// and the builder carries the attached cap for the introspection seam
/// (the F60 `deadline()` shape).
#[tokio::test]
async fn the_default_is_unbounded_and_the_builder_carries_the_cap() {
    let fresh = HttpCacheService::new(TieredCache::new_memory() as Arc<dyn CacheHandler>);
    assert_eq!(
        fresh.max_connections(),
        None,
        "a fresh service rides the pre-F67 unbounded shape"
    );
    let capped = fresh.with_max_connections(7);
    assert_eq!(
        capped.max_connections(),
        Some(7),
        "the accessor carries the attached cap"
    );
}

/// The bound holds: under `cap = 1`, the second connection's exchange
/// does NOT run while the first parks — the handler sees one arrival —
/// and it answers after the first frees (the queued socket drains).
#[tokio::test]
async fn under_cap_one_the_second_exchange_waits_for_the_first() {
    let (addr, gate, arrivals) = gated_plane(Some(1)).await;

    // Shut the gate: every get that runs parks on the mutex.
    let guard = gate.lock().await;

    // Conn A: its exchange runs, arrives, and parks inside the gate.
    let a_req = get_req("route:/a");
    let a = tokio::spawn(raw_exchange(addr, a_req.into_bytes()));
    for _ in 0..200 {
        if arrivals.load(Ordering::SeqCst) == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert_eq!(arrivals.load(Ordering::SeqCst), 1, "A's exchange is parked");

    // Conn B: accepted, but the permit is A's — its get must NOT run
    // while A parks. The negative proof is deterministic: A's parked get
    // holds the permit until the gate opens, so B's arrival can only
    // happen if the bound is broken.
    let b_req = get_req("route:/b");
    let b = tokio::spawn(raw_exchange(addr, b_req.into_bytes()));
    let b_arrived = tokio::time::timeout(Duration::from_millis(50), async {
        while arrivals.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await;
    assert!(
        b_arrived.is_err(),
        "the bound holds: B's exchange must not run while A holds the permit"
    );

    // Open the gate: A proceeds and answers; its task ends and frees the
    // permit, B's queued exchange drains and answers.
    drop(guard);
    let a_resp = tokio::time::timeout(Duration::from_secs(2), a)
        .await
        .expect("A's parked exchange completes once the gate opens")
        .expect("A's exchange task joins");
    assert!(
        a_resp.starts_with("HTTP/1.1 200"),
        "A's parked exchange answers 200: {a_resp}"
    );
    let b_resp = tokio::time::timeout(Duration::from_secs(2), b)
        .await
        .expect("the queued exchange drains after the permit frees")
        .expect("B's exchange task joins");
    assert!(
        b_resp.starts_with("HTTP/1.1 200"),
        "the queued exchange answers correctly: {b_resp}"
    );
    assert_eq!(
        arrivals.load(Ordering::SeqCst),
        2,
        "B's handler ran exactly once, after the permit freed"
    );
}

/// The pre-F67 default serves concurrent exchanges: with no cap, the
/// second exchange RUNS while the first parks — the handler sees both
/// arrivals while the gate holds both parked (the discriminator is the
/// arrival count reaching 2, which under `cap = 1` cannot happen).
#[tokio::test]
async fn the_uncapped_default_runs_exchanges_concurrently() {
    let (addr, gate, arrivals) = gated_plane(None).await;
    let guard = gate.lock().await;
    let a_req = get_req("route:/a");
    let a = tokio::spawn(raw_exchange(addr, a_req.into_bytes()));
    let b_req = get_req("route:/b");
    let b = tokio::spawn(raw_exchange(addr, b_req.into_bytes()));
    let both = tokio::time::timeout(Duration::from_millis(300), async {
        while arrivals.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await;
    assert!(
        both.is_ok(),
        "the uncapped shape runs both exchanges concurrently (no gate)"
    );
    drop(guard);
    let a_resp = tokio::time::timeout(Duration::from_secs(2), a)
        .await
        .unwrap()
        .expect("A's exchange task joins");
    assert!(a_resp.starts_with("HTTP/1.1 200"));
    let b_resp = tokio::time::timeout(Duration::from_secs(2), b)
        .await
        .unwrap()
        .expect("B's exchange task joins");
    assert!(b_resp.starts_with("HTTP/1.1 200"));
}

/// The pure core never touches the gate (the F64 doctrine: the bounds
/// are the shell's concern): a capped service's `respond` answers
/// directly, with no accept loop and no permit in sight.
#[tokio::test]
async fn the_pure_core_never_waits_on_the_bound() {
    let tier = TieredCache::new_memory();
    let svc =
        HttpCacheService::new(Arc::clone(&tier) as Arc<dyn CacheHandler>).with_max_connections(1);
    let resp = svc.respond(&PlaneRequest::get("/entry/route:/core")).await;
    assert_eq!(
        resp.status, 404,
        "the pure core answers the miss directly (no gate in the way)"
    );
}

/// The backlog drains: three exchanges against `cap = 1` — one runs,
/// two queue — and after the first frees, every queued exchange answers
/// correctly (the bound is a wait, never a corruption).
#[tokio::test]
async fn the_backlog_drains_when_the_bound_frees() {
    let (addr, _gate, _arrivals) = gated_plane(Some(1)).await;
    // A real tier behind the gate handler is not needed — the three
    // exchanges hit distinct keys through the SAME gate handler whose
    // get never parks (the mutex is unlocked; the rendezvous above pins
    // the queueing, this pin pins the drain's correctness).
    let mut joins = Vec::new();
    for k in ["route:/d1", "route:/d2", "route:/d3"] {
        let req = get_req(k);
        joins.push(tokio::spawn(raw_exchange(addr, req.into_bytes())));
    }
    for j in joins {
        let resp = tokio::time::timeout(Duration::from_secs(3), j)
            .await
            .expect("the queued exchange drains")
            .expect("the drained exchange task joins");
        assert!(
            resp.starts_with("HTTP/1.1 200"),
            "every drained exchange answers 200: {resp}"
        );
    }
}

/// A generous cap changes no legit answer: the round-trip through a
/// capped shell is byte-shaped exactly like the pre-F67 shell (the F64
/// pin's posture: the bound's cost is the permit, nothing else).
#[tokio::test]
async fn a_generous_cap_changes_no_legit_answer() {
    let tier = TieredCache::new_memory();
    let svc =
        HttpCacheService::new(Arc::clone(&tier) as Arc<dyn CacheHandler>).with_max_connections(64);
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        svc.serve(l).await.expect("the accept loop serves");
    });
    let entry = Entry {
        body: b"<html>capped-v1</html>".to_vec(),
        status: 200,
        headers: vec![],
        tags: vec![],
        policy: oxide_fw_cache::Policy {
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
        etag: "\"capped-v1\"".into(),
    };
    let sealed = oxide_fw_cache::seal_entry(&oxide_fw_cache::encode_entry(&entry).unwrap());
    let put = format!(
        "PUT /entry/route:/capped HTTP/1.1\r\nHost: t\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        sealed.len()
    );
    let mut req = put.into_bytes();
    req.extend_from_slice(&sealed);
    let resp = raw_exchange(addr, req).await;
    assert!(
        resp.starts_with("HTTP/1.1 204"),
        "the legit PUT stores under the cap: {resp}"
    );
    let get = raw_exchange(
        addr,
        b"GET /entry/route:/capped HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n".to_vec(),
    )
    .await;
    assert!(
        get.starts_with("HTTP/1.1 200"),
        "the stored entry serves under the cap: {get}"
    );
}
