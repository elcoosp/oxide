//! TDD pins for the V2-F05 live-loaders substrate (`oxide_fw_server::live`,
//! spec v0.2 §V2-F05 steps 3–5): the SSE framing, the tag hub that answers
//! "which open live connections depend on this cache tag", the per-connection
//! snapshot stream, and the rerun runner's debounce/rate contract.
//!
//! Written FIRST (red): these pins failed on unresolved-symbol errors before
//! the `live` module existed. The endpoint itself (`GET /__oxide/live`) and
//! the rerun closure that calls the CLI's data-route path are the A01-phase-3
//! wiring — see `docs/deviations.md`.

use std::collections::BTreeSet;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use base64::Engine as _;
use bytes::Bytes;
use futures::StreamExt;
use oxide_fw_server::live::{live_response, live_stream, same_origin, sse_frame};
use oxide_fw_server::{Guard, LiveHub, LiveRunner, Rerun, Sub, SubscribeError};

const LOOPBACK: IpAddr = std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);

/// A subscriber with the given tags (and, optionally, a peer ip).
fn sub(route: &str, tags: &[&str], ip: Option<IpAddr>) -> Arc<Sub> {
    Arc::new(Sub::new(
        route,
        "/blog/rust",
        Vec::new(),
        tags.iter().map(|t| (*t).to_string()),
        ip,
    ))
}

fn b64(raw: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(raw)
}

// ------------------------------------------------------------------- framing

/// The spec's own pin name (§V2-F05 tests: `sse_frame_multiline_data`).
#[test]
fn sse_frame_multiline_data() {
    assert_eq!(
        sse_frame("data", Some(7), "line1\nline2"),
        "event: data\nid: 7\ndata: line1\ndata: line2\n\n"
    );
    // No id, single line: the ping and stale spellings.
    assert_eq!(sse_frame("ping", None, "1"), "event: ping\ndata: 1\n\n");
    assert_eq!(
        sse_frame("stale", None, "deadbeef00000001"),
        "event: stale\ndata: deadbeef00000001\n\n"
    );
}

// ----------------------------------------------------------------- the hub

/// The spec's own pin name (§V2-F05 tests: `affected_returns_only_subscribers_of_tag`).
#[test]
fn affected_returns_only_subscribers_of_tag() {
    let hub = LiveHub::new(100, 20);
    let a = sub("routes/blog/[slug]", &["post:rust"], None);
    let b = sub("routes/index", &["home"], None);
    let _ga = hub.subscribe(Arc::clone(&a)).expect("a subscribes");
    let _gb = hub.subscribe(Arc::clone(&b)).expect("b subscribes");

    let hit = hub.affected("post:rust");
    assert_eq!(hit.len(), 1, "only the post:rust subscriber");
    assert!(Arc::ptr_eq(&hit[0], &a));

    assert!(hub.affected("nothing:here").is_empty(), "unknown tag");
}

/// The spec's own pin name (§V2-F05 tests: `guard_drop_unsubscribes`).
#[test]
fn guard_drop_unsubscribes() {
    let hub = LiveHub::new(100, 20);
    let s = sub("routes/index", &["home"], None);
    {
        let _g: Guard = hub.subscribe(Arc::clone(&s)).expect("subscribes");
        assert_eq!(hub.affected("home").len(), 1);
        assert_eq!(hub.count(), 1);
    }
    assert!(
        hub.affected("home").is_empty(),
        "dropping the guard removes the sub"
    );
    assert_eq!(hub.count(), 0);
}

/// The spec's own pin name (§V2-F05 tests: `capacity_exceeded_is_err`).
#[test]
fn capacity_exceeded_is_err() {
    let hub = LiveHub::new(1, 20);
    let g1 = hub
        .subscribe(sub("routes/a", &["t"], None))
        .expect("first fits");
    assert_eq!(
        hub.subscribe(sub("routes/b", &["t"], None)).err(),
        Some(SubscribeError::Capacity),
        "the endpoint answers 503 + retry-after on this"
    );
    assert_eq!(hub.count(), 1, "the rejected sub was rolled back");
    // Dropping the live guard frees the slot.
    drop(g1);
    hub.subscribe(sub("routes/c", &["t"], None))
        .expect("slot freed");
}

