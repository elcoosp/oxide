//! The §V2-F17 build-side cost engine — the per-source
//! `{ bytes, gzip, chunk }` records the landed
//! `GET /__oxide/cost?src=<path>` endpoint serves
//! (`oxide-fw-server::cost_endpoint` is the wire contract; this module
//! is the data): for every client module the bundle output carries, the
//! spec's measurement method runs and the record lands in the same
//! deterministic order every build.
//!
//! The spec's method, implemented verbatim: the byte cost is the
//! module's source length, and the gzip estimate is the length of the
//! REAL RFC 1952 stream over the same bytes — "gzip estimated by
//! compressing each module's rendered code". The module's post-walk
//! source is the estimate basis the record labels honestly: the in-repo
//! bundler carries the walked source (not the minified chunk text), so
//! the overlay's per-component bars compare sources against sources and
//! the endpoint's `chunk` field carries the wire-level truth
//! (`entry.index-abc123.js`) the bytes estimate feeds toward.
//!
//! The `source` key is the ROOT-RELATIVE spelling with forward slashes —
//! the same spelling the `data-ox-src` stamp
//! (`oxide-fw-compiler::data_src`) carries on the DOM and the endpoint's
//! `?src=` query takes, so the overlay's fetch needs no re-spelling. A
//! module outside `root` (a vendored dependency) rides as given and
//! stays fetchable; the CLI adapter decides whether its record is
//! worth serving.
//!
//! Assets and server-only modules are excluded — the overlay attributes
//! CLIENT sources, and a server module never reaches the browser. The
//! records are sorted by the `source` key: two builds over the same
//! output produce byte-equal tables, the determinism property the
//! snapshot-style consumers hold.
//!
//! The render-time half of the spec's record (`renderMsP50`) and the
//! cache tier (`cache`) are NOT computed here — they belong to the
//! render profiler and the cache tier map, the adapters that feed the
//! endpoint's `CostSource` alongside this engine's records. The module
//! never touches the filesystem and never refuses: the build pipeline
//! calls it once per bundle and ships what comes back.

use std::io::Write as _;
use std::path::Path;

use flate2::Compression;
use flate2::write::GzEncoder;
use serde::{Deserialize, Serialize};
use tracing::debug_span;

use crate::{BundleOutput, Module};

/// Persist the cost records beside the client manifest (the dev-side feed
/// the cost endpoint's boot adapter re-reads): pretty JSON, one array of
/// the `{ source, bytes, gzip, chunk }` rows in the given order — the
/// [`source_costs`] sorted order rides through — plus a trailing newline.
/// The parent directory is created when missing (a fresh `dist/` never
/// turns the feed into a caller foot-gun); two writes over the same
/// records are byte-equal, the build's determinism property riding the
/// file (`client.costs.json` is byte-stable across identical builds).
///
/// # Errors
///
/// The parent-directory create or the file write failing surfaces as the
/// `std::io::Error` the caller reports (the build and dev-bundle paths
/// warn and continue — the endpoint answers its documented 404s without
/// the file).
pub fn write_source_costs(records: &[SourceCost], path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(records)
        .map_err(|e| std::io::Error::other(format!("cost records serialize failed: {e}")))?;
    std::fs::write(path, format!("{json}\n"))
}

/// Read a cost-records file back (the boot adapter's load): the rows in
/// file order. The shape is exactly what [`write_source_costs`] wrote —
/// the same rows the `?src=` query and the `data-ox-src` stamp spell.
///
/// # Errors
///
/// A missing file is `NotFound`; an unparsable file is `InvalidData`
/// carrying the serde message — the dev-side adapter answers the
/// endpoint's documented 404s on both and retries on the next mtime move
/// instead of crashing the dev loop.
pub fn read_source_costs(path: &Path) -> std::io::Result<Vec<SourceCost>> {
    let raw = std::fs::read_to_string(path)?;
    serde_json::from_str(&raw).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("cost records parse failed: {e}"),
        )
    })
}

/// One client module's build-side cost record — the `{ bytes, gzip,
/// chunk }` triple the cost endpoint serves under the module's
/// root-relative `source` key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceCost {
    /// The module's root-relative path with forward slashes (the
    /// endpoint's `?src=` spelling; a path outside `root` rides as
    /// given).
    pub source: String,
    /// The carried source's byte length (the estimate basis — the
    /// module doc records what that source is).
    pub bytes: u64,
    /// The RFC 1952 gzip-stream length over the same bytes (the spec's
    /// estimation method; a REAL stream length, not a ratio guess).
    pub gzip: u64,
    /// The file of the FIRST chunk listing the module (`None` when no
    /// chunk claims it — an orphan module still costs bytes).
    pub chunk: Option<String>,
}

/// Compute every client module's cost record over a bundle output,
/// sorted by the `source` key (two calls over the same output are
/// byte-equal). `root` is the project root the `source` spelling strips
/// to; modules outside it ride with their absolute path.
#[must_use]
pub fn source_costs(output: &BundleOutput, root: &Path) -> Vec<SourceCost> {
    let _span = debug_span!("source_costs").entered();
    let mut costs: Vec<SourceCost> = output
        .modules
        .iter()
        .filter(|module: &&Module| !module.is_asset && !module.server_only)
        .map(|module| SourceCost {
            source: relative_spelling(&module.resolved, root),
            bytes: u64::try_from(module.source.len()).unwrap_or(u64::MAX),
            gzip: gzip_len(&module.source),
            chunk: output
                .chunks
                .iter()
                .find(|chunk| chunk.modules.contains(&module.resolved))
                .map(|chunk| chunk.file.clone()),
        })
        .collect();
    costs.sort_by(|a, b| a.source.cmp(&b.source));
    costs
}

/// The gzip-stream length over one source (the spec's method). The
/// encoder writes into a `Vec`, which cannot fail; a failure would
/// produce the honest 0 rather than a fabricated estimate.
fn gzip_len(source: &str) -> u64 {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    if encoder.write_all(source.as_bytes()).is_err() {
        return 0;
    }
    encoder.finish().map_or(0, |compressed| {
        u64::try_from(compressed.len()).unwrap_or(u64::MAX)
    })
}

/// The root-relative spelling with forward slashes (the `?src=`
/// contract); a module outside `root` rides as given.
fn relative_spelling(resolved: &Path, root: &Path) -> String {
    resolved
        .strip_prefix(root)
        .unwrap_or(resolved)
        .to_string_lossy()
        .replace('\\', "/")
}
