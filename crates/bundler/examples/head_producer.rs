//! Consumer example for the adapter-level head producer (the V2-E04 tail):
//! the manifest-driven projection that turns a `ClientManifest` route
//! bundle into the modulepreload + stylesheet URLs a prerendered shell's
//! `<head>` should carry.
//!
//! Shows the three pieces the feature composes:
//!
//! 1. [`oxide_fw_bundler::head::HeadProducer`] — the seam: `name()` for
//!    the log surfaces plus `head_for(route)`, held behind `&dyn` by the
//!    CLI (the `PageRenderer` trait conventions).
//! 2. [`oxide_fw_bundler::head::ManifestHeadProducer`] — the
//!    manifest-driven implementation: the three-shape route-key
//!    resolution the CLI shim's `inject_route_css` uses (exact, `+ /page`,
//!    and the `_param_` sanitized dynamic form), critical-first preload
//!    order preserved, first-seen-order dedup.
//! 3. The wire-up contract: every emitted URL is `/client/<file>` — the
//!    exact spelling the render `Injector`'s `integrity_file_for_url`
//!    resolves to a digest in `ClientManifest.integrity`, so each link the
//!    producer emits is one the Injector stamps with `integrity` +
//!    `crossorigin` (when `security.sri` is on).
//!
//! Run it:
//!
//! ```text
//! cargo run -p oxide-fw-bundler --example head_producer
//! ```

use oxide_fw_bundler::head::{HeadProducer, ManifestHeadProducer};
use oxide_fw_bundler::{ClientManifest, RouteBundle};
use std::collections::BTreeMap;

fn main() {
    // A three-route manifest in the bundler's own key spelling: static,
    // dynamic (the `_param_` sanitized chunk form) and the `__boot__`
    // island entry — with the digest map the Injector reads.
    let mut routes = BTreeMap::new();
    routes.insert(
        "routes/index/page".to_string(),
        RouteBundle {
            entry_chunk: "index-a1b2c3d4.js".to_string(),
            preload: vec![
                "index-a1b2c3d4.js".to_string(),
                "shared-e5f6a7b8.js".to_string(),
            ],
            css: vec!["index-c9d0e1f2.css".to_string()],
            js_bytes: 120,
            modules: Vec::new(),
            layout_chain: Vec::new(),
        },
    );
    routes.insert(
        "routes/blog/_slug_/page".to_string(),
        RouteBundle {
            entry_chunk: "blog-0a1b2c3d.js".to_string(),
            preload: vec!["blog-0a1b2c3d.js".to_string()],
            css: vec!["blog-4e5f6a7b.css".to_string()],
            js_bytes: 200,
            modules: Vec::new(),
            layout_chain: Vec::new(),
        },
    );
    routes.insert(
        "__boot__".to_string(),
        RouteBundle {
            entry_chunk: "boot-1a2b3c4d.js".to_string(),
            preload: vec!["boot-1a2b3c4d.js".to_string()],
            css: Vec::new(),
            js_bytes: 30,
            modules: Vec::new(),
            layout_chain: Vec::new(),
        },
    );
    let manifest = ClientManifest {
        build_id: "example-build".to_string(),
        routes,
        assets: BTreeMap::new(),
        integrity: BTreeMap::new(),
    };

    // The CLI holds the producer behind `&dyn HeadProducer`:
    let producer: &dyn HeadProducer = &ManifestHeadProducer::new(manifest);

    for route in ["routes/index", "routes/blog/[slug]", "__boot__"] {
        println!("{route}:");
        match producer.head_for(route) {
            Some(links) => {
                for url in &links.preload {
                    println!("  <link rel=\"modulepreload\" href=\"{url}\">");
                }
                for url in &links.css {
                    println!("  <link rel=\"stylesheet\" href=\"{url}\">");
                }
            }
            None => println!("  (no bundle for this route — the shell stays as-is)"),
        }
    }

    println!(
        "\nproducer: {} — every URL above is `integrity`-stamped by the render \
         Injector when the manifest carries the digest and `security.sri` is on.",
        producer.name()
    );
}