/// The spec's own pin name (§V2-F05 tests: `wildcard_tag_matches_prefix`).
#[test]
fn wildcard_tag_matches_prefix() {
    let hub = LiveHub::new(100, 20);
    let wildcard = sub("routes/comments", &["comments:*"], None);
    let exact = sub("routes/blog", &["post:1"], None);
    let _gw = hub
        .subscribe(Arc::clone(&wildcard))
        .expect("wildcard subscribes");
    let _ge = hub.subscribe(Arc::clone(&exact)).expect("exact subscribes");

    let hit = hub.affected("comments:42");
    assert_eq!(hit.len(), 1, "comments:42 matches the comments:* prefix");
    assert!(Arc::ptr_eq(&hit[0], &wildcard));
    // Exact tags still route exactly; a prefix hit never pulls in
    // subscribers that only hold unrelated exact tags.
    assert!(
        hub.affected("post:1")
            .iter()
            .all(|s| Arc::ptr_eq(s, &exact))
    );
    assert!(hub.affected("user:9").is_empty());
}

#[test]
fn a_tag_can_be_held_by_several_subscribers_without_duplicates() {
    let hub = LiveHub::new(100, 20);
    let a = sub("routes/a", &["shared"], None);
    let b = sub("routes/b", &["shared"], None);
    let _ga = hub.subscribe(Arc::clone(&a)).expect("a");
    let _gb = hub.subscribe(Arc::clone(&b)).expect("b");
    let hit = hub.affected("shared");
    assert_eq!(hit.len(), 2, "both subscribers, deduplicated");
}

/// The spec security bullet: "per-IP subscriber cap (default 20)".
#[test]
fn per_ip_cap_rejects_the_excess_subscriber() {
    let hub = LiveHub::new(100, 1);
    let g1 = hub
        .subscribe(sub("routes/a", &["t"], Some(LOOPBACK)))
        .expect("first per-ip fits");
    assert_eq!(
        hub.subscribe(sub("routes/b", &["t"], Some(LOOPBACK))).err(),
        Some(SubscribeError::PerIp)
    );
    // A different peer is not affected by loopback's cap.
    let other = std::net::IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 0, 1));
    let g2 = hub
        .subscribe(sub("routes/c", &["t"], Some(other)))
        .expect("other peer unaffected");
    // Dropping the loopback guard frees its per-ip slot.
    drop(g1);
    let g3 = hub
        .subscribe(sub("routes/d", &["t"], Some(LOOPBACK)))
        .expect("slot freed");
    drop(g2);
    drop(g3);
}

#[test]
fn retag_reindexes_the_subscriber() {
    // The rerun worker records the tags the load actually read (spec step
    // 4: "route config tags + tags recorded by the last run"); the hub must
    // follow the sub's tag set.
    let hub = LiveHub::new(100, 20);
    let s = sub("routes/a", &["old"], None);
    let g = hub.subscribe(Arc::clone(&s)).expect("subscribes");
    hub.retag(
        &s,
        BTreeSet::from(["new".to_string(), "comments:*".to_string()]),
    );
    assert!(
        hub.affected("old").is_empty(),
        "the stale index entry is gone"
    );
    assert_eq!(hub.affected("new").len(), 1);
    assert_eq!(
        hub.affected("comments:7").len(),
        1,
        "the new wildcard reindexes"
    );
    // Dropping the guard still cleans up every index entry.
    drop(g);
    assert!(hub.affected("new").is_empty());
    assert!(hub.affected("comments:7").is_empty());
}

// ------------------------------------------------------------- snapshots

