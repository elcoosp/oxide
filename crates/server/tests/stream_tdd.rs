//! TDD pins for the V2-F07 resumable-streams substrate
//! (`oxide_fw_server::stream_store`, spec v0.2 §V2-F07).
//!
//! A long streaming response survives a dropped connection: the server tees
//! the bytes into a bounded, TTL'd buffer keyed by a stream id, the client
//! reattaches at `GET /__oxide/stream/<id>` with `Range: bytes=<n>-`, and
//! receives only the bytes it missed — first the backlog, then the live
//! tail until the producer finishes.
//!
//! Written FIRST (red): these pins failed on unresolved-symbol errors
//! before the `stream_store` module existed. The wiring (teeing the
//! endpoint's response body in the dispatch closure, the route handler in
//! the CLI, the per-IP reattach rate limit) is the A01-phase-3 CLI
//! adoption — see `docs/deviations.md`.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures::StreamExt;
use oxide_fw_server::stream_store::{
    Reattach, StreamStore, parse_range_offset, read_from, stream_gone_response, stream_id,
    stream_resume_response, tee_stream,
};

/// A store with generous limits (single-buffer tests do not want eviction
/// or overflow interference) plus its first buffer seeded with `chunks`.
fn seeded(
    chunks: &[&[u8]],
) -> (
    Arc<StreamStore>,
    Arc<oxide_fw_server::stream_store::StreamBuf>,
) {
    let store = Arc::new(StreamStore::new(Duration::from_secs(60), 1 << 20, 1 << 30));
    let buf = store.begin("s1".into());
    for c in chunks {
        store.append(&buf, Bytes::copy_from_slice(c));
    }
    (store, buf)
}

// ------------------------------------------------------------------ read_from

/// The spec's own pin (§V2-F07 tests: `read_from` offsets — start,
/// mid-chunk, end, beyond end is None).
#[test]
fn read_from_offsets_start_mid_chunk_end_and_beyond_end() {
    let (_s, b) = seeded(&[&[0u8; 64][..], &[9u8; 64][..]]);
    // Start: the whole buffer.
    let all = read_from(&b, 0).expect("offset 0 replays everything");
    assert_eq!(all.len(), 128);
    assert_eq!(all[0], 0);
    assert_eq!(all[64], 9);
    // Mid-chunk: a slice inside the second chunk.
    let mid = read_from(&b, 100).expect("mid-chunk offset replays the tail");
    assert_eq!(mid.len(), 28);
    assert_eq!(mid[0], 9);
    // Chunk boundary.
    assert_eq!(read_from(&b, 64).expect("boundary").len(), 64);
    // The exact end is an empty Some (the complete-reattach case).
    assert_eq!(
        read_from(&b, 128).expect("end offset is an empty Some"),
        Bytes::new()
    );
    // Beyond the end is None (the client skipped bytes it cannot get back).
    assert!(read_from(&b, 129).is_none(), "beyond-end is None");
}

/// Empty buffer: offset 0 replays an empty Some until chunks arrive.
#[test]
fn read_from_on_an_empty_buffer_is_an_empty_some() {
    let (_s, b) = seeded(&[]);
    assert_eq!(
        read_from(&b, 0).expect("empty buffer at 0"),
        Bytes::new(),
        "offset 0 on an empty (in-flight) stream is an empty Some, not Gone"
    );
}

// ----------------------------------------------------------------- stream ids

/// `stream_id` is 16 lowercase hex (the build-id convention), stable per
/// (request id, counter) and distinct per counter.
#[test]
fn stream_id_is_16_hex_and_deterministic() {
    let a = stream_id("req-42", 0);
    let b = stream_id("req-42", 0);
    let c = stream_id("req-42", 1);
    assert_eq!(a, b, "same inputs, same id");
    assert_ne!(a, c, "the counter part distinguishes streams");
    assert_eq!(a.len(), 16, "16 hex chars");
    assert!(
        a.bytes()
            .all(|x| x.is_ascii_hexdigit() && !x.is_ascii_uppercase()),
        "lowercase hex: {a}"
    );
    assert_ne!(stream_id("req-43", 0), a, "distinct per request id");
}

// ------------------------------------------------------------------- the store

