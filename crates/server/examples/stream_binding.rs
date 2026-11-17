//! Walkthrough: the §13.4 stream↔session binding — the consumer the
//! resumable-streams seam never had for its security half.
//!
//! The R05 review (`docs/security-review-v0.2.md`) recorded the gap: the
//! stream id is a 256-bit digest disclosed only in the creating response's
//! head, but anyone who LEARNS an id could reattach another user's stream
//! — the spec's own table row (`bind id to the original request's session
//! cookie hash; mismatch gives 410`) was not implemented. The binding now
//! rides the store: the tee site mints the buffer WITH the original
//! request's hashed session cookie (`CookieBindingSource`, keyed per boot),
//! and the reattach arm demands the same identity back — a foreign or
//! missing cookie is the SAME `410` an unknown id gets, so a refusal never
//! confirms the id exists.
//!
//! Run: `cargo run -p oxide-fw-server --example stream_binding`

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use oxide_fw_server::stream_store::{
    CookieBindingSource, Reattach, SessionBindingSource, StreamStore, read_from,
};

fn main() {
    // 1. Boot: ONE binding source, shared by the tee site and the reattach
    //    arm — the two sides of one stream must hash identically. The key
    //    is random per boot, so bindings never validate across restarts
    //    (the in-memory store dies with the process anyway).
    let src = Arc::new(CookieBindingSource::new("oxide_session").expect("boot rng"));
    println!("boot: CookieBindingSource over 'oxide_session' (per-boot keyed blake3)");

    let store = Arc::new(StreamStore::new(
        Duration::from_secs(3600),
        1 << 20,
        1 << 30,
    ));

    // 2. The producer's request carries the session cookie; the tee site
    //    mints the id and opens the buffer BOUND to that identity.
    let producer_headers = vec![(
        "cookie".to_string(),
        "theme=dark; oxide_session=session-alice-token".to_string(),
    )];
    let binding = src.binding(&producer_headers).expect("the session binds");
    let buf = store.begin_bound(
        oxide_fw_server::stream_store::stream_id("req-42", 0),
        Some(binding),
    );
    println!(
        "tee: stream {} minted, session-bound ({})",
        buf.id(),
        buf.is_session_bound(),
    );

    // 3. The response streams; the tee appends. The consumer drops after
    //    the first chunk — the backlog is what a reattach replays.
    store.append(&buf, Bytes::from_static(b"chunk-0\n"));
    store.append(&buf, Bytes::from_static(b"chunk-1\n"));
    println!("producer: 14 bytes buffered so far");

    // 4. Reattach as the SAME session: the identity matches, the backlog
    //    replays from the offset the client presents.
    let same = store.get_bound(buf.id(), src.binding(&producer_headers).as_ref());
    match same {
        Reattach::Ok(b) => println!(
            "reattach (same session): 206, backlog from byte 8 = {:?}",
            read_from(&b, 8).expect("in-range offset"),
        ),
        Reattach::Gone => unreachable!("the minting session always reattaches"),
    }

    // 5. Reattach with a FOREIGN session cookie: the constant-time compare
    //    fails and the answer is the same `Reattach::Gone` (the endpoint's
    //    `410` with an empty body) an unknown id gets. One loud warn names
    //    the refusal — the cookie value and the digest never appear.
    let foreign_headers = vec![(
        "cookie".to_string(),
        "oxide_session=session-mallory-token".to_string(),
    )];
    let foreign = store.get_bound(buf.id(), src.binding(&foreign_headers).as_ref());
    match foreign {
        Reattach::Gone => println!("reattach (foreign session): Gone → 410 (warn logged)"),
        Reattach::Ok(_) => unreachable!("a foreign session must never reattach"),
    }

    // 6. Reattach with NO cookie at all: a bound stream refuses the
    //    identity-less caller too — same `410`, no information leak.
    let cookieless = store.get_bound(buf.id(), None);
    match cookieless {
        Reattach::Gone => println!("reattach (no cookie): Gone → 410 (warn logged)"),
        Reattach::Ok(_) => unreachable!("a bound stream demands the identity back"),
    }

    // 7. The unbound path is unchanged: `begin` (no binding) keeps today's
    //    any-caller reattach — single-user deployments ride it untouched.
    let legacy = store.begin("req-43-stream-0".to_string());
    store.append(&legacy, Bytes::from_static(b"public\n"));
    assert!(matches!(
        store.get_bound(legacy.id(), None),
        Reattach::Ok(_)
    ));
    println!("legacy: an unbound stream still reattaches without any identity");
}
