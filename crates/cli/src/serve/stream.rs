//! Moved verbatim from lib.rs by scripts/split_cli.py (V2-A01).

use crate::*;

impl oxide_fw_server::InspectorSnapshot for CliInspector {
    fn state_json(&self) -> serde_json::Value {
        serde_json::to_value(&self.state).unwrap_or(serde_json::Value::Null)
    }
    fn render_html(&self) -> String {
        oxide_fw_dev::render_inspector_html(&self.state)
    }
}

/// Construct the production pipeline (§12.1) from config and start serving.
/// The dispatch returns a placeholder page from the router match — the JS host wiring
/// (NodeHost/isolate render) lives in oxide-fw-jshost and plugs into this dispatch later.
/// Production server entry (§12.1): no inspector is attached.
pub async fn start_server(
    root: &str,
    port: Option<u16>,
    stack: crate::framework_shim::FrameworkStack,
) -> anyhow::Result<StartedServer> {
    let bind = port.map(|p| (std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), p));
    start_server_inner_full(root, bind, false, None, Vec::new(), stack, None).await
}

/// Dev server entry (§13): identical pipeline but with the `/__oxide`
/// inspector attached, populated with the route table.
/// Full dev-server starter with optional bind IP + TLS.
/// Dispatch delegate: routing, proxy (§13.5), and — when a `page_renderer` is
/// attached (§7) — framework SSR. Extracted from `start_dev_server_full` so
/// tests can drive it with a fake renderer, and so the framework-adapter
/// wiring has one integration point instead of nesting inside the server
/// startup function.
/// Bridge an `oxide_fw_server::RenderStream` (a `BoxStream` that promises
/// only `Send`) into an `oxide_fw_server::BodyStream` (which promises
/// `Send + Sync` because `BodyExt::boxed` demands it).
///
/// The bridge is an mpsc channel: a spawned task pumps each source chunk
/// into the channel; the channel's `Receiver` is `Sync` (its `recv` takes
/// `&self`), so the resulting body stream satisfies the hyper transport's
/// bound. The alternative — an `UnsyncBoxBody` — would ripple through
/// every serve signature in `server`, `dev`, and `cli`.
///
/// Error conversion: `OxideError` → `std::io::Error`. The code and message
/// are preserved in the io error's text so a hyper-layer log line names
/// the framework error. The kind is `Other` — there is no precise
/// `ErrorKind` for "the worker crashed mid-render".
/// SPEC §serve/stream: `render_stream_to_body_stream` is the no-recovery
/// shim over `render_stream_with_recovery` for callers that stream a
/// rendered document without a boundary or nonce context. An error from
/// the source aborts the stream cleanly (no mid-stream `ox-error`
/// recovery boundary) — exactly what `render_stream_with_recovery(..,
/// None, None, ..)` provides, exposed as its own name because it is the
/// SPEC's documented `serve/stream` entry point. The CLI itself uses the
/// recovery form for page renders; this function remains public so a
/// downstream crate depending on `oxide-fw-cli` as a library can stream a
/// document with abort-on-error semantics without knowing the recovery
/// variant's argument shape.
pub fn render_stream_to_body_stream(
    src: oxide_fw_server::RenderStream,
) -> oxide_fw_server::BodyStream {
    render_stream_with_recovery(src, None, None, String::new())
}

/// SPEC-11 / §10.2.5: the streaming bridge with mid-stream recovery.
///
/// If the shell already committed (we have sent ≥1 chunk) and the render
/// errors, emitting nothing more would truncate the response — the browser
/// shows a half page. Instead we append the `ox-error` recovery boundary (a
/// `data-ox-c` template + self-contained swapper that replaces the matching
/// `data-ox-b` placeholder), then close cleanly. The status stays 200: the
/// shell is valid; only a boundary failed, and the client recovers it.
///
/// `boundary_id`/`nonce` are `Some` for a streamed page; `None` (the
/// `render_stream_to_body_stream` shim) keeps the old abort behaviour for
/// callers that have no boundary context.
pub(crate) fn render_stream_with_recovery(
    src: oxide_fw_server::RenderStream,
    boundary_id: Option<String>,
    nonce: Option<String>,
    route_id: String,
) -> oxide_fw_server::BodyStream {
    let (tx, mut rx) =
        tokio::sync::mpsc::channel::<std::result::Result<bytes::Bytes, std::io::Error>>(8);
    tokio::spawn(async move {
        use futures::StreamExt as _;
        let mut src = src;
        let mut sent_any = false;
        while let Some(item) = src.next().await {
            match item {
                Ok(chunk) => {
                    sent_any = true;
                    if tx.send(Ok(chunk)).await.is_err() {
                        // Receiver dropped — client disconnected.
                        return;
                    }
                }
                Err(e) => {
                    if let (true, Some(id), Some(n)) =
                        (sent_any, boundary_id.as_deref(), nonce.as_deref())
                    {
                        // Shell committed: emit the recovery boundary, then
                        // close cleanly (no io::Error → no connection reset).
                        let html = oxide_fw_render::stream_error_recovery(id, &e.0, n);
                        let _ = tx.send(Ok(bytes::Bytes::from(html))).await;
                        return;
                    }
                    // No shell yet (or no boundary context): the old abort.
                    let _ = tx
                        .send(Err(std::io::Error::other(format!(
                            "{}: {}",
                            e.0.code, e.0.message
                        ))))
                        .await;
                    return;
                }
            }
        }
        let _ = route_id;
        // `tx` drops here, closing the stream cleanly.
    });
    Box::pin(futures::stream::poll_fn(move |cx| rx.poll_recv(cx)))
}