#[test]
fn snapshot_push_and_read_round_trips() {
    let s = sub("routes/a", &["t"], None);
    assert!(
        s.snapshot_now().is_none(),
        "no snapshot before the first rerun"
    );
    s.push_snapshot(Bytes::from_static(b"hello"));
    assert_eq!(
        s.snapshot_now().as_deref(),
        Some(&b"hello"[..]),
        "the watch keeps the latest snapshot"
    );
    // A cleared snapshot (failed rerun) is observable too.
    s.clear_snapshot();
    assert!(s.snapshot_now().is_none());
}

// ------------------------------------------------------------- the stream

/// Read up to `n` frames (or fail after a short timeout — paused time where
/// it matters keeps the waits deterministic).
async fn frames(stream: &mut oxide_fw_server::BodyStream, n: usize) -> Vec<String> {
    let mut out = Vec::new();
    for _ in 0..n {
        match tokio::time::timeout(Duration::from_millis(200), stream.next()).await {
            Ok(Some(Ok(chunk))) => out.push(String::from_utf8(chunk.to_vec()).expect("utf8 frame")),
            other => panic!("expected a frame, got {other:?}"),
        }
    }
    out
}

#[tokio::test]
async fn fresh_connection_receives_the_current_snapshot_first() {
    // Spec step 3: "A fresh connection immediately receives a full `data`
    // snapshot, so reconnect after a drop never needs replay."
    let s = sub("routes/a", &["t"], None);
    s.push_snapshot(Bytes::from_static(b"ODF-SNAPSHOT"));
    let rx = s.tx.subscribe();
    let mut stream = live_stream(rx, Duration::from_secs(20));
    let out = frames(&mut stream, 1).await;
    assert!(
        out[0].starts_with("event: data\nid: 0\ndata: "),
        "the data event with the connection's first id: {:?}",
        out[0]
    );
    assert!(
        out[0].contains(&b64("ODF-SNAPSHOT")),
        "the payload is the base64 ODF snapshot: {:?}",
        out[0]
    );
    assert!(out[0].ends_with("\n\n"), "SSE event terminator");
}

/// The spec runner pin, applied to the stream side: "`watch` keeps only the
/// latest snapshot" — a slow client skips intermediate states.
#[tokio::test]
async fn latest_snapshot_wins_and_intermediate_states_are_skipped() {
    let s = sub("routes/a", &["t"], None);
    let rx = s.tx.subscribe();
    let mut stream = live_stream(rx, Duration::from_secs(20));
    // Three reruns land before the client reads anything.
    s.push_snapshot(Bytes::from_static(b"v1"));
    s.push_snapshot(Bytes::from_static(b"v2"));
    s.push_snapshot(Bytes::from_static(b"v3"));
    let out = frames(&mut stream, 1).await;
    assert!(
        out[0].contains(&b64("v3")),
        "the delivered frame carries the latest snapshot: {:?}",
        out[0]
    );
    assert!(
        !out[0].contains(&b64("v1")) && !out[0].contains(&b64("v2")),
        "no intermediate snapshots leak through: {:?}",
        out[0]
    );
}

#[tokio::test(start_paused = true)]
async fn ping_frames_follow_the_keepalive_cadence() {
    let s = sub("routes/a", &["t"], None);
    let rx = s.tx.subscribe();
    // No snapshot yet: the first frame must be the keepalive ping, one
    // keepalive period after connect — not immediately (interval's
    // first-tick semantics would ping-spam a fresh connection).
    let mut stream = live_stream(rx, Duration::from_millis(20));
    let out = frames(&mut stream, 1).await;
    assert_eq!(out[0], "event: ping\ndata: 1\n\n");
    // The next ping lands one period later, not immediately after.
    let out = frames(&mut stream, 1).await;
    assert_eq!(out[0], "event: ping\ndata: 1\n\n");
}

