//! TDD suite for the V2-A09 prerender module-source seam (`module_source_tdd`).
//!
//! SPEC-17 remainder (spec §V2-A09): the prerender path renders through the
//! page renderer's module loader, which imports SOURCE modules — so a plugin
//! transform that only ships in the build-emitted server bundle never reaches
//! the prerendered HTML. The WP's fix threads a `ModuleSource` knob through
//! the framework renderer's config: `phase_prerender` constructs the renderer
//! with `ModuleSource::Bundle(dist/server)`, dev and `oxide render` keep
//! `Source`, and the worker's real-ssr loader imports from the bundle path
//! when given.
//!
//! These pins hold the Rust half of that contract (the V8-free substrate —
//! the worker glue and the SPEC-17 fixture assertion ride CI):
//!
//! 1. [`module_source_defaults_to_source_and_the_wire_stays_shaped`] — a
//!    pre-A09 payload (no module-source keys) parses as `Source`, `new()`
//!    keeps `Source`, and a `Source` render emits NO module-source keys, so
//!    the wire every existing worker sees is byte-identical.
//! 2. [`bundle_source_names_the_bundle_dir_in_the_worker_payload`] — a
//!    `Bundle(dir)` render stamps `moduleSource: "bundle"` and the bundle
//!    directory into the payload the worker's loader reads.
//! 3. [`module_source_round_trips_through_the_render_input`] — the field
//!    survives the serde round-trip the host↔worker boundary performs.
//!
//! No worker, no clock, no network: pure shapes.

use oxide_fw_jshost::{FrameworkRenderInput, ModuleSource};
use std::path::PathBuf;

/// The default is `Source` and the wire does not change for it: a payload
/// the pre-A09 pipeline produced keeps parsing, `FrameworkRenderInput::new`
/// (the dev / `oxide render` shape) stays `Source`, and a `Source` render
/// emits neither `moduleSource` nor `moduleDir` — the worker payload is
/// byte-identical to the pre-A09 shape.
#[test]
fn module_source_defaults_to_source_and_the_wire_stays_shaped() {
    // A pre-A09 payload (the field absent) parses with the default. The
    // required §7 fields ride along; only `module_source` is additive.
    let input: FrameworkRenderInput = serde_json::from_str(
        r#"{"route":"routes/index","url":"/","params":{},"data":{},"head":{}}"#,
    )
    .expect("parse a pre-A09 payload");
    assert_eq!(input.module_source, ModuleSource::Source);

    // The convenience constructor — the dev and `oxide render` shape —
    // keeps the default.
    let input = FrameworkRenderInput::new("routes/index", "/");
    assert_eq!(input.module_source, ModuleSource::Source);

    // `Source` emits NO module-source keys: the worker payload the
    // existing worker pool receives is unchanged.
    let payload = input.to_payload();
    assert!(
        payload.get("moduleSource").is_none(),
        "Source must not emit `moduleSource`"
    );
    assert!(
        payload.get("moduleDir").is_none(),
        "Source must not emit `moduleDir`"
    );
}

/// A `Bundle(dir)` render names the bundle in the payload: `moduleSource`
/// is the `"bundle"` kind and `moduleDir` is the directory the worker's
/// real-ssr loader imports from (the build-emitted server bundle).
#[test]
fn bundle_source_names_the_bundle_dir_in_the_worker_payload() {
    let mut input = FrameworkRenderInput::new("routes/boom", "/boom");
    input.module_source = ModuleSource::Bundle(PathBuf::from("dist/server"));

    let payload = input.to_payload();
    assert_eq!(
        payload.get("moduleSource"),
        Some(&serde_json::json!("bundle")),
        "the kind must ride the payload"
    );
    assert_eq!(
        payload.get("moduleDir"),
        Some(&serde_json::json!("dist/server")),
        "the bundle directory must ride the payload"
    );
}

/// The field survives the serde round-trip the host↔worker boundary
/// performs, and an explicit `Source` still round-trips (it never emits
/// the absent-field ambiguity back as something else).
#[test]
fn module_source_round_trips_through_the_render_input() {
    let mut input = FrameworkRenderInput::new("routes/index", "/");
    input.module_source = ModuleSource::Bundle(PathBuf::from("/abs/project/dist/server"));

    let bytes = serde_json::to_string(&input).expect("serialize the render input");
    let back: FrameworkRenderInput = serde_json::from_str(&bytes).expect("deserialize");
    assert_eq!(back, input, "the render input round-trips");
    assert_eq!(
        back.module_source,
        ModuleSource::Bundle(PathBuf::from("/abs/project/dist/server")),
        "the bundle source survives the round-trip"
    );

    // An explicit `Source` round-trips as `Source`.
    let mut input = FrameworkRenderInput::new("routes/index", "/");
    input.module_source = ModuleSource::Source;
    let bytes = serde_json::to_string(&input).expect("serialize the render input");
    let back: FrameworkRenderInput = serde_json::from_str(&bytes).expect("deserialize");
    assert_eq!(back.module_source, ModuleSource::Source);
}
