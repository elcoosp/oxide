//! Walkthrough: the V2-F17 component-cost wire contract, end to end — the
//! consumer `oxide-fw-server` never had for this seam.
//!
//! The dev overlay (`?ox-cost` / Alt+Shift+O) highlights the hovered
//! component and lists the three heaviest; `GET /__oxide/cost?src=<path>`
//! (the §V2.J map's dev-plane row the open-in-editor endpoint shares) is
//! the wire half that feeds it. The record is the spec's
//! `{ bytes, gzip, chunk, renderMsP50, cache }`; the data rides the
//! injected [`CostSource`] seam — the production adapter reads the
//! bundler's module map and the render profiler, and the compiler's
//! `data-ox-src` attribute injection plus the overlay UI are the recorded
//! remaining halves.
//!
//! This example walks the shapes against a two-record fixture source (no
//! build data needed): the hit, the bytes-only record, the 404 miss, the
//! 400/405 contracts and the pipeline's token guard.
//!
//! Run: `cargo run -p oxide-fw-server --example cost_endpoint`

use std::sync::{Arc, OnceLock};

use bytes::Bytes;
use oxide_fw_server::{
    ComponentCost, CostEndpoint, CostSource, FileCostsSource, OxideRequest, OxideResponse,
    Pipeline, ProfiledCostSource, SnapshotCostSource,
};

/// Two records: one fully measured (React-shaped), one bytes-only
/// (Svelte/Solid-shaped — the overlay renders the "render time
/// unavailable" note off the null `renderMsP50`).
struct FixtureSource;

impl CostSource for FixtureSource {
    fn cost(&self, src: &str) -> Option<ComponentCost> {
        match src {
            "src/hero.tsx" => Some(ComponentCost {
                bytes: 4812,
                gzip: 1901,
                chunk: "chunk-abc.js".into(),
                render_ms_p50: Some(2.5),
                cache: Some("L0".into()),
            }),
            "src/counter.svelte" => Some(ComponentCost {
                bytes: 921,
                gzip: 430,
                chunk: "chunk-def.js".into(),
                render_ms_p50: None,
                cache: None,
            }),
            _ => None,
        }
    }
}

/// A minimal GET request helper (the `navgraph_tdd.rs` shape).
fn get(url: &str) -> OxideRequest {
    OxideRequest {
        method: "GET".into(),
        url: url.into(),
        headers: Arc::new(Vec::new()),
        header_index: OnceLock::default(),
        body: Bytes::new(),
        request_id: "ex-f17".into(),
        trace: None,
        peer: None,
        rewrite_hops: 0,
        mw_resume_after: None,
    }
}

/// The dispatch delegate — an attached endpoint must never need it (the
/// `serve_request` boxed-future shape).
fn dispatch()
-> impl Fn(OxideRequest) -> std::pin::Pin<Box<dyn std::future::Future<Output = OxideResponse> + Send>>
{
    |_r: OxideRequest| {
        Box::pin(async { OxideResponse::new(500, "dispatch must not run") })
            as std::pin::Pin<Box<dyn std::future::Future<Output = OxideResponse> + Send>>
    }
}

/// The file-backed walk: the no-build miss, the first load, the
/// orphan's empty chunk, and the rebuild's reload (no re-attach).
fn walk_file_backed_feed() {
    println!("\nfile-backed feed (the dev-side adapter the boot attaches):");
    // The dev-side design: the build persists the records beside the
    // client manifest (the bundler's `write_source_costs` — the same
    // `{ source, bytes, gzip, chunk }` rows), and the boot attaches
    // `FileCostsSource` once; a rebuild rewrites the file, the NEXT query
    // reloads (the mtime check), no re-attach. The walk: the no-build
    // miss, the first load, the rebuild's reload.
    let dir = tempfile::tempdir().expect("example tempdir");
    let costs_path = dir.path().join("client.costs.json");
    let file_source = FileCostsSource::from_path(&costs_path);
    let file_ep = CostEndpoint::new(Arc::new(file_source) as Arc<dyn CostSource>);
    println!(
        "  no build yet: {} {}",
        file_ep.serve("GET", "src=src/hero.tsx").status,
        String::from_utf8_lossy(&file_ep.serve("GET", "src=src/hero.tsx").body)
    );
    std::fs::write(
        &costs_path,
        serde_json::to_string_pretty(&serde_json::json!([
            { "source": "src/hero.tsx", "bytes": 4812, "gzip": 1901, "chunk": "chunk-abc.js" },
            { "source": "src/orphan.tsx", "bytes": 921, "gzip": 430, "chunk": null }
        ]))
        .expect("records json"),
    )
    .expect("records write");
    let resp = file_ep.serve("GET", "src=src/hero.tsx");
    println!(
        "  first build:  {} {}",
        resp.status,
        String::from_utf8_lossy(&resp.body)
    );
    let resp = file_ep.serve("GET", "src=src/orphan.tsx");
    println!(
        "  orphan row:   {} {}",
        resp.status,
        String::from_utf8_lossy(&resp.body)
    );
    std::fs::write(
        &costs_path,
        serde_json::to_string_pretty(&serde_json::json!([
            { "source": "src/hero.tsx", "bytes": 2222, "gzip": 999, "chunk": "chunk-rebuilt.js" }
        ]))
        .expect("records json"),
    )
    .expect("records rewrite");
    std::thread::sleep(std::time::Duration::from_millis(25));
    let resp = file_ep.serve("GET", "src=src/hero.tsx");
    println!(
        "  after rebuild: {} {} (the stat pair moved: reloaded, no re-attach)",
        resp.status,
        String::from_utf8_lossy(&resp.body)
    );
}

