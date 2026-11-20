//! TDD pins for the client's inflight bound (V2-F68): the client's fan-
//! out learns the bound the wire's other resources already had.
//!
//! The §11.2 client opens ONE fresh socket per exchange (`Connection:
//! close`) and degrades transport failures through the file's arms — but
//! nothing bounded HOW MANY exchanges ran at once: a render fanning out
//! over a hundred keys opened a hundred concurrent sockets against the
//! same plane, and a struggling peer met a thundering herd instead of a
//! queue. The F60 deadline bounds an exchange's TIME, the F65 read
//! bound bounds its MEMORY, the F66 guard bounds its NAMES — the F68
//! inflight cap bounds the client's CONCURRENCY: `with_inflight_cap`
//! gates `request` on a permit per concurrent exchange (probes included
//! — every `request` consumer rides the same gate), and the permit
//! rides INSIDE the deadline, so a saturated cap degrades within the
//! F60 clock, never past it. The default stays the pre-F68 unbounded
//! shape (the F60 posture — the operator opts in, the choice is
//! greppable), and a generous cap changes no legit answer.
//!
//! Written FIRST (red): the pins fail with E0599 over the missing
//! `with_inflight_cap`/`inflight_cap`.

use oxide_fw_cache::{
    CacheHandler, Entry, HttpCacheHandler, HttpCacheService, PlanePeer, Policy, Result, TieredCache,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// A backing handler whose `get` counts its arrival, then parks on a
/// gate the test holds shut — the deterministic slow plane the
/// rendezvous pins ride (no sleeps; the parking IS the fixture).
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

/// A real plane over the gate handler; the caller picks nothing (the
/// SHELL is unbounded here — the bound under test is the CLIENT's).
async fn gated_plane() -> (
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
    let svc = HttpCacheService::new(handler as Arc<dyn CacheHandler>);
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        svc.serve(l).await.expect("the accept loop serves");
    });
    (addr, gate, arrivals)
}

/// The default is the pre-F68 shape — unbounded — and the builder
/// carries the attached cap for the introspection seam (the F60
/// `deadline()` shape).
#[tokio::test]
async fn the_default_is_unbounded_and_the_builder_carries_the_cap() {
    let bare = HttpCacheHandler::new("http://127.0.0.1:1");
    assert_eq!(
        bare.inflight_cap(),
        None,
        "the default shape is the pre-F68 unbounded client"
    );
    let capped = HttpCacheHandler::new("http://127.0.0.1:1").with_inflight_cap(4);
    assert_eq!(
        capped.inflight_cap(),
        Some(4),
        "the accessor carries the attached cap"
    );
}