#[test]
fn get_returns_the_same_buffer_and_unknown_ids_are_gone() {
    let store = Arc::new(StreamStore::new(Duration::from_secs(60), 1 << 20, 1 << 30));
    assert!(
        matches!(store.get("nope"), Reattach::Gone),
        "an unknown id is Gone"
    );
    let b = store.begin("s1".into());
    store.append(&b, Bytes::from_static(b"hello"));
    match store.get("s1") {
        Reattach::Ok(found) => assert!(Arc::ptr_eq(&found, &b), "the same buffer"),
        Reattach::Gone => panic!("a live buffer within ttl is Ok"),
    }
}

/// The spec's own pin (§V2-F07 tests: `reattach after complete yields the
/// tail`): a completed buffer stays reattachable for the whole ttl, and the
/// reattach yields exactly the bytes after the offset.
#[test]
fn reattach_after_complete_yields_the_tail() {
    let (store, b) = seeded(&[&b"line-1\n"[..], &b"line-2\n"[..], &b"line-3\n"[..]]);
    store.finish(&b);
    assert!(b.is_done());
    match store.get("s1") {
        Reattach::Ok(found) => {
            let tail = read_from(&found, 7).expect("the offset is within len");
            assert_eq!(tail.as_ref(), b"line-2\nline-3\n");
        }
        Reattach::Gone => panic!("a completed buffer within ttl stays reattachable"),
    }
}

/// The spec's own pin (§V2-F07 tests: `overflow disables replay`): a buffer
/// larger than `max_each` is dropped from replay entirely.
#[test]
fn overflow_disables_replay() {
    let store = Arc::new(StreamStore::new(
        Duration::from_secs(60),
        8, // max_each: 8 bytes
        1 << 30,
    ));
    let b = store.begin("s1".into());
    store.append(&b, Bytes::copy_from_slice(b"12345678"));
    assert!(!b.overflowed(), "the first chunk fits");
    store.append(&b, Bytes::copy_from_slice(b"9"));
    assert!(b.overflowed(), "the ninth byte overflows");
    assert_eq!(b.len_now(), 8, "len stops growing past the limit");
    assert!(
        matches!(store.get("s1"), Reattach::Gone),
        "an overflowed buffer is Gone: the client falls back to a fresh fetch"
    );
    store.append(&b, Bytes::copy_from_slice(b"0"));
    assert_eq!(b.len_now(), 8, "appends after overflow are no-ops");
}

/// The spec's own pin (§V2-F07 tests: `TTL expiry returns Gone`).
#[test]
fn ttl_expiry_returns_gone() {
    let store = Arc::new(StreamStore::new(
        Duration::from_millis(10),
        1 << 20,
        1 << 30,
    ));
    let b = store.begin("s1".into());
    store.append(&b, Bytes::from_static(b"data"));
    assert!(matches!(store.get("s1"), Reattach::Ok(_)), "fresh is Ok");
    std::thread::sleep(Duration::from_millis(25));
    assert!(matches!(store.get("s1"), Reattach::Gone), "expired is Gone");
}

#[test]
fn sweep_evicts_expired_buffers_and_keeps_live_ones() {
    let store = Arc::new(StreamStore::new(
        Duration::from_millis(10),
        1 << 20,
        1 << 30,
    ));
    let old = store.begin("old".into());
    store.append(&old, Bytes::from_static(b"old"));
    std::thread::sleep(Duration::from_millis(25));
    let fresh = store.begin("fresh".into());
    store.append(&fresh, Bytes::from_static(b"fresh"));
    store.sweep();
    assert!(matches!(store.get("old"), Reattach::Gone), "expired swept");
    assert!(
        matches!(store.get("fresh"), Reattach::Ok(_)),
        "live buffer survives the sweep"
    );
}

/// The spec's own pin (§V2-F07 wiring: "total accounting evicts oldest
/// completed buffers first").
#[test]
fn total_accounting_evicts_the_oldest_completed_buffer_first() {
    let store = Arc::new(StreamStore::new(Duration::from_secs(60), 1 << 20, 100));
    let first = store.begin("first".into());
    store.append(&first, Bytes::copy_from_slice(&[0u8; 60]));
    store.finish(&first);
    let second = store.begin("second".into());
    store.append(&second, Bytes::copy_from_slice(&[0u8; 60]));
    assert_eq!(store.total_bytes(), 60, "the oldest COMPLETED buffer paid");
    assert!(
        matches!(store.get("first"), Reattach::Gone),
        "the completed buffer was evicted"
    );
    assert!(matches!(store.get("second"), Reattach::Ok(_)));
}

