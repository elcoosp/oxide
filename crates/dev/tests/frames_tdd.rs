//! TDD pins for the §V2-E08 overlay's source-mapped stack frames — the
//! recorded remaining half of item 1 (the open-in-editor endpoint landed as
//! `feat/open-in-editor-endpoint`; the frames did not exist anywhere in the
//! tree before this seam).
//!
//! Written FIRST (red): failed to compile before `dev::frames` existed and
//! `Diagnostic` carried no `frames` field.
//!
//! The contract (spec §V2-E08 item 1 — "**Overlay:** source-mapped stack
//! frames"; spec §13.4 — "Errors are Rust `miette` diagnostics serialized
//! as JSON (code frames, **source-mapped stacks**, hints with fixes) and
//! drawn by a small overlay"):
//! - a diagnostic's stack frames in generated-output coordinates map back
//!   to the ORIGINAL source coordinates through the module's source map —
//!   the payload the overlay draws carries source frames, not bundle
//!   frames;
//! - a frame with no map speaking for its file (or a lookup miss) passes
//!   through honestly, still naming the generated position — mapped
//!   `false`, never fabricated;
//! - a frame already carrying source coordinates (`mapped: true` — a
//!   producer that knew its provenance) is never mapped twice;
//! - the frame-less payload stays byte-identical to the pre-frames shape
//!   (the default-shape doctrine every seam here keeps), and pre-frames
//!   payloads still decode (`serde(default)`).

use oxide_fw_dev::frames::{FrameMapper, ModuleMaps, NoMaps, OriginalPosition};
use oxide_fw_dev::{HmrMessage, overlay_payload, overlay_payload_with_maps};
use oxide_fw_diagnostics::{Diagnostic, Location, StackFrame};

use oxc_sourcemap::Token;

/// The generated module the frames point into (the dev-pool output path
/// spelling) and the original source it was compiled from.
const GEN_MODULE: &str = "dist/.oxide/pool/comp.tsx";
const SRC_MODULE: &str = "src/comp.tsx";

/// Build the module map the way the compile outputs carry one: an
/// `OwnedSourceMap` serialized from tokens, as `TransformOutput.map` holds.
/// Generated lines 0.. hold one token each mapping column 0 of generated
/// line `g` to source line `g * 2`, column 4 (1-based: line g+1 → g*2+1,
/// column 5).
fn module_map() -> oxc_sourcemap::OwnedSourceMap {
    let tokens: Vec<Token> = (0..8)
        .map(|g| Token::new(g, 0, g * 2, 4, Some(0), None))
        .collect();
    let map = oxc_sourcemap::SourceMap::new(
        None,
        Vec::new(),
        None,
        vec![SRC_MODULE.into()],
        vec![None],
        tokens.into_boxed_slice(),
        None,
    );
    oxc_sourcemap::OwnedSourceMap::new(map)
}

/// The mapper with exactly one module map keyed by the generated path.
fn maps() -> ModuleMaps {
    ModuleMaps::new().with_module(GEN_MODULE, module_map())
}

/// A generated-coordinate frame: generated line 4 (1-based), column 1 —
/// inside the line-3 token's span (`lookup_token_approx` takes the greatest
/// lower bound, the browsers' own remap semantics for a mid-line runtime
/// column).
fn generated_frame(function: &str) -> StackFrame {
    StackFrame {
        file: GEN_MODULE.to_string(),
        line: 4,
        column: 1,
        function: Some(function.to_string()),
        mapped: false,
    }
}

fn diag_with(frame: StackFrame) -> Diagnostic {
    Diagnostic::error("RT-POOL", "boom in the pooled render")
        .at(Location::file(&frame.file, frame.line, frame.column))
        .with_frames(vec![frame])
}

