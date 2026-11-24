//! The §V2-E08 overlay's source-mapped stack frames walkthrough (item 1's
//! frame half — the seam the error path runs per frame): a module's source
//! map registered on the mapper, a runtime diagnostic carrying
//! generated-output stack frames, and the mapped §13.4 payloads — one
//! frame mapped to its original source, one left honest where no map
//! speaks for its file.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-dev --example overlay_frames
//! ```

use oxc_sourcemap::{OwnedSourceMap, SourceMap, Token};
use oxide_fw_dev::frames::{ModuleMaps, map_diagnostic_frames, overlay_payload_with_maps};
use oxide_fw_dev::overlay_payload;
use oxide_fw_diagnostics::{Diagnostic, StackFrame};

/// The generated module the pooled render reports (the dev-pool output
/// path) and the source it came from.
const GEN_MODULE: &str = "dist/.oxide/pool/counter.tsx";
const SRC_MODULE: &str = "src/components/counter.tsx";

/// The module's map the way the compile output carries it: tokens from
/// generated coordinates back to source coordinates (the bench/test
/// helper builds the same shape — `TransformOutput.map` in production).
fn counter_map() -> OwnedSourceMap {
    let tokens: Vec<Token> = (0..6)
        .map(|g| Token::new(g, 0, g * 3, 8, Some(0), None))
        .collect();
    OwnedSourceMap::new(SourceMap::new(
        None,
        Vec::new(),
        None,
        vec![SRC_MODULE.into()],
        vec![None],
        tokens.into_boxed_slice(),
        None,
    ))
}

fn main() {
    // 1. The mapper: the compile outputs' maps keyed by the exact
    //    generated module path the frames name (`ModuleMaps::with_module`;
    //    the boot fills the store from the `TransformOutput.map` values it
    //    already holds).
    let maps = ModuleMaps::new().with_module(GEN_MODULE, counter_map());

    // 2. The runtime diagnostic: the pooled render threw inside the
    //    compiled module — the stack the host carried names the GENERATED
    //    output (`mapped: false`, the coordinate system the error saw).
    let generated = StackFrame {
        file: GEN_MODULE.to_string(),
        line: 4,
        column: 17,
        function: Some("Counter".to_string()),
        mapped: false,
    };
    // A second frame from a dependency no map speaks for (a prebundled
    // dep): the honest unmapped arm.
    let stranger = StackFrame {
        file: "node_modules/.vite/deps/react.js".to_string(),
        line: 41,
        column: 9,
        function: None,
        mapped: false,
    };
    let diag = Diagnostic::error("RT-POOL", "cannot read properties of undefined")
        .with_frames(vec![generated, stranger]);

    // 3. The two payload entry points (§13.4): the unmapped default keeps
    //    the pre-frames bytes; the mapped builder runs the stack through
    //    the mapper before serializing.
    println!("== the unmapped payload (the NoMaps default) ==");
    println!("{}", overlay_payload(&diag));
    println!();
    println!("== the mapped payload (§V2-E08 item 1) ==");
    println!("{}", overlay_payload_with_maps(&diag, &maps));

    // 4. The mapper's own table: what mapped and what did not — a mapped
    //    frame names its original source (`mapped: true`), the stranger
    //    keeps its generated position verbatim (`mapped: false`), and the
    //    function names survive the map.
    let mapped = map_diagnostic_frames(&diag, &maps);
    println!();
    println!("== the stack the overlay draws ==");
    for f in &mapped.frames {
        let name = f.function.as_deref().unwrap_or("?");
        let origin = if f.mapped { "source" } else { "generated" };
        println!(
            "  at {}:{}:{} ({name}) [{origin}]",
            f.file, f.line, f.column
        );
    }
    println!("\nsuccess: the pooled error's stack names src/components/counter.tsx");
}
