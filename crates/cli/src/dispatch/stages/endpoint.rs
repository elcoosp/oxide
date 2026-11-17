//! Moved verbatim from dispatch/entry.rs by scripts/build_stage_split.py
//! (V2-A01 phase 3).
//!
//! stage_endpoint — the §8.1 route.ts endpoint arm (the sketch's
//! /// `stage_endpoint`): the dispatcher call, the F07 resumable tee and
//! /// the F15 recording tee.

use crate::*;
use oxide_fw_dispatch::flow::Flow;

// >>> stage-wrapper (not part of the moved body; split_selfcheck --stages strips this block)
pub(crate) async fn stage_endpoint(d: &DispatchDeps, cx: &mut RequestCtx) -> Flow {
    let req_owned = &cx.req;
    let endpoint_for_dispatch = cx.endpoint_for_dispatch.clone();
    let endpoint_module_path = &cx.endpoint_module_path;
    let endpoint_params_json = &mut cx.endpoint_params_json;
    let endpoint_body = &cx.endpoint_body;
    let params = cx.params.as_ref().expect("the stage order sets params");
    let middleware_locals = &cx.middleware_locals;
    let middleware_headers = cx.middleware_headers.clone();
    let streams_for_dispatch = &d.streams;
    let stream_counter = std::sync::Arc::clone(&d.stream_counter);
    let req_recording = &cx.req_recording;
    let metrics = std::sync::Arc::clone(&d.metrics);
    let label = &cx.label;
    let started = cx.started;
    // <<< stage-wrapper
    // §8.1 route.ts / §10.2 step 7: an Endpoint route returns a
    // Web `Response` directly, bypassing the loader + render
    // phases. The dispatcher is a worker op (`endpoint:run`),
    // async but bounded; the response bytes are the raw body
    // from the `Response` (a `route.ts` can serve a download).
    let endpoint_for_dispatch_inner = endpoint_for_dispatch.clone();
    if let (Some(dispatcher), Some(mp)) = (
        endpoint_for_dispatch_inner.as_deref(),
        endpoint_module_path.as_ref(),
    ) {
        let module_path_s = mp.to_string_lossy().to_string();
        let endpoint_trace = req_owned.trace.as_ref().and_then(|t| {
            t.to_traceparent(&t.parent_id)
                .map(|tp| serde_json::json!({"traceparent": tp, "tracestate": t.tracestate}))
        });
        match dispatcher
            .dispatch(EndpointDispatchArgs {
                module_path: &module_path_s,
                method: &req_owned.method,
                url: &req_owned.url,
                params: endpoint_params_json.get_or_insert_with(|| build_params_json(params)),
                headers: &req_owned.headers,
                body: endpoint_body.as_ref(),
                locals: middleware_locals,
                trace: endpoint_trace.as_ref(),
            })
            .await
        {
            Ok(out) => {
                metrics.observe(label, started.elapsed(), out.status);
                // §9.6: a streaming route.ts (SSE, downloads)
                // drives the response body from `out.stream`;
                // a buffered one uses `out.body`.
                let mut resp = match out.stream {
                    Some(stream) => oxide_fw_server::OxideResponse::stream(out.status, stream),
                    None => oxide_fw_server::OxideResponse::new(
                        out.status,
                        bytes::Bytes::from(out.body),
                    ),
                };
                for (k, v) in &out.headers {
                    resp = resp.with_header(k, v.clone());
                }
                // §8.5: headers the middleware phase added ride the
                // endpoint response (previously accumulated and
                // dropped).
                for (k, v) in &middleware_headers {
                    resp = resp.with_header(k, v.clone());
                }
                // V2-F07 wiring step 1: a streamed endpoint response
                // the handler opted in through the `oxide/server`
                // `resumable` helper (`x-oxide-resumable: 1`) is
                // teed into the stream store — every byte the
                // consumer sees is buffered under a minted
                // `stream_id(request_id, counter)`, the head gains
                // `x-oxide-stream: <id>` (the client reattaches at
                // `GET /__oxide/stream/<id>` with
                // `Range: bytes=<n>-`), and the tee marks the
                // buffer done at the natural end or a producer
                // error. A buffered response, a response without
                // the marker, and a `None` store (the test helper)
                // all keep the pre-F07 byte shapes.
                if let Some(sw) = streams_for_dispatch.as_ref()
                    && resp.stream.is_some()
                    && out.headers.iter().any(|(k, v)| {
                        k.eq_ignore_ascii_case(oxide_fw_server::stream_store::RESUMABLE_HEADER)
                            && v == oxide_fw_server::stream_store::RESUMABLE_VALUE
                    })
                    && let Some(inner) = resp.stream.take()
                {
                    let counter = stream_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let id =
                        oxide_fw_server::stream_store::stream_id(&req_owned.request_id, counter);
                    // §13.4: the buffer mints BOUND to the ORIGINAL
                    // request's session identity (the wiring's one
                    // `CookieBindingSource` over this request's cookie
                    // header) — the reattach arm demands the same
                    // identity back and answers the same `410` an
                    // unknown id gets on a mismatch.
                    let buf = sw
                        .store
                        .begin_bound(id.clone(), sw.binding_of(&req_owned.headers));
                    resp = resp.with_header(oxide_fw_server::stream_store::STREAM_HEADER, id);
                    resp.stream = Some(oxide_fw_server::stream_store::tee_stream(
                        &sw.store, &buf, inner,
                    ));
                }
                // V2-F15: a streamed endpoint response records
                // through the tee (the observer sees the headers;
                // the tee supplies the body when the stream ends).
                // Composition order: the F07 stream tee sits
                // INNERMOST (closest to the producer) so the store
                // and the recorder observe the same bytes in the
                // same order.
                if let Some(rec) = req_recording.as_ref()
                    && let Some(stream) = resp.stream.take()
                {
                    resp.stream = Some(rec.tee(stream, resp.status));
                }
                return Flow::Respond(resp);
            }
            Err(e) => {
                metrics.observe(label, started.elapsed(), e.status);
                return Flow::Respond(oxide_fw_server::OxideResponse::json(
                    e.status,
                    serde_json::json!({"error": {
                    "code": e.code,
                    "message": e.message,
                    "route": label,
                    }}),
                ));
            }
        }
    }
    // >>> stage-wrapper (not part of the moved body; split_selfcheck --stages strips this block)
    Flow::Continue
    // <<< stage-wrapper
}
