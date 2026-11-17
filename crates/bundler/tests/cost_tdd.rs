//! TDD pins for the §V2-F17 build-side cost engine
//! (`oxide_fw_bundler::cost`): the per-source `{ bytes, gzip, chunk }`
//! records the landed `/__oxide/cost?src=<path>` endpoint serves — the
//! build-side producer the endpoint's `CostSource` seam was waiting for
//! (the endpoint is the wire contract; this is the data).
//!
//! Written BEFORE the implementation; every pin imports public API only
//! (red-first: E0432 over `oxide_fw_bundler::cost`).
//!
//! The contracts pinned here:
//!
//! - Every client module the bundle output carries (its post-walk
//!   source, the estimate basis the record labels honestly) gets a
//!   record: `bytes` is the source length, `gzip` is the RFC 1952
//!   gzip-stream length over the same bytes (the spec's estimation
//!   method — "gzip estimated by compressing each module's rendered
//!   code"), `chunk` is the file of the FIRST chunk listing the module,
//!   `None` when no chunk claims it.
//! - The `source` key is the ROOT-RELATIVE spelling with forward slashes
//!   (the endpoint's `?src=` contract) — an absolute path inside `root`
//!   strips; one outside it rides as given.
//! - Assets (`is_asset`) and server-only modules are excluded — the
//!   overlay attributes CLIENT sources.
//! - Deterministic: sorted by the `source` key, two calls byte-equal.
//! - An empty output costs nothing.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use oxide_fw_bundler::cost::{SourceCost, read_source_costs, source_costs, write_source_costs};
use oxide_fw_bundler::{BundleOutput, Chunk, ChunkKind, Module};

/// A fake module with the fake's source — the bundle output's module
/// shape, nothing more.
fn module(resolved: &str, source: &str, is_asset: bool, server_only: bool) -> Module {
    Module {
        specifier: Path::new(resolved)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
        resolved: PathBuf::from(resolved),
        source: source.to_string(),
        hash: format!("h{}", resolved.len()),
        imports: Vec::new(),
        is_asset,
        server_only,
    }
}

/// A fake chunk claiming `modules` and naming itself `file`.
fn chunk(file: &str, modules: &[&str]) -> Chunk {
    Chunk {
        name: file.trim_end_matches(".js").to_string(),
        file: file.to_string(),
        hash: format!("h{}", file.len()),
        modules: modules.iter().map(PathBuf::from).collect(),
        imports: Vec::new(),
        css: Vec::new(),
        kind: ChunkKind::Entry,
    }
}

/// A two-module, one-chunk output — the shape every attribution pin
/// builds on.
fn output() -> BundleOutput {
    BundleOutput {
        modules: vec![
            module(
                "/proj/src/routes/index.tsx",
                "export default function Page() {\n  return <main>hi</main>;\n}\n",
                false,
                false,
            ),
            module(
                "/proj/src/islands/card.tsx",
                "export function Card() {\n  return <b>card</b>;\n}\n",
                false,
                false,
            ),
        ],
        chunks: vec![chunk(
            "entry.index-abc123.js",
            &["/proj/src/routes/index.tsx", "/proj/src/islands/card.tsx"],
        )],
        client_manifest: None,
        server_manifest: None,
        diagnostics: Vec::new(),
    }
}

/// The RFC 1952 gzip stream length over `bytes` (the algorithm the
/// record's `gzip` field is pinned against).
fn gzip_of(bytes: &[u8]) -> u64 {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(bytes)
        .expect("writing to a Vec-backed encoder never fails");
    u64::try_from(
        encoder
            .finish()
            .expect("finishing a Vec-backed encoder never fails")
            .len(),
    )
    .expect("a gzip length fits u64")
}

fn root() -> &'static Path {
    Path::new("/proj")
}

// ---------------- the record shape ----------------

#[test]
fn every_client_module_gets_a_record() {
    let costs = source_costs(&output(), root());
    assert_eq!(costs.len(), 2, "{costs:?}");
}

#[test]
fn chunk_attribution_names_the_first_claiming_chunk() {
    let costs = source_costs(&output(), root());
    for cost in &costs {
        assert_eq!(
            cost.chunk.as_deref(),
            Some("entry.index-abc123.js"),
            "{cost:?}"
        );
    }
}