/// THE plan pin (§V2-E08 item 1 + §13.4): the overlay payload carries the
/// diagnostic's stack frames MAPPED to source coordinates — the frame the
/// runtime reported against `dist/.oxide/pool/comp.tsx:4:1` names
/// `src/comp.tsx:7:5` in the payload, `mapped: true`.
#[test]
fn payload_carries_source_mapped_frames() {
    let d = diag_with(generated_frame("Counter"));
    let payload = overlay_payload_with_maps(&d, &maps());
    let back = Diagnostic::from_json(&payload).expect("payload decodes");
    let frames = back.frames;
    assert_eq!(frames.len(), 1, "the frame rides the payload");
    let f = &frames[0];
    assert_eq!(f.file, SRC_MODULE, "the ORIGINAL file names the source");
    assert_eq!(f.line, 7, "generated line 4 → source line 3*2+1");
    assert_eq!(f.column, 5, "source column 4 → 1-based 5");
    assert!(f.mapped, "the frame carries its provenance");
    assert_eq!(
        f.function.as_deref(),
        Some("Counter"),
        "the function name survives the map"
    );
}

/// The honesty pin: a frame whose file no map speaks for (and a lookup miss
/// inside a mapped module) passes through unmapped — the generated position
/// named verbatim, `mapped: false`, never fabricated.
#[test]
fn unmapped_frames_pass_through_honestly() {
    let stranger = StackFrame {
        file: "node_modules/.vite/deps/react.js".to_string(),
        line: 41,
        column: 9,
        function: None,
        mapped: false,
    };
    let beyond = StackFrame {
        file: GEN_MODULE.to_string(),
        line: 64,
        column: 1, // generated line 63 has no token (the map holds 0..8)
        function: None,
        mapped: false,
    };
    let d = Diagnostic::error("RT-POOL", "two frames").with_frames(vec![stranger, beyond]);
    let back = Diagnostic::from_json(&overlay_payload_with_maps(&d, &maps())).unwrap();
    let frames = &back.frames;
    assert_eq!(frames.len(), 2);
    assert_eq!(
        frames[0].file, "node_modules/.vite/deps/react.js",
        "verbatim"
    );
    assert!(!frames[0].mapped, "no map for the file → unmapped");
    assert_eq!(
        frames[1].file, GEN_MODULE,
        "the map's module, still unmapped"
    );
    assert_eq!(frames[1].line, 64, "the miss keeps the generated line");
    assert!(!frames[1].mapped, "a lookup miss is honest, not fabricated");
}

/// The default-shape pin: a frame-less diagnostic's payload is
/// byte-identical to `Diagnostic::to_json` — the field does not appear, the
/// mapper never invents rows, every existing payload keeps its bytes.
#[test]
fn frameless_payload_is_byte_identical() {
    let d = Diagnostic::error("CFG-001", "unknown config key")
        .at(Location::file("oxide.config.ts", 3, 3))
        .with_fix("did you mean `framework`?");
    let plain = d.to_json();
    assert_eq!(
        overlay_payload_with_maps(&d, &maps()),
        plain,
        "the mapped payload of a frame-less diagnostic is the plain payload"
    );
    assert_eq!(
        overlay_payload(&d),
        plain,
        "the unmapped payload entry point is the plain payload too"
    );
    assert!(!plain.contains("frames"), "no field, no bytes");
}

/// `NoMaps` is the pass-through default spelled as a type (the
/// `AdmitAllKeys` shape): every generated frame keeps its coordinates.
#[test]
fn no_maps_is_the_pass_through_default() {
    let d = diag_with(generated_frame("Counter"));
    let back = Diagnostic::from_json(&overlay_payload_with_maps(&d, &NoMaps)).unwrap();
    let f = &back.frames[0];
    assert_eq!(f.file, GEN_MODULE);
    assert_eq!(f.line, 4);
    assert_eq!(f.column, 1);
    assert!(!f.mapped);
}

