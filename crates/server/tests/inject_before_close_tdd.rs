//! `inject_before_close` — the chunked-body stream transform that splices a
//! tag before the first occurrence of a closing marker. Used by the CLI to
//! put the island boot `<script>` before `</body>` in a live SSR response.
//!
//! The interesting cases are all about the marker landing on a chunk
//! boundary: the transform keeps a rolling tail of `marker.len() - 1` bytes
//! so a `</body>` split across two chunks still matches.

use bytes::Bytes;
use futures::stream;
use oxide_server::{inject_before_close, BodyStream};

fn stream_of(chunks: Vec<&'static [u8]>) -> BodyStream {
    let owned: Vec<Result<Bytes, std::io::Error>> =
        chunks.into_iter().map(|c| Ok(Bytes::from_static(c))).collect();
    Box::pin(stream::iter(owned))
}

async fn collect(mut s: BodyStream) -> Vec<u8> {
    use futures::StreamExt as _;
    let mut out = Vec::new();
    while let Some(item) = s.next().await {
        out.extend_from_slice(&item.expect("no error"));
    }
    out
}

const TAG: &[u8] = br#"<script type="module" src="/client/__boot__.js"></script>"#;
const CLOSE: &[u8] = b"</body>";

#[tokio::test]
async fn inserts_before_close_in_a_single_chunk() {
    let s = stream_of(vec![b"<html><body>hi</body></html>"]);
    let out = collect(inject_before_close(s, CLOSE, TAG)).await;
    assert_eq!(
        out,
        br#"<html><body>hi<script type="module" src="/client/__boot__.js"></script></body></html>"#,
    );
}

#[tokio::test]
async fn inserts_before_close_split_across_chunks() {
    // Each case is a different way to chunk up `PREFIX + </body> + SUFFIX`.
    // The chunk boundaries stress the sliding window at every internal
    // position of the closing marker.
    //
    // The invariant we assert: `concat(chunks) with TAG inserted immediately
    // before the first `</body>``. Parameterize over the input rather than
    // over a fixed literal so a case that omits the `hi` prefix (the
    // pathological `[a, b, c, </body>, d, e]` case) doesn't produce a false
    // negative.
    let cases: Vec<Vec<&'static [u8]>> = vec![
        vec![b"<html><body>hi</bo", b"dy></html>"],
        vec![b"<html><body>hi</b", b"ody></html>"],
        vec![b"<html><body>hi<", b"/body></html>"],
        vec![b"<html><body>hi", b"</body></html>"],
        vec![b"<html><body>hi</body", b"></html>"],
        vec![b"a", b"b", b"c", b"</body>", b"d", b"e"],
        vec![b"before</body>after"],
    ];
    for case in cases {
        let joined: Vec<u8> = case.iter().flat_map(|c| c.iter().copied()).collect();
        let close_idx = joined
            .windows(CLOSE.len())
            .position(|w| w == CLOSE)
            .expect("each case has a `</body>`");

        let mut expected = Vec::with_capacity(joined.len() + TAG.len());
        expected.extend_from_slice(&joined[..close_idx]);
        expected.extend_from_slice(TAG);
        expected.extend_from_slice(&joined[close_idx..]);

        let s = stream_of(case.clone());
        let out = collect(inject_before_close(s, CLOSE, TAG)).await;
        assert_eq!(
            out,
            expected,
            "case {case:?} mismatched; got `{}`",
            String::from_utf8_lossy(&out),
        );
    }
}

#[tokio::test]
async fn flushes_tail_and_appends_when_close_never_arrives() {
    // Malformed HTML — no `</body>`. The tag is still appended so the boot
    // script loads; the response is already invalid markup at that point.
    let s = stream_of(vec![b"<html><body>hi"]);
    let out = collect(inject_before_close(s, CLOSE, TAG)).await;
    assert_eq!(
        out,
        br#"<html><body>hi<script type="module" src="/client/__boot__.js"></script>"#,
    );
}

#[tokio::test]
async fn injects_only_before_the_first_close() {
    // Two `</body>` markers: the tag goes before the first, the second is
    // untouched. (Real HTML has one; malformed docs should not be double-
    // injected.)
    let s = stream_of(vec![b"</body><p>mid</p></body>"]);
    let out = collect(inject_before_close(s, CLOSE, TAG)).await;
    let html = String::from_utf8(out).unwrap();
    assert_eq!(html.matches(r#"<script type="module""#).count(), 1);
    assert!(html.starts_with(r#"<script type="module""#));
}

#[tokio::test]
async fn empty_needle_is_a_noop() {
    let s = stream_of(vec![b"<html><body>hi</body></html>"]);
    let out = collect(inject_before_close(s, b"", TAG)).await;
    assert_eq!(out, b"<html><body>hi</body></html>");
}

#[tokio::test]
async fn passes_errors_through() {
    // The transform must propagate a stream error unchanged (mid-response
    // failure aborts the HTTP connection; it must not be silently swallowed).
    let chunks: Vec<Result<Bytes, std::io::Error>> = vec![
        Ok(Bytes::from_static(b"<html><body>hi")),
        Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "client gone")),
    ];
    let s: BodyStream = Box::pin(stream::iter(chunks));
    let mut s = inject_before_close(s, CLOSE, TAG);
    use futures::StreamExt as _;
    let first = s.next().await.expect("first item");
    assert!(first.is_ok());
    let second = s.next().await.expect("second item");
    assert!(second.is_err(), "error must propagate");
}