#[test]
fn total_accounting_evicts_the_oldest_active_buffer_when_none_completed() {
    // A hard memory bound: with nothing completed, the oldest buffer goes
    // regardless of liveness — the store never holds more than max_total.
    let store = Arc::new(StreamStore::new(Duration::from_secs(60), 1 << 20, 100));
    let first = store.begin("first".into());
    store.append(&first, Bytes::copy_from_slice(&[0u8; 60]));
    let second = store.begin("second".into());
    store.append(&second, Bytes::copy_from_slice(&[0u8; 60]));
    assert!(store.total_bytes() <= 100, "the bound is hard");
    assert!(
        matches!(store.get("first"), Reattach::Gone),
        "the oldest (active) buffer was evicted"
    );
    assert!(matches!(store.get("second"), Reattach::Ok(_)));
}

// ----------------------------------------------------------------- the tee

#[test]
fn tee_stream_forwards_chunks_and_feeds_the_buffer() {
    let store = Arc::new(StreamStore::new(Duration::from_secs(60), 1 << 20, 1 << 30));
    let buf = store.begin("s1".into());
    let inner = futures::stream::iter(vec![
        Ok::<Bytes, std::io::Error>(Bytes::from_static(b"aaa")),
        Ok(Bytes::from_static(b"bb")),
    ]);
    let tee = tee_stream(&store, &buf, Box::pin(inner));
    let got: Vec<Bytes> = futures::executor::block_on(tee.collect::<Vec<_>>())
        .into_iter()
        .map(|r| r.expect("no io error"))
        .collect();
    assert_eq!(got.len(), 2, "the consumer sees every chunk");
    assert_eq!(&got[0][..], b"aaa");
    assert_eq!(&got[1][..], b"bb");
    assert!(buf.is_done(), "the natural end marks the buffer done");
    assert_eq!(
        buf.len_now(),
        5,
        "every forwarded byte landed in the buffer"
    );
    assert_eq!(
        read_from(&buf, 0).expect("replayable").as_ref(),
        b"aaabb",
        "the buffer replays exactly what the consumer saw"
    );
}

#[test]
fn tee_stream_propagates_errors_and_marks_done() {
    let store = Arc::new(StreamStore::new(Duration::from_secs(60), 1 << 20, 1 << 30));
    let buf = store.begin("s1".into());
    let inner = futures::stream::iter(vec![
        Ok::<Bytes, std::io::Error>(Bytes::from_static(b"partial")),
        Err(std::io::Error::other("boom")),
    ]);
    let tee = tee_stream(&store, &buf, Box::pin(inner));
    let got: Vec<Result<Bytes, std::io::Error>> = futures::executor::block_on(tee.collect());
    assert_eq!(got.len(), 2);
    assert!(got[0].is_ok());
    assert!(got[1].is_err(), "the producer's error reaches the consumer");
    assert!(buf.is_done(), "an erroring producer marks the buffer done");
    assert_eq!(
        buf.len_now(),
        7,
        "the bytes before the error stay replayable"
    );
}

// ------------------------------------------------------------------- Range

#[test]
fn parse_range_bytes_n_dash() {
    // Absent header: the spec's default 0.
    assert_eq!(parse_range_offset(&[]), Some(0));
    let h = |v: &str| vec![("Range".to_string(), v.to_string())];
    assert_eq!(parse_range_offset(&h("bytes=0-")), Some(0));
    assert_eq!(parse_range_offset(&h("bytes=123-")), Some(123));
    assert_eq!(parse_range_offset(&h("bytes=0007-")), Some(7));
    // The endpoint only serves open single ranges: suffix, bounded and
    // multi ranges are refused (the caller answers 410).
    assert_eq!(parse_range_offset(&h("bytes=-5")), None);
    assert_eq!(parse_range_offset(&h("bytes=0-99")), None);
    assert_eq!(parse_range_offset(&h("bytes=0-1,5-")), None);
    assert_eq!(parse_range_offset(&h("items=3-")), None);
    assert_eq!(parse_range_offset(&h("bytes=")), None);
}

// --------------------------------------------------------------- responses