/// The bound holds: under `cap = 1`, the second exchange does not reach
/// the wire while the first parks — the plane sees ONE arrival — and it
/// answers after the first frees (the queue drains, the answers are
/// correct).
#[tokio::test]
async fn under_cap_one_the_second_exchange_waits_for_the_wire() {
    let (addr, gate, arrivals) = gated_plane().await;
    let h = Arc::new(HttpCacheHandler::new(format!("http://{addr}")).with_inflight_cap(1));

    // Shut the plane's gate: every exchange that reaches the wire parks
    // server-side.
    let guard = gate.lock().await;

    // Exchange A: runs, arrives, parks server-side.
    let a_client = Arc::clone(&h);
    let a = tokio::spawn(async move {
        a_client
            .get("route:/a")
            .await
            .expect("A's exchange answers");
    });
    for _ in 0..200 {
        if arrivals.load(Ordering::SeqCst) == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert_eq!(arrivals.load(Ordering::SeqCst), 1, "A's exchange is parked");

    // Exchange B: queued client-side (the permit is A's) — its request
    // must NOT reach the wire while A parks.
    let b_client = Arc::clone(&h);
    let b = tokio::spawn(async move {
        b_client
            .get("route:/b")
            .await
            .expect("B's queued exchange answers");
    });
    let b_arrived = tokio::time::timeout(Duration::from_millis(50), async {
        while arrivals.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await;
    assert!(
        b_arrived.is_err(),
        "the bound holds: B's exchange must not reach the wire while A parks"
    );

    // Open the plane's gate: A completes and frees its permit, B's
    // queued exchange runs and answers.
    drop(guard);
    tokio::time::timeout(Duration::from_secs(2), a)
        .await
        .expect("A completes once the gate opens")
        .expect("A's exchange task joins");
    tokio::time::timeout(Duration::from_secs(2), b)
        .await
        .expect("B drains once the permit frees")
        .expect("B's exchange task joins");
    assert_eq!(
        arrivals.load(Ordering::SeqCst),
        2,
        "B's exchange ran exactly once, after the permit freed"
    );
}

/// The pre-F68 default runs exchanges concurrently: with no cap, BOTH
/// requests reach the wire while both park — the plane sees two arrivals
/// (the discriminator the cap=1 pin proves cannot happen).
#[tokio::test]
async fn the_uncapped_default_runs_exchanges_concurrently() {
    let (addr, gate, arrivals) = gated_plane().await;
    let h = Arc::new(HttpCacheHandler::new(format!("http://{addr}")));
    let guard = gate.lock().await;
    let a_client = Arc::clone(&h);
    let a = tokio::spawn(async move {
        a_client
            .get("route:/a")
            .await
            .expect("A's exchange answers");
    });
    let b_client = Arc::clone(&h);
    let b = tokio::spawn(async move {
        b_client
            .get("route:/b")
            .await
            .expect("B's exchange answers");
    });
    let both = tokio::time::timeout(Duration::from_millis(300), async {
        while arrivals.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await;
    assert!(
        both.is_ok(),
        "the uncapped shape puts both exchanges on the wire concurrently"
    );
    drop(guard);
    tokio::time::timeout(Duration::from_secs(2), a)
        .await
        .unwrap()
        .expect("A's exchange task joins");
    tokio::time::timeout(Duration::from_secs(2), b)
        .await
        .unwrap()
        .expect("B's exchange task joins");
}

/// The queue rides the F60 clock: under `cap = 1` with a deadline
/// attached, a queued exchange degrades LOUD inside the deadline — the
/// F60 arm fires on the F68 queue, the §11.2 contract holds even under a
/// saturated cap.
#[tokio::test]
async fn the_deadline_bounds_the_queue_too() {
    let (addr, gate, arrivals) = gated_plane().await;
    let h = Arc::new(
        HttpCacheHandler::new(format!("http://{addr}"))
            .with_inflight_cap(1)
            .with_timeout(Duration::from_millis(250)),
    );

    let guard = gate.lock().await;
    let a_client = Arc::clone(&h);
    let a = tokio::spawn(async move {
        // A parks past its deadline too — the deadline is the WHOLE
        // exchange's clock (the F60 shape); its Err is the cleanup arm.
        let _ = a_client.get("route:/a").await;
    });
    for _ in 0..200 {
        if arrivals.load(Ordering::SeqCst) == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }

    // B queues behind A's permit — the deadline fires on the QUEUE: a
    // loud `Err` under `CACHE-HTTP` naming the deadline, never a hang.
    // The verb is `set` — the F60 pins' loud shape (`get` degrades a
    // deadline to a miss by contract; `set` errs).
    let b_client = Arc::clone(&h);
    let b = tokio::spawn(async move {
        b_client
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
    let b_out = tokio::time::timeout(Duration::from_secs(2), b)
        .await
        .expect("the queued exchange degrades inside its deadline")
        .expect("B's exchange task joins");
    let b_err = b_out.expect_err("the queued exchange degrades LOUD under the deadline");
    assert!(
        b_err.0.message.contains("deadline"),
        "the degradation names the arm that fired: {}",
        b_err.0.message
    );

    drop(guard);
    tokio::time::timeout(Duration::from_secs(2), a)
        .await
        .expect("A's cleanup completes once the gate opens")
        .expect("A's exchange task joins");
}

/// The probes ride the same gate: under `cap = 1` with an exchange
/// parked, `peer_health` waits in the queue (its request never reaches
/// the wire) and answers — loudly, the provider-less 404 — once the
/// permit frees. The F63 probes are `request` consumers like any verb.
#[tokio::test]
async fn the_probes_ride_the_same_gate() {
    let (addr, gate, arrivals) = gated_plane().await;
    let h = Arc::new(HttpCacheHandler::new(format!("http://{addr}")).with_inflight_cap(1));

    let guard = gate.lock().await;
    let a_client = Arc::clone(&h);
    let a = tokio::spawn(async move {
        let _ = a_client.get("route:/a").await;
    });
    for _ in 0..200 {
        if arrivals.load(Ordering::SeqCst) == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }

    // The probe: queued behind A's permit — no answer (not even the
    // provider-less 404) may arrive while A parks.
    let probe_client = Arc::clone(&h);
    let probe = tokio::spawn(async move { probe_client.peer_health().await });
    let probe_answered = tokio::time::timeout(Duration::from_millis(50), async {
        loop {
            if probe.is_finished() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await;
    assert!(
        probe_answered.is_err(),
        "the bound holds for the probes: peer_health waits in the queue while A parks"
    );

    drop(guard);
    tokio::time::timeout(Duration::from_secs(2), a)
        .await
        .expect("A completes once the gate opens")
        .expect("A's exchange task joins");
    let posture = tokio::time::timeout(Duration::from_secs(2), probe)
        .await
        .expect("the probe drains once the permit frees")
        .expect("the probe task joins");
    let probe_err = posture.expect_err("the provider-less plane answers the F40 404");
    assert!(
        probe_err.0.message.contains("404"),
        "the probe's loud error names the status: {}",
        probe_err.0.message
    );
}

/// A generous cap changes no legit answer: the sealed round-trip through
/// a capped client is exactly the pre-F68 shape (the F64 pin's posture:
/// the bound's cost is the permit, nothing else).
#[tokio::test]
async fn a_generous_cap_changes_no_legit_answer() {
    let tier = TieredCache::new_memory();
    let svc = HttpCacheService::new(Arc::clone(&tier) as Arc<dyn CacheHandler>);
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        svc.serve(l).await.expect("the accept loop serves");
    });
    let h = HttpCacheHandler::new(format!("http://{addr}")).with_inflight_cap(64);
    let entry = Entry {
        body: b"<html>inflight-v1</html>".to_vec(),
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
        etag: "\"inflight-v1\"".into(),
    };
    h.set("route:/inflight", entry)
        .await
        .expect("the set stores");
    let got = h
        .get("route:/inflight")
        .await
        .expect("the get answers")
        .expect("the entry hits");
    assert_eq!(got.body, b"<html>inflight-v1</html>");
}
