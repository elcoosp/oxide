//! TDD pins for the V2-F17 profiler + tier-map adapter (the cost record's
//! remaining halves): [`oxide_fw_server::ProfiledCostSource`] — the
//! layered production `CostSource` the §V2-F17 record's `renderMsP50` and
//! `cache` halves ride, decorating the build's snapshot source.
//!
//! The contract (the recorded F17 shape — "the render and cache halves
//! belong to the profiler and the tier map, which ride a different
//! adapter"):
//! - the base record is authoritative: a half the snapshot already
//!   carries is never overwritten (the decorator must not contradict the
//!   build);
//! - a base miss stays a miss — the profiler map alone never fabricates a
//!   record (the endpoint's documented 404 all the way down);
//! - empty maps leave the `bytes_only` shape byte-identical.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use bytes::Bytes;
use oxide_fw_server::{
    ComponentCost, CostEndpoint, CostSource, OxideRequest, OxideResponse, Pipeline,
    ProfiledCostSource, SnapshotCostSource,
};

// ------------------------------------------------------------------ helpers

/// The build's snapshot: two records, one bytes-only (the Svelte/Solid
/// shape), one already carrying both halves (the fully-measured shape).
fn base() -> SnapshotCostSource {
    SnapshotCostSource::from_pairs([
        (
            "src/hero.tsx".to_string(),
            ComponentCost {
                bytes: 4812,
                gzip: 1901,
                chunk: "chunk-abc.js".into(),
                render_ms_p50: Some(2.5),
                cache: Some("L1".into()),
            },
        ),
        (
            "src/counter.svelte".to_string(),
            ComponentCost::bytes_only(921, 430, "chunk-def.js"),
        ),
    ])
}

/// The profiler's per-source p50 and the pipeline's tier map — the two
/// records the render/cache halves ride (keyed by the `?src=` spelling).
fn profiler_maps() -> (BTreeMap<String, f64>, BTreeMap<String, String>) {
    let render: BTreeMap<String, f64> = [("src/counter.svelte", 1.25)]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
    let tiers: BTreeMap<String, String> = [("src/counter.svelte", "L0")]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    (render, tiers)
}

fn get(url: &str) -> OxideRequest {
    OxideRequest {
        method: "GET".into(),
        url: url.into(),
        headers: Arc::new(Vec::new()),
        header_index: OnceLock::default(),
        body: Bytes::new(),
        request_id: "req-f17b".into(),
        trace: None,
        peer: None,
        rewrite_hops: 0,
        mw_resume_after: None,
    }
}

fn dispatch()
-> impl Fn(OxideRequest) -> std::pin::Pin<Box<dyn std::future::Future<Output = OxideResponse> + Send>>
{
    |_r: OxideRequest| {
        Box::pin(async { OxideResponse::new(500, "dispatch must not run") })
            as std::pin::Pin<Box<dyn std::future::Future<Output = OxideResponse> + Send>>
    }
}

// ------------------------------------------------------------- the contracts

#[test]
fn the_adapter_fills_the_render_half_from_the_profiler_map() {
    let (render, tiers) = profiler_maps();
    let source = ProfiledCostSource::new(Arc::new(base()), render, tiers);
    let cost = source.cost("src/counter.svelte").expect("the snapshot hit");
    assert_eq!(cost.render_ms_p50, Some(1.25));
    assert_eq!(cost.bytes, 921, "the bytes half rides the base record");
}

#[test]
fn the_adapter_fills_the_cache_half_from_the_tier_map() {
    let (render, tiers) = profiler_maps();
    let source = ProfiledCostSource::new(Arc::new(base()), render, tiers);
    let cost = source.cost("src/counter.svelte").expect("the snapshot hit");
    assert_eq!(cost.cache.as_deref(), Some("L0"));
}