#[tokio::test]
async fn sender_drop_ends_the_stream() {
    // The disconnect semantics: when the subscriber goes away (or the
    // endpoint closes), the response body ends — no zombie connections.
    let s = sub("routes/a", &["t"], None);
    let rx = s.tx.subscribe();
    let mut stream = live_stream(rx, Duration::from_secs(20));
    drop(s);
    let next = tokio::time::timeout(Duration::from_millis(200), stream.next()).await;
    assert!(
        matches!(next, Ok(None)),
        "the stream ends cleanly: {next:?}"
    );
}

// ------------------------------------------------------- response surface

#[test]
fn live_response_carries_the_sse_head() {
    let stream: oxide_fw_server::BodyStream = Box::pin(futures::stream::empty());
    let resp = live_response(stream);
    assert_eq!(resp.status, 200, "spec step 2: 200");
    let headers = &resp.headers;
    assert!(
        headers
            .iter()
            .any(|(k, v)| k == "content-type" && v == "text/event-stream"),
        "content-type: text/event-stream, got {headers:?}"
    );
    assert!(
        headers
            .iter()
            .any(|(k, v)| k == "cache-control" && v == "no-store")
    );
    assert!(
        headers
            .iter()
            .any(|(k, v)| k == "x-accel-buffering" && v == "no")
    );
    assert!(resp.stream.is_some(), "the body is a stream");
}

// ----------------------------------------------------------- origin check

#[test]
fn same_origin_accepts_matching_evidence_only() {
    // Sec-Fetch-Site is the strongest signal when present.
    assert!(same_origin(None, Some("same-origin"), Some("example.com")));
    assert!(!same_origin(None, Some("cross-site"), Some("example.com")));
    assert!(!same_origin(None, Some("same-site"), Some("example.com")));
    // Fall back to the Origin authority vs the Host header.
    assert!(same_origin(
        Some("https://example.com"),
        None,
        Some("example.com")
    ));
    assert!(same_origin(
        Some("https://example.com:8443"),
        None,
        Some("example.com:8443")
    ));
    assert!(!same_origin(
        Some("https://evil.example"),
        None,
        Some("example.com")
    ));
    assert!(!same_origin(
        Some("https://example.com:8444"),
        None,
        Some("example.com:8443")
    ));
    // Host header absent: no ground truth, deny.
    assert!(!same_origin(Some("https://example.com"), None, None));
    // Neither Origin nor Sec-Fetch-Site: not a browser same-origin fetch, deny.
    assert!(!same_origin(None, None, Some("example.com")));
    // Host names compare case-insensitively (DNS is case-insensitive).
    assert!(same_origin(
        Some("https://EXAMPLE.com"),
        None,
        Some("example.com")
    ));
}

// ---------------------------------------------------------------- runner

/// The spec's own pin (§V2-F05 runner tests): "debounce collapses 5
/// notifications in 50 ms into one rerun (paused time)".
#[tokio::test(start_paused = true)]
async fn debounce_collapses_notifications_into_one_rerun() {
    let (tx, rx) = tokio::sync::mpsc::channel::<Arc<Sub>>(64);
    let s = sub("routes/a", &["t"], None);
    let calls = Arc::new(AtomicUsize::new(0));
    let calls2 = Arc::clone(&calls);
    let rerun: Rerun = Arc::new(move |_sub| {
        let calls = Arc::clone(&calls2);
        Box::pin(async move {
            calls.fetch_add(1, Ordering::SeqCst);
            Some(Bytes::from_static(b"snapshot"))
        })
    });
    let runner = LiveRunner::new(rx, Duration::from_millis(50), 20, rerun);
    let handle = tokio::spawn(runner.run());
    for _ in 0..5 {
        tx.send(Arc::clone(&s)).await.expect("send");
    }
    // The runner collapses the burst into one rerun and pushes the snapshot.
    tokio::time::advance(Duration::from_millis(120)).await;
    drop(tx); // close the channel: the runner finishes its batch and exits
    handle.await.expect("runner ends when the channel closes");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "one rerun for the whole burst"
    );
    assert_eq!(
        s.snapshot_now().as_deref(),
        Some(&b"snapshot"[..]),
        "the rerun result lands in the subscriber's watch channel"
    );
}