/// §10.2 step 5 (error-status contract): peek the render stream's FIRST
/// chunk and scan it for an error-status marker. Returns
/// `(500 | 0, reassembled stream)` — the stream is byte-identical to the
/// input (the peeked chunk is chained back in front).
///
/// Why only the first chunk: the response status is fixed before the body
/// streams; HTTP cannot retroactively change a 200 to a 500 once bytes are
/// on the wire. The adapter therefore announces an error render in the
/// document's FIRST chunk:
///   * `<!--oxide-status:500-->` — the canonical contract marker the
///     adapter emits at the top of an error document (both the project's
///     `error.tsx` render and the generic OX-500 fallback);
///   * `data-ox-error='OX-500'` — the generic fallback boundary the adapter
///     already emits (observable when the whole document lands in one
///     chunk, e.g. the buffered `render_streaming` default).
///
/// TTFB cost: one first-chunk await — the shell chunk real adapters yield
/// before awaiting the body stream, so the shell flush timing is unchanged.
pub(crate) fn probe_render_error_status(
    mut src: oxide_fw_server::RenderStream,
) -> impl std::future::Future<Output = (u16, oxide_fw_server::RenderStream)> + Send {
    use futures::StreamExt as _;
    async move {
        match src.next().await {
            Some(Ok(first)) => {
                let status = if chunk_signals_error(&first) { 500 } else { 0 };
                let reassembled = futures::stream::once(async move { Ok(first) })
                    .chain(src)
                    .boxed();
                (status, reassembled)
            }
            Some(Err(e)) => {
                let reassembled = futures::stream::once(async move { Err(e) })
                    .chain(src)
                    .boxed();
                (0, reassembled)
            }
            None => (0, src),
        }
    }
}

/// The error markers `probe_render_error_status` scans the first chunk for.
pub(crate) fn chunk_signals_error(chunk: &bytes::Bytes) -> bool {
    const CONTRACT_MARKER: &[u8] = b"<!--oxide-status:500-->";
    const GENERIC_MARKER: &[u8] = b"data-ox-error='OX-500'";
    fn has(hay: &[u8], needle: &[u8]) -> bool {
        hay.windows(needle.len()).any(|w| w == needle)
    }
    has(chunk, CONTRACT_MARKER) || has(chunk, GENERIC_MARKER)
}

/// Same contract as [`chunk_signals_error`], for the buffered (prerender)
/// path where the adapter's output is a `String` rather than a stream of
/// `Bytes`.
pub(crate) fn str_signals_error(s: &str) -> bool {
    chunk_signals_error(&bytes::Bytes::copy_from_slice(s.as_bytes()))
}

/// §10.3: splice `<script type="module" src="/<url>">` into a streaming HTML
/// response right before the closing `</body>` tag. Works when `</body>` is
/// split across chunk boundaries by keeping a rolling tail of the last
/// `CLOSE.len() - 1` bytes and scanning for the tag on each new chunk.
///
/// §9.3: parse an `application/x-www-form-urlencoded` body into the
/// `[key, value]` entry list the worker's `action:run` op accepts. Percent
/// decoding handles `+` as space. A malformed pair is skipped, not fatal.
pub(crate) fn parse_form_urlencoded(body: &[u8]) -> Vec<(String, String)> {
    let s = String::from_utf8_lossy(body);
    s.split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (
                oxide_fw_server::percent_decode(&k.replace('+', " ")),
                oxide_fw_server::percent_decode(&v.replace('+', " ")),
            )
        })
        .collect()
}

/// §9.3: the action name from a form-action query. `?/comment` → `comment`.
/// The SvelteKit convention: a query that begins with `/` names an action.
pub(crate) fn action_from_query(url: &str) -> Option<String> {
    let q = url.split_once('?')?.1;
    let q = q.split('#').next().unwrap_or(q);
    let rest = q.strip_prefix('/')?;
    if rest.is_empty() || rest.contains('/') {
        return None;
    }
    Some(oxide_fw_server::percent_decode(rest))
}