#[test]
fn the_base_record_is_authoritative_for_existing_halves() {
    // `src/hero.tsx` already carries both halves in the snapshot; a
    // profiler map naming OTHER values must never overwrite them — the
    // decorator decorates, it does not contradict the build.
    let render: BTreeMap<String, f64> = [("src/hero.tsx", 99.0)]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
    let tiers: BTreeMap<String, String> = [("src/hero.tsx", "remote")]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let source = ProfiledCostSource::new(Arc::new(base()), render, tiers);
    let cost = source.cost("src/hero.tsx").expect("the snapshot hit");
    assert_eq!(cost.render_ms_p50, Some(2.5), "the build's p50 stands");
    assert_eq!(cost.cache.as_deref(), Some("L1"), "the build's tier stands");
}

#[test]
fn a_base_miss_stays_a_miss_never_fabricated() {
    // A path the profiler measured but the build never produced is the
    // documented miss — the decorator cannot answer a record the build
    // did not produce (the endpoint's 404 contract).
    let render: BTreeMap<String, f64> = [("src/ghost.tsx", 3.0)]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
    let source = ProfiledCostSource::new(Arc::new(base()), render, BTreeMap::new());
    assert!(source.cost("src/ghost.tsx").is_none());
}

#[test]
fn empty_maps_leave_the_bytes_only_shape() {
    // No profiler, no tier map: the decorated source answers exactly the
    // base snapshot's records (the bytes-only shape, null halves on the
    // wire).
    let source = ProfiledCostSource::new(Arc::new(base()), BTreeMap::new(), BTreeMap::new());
    let cost = source.cost("src/counter.svelte").expect("the snapshot hit");
    assert_eq!(cost, ComponentCost::bytes_only(921, 430, "chunk-def.js"));
}

#[test]
fn from_maps_takes_the_caller_shapes() {
    // The boot shape: `(source, p50)` and `(source, tier)` iterator pairs
    // (the profiler ledger and the tier recorder's own rows), not the
    // caller's `BTreeMap`s.
    let source = ProfiledCostSource::from_maps(
        Arc::new(base()),
        [("src/counter.svelte".to_string(), 1.25)],
        [("src/counter.svelte".to_string(), "L0".to_string())],
    );
    let cost = source.cost("src/counter.svelte").expect("the snapshot hit");
    assert_eq!(cost.render_ms_p50, Some(1.25));
    assert_eq!(cost.cache.as_deref(), Some("L0"));
}

// ------------------------------------------------------------- the wire

#[tokio::test]
async fn the_endpoint_answers_the_full_record_through_the_adapter() {
    // The dev-boot wire-up: the snapshot source decorated with the
    // profiler + tier maps — the endpoint's JSON carries every spec half.
    let (render, tiers) = profiler_maps();
    let source: Arc<dyn CostSource> =
        Arc::new(ProfiledCostSource::new(Arc::new(base()), render, tiers));
    let ep = CostEndpoint::new(source);
    let resp = ep.serve("GET", "src=src/counter.svelte");
    assert_eq!(resp.status, 200);
    let body: serde_json::Value = serde_json::from_slice(&resp.body).expect("json");
    assert_eq!(body["bytes"], 921);
    assert_eq!(
        body["renderMsP50"], 1.25,
        "the profiler half rides the wire"
    );
    assert_eq!(body["cache"], "L0", "the tier half rides the wire");

    let mut p = Pipeline::default().with_cost_endpoint(Arc::new(CostEndpoint::new(Arc::new(
        ProfiledCostSource::new(Arc::new(base()), profiler_maps().0, profiler_maps().1),
    )
        as Arc<dyn CostSource>)));
    p.dev_token = Some("tok".into());
    let resp = p
        .serve_request(
            get("/__oxide/cost?src=src/counter.svelte&token=tok"),
            &dispatch(),
        )
        .await;
    assert_eq!(resp.status, 200);
    let body: serde_json::Value = serde_json::from_slice(&resp.body).expect("json");
    assert_eq!(body["renderMsP50"], 1.25);
    assert_eq!(body["cache"], "L0");
}

#[test]
fn the_decorated_snapshot_misses_answer_the_documented_404() {
    let source: Arc<dyn CostSource> = Arc::new(ProfiledCostSource::new(
        Arc::new(base()),
        BTreeMap::new(),
        BTreeMap::new(),
    ));
    let ep = CostEndpoint::new(source);
    let resp = ep.serve("GET", "src=src/ghost.tsx");
    assert_eq!(resp.status, 404, "the miss survives the decorator");
}