#[tokio::test(start_paused = true)]
async fn rate_cap_drops_excess_reruns() {
    let (tx, rx) = tokio::sync::mpsc::channel::<Arc<Sub>>(64);
    let s = sub("routes/a", &["t"], None);
    let calls = Arc::new(AtomicUsize::new(0));
    let calls2 = Arc::clone(&calls);
    let rerun: Rerun = Arc::new(move |_| {
        let calls = Arc::clone(&calls2);
        Box::pin(async move {
            calls.fetch_add(1, Ordering::SeqCst);
            None
        })
    });
    // Max 1 rerun per second.
    let runner = LiveRunner::new(rx, Duration::from_millis(10), 1, rerun);
    let handle = tokio::spawn(runner.run());
    // First notification: the rerun happens.
    tx.send(Arc::clone(&s)).await.expect("send");
    tokio::time::advance(Duration::from_millis(30)).await;
    // Second burst 30ms later — inside the 1s rate window: dropped.
    tx.send(Arc::clone(&s)).await.expect("send");
    tokio::time::advance(Duration::from_millis(120)).await;
    drop(tx);
    handle.await.expect("runner ends");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the capped rerun never ran"
    );
}

#[tokio::test(start_paused = true)]
async fn failed_rerun_clears_the_snapshot_for_the_stale_signal() {
    // Spec step 5: "If the rerun redirects or errors, send `event: stale`
    // and close so the client does a normal navigation." The substrate
    // form: a `None` rerun result clears the snapshot; the wiring watches
    // the cleared channel to emit the stale frame and end the response.
    let (tx, rx) = tokio::sync::mpsc::channel::<Arc<Sub>>(64);
    let s = sub("routes/a", &["t"], None);
    s.push_snapshot(Bytes::from_static(b"old"));
    let rerun: Rerun = Arc::new(|_| Box::pin(async { None }));
    let runner = LiveRunner::new(rx, Duration::from_millis(5), 20, rerun);
    let handle = tokio::spawn(runner.run());
    tx.send(Arc::clone(&s)).await.expect("send");
    tokio::time::advance(Duration::from_millis(30)).await;
    drop(tx);
    handle.await.expect("runner ends");
    assert!(
        s.snapshot_now().is_none(),
        "the failed rerun cleared the snapshot"
    );
}

// --------------------------------------------------------------------- F06

/// Written FIRST (red): these pins failed on unresolved-symbol errors before
/// the F06 delta policy existed. The spec's sending rule (§V2-F06 / D30):
/// keep, per live subscriber, the last full encoded snapshot and its id; on
/// a rerun diff old against new and ship `ODP1` only when
/// `patch.len() * 2 < full.len()` and `ops.len() <= 64` — otherwise the
/// full payload. A fresh connection (no base yet) always gets the full.
mod f06 {
    use super::*;

    use oxide_fw_odf::Value;
    use oxide_fw_odf::patch::{apply as patch_apply, base_id, decode_patch};

    fn v(j: serde_json::Value) -> Value {
        Value::from_json(&j)
    }

    /// ODF bytes of a JSON value (the rerun closure's return shape).
    fn enc(j: serde_json::Value) -> Bytes {
        Bytes::from(oxide_fw_odf::encode(&v(j)).expect("test value encodes"))
    }

    fn rows(n: usize) -> serde_json::Value {
        serde_json::Value::Array(
            (0..n)
                .map(|k| {
                    serde_json::json!({
                        "i": k,
                        "name": format!("row-{k}"),
                        "body": format!("The body text of row {k}, long enough to dominate the payload."),
                    })
                })
                .collect(),
        )
    }

    /// The two-row list with one row appended — the smallest patching shape.
    fn rows_appended(n: usize) -> serde_json::Value {
        let mut r = rows(n);
        r.as_array_mut()
            .expect("rows is an array")
            .push(serde_json::json!({ "i": n, "name": format!("row-{n}"), "body": "appended" }));
        r
    }