#[test]
fn unclaimed_module_rides_chunkless() {
    let mut out = output();
    out.chunks.clear();
    let costs = source_costs(&out, root());
    for cost in &costs {
        assert_eq!(cost.chunk, None, "no chunk claims it: {cost:?}");
    }
}

#[test]
fn first_chunk_wins_when_two_chunks_claim_one_module() {
    let mut out = output();
    out.chunks
        .push(chunk("shared-def456.js", &["/proj/src/islands/card.tsx"]));
    let costs = source_costs(&out, root());
    let card = costs
        .iter()
        .find(|c| c.source == "src/islands/card.tsx")
        .expect("the card's record");
    assert_eq!(
        card.chunk.as_deref(),
        Some("entry.index-abc123.js"),
        "{card:?}"
    );
}

// ---------------- bytes and gzip ----------------

#[test]
fn bytes_and_gzip_measure_the_carried_source() {
    let costs = source_costs(&output(), root());
    let page = costs
        .iter()
        .find(|c| c.source == "src/routes/index.tsx")
        .expect("the page's record");
    let raw = "export default function Page() {\n  return <main>hi</main>;\n}\n";
    assert_eq!(page.bytes, raw.len() as u64, "{page:?}");
    assert_eq!(page.gzip, gzip_of(raw.as_bytes()), "{page:?}");
    assert!(page.gzip > 0, "a real stream length: {page:?}");
    // A 61-byte source does NOT shrink (the gzip header dominates tiny
    // inputs) — the shrink property is pinned on realistic sizes by
    // `gzip_is_the_real_stream_length_over_the_same_bytes`.
}

#[test]
fn gzip_is_the_real_stream_length_over_the_same_bytes() {
    // The record's `gzip` is the length of the REAL RFC 1952 stream over
    // the carried source (the spec's estimation method), so re-encoding
    // the same bytes with the same algorithm lands exactly on it — the
    // overlay can render the estimate against the wire size without a
    // second spelling of the algorithm.
    let mut out = output();
    out.modules.clear();
    let repetitive = "const banner = 'oxide oxide oxide oxide oxide';\n".repeat(64);
    out.modules
        .push(module("/proj/src/banner.js", &repetitive, false, false));
    let costs = source_costs(&out, root());
    assert_eq!(costs.len(), 1);
    assert_eq!(costs[0].gzip, gzip_of(repetitive.as_bytes()));
    assert!(
        costs[0].gzip < costs[0].bytes / 4,
        "repetitive text compresses hard: {costs:?}"
    );
}

// ---------------- the source spelling ----------------

#[test]
fn source_key_is_root_relative_with_forward_slashes() {
    let costs = source_costs(&output(), root());
    let sources: Vec<&str> = costs.iter().map(|c| c.source.as_str()).collect();
    assert!(sources.contains(&"src/routes/index.tsx"), "{sources:?}");
    assert!(sources.contains(&"src/islands/card.tsx"), "{sources:?}");
}

#[test]
fn source_outside_root_rides_as_given() {
    let mut out = output();
    out.modules.push(module(
        "/vendor/pkg/dist/chunk.js",
        "console.log(1);\n",
        false,
        false,
    ));
    let costs = source_costs(&out, root());
    let vendor = costs
        .iter()
        .find(|c| c.source == "/vendor/pkg/dist/chunk.js")
        .expect("the outside-root module rides as given");
    assert_eq!(vendor.chunk, None, "no chunk claims it");
}

// ---------------- the exclusions ----------------

#[test]
fn assets_and_server_modules_are_excluded() {
    let mut out = output();
    out.modules
        .push(module("/proj/src/logo.svg", "<svg/>", true, false));
    out.modules.push(module(
        "/proj/src/routes/index.server.ts",
        "export const k = 1;\n",
        false,
        true,
    ));
    let costs = source_costs(&out, root());
    let sources: Vec<&str> = costs.iter().map(|c| c.source.as_str()).collect();
    assert_eq!(sources.len(), 2, "only client modules: {sources:?}");
    assert!(
        !sources.iter().any(|s| s.contains("logo.svg")),
        "{sources:?}"
    );
    assert!(!sources.iter().any(|s| s.contains("server")), "{sources:?}");
}

// ---------------- determinism ----------------

