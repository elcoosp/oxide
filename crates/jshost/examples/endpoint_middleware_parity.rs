//! V2-A08: the §8.4 endpoint + §8.1 middleware dispatch on the V8 backend,
//! walked (`cargo run -p oxide-fw-jshost --example endpoint_middleware_parity
//! --features v8`).
//!
//! Before the parity arms existed, `endpoint:run` and `middleware:run` on
//! `server.backend = "v8"` fell through to the generic `__oxideDispatch`
//! arm, which handed the raw payload to a handler expecting `(request, ctx)`
//! — a Web `Response` could not even be constructed ("Response is not
//! defined"). The parity work lands three pieces:
//!
//! 1. `init/web_polyfill.js` — the `Headers` / `Request` / `Response`
//!    globals the fetch-dialect handlers speak (the tier-1 web-globals
//!    pattern beside the URL polyfill).
//! 2. `__oxideEndpointRun` / `__oxideMiddlewareRun` bootstrap entry points —
//!    the Node worker's `dispatchEndpointRun` / `dispatchMiddlewareRun`
//!    semantics ported verbatim (method handler resolution, the `(request,
//!    ctx)` call convention, the §8.4 `Response` projection, the §8.1
//!    directive envelope).
//! 3. `v8_dispatch` arms routing both kinds to those entry points, with a
//!    streamed `Response` body riding the stream sink (header frame +
//!    chunks, the Node protocol).
//!
//! The both-hosts proof is the shared test table (`tests/op_parity.rs`,
//! `cargo test -p oxide-fw-jshost --features v8 --test op_parity --
//! --ignored`): the same fixture inputs go to the Node worker and the V8
//! isolate and the normalized envelopes must agree. This example drives the
//! V8 half alone so the wire shapes are visible without a Node child.

#![cfg(feature = "v8")]

use oxide_fw_jshost::{CallInput, CallStream, EntryRef, JsHost, PoolConfig, V8Host};

fn call_input(payload: serde_json::Value) -> CallInput {
    CallInput {
        payload,
        headers: Default::default(),
        has_body: false,
        request_id: "ex-a08".to_string(),
        route: None,
    }
}

/// The §8.4 payload shape `EndpointHost::run` builds (`module_path` is the
/// EntryRef's module; the V8 arm imports it by `file://` URL).
fn endpoint_payload(method: &str, url: &str, body_b64: &str) -> serde_json::Value {
    serde_json::json!({
        "method": method,
        "url": url,
        "params": {},
        "headers": [["accept", "application/json"]],
        "body_b64": body_b64,
        "locals": {},
    })
}

/// The §8.1 payload shape `MiddlewareHost::run` builds.
fn middleware_payload(locals: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "method": "GET",
        "url": "/admin",
        "params": {},
        "headers": [],
        "body_b64": "",
        "locals": locals,
    })
}

fn main() {
    println!("== V2-A08: endpoint + middleware dispatch on the V8 backend ==");
    println!();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio rt");
    rt.block_on(async {
        // (1) Two fixture modules in a temp dir — the shapes a real project
        // ships as `route.ts` / `middleware.ts` (here plain `.mjs`, so the
        // walk needs no TS transform).
        let dir = tempfile::tempdir().expect("tempdir");
        let endpoint = dir.path().join("users.route.mjs");
        std::fs::write(
            &endpoint,
            r#"
export async function GET(request) {
  const url = new URL(request.url);
  return new Response(JSON.stringify({ hello: url.searchParams.get('name') ?? 'world' }), {
    status: 200,
    headers: { 'content-type': 'application/json' },
  });
}
"#,
        )
        .expect("write endpoint fixture");
        let middleware = dir.path().join("admin.middleware.mjs");
        std::fs::write(
            &middleware,
            r#"
export function middleware(request, ctx) {
  if (ctx.locals.blocked) return { redirect: '/login', status: 307 };
  return { locals: { ...ctx.locals, user: 'ada' } };
}
"#,
        )
        .expect("write middleware fixture");
        let module = endpoint.to_string_lossy().to_string();
        let mw_module = middleware.to_string_lossy().to_string();

        // (2) The V8 render pool — on `server.backend = "v8"` the pipeline's
        // `EndpointHost` / `MiddlewareHost` dispatch into exactly this pool.
        let pool = V8Host::new()
            .render_pool(PoolConfig {
                size: 1,
                ..Default::default()
            })
            .expect("v8 pool");

        // (3) `endpoint:run` GET. The bootstrap resolves the method handler,
        // builds the Web `Request` from the payload, and projects the
        // `Response` — a streamed body arrives as a header frame followed by
        // raw chunks; a buffered one as the same envelope with `streaming`
        // telling the transport apart.
        println!("(3) endpoint:run GET /api/users?name=v8");
        let mut stream = pool
            .call(
                EntryRef {
                    kind: "endpoint:run".into(),
                    module: module.clone(),
                    export: "GET".into(),
                },
                call_input(endpoint_payload("GET", "/api/users?name=v8", "")),
            )
            .await
            .expect("endpoint call");
        match &mut stream {
            CallStream::Chunked(rx) => {
                let header = rx.recv().await.expect("header frame").expect("header ok");
                let v: serde_json::Value = serde_json::from_slice(&header).expect("header JSON");
                let mut body = Vec::new();
                while let Some(chunk) = rx.recv().await {
                    body.extend_from_slice(&chunk.expect("chunk ok"));
                }
                println!("    status : {}", v["status"]);
                println!("    headers: {}", v["headers"]);
                println!("    body   : {}", String::from_utf8_lossy(&body));
            }
            CallStream::Complete(bytes) => {
                println!("    envelope: {}", String::from_utf8_lossy(bytes));
            }
        }
        println!();

        // (4) `middleware:run`. The directive envelope never streams:
        // `{ locals: {...} }` propagates to the loader; `{ redirect }`
        // short-circuits with the worker's exact envelope shape.
        println!("(4) middleware:run — locals directive, then a redirect");
        for locals in [
            serde_json::json!({}),
            serde_json::json!({ "blocked": true }),
        ] {
            let mut stream = pool
                .call(
                    EntryRef {
                        kind: "middleware:run".into(),
                        module: mw_module.clone(),
                        export: "middleware".into(),
                    },
                    call_input(middleware_payload(locals)),
                )
                .await
                .expect("middleware call");
            let bytes = stream.collect().await.expect("collect");
            println!("    {}", String::from_utf8_lossy(&bytes));
        }
        println!();
        println!("The Node backend answers the same payloads with the same");
        println!("envelopes — the proof is `tests/op_parity.rs` (--ignored).");
    });
}