    #[test]
    fn delta_payload_first_push_is_the_full_snapshot() {
        let next = enc(rows(2));
        let out = oxide_fw_server::live::delta_payload(None, &next);
        assert_eq!(out, next, "no base yet: the full snapshot ships unchanged");
    }

    #[test]
    fn delta_payload_small_change_sends_odp1() {
        let prev = enc(rows(2));
        let next = enc(rows_appended(2));
        let out = oxide_fw_server::live::delta_payload(Some(&prev), &next);
        assert_eq!(&out[..4], b"ODP1", "a one-row append patches");
        let (b, ops) = decode_patch(&out).expect("the patch decodes");
        assert_eq!(b, base_id(&prev), "the base id hashes the previous full");
        assert_eq!(ops.len(), 1, "an append is exactly one Splice");
        let applied = patch_apply(&oxide_fw_odf::decode(&prev).expect("prev decodes"), &ops)
            .expect("the ops apply to the previous snapshot");
        assert_eq!(
            applied,
            oxide_fw_odf::decode(&next).expect("next decodes"),
            "apply(prev, diff) == next"
        );
    }

    #[test]
    fn delta_payload_large_change_sends_the_full_payload() {
        // Every row's body changes: one Splice whose insert is nearly as
        // large as the payload — `patch.len() * 2 < full.len()` fails.
        let prev = enc(rows(40));
        let mut next = rows(40);
        for row in next.as_array_mut().expect("rows is an array") {
            row["body"] =
                serde_json::json!("completely rewritten body text for the large-change pin");
        }
        let next = enc(next);
        let out = oxide_fw_server::live::delta_payload(Some(&prev), &next);
        assert_eq!(out, next, "an almost-rewrite ships the full payload");
    }

    #[test]
    fn delta_payload_over_the_op_cap_sends_the_full_payload() {
        // 65 independent key changes → 65 Set ops → over the D30 cap (64).
        let prev = enc(serde_json::Value::Object(
            (0..65)
                .map(|k| (format!("k{k:02}"), serde_json::json!(k)))
                .collect(),
        ));
        let next = enc(serde_json::Value::Object(
            (0..65)
                .map(|k| (format!("k{k:02}"), serde_json::json!(k + 1)))
                .collect(),
        ));
        let out = oxide_fw_server::live::delta_payload(Some(&prev), &next);
        assert_eq!(out, next, "more than 64 ops ships the full payload");
    }

    #[test]
    fn delta_payload_undecodable_bytes_pass_through() {
        // The rerun closure may hand back anything (e.g. an error page);
        // a payload that does not decode is forwarded untouched — the
        // client's decoder refuses it and reconnects, never corrupts.
        let garbage = Bytes::from_static(b"not-odf");
        let out = oxide_fw_server::live::delta_payload(None, &garbage);
        assert_eq!(out, garbage);
        let prev = enc(rows(2));
        let out = oxide_fw_server::live::delta_payload(Some(&prev), &garbage);
        assert_eq!(out, garbage);
    }

    #[test]
    fn prepare_rerun_chains_the_patch_base_across_reruns() {
        let s = sub("routes/a", &["t"], None);
        // 40-row lists: the patch envelope overhead is amortized, so the
        // D30 size predicate genuinely holds for both steps of the chain.
        let full1 = enc(rows(40));
        let full2 = enc(rows_appended(40));
        let full3 = enc(rows(41));
        // First rerun on a fresh subscriber: full.
        let p1 = s.prepare_rerun(Some(full1.clone()));
        assert_eq!(p1, Some(full1.clone()));
        assert_eq!(s.last_full_snapshot(), Some(full1.clone()));
        // Second rerun: a patch chained from full1.
        let p2 = s.prepare_rerun(Some(full2.clone())).expect("a payload");
        assert_eq!(&p2[..4], b"ODP1");
        let (b2, _) = decode_patch(&p2).expect("p2 decodes");
        assert_eq!(b2, base_id(&full1));
        // Third rerun: chained from full2 (the FULL snapshot, not the patch).
        let p3 = s.prepare_rerun(Some(full3.clone())).expect("a payload");
        assert_eq!(&p3[..4], b"ODP1");
        let (b3, _) = decode_patch(&p3).expect("p3 decodes");
        assert_eq!(b3, base_id(&full2));
        assert_eq!(s.last_full_snapshot(), Some(full3));
    }

