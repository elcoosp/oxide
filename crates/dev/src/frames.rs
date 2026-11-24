//! The §V2-E08 overlay's source-mapped stack frames — the seam between an
//! error's generated-output stack and the sources it came from (§V2-E08
//! item 1; §13.4's "source-mapped stacks").
//!
//! A runtime error in a pool-compiled module points at the GENERATED
//! output (`dist/.oxide/pool/comp.tsx:12:7`); the overlay must name the
//! app source (`src/comp.tsx:3:8`). The compile outputs already carry the
//! maps (`TransformOutput.map`, the oxc map every compile produces); until
//! this seam nothing consumed them for frames. [`map_diagnostic_frames`]
//! walks the diagnostic's [`StackFrame`] stack through a [`FrameMapper`]:
//! generated frames map to original coordinates and say so (`mapped:
//! true`), frames no map speaks for pass through honestly (the generated
//! position named verbatim, `mapped: false` — never fabricated), and
//! frames already carrying source coordinates are never mapped twice.
//!
//! [`NoMaps`] is the pass-through default spelled as a type (the
//! `AdmitAllKeys` shape): a caller without maps keeps today's payload
//! byte-identically. [`ModuleMaps`] is the production mapper over the
//! compiled modules' maps — the SAME oxc implementation the compile
//! outputs already carry, so no second sourcemap implementation enters
//! the graph.

use std::collections::BTreeMap;

use oxc_sourcemap::OwnedSourceMap;
use oxide_fw_diagnostics::{Diagnostic, StackFrame};

/// Where a generated position came from: the original source coordinates,
/// 1-based ([`StackFrame`]'s convention).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OriginalPosition {
    /// The original source path, as the map's `sources` entry names it.
    pub file: String,
    /// 1-based line in `file`.
    pub line: u32,
    /// 1-based column in `file`.
    pub column: u32,
}

/// The frame-mapping seam (§V2-E08 item 1): the lookup a boot implements
/// over the source maps it holds. Object-safe — the serve loop composes it
/// behind an `Arc<dyn FrameMapper>` (the `Arc<dyn PlanePeer>` shape).
pub trait FrameMapper: Send + Sync {
    /// Map one generated position (`line`/`column` 1-based) to its
    /// original source coordinates. `None` — no map speaks for `file` or
    /// the lookup misses; the caller keeps the generated position and says
    /// so. A mid-token column takes the nearest preceding token (the
    /// browsers' own remap semantics for a runtime column).
    fn original_of(&self, file: &str, line: u32, column: u32) -> Option<OriginalPosition>;
}

/// The pass-through default spelled as a type: no maps, no mappings, every
/// generated frame keeps its coordinates. The no-map caller's payload is
/// byte-identical to the pre-frames shape.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NoMaps;

impl FrameMapper for NoMaps {
    fn original_of(&self, _file: &str, _line: u32, _column: u32) -> Option<OriginalPosition> {
        None
    }
}

/// The production mapper: the compiled modules' maps keyed by the exact
/// generated module path the frames name. Built from the compile outputs'
/// `TransformOutput.map` values (the oxc map every lower produces); the
/// per-lookup table is oxc's own (`generate_lookup_table` +
/// `lookup_token_approx`) — the remap implementation stays oxc's, the same
/// crate the compile outputs carry.
#[derive(Debug, Default)]
pub struct ModuleMaps {
    maps: BTreeMap<String, OwnedSourceMap>,
}

impl ModuleMaps {
    /// An empty mapper — the honest no-map shape until a module's map
    /// arrives.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
    /// Register one module's map (builder style; the boot fills the store
    /// from the compile outputs it holds).
    #[must_use]
    pub fn with_module(mut self, module: impl Into<String>, map: OwnedSourceMap) -> Self {
        self.maps.insert(module.into(), map);
        self
    }
    /// Register one module's map on an existing store.
    pub fn insert(&mut self, module: impl Into<String>, map: OwnedSourceMap) {
        self.maps.insert(module.into(), map);
    }
}

impl FrameMapper for ModuleMaps {
    fn original_of(&self, file: &str, line: u32, column: u32) -> Option<OriginalPosition> {
        let map = self.maps.get(file)?;
        let table = map.generate_lookup_table();
        // StackFrame coordinates are 1-based (the crate's Location
        // convention); the map speaks 0-based.
        let token =
            map.lookup_token_approx(&table, line.saturating_sub(1), column.saturating_sub(1))?;
        let source = map.get_source(token.get_source_id()?)?;
        Some(OriginalPosition {
            file: source.to_string(),
            line: token.get_src_line().saturating_add(1),
            column: token.get_src_col().saturating_add(1),
        })
    }
}

/// Map one frame through the mapper. A source-coordinate frame
/// (`mapped: true`) is never touched — applying a generated→original map
/// to original coordinates would corrupt it; an unmapped frame with no
/// map (or a lookup miss) keeps the generated position and stays
/// `mapped: false`.
fn map_frame(frame: &StackFrame, mapper: &dyn FrameMapper) -> StackFrame {
    if frame.mapped {
        return frame.clone();
    }
    match mapper.original_of(&frame.file, frame.line, frame.column) {
        Some(pos) => StackFrame {
            file: pos.file,
            line: pos.line,
            column: pos.column,
            function: frame.function.clone(),
            mapped: true,
        },
        None => frame.clone(),
    }
}

/// Map a diagnostic's stack through the mapper (§V2-E08 item 1 — the ONE
/// mapping seam the overlay path runs). A frame-less diagnostic passes
/// through unchanged (a clone, no remap work); every generated frame
/// either names its original source (`mapped: true`) or keeps the
/// generated position honestly (`mapped: false`).
#[must_use]
pub fn map_diagnostic_frames(diag: &Diagnostic, mapper: &dyn FrameMapper) -> Diagnostic {
    if diag.frames.is_empty() {
        return diag.clone();
    }
    let mut out = diag.clone();
    out.frames = diag.frames.iter().map(|f| map_frame(f, mapper)).collect();
    out
}

/// The §13.4 overlay payload with the stack mapped: the mapped diagnostic
/// serialized as JSON. The frame-less payload is byte-identical to
/// [`Diagnostic::to_json`] by construction — the field skips while empty
/// and [`NoMaps`] never adds rows.
#[must_use]
pub fn overlay_payload_with_maps(diag: &Diagnostic, mapper: &dyn FrameMapper) -> String {
    map_diagnostic_frames(diag, mapper).to_json()
}
