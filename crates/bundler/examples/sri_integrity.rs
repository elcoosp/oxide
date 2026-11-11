//! Consumer example for the V2-E04 SRI seam: the sha384 subresource-integrity
//! digests every emitted client chunk carries in `client.manifest.json`
//! (`integrity: { file: value }`).
//!
//! Shows the three pieces the feature composes:
//!
//! 1. [`oxide_fw_bundler::sri::sri_digest`] — the digest form
//!    (`sha384-<standard base64>`) over the exact emitted bytes; a known
//!    answer pins the spelling.
//! 2. [`oxide_fw_bundler::sri::integrity_map`] — the rolldown path's
//!    read-back walk: digest every named file under `out_dir`; a file that
//!    fails to read is skipped with a `tracing::warn!` (the manifest simply
//!    carries fewer entries).
//! 3. [`oxide_fw_bundler::ChunkBundler`] — the chunk path digests memory
//!    (the JS bytes are in `bodies`, the CSS bytes collected during emit),
//!    so the recorded digest is byte-exact with what was just written; the
//!    `integrity` map then rides `ClientManifest` like `routes`/`assets`.
//!
//! The CLI stamps the boot `<script type=module>` tag with the boot chunk's
//! digest (`integrity` + `crossorigin="anonymous"`) when `security.sri` is
//! on (the default), and `oxide-fw-render`'s `Injector` carries the map for
//! modulepreload/stylesheet links. Run it:
//!
//! ```text
//! cargo run -p oxide-fw-bundler --example sri_integrity
//! ```

use oxide_fw_bundler::{BundleEnv, BundleOptions, Bundler, ChunkBundler};
use std::collections::BTreeMap;

fn main() {
    // 1. The digest form, pinned to an independent known-answer vector:
    println!(
        "sri_digest(b\"oxide-sri-known-answer\") = {}",
        oxide_fw_bundler::sri::sri_digest(b"oxide-sri-known-answer")
    );

    // 2 + 3. Bundle the §6.4 mini fixture (one page entry importing a CSS
    // module) and read the manifest's integrity map the browser (and a CDN)
    // verify against.
    let dir = tempfile::tempdir().expect("tempdir");
    for (rel, body) in [
        (
            "src/routes/index/page.tsx",
            "import \"./card.css\";\nexport default function Page() {\n  return \"home\";\n}\n",
        ),
        (
            "src/routes/index/card.css",
            ".card { color: rebeccapurple; }\n",
        ),
    ] {
        let p = dir.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }
    let mut entries = BTreeMap::new();
    entries.insert(
        "index".to_string(),
        dir.path().join("src/routes/index/page.tsx"),
    );
    let out = ChunkBundler::new(dir.path())
        .bundle(&BundleOptions {
            env: BundleEnv::Client,
            entries,
            out_dir: dir.path().join("dist/client"),
            minify: false,
            sourcemap: false,
            build_secret: "example-secret".to_string(),
            ..Default::default()
        })
        .expect("bundle");

    let manifest = out.client_manifest.expect("client manifest");
    println!(
        "\nclient.manifest.json integrity ({} files):",
        manifest.integrity.len()
    );
    for (file, digest) in &manifest.integrity {
        println!("  {file} — {digest}");
    }

    // The digests are the sha384 of the exact bytes on disk — recompute one
    // independently of the bundler pass:
    if let Some((file, _)) = manifest.integrity.iter().next() {
        let body = std::fs::read(dir.path().join("dist/client").join(file)).unwrap();
        println!(
            "\nrecomputed {} matches the manifest: {}",
            file,
            oxide_fw_bundler::sri::sri_digest(&body) == manifest.integrity[file]
        );
    }
}