    #[test]
    fn prepare_rerun_none_leaves_the_base_and_returns_none() {
        let s = sub("routes/a", &["t"], None);
        let full1 = enc(rows(2));
        let _ = s.prepare_rerun(Some(full1.clone()));
        assert_eq!(
            s.prepare_rerun(None),
            None,
            "the stale signal passes through"
        );
        assert_eq!(
            s.last_full_snapshot(),
            Some(full1),
            "a failed rerun keeps the base for the reconnecting client"
        );
    }

    #[test]
    fn push_snapshot_seeds_the_patch_base() {
        // The connect path ships the current snapshot as a full payload and
        // the client hashes it — the base for the FIRST rerun patch.
        let s = sub("routes/a", &["t"], None);
        let full1 = enc(rows(2));
        s.push_snapshot(full1.clone());
        let next = enc(rows_appended(2));
        let payload = s.prepare_rerun(Some(next)).expect("a payload");
        assert_eq!(
            &payload[..4],
            b"ODP1",
            "the seeded base patches immediately"
        );
        let (b, _) = decode_patch(&payload).expect("decodes");
        assert_eq!(b, base_id(&full1));
    }

    // Real time (not the paused clock): the rerun batches must be spaced
    // far enough apart that each clears the 5 ms debounce window AND the
    // 50 ms rate interval — 60 ms spacing makes all three reruns land.
    #[tokio::test]
    async fn the_runner_streams_patches_after_the_first_full() {
        let fulls = vec![enc(rows(40)), enc(rows_appended(40)), enc(rows(41))];
        let idx = Arc::new(AtomicUsize::new(0));
        let rerun: Rerun = {
            let idx = Arc::clone(&idx);
            let fulls = fulls.clone();
            Arc::new(move |_| {
                let i = idx.fetch_add(1, Ordering::SeqCst).min(fulls.len() - 1);
                let full = fulls[i].clone();
                Box::pin(async move { Some(full) })
            })
        };
        let (tx, rx) = tokio::sync::mpsc::channel::<Arc<Sub>>(64);
        let s = sub("routes/a", &["t"], None);
        let runner = LiveRunner::new(rx, Duration::from_millis(5), 20, rerun);
        let handle = tokio::spawn(runner.run());
        tx.send(Arc::clone(&s)).await.expect("send");
        tokio::time::sleep(Duration::from_millis(60)).await;
        tx.send(Arc::clone(&s)).await.expect("send");
        tokio::time::sleep(Duration::from_millis(60)).await;
        tx.send(Arc::clone(&s)).await.expect("send");
        tokio::time::sleep(Duration::from_millis(60)).await;
        drop(tx);
        handle.await.expect("runner ends");
        // The channel holds what the CLIENT receives: the first rerun ships
        // fulls[0] as-is; every later rerun patches from the previous full.
        assert_eq!(idx.load(Ordering::SeqCst), 3, "three spaced reruns ran");
        let current = s.snapshot_now().expect("a payload in the channel");
        assert_eq!(
            &current[..4],
            b"ODP1",
            "the last shipped payload is a patch"
        );
        let (b, _) = decode_patch(&current).expect("decodes");
        assert_eq!(b, base_id(&fulls[1]), "chained from the previous full");
        assert_eq!(
            s.last_full_snapshot(),
            Some(fulls[2].clone()),
            "the base tracks the fulls"
        );
    }
}
