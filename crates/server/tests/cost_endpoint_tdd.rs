//! TDD pins for the V2-F17 component-cost endpoint (the server half):
//! `GET /__oxide/cost?src=<path>` — the §V2.J map's dev-plane row the
//! open-in-editor endpoint shares (`/__oxide/open, /__oxide/cost`).
//!
//! The spec contract (§V2-F17 + the §V2.J dev-plane row):
//! - `GET /__oxide/cost?src=<path>` (dev-token guarded, D41) answers the
//!   component's cost record `{ bytes, gzip, chunk, renderMsP50, cache }`
//!   — bytes and gzip from the bundler's module map, the chunk the file
//!   landed in, the p50 server render time when the adapter measured it
//!   (`None` for the adapters that show bytes only — the overlay says so).
//! - The data comes from an injected [`CostSource`] seam: the endpoint is
//!   the wire contract, the build-side feed is the caller's. A source
//!   that does not know the path is a documented 404, never a guess.
//! - The endpoint only exists when attached (`cost_endpoint` is `None` in
//!   the default pipeline — production never attaches it, the unattached
//!   shape falls through to the normal dispatch, the inspector's posture).

use std::sync::{Arc, OnceLock};

use bytes::Bytes;
use oxide_fw_server::{
    ComponentCost, CostEndpoint, CostSource, FileCostsSource, OxideRequest, OxideResponse,
    Pipeline, SnapshotCostSource,
};

// ------------------------------------------------------------------ helpers

/// A minimal GET request against `url` (the `navgraph_tdd.rs` shape).
fn get_with(url: &str, headers: &[(&str, &str)]) -> OxideRequest {
    OxideRequest {
        method: "GET".into(),
        url: url.into(),
        headers: Arc::new(
            headers
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect::<Vec<_>>(),
        ),
        header_index: OnceLock::default(),
        body: Bytes::new(),
        request_id: "req-f17".into(),
        trace: None,
        peer: None,
        rewrite_hops: 0,
        mw_resume_after: None,
    }
}

/// The dispatch delegate the pipeline pins use — the endpoint arm must
/// answer the request itself, never delegate.
fn refused_dispatch()
-> impl Fn(OxideRequest) -> std::pin::Pin<Box<dyn std::future::Future<Output = OxideResponse> + Send>>
{
    |_r: OxideRequest| {
        Box::pin(async { OxideResponse::new(500, "dispatch must not run") })
            as std::pin::Pin<Box<dyn std::future::Future<Output = OxideResponse> + Send>>
    }
}

/// A fixture source over two components (the records the pins probe).
struct FakeSource;

