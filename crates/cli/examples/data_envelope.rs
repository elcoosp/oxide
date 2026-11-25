//! P0-2 walkthrough: the `/__data` response envelope's bound — the twin of
//! the request path's `limits.max_body` (SRV-413). The route-data envelope
//! materializes every resolved `defer()` value into one payload; the bound
//! refuses loud (the minted `DSP-117`, a coded 500 naming the sizes) both
//! when the encoded envelope passes the cap and when the ODF encoder's
//! §17.1 hardening refuses the payload — the silent empty-200 the old
//! `unwrap_or_default()` fall-through answered is gone.
//!
//! The example drives `route_data_response` directly over a fake
//! [`LoaderRunner`] (no Node, no pnpm, no V8), so the three shapes print
//! side by side: the under-cap default (200 ODF, byte-identical to the
//! pre-P0-2 shape), the over-cap refusal, the unencodable refusal.
//!
//! ```text
//! cargo run -p oxide-fw-cli --no-default-features --example data_envelope
//! ```

use oxide_fw_cli::{RouteDataArgs, route_data_response};
use oxide_fw_server::{LoaderOutput, LoaderRunInput, LoaderRunner, Metrics, OxideRequest};
use std::collections::BTreeMap;
use std::sync::Arc;

/// A loader whose page data is a configurable blob — or a reserved object
/// key the ODF encoder refuses (`ODF-KEY`, §17.1).
struct FixtureLoader {
    bytes: usize,
    reserved: bool,
}

#[async_trait::async_trait]
impl LoaderRunner for FixtureLoader {
    fn name(&self) -> &str {
        "envelope-fixture"
    }
    async fn run(&self, _input: LoaderRunInput) -> oxide_fw_server::Result<LoaderOutput> {
        let data = if self.reserved {
            serde_json::json!({ "__proto__": { "polluted": true } })
        } else {
            serde_json::json!({ "blob": "x".repeat(self.bytes) })
        };
        Ok(LoaderOutput::ok(data))
    }
}

fn data_request() -> OxideRequest {
    OxideRequest {
        method: "GET".into(),
        url: "/__data?route=%2Fblog".into(),
        headers: Arc::new(Vec::new()),
        header_index: Default::default(),
        body: bytes::Bytes::new(),
        request_id: "example-envelope".into(),
        trace: None,
        peer: None,
        rewrite_hops: 0,
        mw_resume_after: None,
    }
}

fn describe(label: &str, resp: &oxide_fw_server::OxideResponse) {
    let content_type = resp
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.as_str())
        .unwrap_or("<none>");
    println!(
        "{label}: status {} content-type {content_type}",
        resp.status
    );
    if resp.status == 200 {
        let json = oxide_fw_odf::decode(&resp.body)
            .map(|v| v.to_json())
            .expect("the under-cap envelope decodes");
        let blob = json
            .get("data")
            .and_then(|d| d.get("blob"))
            .and_then(|v| v.as_str())
            .map(str::len)
            .unwrap_or(0);
        println!(
            "  route {:} — the loader's {blob}-byte blob survives the round trip",
            json["route"]
        );
    } else {
        println!(
            "  {}",
            String::from_utf8_lossy(&resp.body)
                .lines()
                .next()
                .unwrap_or("")
        );
    }
}

fn main() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let params: BTreeMap<String, String> = BTreeMap::new();
    let locals = serde_json::Value::Null;
    let mw_headers: Vec<(String, String)> = Vec::new();
    let default_cap = oxide_fw_server::Limits::default().max_data_envelope;

    let run = |bytes: usize, cap: usize, reserved: bool| -> oxide_fw_server::OxideResponse {
        let loader: Arc<dyn LoaderRunner> = Arc::new(FixtureLoader { bytes, reserved });
        let metrics = Arc::new(Metrics::new());
        rt.block_on(route_data_response(RouteDataArgs {
            live: false,
            req: &data_request(),
            loader_runner: Some(&loader),
            module_path: Some(std::path::Path::new("/fixture/blog/page.server.ts")),
            route_id: Some("routes/blog/page"),
            params: &params,
            metrics: &metrics,
            started: std::time::Instant::now(),
            middleware_locals: &locals,
            middleware_headers: &mw_headers,
            layout_module_paths: &[],
            page_needs_parent: false,
            locale: "",
            session_codec: None,
            middleware_ms: 0.0,
            waterfall: None,
            max_envelope: cap,
        }))
    };

    println!("the request max_body twin (the default bound): {default_cap} bytes\n");
    let under = run(1024, default_cap, false);
    describe("under the default cap", &under);
    let over = run(64 * 1024, 1024, false);
    describe("over a 1024-byte cap", &over);
    let refused = run(0, default_cap, true);
    describe("unencodable (reserved key)", &refused);
}