/// The profiled-adapter walk: the snapshot base decorated with the
/// profiler's p50 and the tier map (the record's remaining halves).
fn walk_profiled_adapter() {
    let base = SnapshotCostSource::from_pairs([
        (
            "src/hero.tsx".to_string(),
            ComponentCost::bytes_only(4812, 1901, "chunk-abc.js"),
        ),
        (
            "src/counter.svelte".to_string(),
            ComponentCost::bytes_only(921, 430, "chunk-def.js"),
        ),
    ]);
    let profiled = ProfiledCostSource::from_maps(
        Arc::new(base) as Arc<dyn CostSource>,
        [("src/counter.svelte".to_string(), 1.25)],
        [("src/counter.svelte".to_string(), "L0".to_string())],
    );
    let ep = CostEndpoint::new(Arc::new(profiled) as Arc<dyn CostSource>);
    for src in ["src/counter.svelte", "src/hero.tsx", "src/ghost.tsx"] {
        let resp = ep.serve("GET", &format!("src={src}"));
        println!(
            "  {src}: {} {}",
            resp.status,
            String::from_utf8_lossy(&resp.body)
        );
    }
}

fn main() {
    let ep = CostEndpoint::new(Arc::new(FixtureSource) as Arc<dyn CostSource>);

    println!("GET /__oxide/cost?src=src/hero.tsx (the fully measured record):");
    let resp = ep.serve("GET", "src=src/hero.tsx");
    println!(
        "  -> {} {}",
        resp.status,
        String::from_utf8_lossy(&resp.body)
    );

    println!("GET /__oxide/cost?src=src/counter.svelte (bytes only):");
    let resp = ep.serve("GET", "src=src/counter.svelte");
    println!(
        "  -> {} {}",
        resp.status,
        String::from_utf8_lossy(&resp.body)
    );

    println!("\nthe contracts:");
    for (label, method, query) in [
        ("unknown src", "GET", "src=src/ghost.tsx"),
        ("missing src", "GET", ""),
        ("non-GET", "POST", "src=src/hero.tsx"),
    ] {
        let resp = ep.serve(method, query);
        println!(
            "  {label}: {} {}",
            resp.status,
            String::from_utf8_lossy(&resp.body)
                .lines()
                .next()
                .unwrap_or("")
        );
    }

    println!("\nthe production adapter (SnapshotCostSource):");
    // The build-side records map one-to-one: the bundler's `source_costs`
    // triple rides `bytes_only` (the render and cache halves belong to
    // the profiler and the tier map), and the `?src=` spelling is the
    // `data-ox-src` stamp's. Misses stay the documented 404.
    let snapshot = SnapshotCostSource::from_pairs([
        (
            "src/hero.tsx".to_string(),
            ComponentCost {
                bytes: 4812,
                gzip: 1901,
                chunk: "chunk-abc.js".into(),
                render_ms_p50: Some(2.5),
                cache: Some("L0".into()),
            },
        ),
        (
            "src/counter.svelte".to_string(),
            ComponentCost::bytes_only(921, 430, "chunk-def.js"),
        ),
    ]);
    let ep = CostEndpoint::new(Arc::new(snapshot) as Arc<dyn CostSource>);
    let resp = ep.serve("GET", "src=src/counter.svelte");
    println!(
        "  bytes-only hit: {} {}",
        resp.status,
        String::from_utf8_lossy(&resp.body)
    );
    let resp = ep.serve("GET", "src=src/ghost.tsx");
    println!(
        "  miss: {} {}",
        resp.status,
        String::from_utf8_lossy(&resp.body)
    );

    println!("\nprofiled adapter (snapshot + profiler p50 + tier map):");
    walk_profiled_adapter();

    walk_file_backed_feed();

    println!("\npipeline:");
    let mut p = Pipeline::default().with_cost_endpoint(Arc::new(ep));
    p.dev_token = Some("tok".into());
    for (label, url) in [
        ("with the token", "/__oxide/cost?src=src/hero.tsx&token=tok"),
        ("wrong token", "/__oxide/cost?src=src/hero.tsx&token=nope"),
    ] {
        let resp = futures::executor::block_on(p.serve_request(get(url), &dispatch()));
        println!("  {label}: {}", resp.status);
    }
    let unattached = Pipeline::default();
    let resp = futures::executor::block_on(
        unattached.serve_request(get("/__oxide/cost?src=src/hero.tsx&token=tok"), &dispatch()),
    );
    println!(
        "  unattached pipeline: {} (the dispatch ran — fall-through)",
        resp.status
    );
}