#[test]
fn records_are_sorted_and_deterministic() {
    let first = source_costs(&output(), root());
    let second = source_costs(&output(), root());
    assert_eq!(first, second, "two calls, byte-equal records");
    let keys: Vec<&str> = first.iter().map(|c| c.source.as_str()).collect();
    let mut sorted = keys.clone();
    sorted.sort_unstable();
    assert_eq!(keys, sorted, "sorted by the source key");
}

#[test]
fn empty_output_costs_nothing() {
    let costs = source_costs(&BundleOutput::default(), root());
    assert!(costs.is_empty(), "an empty output costs nothing");
}

// ---------------- the record's wire shape ----------------

#[test]
fn record_serializes_camel_case_for_the_cli_json_surface() {
    let mut out = output();
    out.chunks.clear();
    let costs = source_costs(&out, root());
    let page: &SourceCost = costs.first().expect("the page's record");
    let json = serde_json::to_value(page).expect("a plain record serializes");
    let map: std::collections::BTreeMap<String, serde_json::Value> =
        serde_json::from_value(json).expect("an object shape");
    for key in ["source", "bytes", "gzip", "chunk"] {
        assert!(map.contains_key(key), "{key} on the wire: {map:?}");
    }
    assert!(
        map.keys().all(|k| !k.contains('_')),
        "no snake_case keys: {map:?}"
    );
}

// ---------------- the records-file seam (the dev-side feed) ----------------
//
// The dev boot cannot hold the build's `BundleOutput` in memory across
// rebuilds — the feed the cost endpoint's dev attach rides is the build's
// records PERSISTED beside the client manifest (`client.costs.json`), and
// the dev-side adapter re-reads the file when its mtime moves. The seam is
// the file: `write_source_costs` (the build and dev-bundle paths call it
// after a successful bundle) and `read_source_costs` (the boot's adapter),
// both over the same `{ source, bytes, gzip, chunk }` rows the wire
// contract serves.

/// A hand-built record — the file rows are the record, not a bundle
/// output, so the round-trip pins construct them directly.
fn record(source: &str, bytes: u64, gzip: u64, chunk: Option<&str>) -> SourceCost {
    SourceCost {
        source: source.to_string(),
        bytes,
        gzip,
        chunk: chunk.map(str::to_string),
    }
}

#[test]
fn records_file_roundtrips_the_records() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("client.costs.json");
    let records = vec![
        record("src/hero.tsx", 4812, 1901, Some("entry.index-abc123.js")),
        // The orphan shape: no chunk claims the module, the record still
        // costs bytes (`chunk: null` on the wire, `None` on read-back).
        record("src/orphan.tsx", 921, 430, None),
    ];
    write_source_costs(&records, &path).expect("write");
    let read_back = read_source_costs(&path).expect("read");
    assert_eq!(read_back, records, "the file round-trips the rows");
}

#[test]
fn records_file_write_is_deterministic() {
    // Two writes over the same records are byte-equal (the build's
    // determinism property rides the file: two builds over the same
    // output produce byte-equal `client.costs.json` files).
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("client.costs.json");
    let records = vec![
        record("src/hero.tsx", 4812, 1901, Some("entry.index-abc123.js")),
        record("src/counter.svelte", 921, 430, None),
    ];
    write_source_costs(&records, &path).expect("first write");
    let first = std::fs::read(&path).expect("read first");
    write_source_costs(&records, &path).expect("second write");
    let second = std::fs::read(&path).expect("read second");
    assert_eq!(first, second, "two writes, byte-equal file");
}

#[test]
fn records_file_write_creates_the_parent_dir() {
    // The dist/client dir exists at the call sites; the write still owns
    // the parent creation so a fresh dist never turns the feed into a
    // caller foot-gun.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("dist/client/client.costs.json");
    write_source_costs(&[record("src/hero.tsx", 10, 5, None)], &path).expect("write");
    assert!(path.is_file(), "the file exists under the created parent");
}

#[test]
fn records_file_read_missing_is_an_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("client.costs.json");
    let err = read_source_costs(&path).expect_err("a missing file is an error");
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
}

#[test]
fn records_file_read_garbage_is_invalid_data() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("client.costs.json");
    std::fs::write(&path, "not json at all").expect("write garbage");
    let err = read_source_costs(&path).expect_err("garbage is an error");
    assert_eq!(
        err.kind(),
        std::io::ErrorKind::InvalidData,
        "the parse failure surfaces as InvalidData: {err}"
    );
}