impl CostSource for FakeSource {
    fn cost(&self, src: &str) -> Option<ComponentCost> {
        match src {
            "src/hero.tsx" => Some(ComponentCost {
                bytes: 4812,
                gzip: 1901,
                chunk: "chunk-abc.js".into(),
                render_ms_p50: Some(2.5),
                cache: Some("L0".into()),
            }),
            // The Svelte/Solid shape: bytes only, the overlay says so.
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

fn endpoint() -> CostEndpoint {
    CostEndpoint::new(Arc::new(FakeSource) as Arc<dyn CostSource>)
}

// ------------------------------------------------------------- the contracts

#[test]
fn non_get_is_405_with_allow() {
    let resp = endpoint().serve("POST", "src=src/hero.tsx");
    assert_eq!(resp.status, 405);
    assert_eq!(resp.header("allow"), Some("GET"));
}

#[test]
fn missing_src_param_is_400_with_the_usage_line() {
    let resp = endpoint().serve("GET", "");
    assert_eq!(resp.status, 400);
    let body = String::from_utf8(resp.body.to_vec()).expect("utf8");
    assert!(body.contains("`src`"), "{body}");
}

#[test]
fn unknown_src_is_404_never_a_guess() {
    // A source that does not know the path answers the documented 404 —
    // the endpoint never fabricates a record the build did not produce.
    let resp = endpoint().serve("GET", "src=src/ghost.tsx");
    assert_eq!(resp.status, 404);
    let body = String::from_utf8(resp.body.to_vec()).expect("utf8");
    assert!(body.contains("src/ghost.tsx"), "{body}");
}

#[test]
fn hit_answers_the_spec_json_shape() {
    // §V2-F17: `{ bytes, gzip, chunk, renderMsP50, cache }` — camelCase on
    // the wire (the §14 config-key convention).
    let resp = endpoint().serve("GET", "src=src/hero.tsx");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.header("content-type"), Some("application/json"));
    let body: serde_json::Value = serde_json::from_slice(&resp.body).expect("json");
    assert_eq!(body["bytes"], 4812);
    assert_eq!(body["gzip"], 1901);
    assert_eq!(body["chunk"], "chunk-abc.js");
    assert_eq!(body["renderMsP50"], 2.5);
    assert_eq!(body["cache"], "L0");
}

#[test]
fn bytes_only_record_leaves_render_and_cache_null() {
    // The adapters that do not measure render time leave the field null —
    // the overlay renders the "render time unavailable" note off this.
    let resp = endpoint().serve("GET", "src=src/counter.svelte");
    assert_eq!(resp.status, 200);
    let body: serde_json::Value = serde_json::from_slice(&resp.body).expect("json");
    assert_eq!(body["bytes"], 921);
    assert!(body["renderMsP50"].is_null(), "{body}");
    assert!(body["cache"].is_null(), "{body}");
}

#[test]
fn src_param_is_percent_decoded() {
    // A single-record source under a space-bearing path (the fixture shape
    // the real adapter's records carry on macOS/Windows too).
    struct SingleSource;
    impl CostSource for SingleSource {
        fn cost(&self, src: &str) -> Option<ComponentCost> {
            (src == "src/my file.tsx").then(|| ComponentCost {
                bytes: 10,
                gzip: 5,
                chunk: "c.js".into(),
                render_ms_p50: None,
                cache: None,
            })
        }
    }
    let ep = CostEndpoint::new(Arc::new(SingleSource) as Arc<dyn CostSource>);
    let resp = ep.serve("GET", "src=src/my%20file.tsx");
    assert_eq!(resp.status, 200, "the encoded space decoded to the record");
}

// ------------------------------------------------------------- the pipeline arm

#[tokio::test]
async fn pipeline_without_the_endpoint_falls_through() {
    // The default pipeline never attaches the endpoint: the request falls
    // through to the normal dispatch (production byte-identical).
    let p = Pipeline::default();
    let resp = p
        .serve_request(
            get_with("/__oxide/cost?src=src/hero.tsx", &[]),
            &refused_dispatch(),
        )
        .await;
    assert_eq!(resp.status, 500, "the dispatch delegate ran (fall-through)");
}

#[tokio::test]
async fn pipeline_answers_200_with_the_token() {
    let mut p = Pipeline::default().with_cost_endpoint(Arc::new(endpoint()));
    p.dev_token = Some("tok".into());
    let resp = p
        .serve_request(
            get_with("/__oxide/cost?src=src/hero.tsx&token=tok", &[]),
            &refused_dispatch(),
        )
        .await;
    assert_eq!(resp.status, 200);
}

#[tokio::test]
async fn pipeline_refuses_a_wrong_or_missing_token_with_403() {
    let mut p = Pipeline::default().with_cost_endpoint(Arc::new(endpoint()));
    p.dev_token = Some("tok".into());
    let missing = p
        .serve_request(
            get_with("/__oxide/cost?src=src/hero.tsx", &[]),
            &refused_dispatch(),
        )
        .await;
    assert_eq!(missing.status, 403);
    let wrong = p
        .serve_request(
            get_with(
                "/__oxide/cost?src=src/hero.tsx",
                &[("x-oxide-fw-dev-token", "nope")],
            ),
            &refused_dispatch(),
        )
        .await;
    assert_eq!(wrong.status, 403);
}

// ------------------------------------------------------------------
// §V2-F17 adapter half: the snapshot-backed cost source
// ------------------------------------------------------------------
//
// The endpoint doc records "the build-side adapter is the caller's" —
// the adapter IS the mapping from the build's snapshot records into the
// seam. The production shape the dev boot feeds: the bundler's
// `source_costs` records mapped one-to-one (`bytes_only` — the render
// and cache halves belong to the profiler and the tier map, absent for
// the bytes-only adapters), plus the empty snapshot the tests inject.

/// Hits answer from the build's records; anything the build did not
/// produce is the documented miss (the endpoint's 404) — including on
/// the empty snapshot the no-build shapes feed.
#[test]
fn snapshot_source_answers_hits_and_documented_misses() {
    let source = SnapshotCostSource::from_pairs([
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
    assert_eq!(
        source.cost("src/hero.tsx"),
        Some(ComponentCost {
            bytes: 4812,
            gzip: 1901,
            chunk: "chunk-abc.js".into(),
            render_ms_p50: Some(2.5),
            cache: Some("L0".into()),
        })
    );
    assert_eq!(source.cost("src/ghost.tsx"), None);
    assert_eq!(SnapshotCostSource::default().cost("src/hero.tsx"), None);
}

/// The bytes-only constructor is the spec's Svelte/Solid shape: the
/// render and cache halves stay `None` — the overlay renders the
/// "render time unavailable" note off the null `renderMsP50`.
#[test]
fn bytes_only_record_carries_null_render_and_cache() {
    let rec = ComponentCost::bytes_only(921, 430, "chunk-def.js");
    assert_eq!(rec.bytes, 921);
    assert_eq!(rec.gzip, 430);
    assert_eq!(rec.chunk, "chunk-def.js");
    assert_eq!(rec.render_ms_p50, None);
    assert_eq!(rec.cache, None);
}

/// The adapter carries the endpoint's seam: the dev boot attaches it as
/// `Arc<dyn CostSource>` (the object-safety the `CostEndpoint::new`
/// signature demands).
#[test]
fn snapshot_source_impls_the_cost_source_seam() {
    let source: Arc<dyn CostSource> = Arc::new(SnapshotCostSource::default());
    assert_eq!(source.cost("src/hero.tsx"), None);
}

// ------------------------------------------------------------------
// §V2-F17 adapter half: the file-backed cost source (the dev-side feed)
// ------------------------------------------------------------------
//
// The dev boot cannot hold the build's `BundleOutput` across rebuilds —
// the feed it attaches is the build's PERSISTED records file
// (`client.costs.json`, the bundler's `write_source_costs` rows), and
// `FileCostsSource` re-reads it when the file's (mtime, length) stat
// pair moves, so a dev rebuild's costs answer without re-attaching. The honesty
// rules ride the seam: a missing, unreadable or unparsable file answers
// the documented miss (the endpoint's 404), never a fabricated record.

use std::path::Path;

/// Write a records file in the bundler's `write_source_costs` row shape
/// (the `{ source, bytes, gzip, chunk }` array the file contract pins).
fn write_records_file(path: &Path, rows: &serde_json::Value) {
    std::fs::write(
        path,
        serde_json::to_string_pretty(&rows).expect("rows json"),
    )
    .expect("records file write");
}

/// The two-row fixture: a chunked record and the orphan shape
/// (`chunk: null` — no chunk claims the module).
fn records_fixture() -> serde_json::Value {
    serde_json::json!([
        { "source": "src/hero.tsx", "bytes": 4812, "gzip": 1901, "chunk": "chunk-abc.js" },
        { "source": "src/orphan.tsx", "bytes": 921, "gzip": 430, "chunk": null }
    ])
}

#[test]
fn file_source_answers_hits_from_the_records_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("client.costs.json");
    write_records_file(&path, &records_fixture());
    let source = FileCostsSource::from_path(&path);
    assert_eq!(
        source.cost("src/hero.tsx"),
        Some(ComponentCost::bytes_only(4812, 1901, "chunk-abc.js")),
        "the row maps one-to-one into the bytes-only record"
    );
    // The orphan shape: a null chunk is the honest empty on the served
    // record (the overlay displays "no chunk"), never a fabricated name.
    assert_eq!(
        source.cost("src/orphan.tsx"),
        Some(ComponentCost::bytes_only(921, 430, ""))
    );
    assert_eq!(source.cost("src/ghost.tsx"), None, "the documented miss");
}

#[test]
fn file_source_misses_when_the_file_is_missing() {
    // A project with no build yet: the boot attaches anyway (the file may
    // appear on the first rebuild) and every query answers the documented
    // miss until the records land.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("client.costs.json");
    let source = FileCostsSource::from_path(&path);
    assert_eq!(source.cost("src/hero.tsx"), None);
}

#[test]
fn file_source_misses_on_garbage_without_panicking() {
    // A truncated or hand-edited file is a miss, not a crash — the dev
    // loop stays up and the endpoint stays honest (the reload failure is
    // logged, the next mtime move retries).
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("client.costs.json");
    std::fs::write(&path, "truncated{").expect("garbage write");
    let source = FileCostsSource::from_path(&path);
    assert_eq!(source.cost("src/hero.tsx"), None);
}

#[test]
fn file_source_reloads_after_the_file_changes() {
    // The dev loop's contract: a rebuild rewrites the records file, the
    // NEXT query serves the new costs without re-attaching the endpoint.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("client.costs.json");
    write_records_file(
        &path,
        &serde_json::json!([{ "source": "src/hero.tsx", "bytes": 100, "gzip": 50, "chunk": "a.js" }]),
    );
    let source = FileCostsSource::from_path(&path);
    assert_eq!(
        source.cost("src/hero.tsx"),
        Some(ComponentCost::bytes_only(100, 50, "a.js")),
        "the first load answers the v1 records"
    );
    write_records_file(
        &path,
        &serde_json::json!([{ "source": "src/hero.tsx", "bytes": 222, "gzip": 111, "chunk": "b.js" }]),
    );
    std::thread::sleep(std::time::Duration::from_millis(25));
    assert_eq!(
        source.cost("src/hero.tsx"),
        Some(ComponentCost::bytes_only(222, 111, "b.js")),
        "the mtime move reloads the file (no re-attach)"
    );
}

#[test]
fn file_source_serves_identical_records_when_the_file_moves_neither_half() {
    // The stat-pair rule's other half: a rewrite that moves NEITHER the
    // mtime nor the length (a same-length write inside one timestamp
    // tick) carries identical content anyway — the answer is the same
    // records either way, no flake either way.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("client.costs.json");
    write_records_file(
        &path,
        &serde_json::json!([{ "source": "src/hero.tsx", "bytes": 100, "gzip": 50, "chunk": "a.js" }]),
    );
    let source = FileCostsSource::from_path(&path);
    assert_eq!(
        source.cost("src/hero.tsx"),
        Some(ComponentCost::bytes_only(100, 50, "a.js"))
    );
    write_records_file(
        &path,
        &serde_json::json!([{ "source": "src/hero.tsx", "bytes": 100, "gzip": 50, "chunk": "a.js" }]),
    );
    std::thread::sleep(std::time::Duration::from_millis(25));
    assert_eq!(
        source.cost("src/hero.tsx"),
        Some(ComponentCost::bytes_only(100, 50, "a.js")),
        "the same content serves the same records"
    );
}

#[test]
fn file_source_impls_the_cost_source_seam() {
    // The dev boot attaches it as `Arc<dyn CostSource>` (the object
    // safety `CostEndpoint::new` demands).
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("client.costs.json");
    write_records_file(&path, &records_fixture());
    let source: Arc<dyn CostSource> = Arc::new(FileCostsSource::from_path(&path));
    assert_eq!(
        source.cost("src/hero.tsx"),
        Some(ComponentCost::bytes_only(4812, 1901, "chunk-abc.js"))
    );
}

#[test]
fn endpoint_over_file_source_answers_the_wire_contracts() {
    // The full wire shape: a hit is the 200 camelCase JSON, an unknown
    // path is the documented 404 — over the file-backed adapter the boot
    // attaches.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("client.costs.json");
    write_records_file(&path, &records_fixture());
    let ep = CostEndpoint::new(Arc::new(FileCostsSource::from_path(&path)) as Arc<dyn CostSource>);
    let hit = ep.serve("GET", "src=src/hero.tsx");
    assert_eq!(hit.status, 200);
    let body: serde_json::Value = serde_json::from_slice(&hit.body).expect("json");
    assert_eq!(body["bytes"], 4812);
    assert_eq!(body["chunk"], "chunk-abc.js");
    assert!(body["renderMsP50"].is_null(), "{body}");
    let miss = ep.serve("GET", "src=src/ghost.tsx");
    assert_eq!(miss.status, 404);
}