/// Back-compat pin: a pre-frames payload decodes with an empty frame list
/// (`serde(default)`) — the producers that never carried frames keep
/// working, on the wire and in the pins.
#[test]
fn pre_frames_payloads_decode() {
    let d = Diagnostic::error("CFG-001", "old shape").at(Location::file("oxide.config.ts", 1, 1));
    let old = d.to_json();
    assert!(!old.contains("frames"));
    let back = Diagnostic::from_json(&old).unwrap();
    assert!(back.frames.is_empty(), "the default is the empty stack");
    let empty = Diagnostic::from_json(
        "{\"code\":\"CFG-001\",\"severity\":\"error\",\"message\":\"m\",\"location\":null,\"code_frame\":null,\"causes\":[],\"fix\":null,\"frames\":[]}",
    )
    .unwrap();
    assert_eq!(empty.frames, Vec::<StackFrame>::new());
}

/// The no-double-map pin: a frame already carrying source coordinates
/// (`mapped: true`) passes through UNTOUCHED — the mapper never applies a
/// generated→original map to original coordinates.
#[test]
fn already_mapped_frames_never_remap() {
    let source_native = StackFrame {
        file: SRC_MODULE.to_string(),
        line: 7,
        column: 5,
        function: Some("Counter".to_string()),
        mapped: true,
    };
    let d = diag_with(source_native);
    let back = Diagnostic::from_json(&overlay_payload_with_maps(&d, &maps())).unwrap();
    let f = &back.frames[0];
    assert_eq!(f.file, SRC_MODULE);
    assert_eq!(f.line, 7, "source coordinates survive verbatim");
    assert_eq!(f.column, 5);
    assert!(f.mapped, "the provenance the producer stamped stands");
}

/// The remap-semantics pin: a runtime column between tokens takes the
/// nearest PRECEDING token (the browsers' own `lookup_token_approx`
/// semantics) — a column inside a mapped line maps, not just exact hits.
#[test]
fn approx_column_maps_to_the_token() {
    let mapper = maps();
    // Generated line 4 (0-based 3) holds its token at column 0; a runtime
    // column mid-line (1-based 12) still maps through that token.
    let pos = mapper
        .original_of(GEN_MODULE, 4, 12)
        .expect("a mid-line column maps");
    assert_eq!(pos.file, SRC_MODULE);
    assert_eq!(pos.line, 7);
    assert_eq!(pos.column, 5);
    // A column BEFORE the line's token maps too (the approx floor).
    let pos = mapper
        .original_of(GEN_MODULE, 1, 1)
        .expect("the first line's token maps");
    assert_eq!(pos.line, 1, "generated line 1 → source line 1");
}

/// The wire-up pin: the HMR error frame carries the diagnostic verbatim —
/// the mapping is the payload builder's job (`overlay_payload_with_maps`),
/// so the v1 envelope round-trips a mapped diagnostic unchanged and the
/// mapped frames ride it.
#[test]
fn hmr_error_frame_roundtrips_mapped_frames() {
    let d = map_diagnostic_frames(&diag_with(generated_frame("Counter")), &maps());
    let msg = HmrMessage::Error { diagnostic: d };
    let j = msg.to_json();
    let back = HmrMessage::try_from_json(&j).expect("the v1 gate accepts");
    match back {
        HmrMessage::Error { diagnostic } => {
            let f = &diagnostic.frames[0];
            assert_eq!(f.file, SRC_MODULE, "the mapped frame rides the wire");
            assert!(f.mapped);
        }
        other => panic!("wrong frame: {other:?}"),
    }
}

use oxide_fw_dev::frames::map_diagnostic_frames;

#[test]
fn mapper_trait_object_shape() {
    // The seam is object-safe: a boot composes it behind an `Arc<dyn
    // FrameMapper>` (the `Arc<dyn PlanePeer>` shape).
    let mapper: std::sync::Arc<dyn FrameMapper> = std::sync::Arc::new(maps());
    let pos = mapper
        .original_of(GEN_MODULE, 2, 1)
        .expect("the trait object maps");
    assert_eq!(
        pos,
        OriginalPosition {
            file: SRC_MODULE.to_string(),
            line: 3,
            column: 5,
        }
    );
}