#[test]
fn resume_response_206_head() {
    let (_s, b) = seeded(&[&b"0123456789"[..]]);
    let resp = stream_resume_response(&b, 4);
    assert_eq!(resp.status, 206);
    assert_eq!(
        resp.header("content-range"),
        Some("bytes 4-*/*"),
        "the spec's open-ended content-range spelling"
    );
    assert!(resp.stream.is_some(), "the body is a stream");
    assert!(
        resp.body.is_empty(),
        "streaming responses carry no collected body"
    );
}

#[test]
fn resume_response_complete_is_200_with_the_complete_marker() {
    let (store, b) = seeded(&[&b"all"[..]]);
    store.finish(&b);
    // A client that saw everything: 200, empty body, the complete marker.
    let resp = stream_resume_response(&b, 3);
    assert_eq!(resp.status, 200);
    assert_eq!(resp.header("x-oxide-stream-complete"), Some("1"));
    assert!(resp.body.is_empty());
    assert!(resp.stream.is_none(), "nothing to stream");
    // A client that missed the tail still gets a 206 with the backlog.
    let resp = stream_resume_response(&b, 1);
    assert_eq!(resp.status, 206);
}

#[test]
fn resume_response_offset_past_len_is_410() {
    let (_s, live) = seeded(&[&b"01"[..]]);
    let resp = stream_resume_response(&live, 3);
    assert_eq!(
        resp.status, 410,
        "in-flight: an offset past len skipped bytes"
    );
    let gone = stream_gone_response();
    assert_eq!(gone.status, 410);
    assert!(gone.body.is_empty());
}

// ---------------------------------------------------- reattach body streams

/// The reattach body: the backlog first, then the live tail until done.
#[tokio::test]
async fn reattach_stream_emits_backlog_then_live_tail_until_done() {
    let store = Arc::new(StreamStore::new(Duration::from_secs(60), 1 << 20, 1 << 30));
    let buf = store.begin("s1".into());
    store.append(&buf, Bytes::from_static(b"one"));
    store.append(&buf, Bytes::from_static(b"two"));

    let resp = stream_resume_response(&buf, 3); // the client saw "one"
    assert_eq!(resp.status, 206);
    let mut body = resp.stream.expect("a streaming body");

    // Backlog: everything the client missed so far, one reattach read.
    let backlog = body.next().await.expect("the backlog arrives").expect("ok");
    assert_eq!(backlog.as_ref(), b"two");

    // Live tail: a chunk appended after the reattach lands on the open
    // stream, and the finish ends it.
    store.append(&buf, Bytes::from_static(b"three"));
    let tail = body
        .next()
        .await
        .expect("the live tail arrives")
        .expect("ok");
    assert_eq!(tail.as_ref(), b"three");
    store.finish(&buf);
    assert!(body.next().await.is_none(), "done ends the stream");
}

/// The spec's own pin (§V2-F07 tests: `concurrent append and read`): a
/// reader that reattaches mid-production consumes the byte-exact full
/// stream while the producer is still appending.
#[tokio::test]
async fn concurrent_append_and_read_reassembles_the_full_stream() {
    let store = Arc::new(StreamStore::new(Duration::from_secs(60), 1 << 20, 1 << 30));
    let buf = store.begin("s1".into());

    // The producer: 20 numbered lines appended while the reader attaches.
    let producer_store = Arc::clone(&store);
    let producer_buf = Arc::clone(&buf);
    let producer = tokio::spawn(async move {
        for i in 0..20u64 {
            producer_store.append(&producer_buf, Bytes::from(format!("line-{i:02}\n")));
            tokio::task::yield_now().await;
        }
        producer_store.finish(&producer_buf);
    });

    // The reader attaches immediately: it saw nothing yet, so it must
    // receive every byte, in order.
    let resp = stream_resume_response(&buf, 0);
    let mut body = resp.stream.expect("a streaming body");
    let mut got: Vec<u8> = Vec::new();
    while let Some(item) = body.next().await {
        got.extend_from_slice(&item.expect("no io error"));
    }
    producer.await.expect("the producer finished");

    let want: Vec<u8> = (0..20u64)
        .flat_map(|i| format!("line-{i:02}\n").into_bytes())
        .collect();
    assert_eq!(got, want, "byte-exact reassembly across the live tail");
}
