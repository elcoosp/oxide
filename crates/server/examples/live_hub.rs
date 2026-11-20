//! Consumer walkthrough for the live-loaders substrate (V2-F05 steps 3-5,
//! the `Hub as LiveHub` / `Runner as LiveRunner` re-exports): the pieces a
//! host composes to push fresh loader data to open tabs with no
//! hand-written socket code.
//!
//! The endpoint itself (`GET /__oxide/live`) is the CLI's dispatch arm;
//! this walkthrough shows the crate-owned story end to end:
//!
//! 1. [`oxide_fw_server::live::Hub::new`] — the capacity the endpoint
//!    turns into `503` + `retry-after` (global and per-peer).
//! 2. [`oxide_fw_server::live::Sub::new`] + `subscribe` — one open
//!    connection: route id, URL, the tags it depends on, the `watch`
//!    channel the latest snapshot lands in; the [`oxide_fw_server::live::Guard`]
//!    unsubscribes on drop.
//! 3. `affected(tag)` — the fan-out: which open connections depend on a
//!    tag (exact tags plus `prefix:*` wildcards) — the question a
//!    revalidation asks.
//! 4. The snapshot push: `sub.tx.send` — latest wins; slow clients skip
//!    intermediate states.
//! 5. [`oxide_fw_server::live::live_stream`] — the response body: the
//!    current snapshot first (a fresh connection never needs a replay),
//!    then one `data` frame per new snapshot.
//! 6. The tag index emptied by the `Guard`'s drop.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-server --example live_hub
//! ```

use bytes::Bytes;
use oxide_fw_server::live::{Hub, Sub, live_stream};
use std::sync::Arc;
use std::time::Duration;
use tokio_stream::StreamExt;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    rt.block_on(async {
        // 1. The hub: the capacity contract the endpoint answers 503 on.
        let hub = Hub::new(64, 8);

        // 2. One open tab depends on the `content:blog` tag.
        let sub = Arc::new(Sub::new(
            "routes/blog/[slug]",
            "/blog/hello",
            Vec::new(),
            ["content:blog".to_string()],
            None,
        ));
        let guard = hub
            .subscribe(Arc::clone(&sub))
            .expect("the hub admits the tab");

        // 3. The revalidation's fan-out question: who depends on this tag?
        let affected = hub.affected("content:blog");
        println!(
            "affected(\"content:blog\") -> {} connection(s): {}",
            affected.len(),
            affected
                .iter()
                .map(|s| s.route.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );

        // 3b. The wildcard rides the SUBSCRIBER's side: a tab that depends
        //     on `images:*` is affected by ANY `images:<something>`
        //     revalidation — the query itself is always an exact tag.
        let wildcard_sub = Arc::new(Sub::new(
            "routes/gallery",
            "/gallery",
            Vec::new(),
            ["images:*".to_string()],
            None,
        ));
        let _wildcard_guard = hub
            .subscribe(Arc::clone(&wildcard_sub))
            .expect("the hub admits the gallery tab");
        println!(
            "affected(\"images:hero\") -> {} connection(s) (the tab declared images:*)",
            hub.affected("images:hero").len()
        );

        // 4. The rerun worker's push: the latest snapshot lands in the
        //    subscriber's channel (latest wins).
        sub.tx
            .send(Some(Bytes::from_static(br#"{"title":"hello"}"#)))
            .expect("the primary receiver is alive");

        // 5. The response body: the current snapshot first — a fresh
        //    connection never needs a replay — then one frame per push.
        let mut body = live_stream(sub.tx.subscribe(), Duration::from_secs(15));
        let first = body.next().await;
        match first {
            Some(Ok(bytes)) => println!(
                "the fresh connection reads: {}",
                String::from_utf8_lossy(&bytes).trim()
            ),
            other => println!("the stream answered: {:?}", other.map(|r| r.is_err())),
        }

        // 6. The tab closes: the Guard's drop unsubscribes, the tag index
        //    empties.
        drop(guard);
        println!(
            "after the tab closes: affected(\"content:blog\") -> {} connection(s)",
            hub.affected("content:blog").len()
        );
    });
    Ok(())
}
