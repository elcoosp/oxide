//! V2-E04 (SRI half): sha384 subresource-integrity digests for the emitted
//! client chunks (spec §V2-E04: "At client bundle end, compute
//! `sha384-<base64>` for every emitted JS and CSS chunk and store it in
//! `client.manifest.json` as `integrity: { file: value }`").
//!
//! The map is keyed by the client-dir-relative file name — the same
//! spelling `RouteBundle.entry_chunk` / `preload` / `css` use — so a
//! consumer resolves a digest with one lookup and no path munging. The
//! bundler always records the digests (the emitted bytes are already in
//! hand; the cost is one SHA-384 pass per chunk); the head-tag injection
//! is gated by `security.sri` in the CLI.

use sha2::Digest;
use std::collections::BTreeMap;
use std::path::Path;

/// The SRI digest form the head tags and the manifest carry:
/// `sha384-<standard base64>` over the exact emitted bytes.
#[must_use]
pub fn sri_digest(body: &[u8]) -> String {
    use base64::Engine as _;
    format!(
        "sha384-{}",
        base64::engine::general_purpose::STANDARD.encode(sha2::Sha384::digest(body))
    )
}

/// Digest every named file under `out_dir` (the emitted JS chunks and CSS
/// assets). A file that fails to read is skipped with a `tracing::warn!` —
/// the bundler only names files it just wrote, so a miss means an
/// environment-level IO failure, not a content problem, and the manifest
/// simply carries fewer entries (the head injector injects nothing for a
/// file it cannot find a digest for).
#[must_use]
pub fn integrity_map(
    out_dir: &Path,
    files: impl IntoIterator<Item = impl AsRef<str>>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for file in files {
        let name = file.as_ref();
        match std::fs::read(out_dir.join(name)) {
            Ok(body) => {
                out.insert(name.to_string(), sri_digest(&body));
            }
            Err(e) => {
                tracing::warn!(file = %name, error = %e, "sri: emitted file unreadable, no digest recorded");
            }
        }
    }
    out
}

#[cfg(test)]
mod sri_tests {
    use super::*;
    use crate::{BundleEnv, BundleOptions, Bundler, ChunkBundler};
    use std::collections::BTreeMap as Map;

    /// Write the §6.4 mini fixture: one page entry importing a CSS module.
    fn fixture(root: &Path) {
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
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
    }

    fn opts(root: &Path) -> BundleOptions {
        let mut entries = Map::new();
        entries.insert("index".to_string(), root.join("src/routes/index/page.tsx"));
        BundleOptions {
            env: BundleEnv::Client,
            entries,
            out_dir: root.join("dist/client"),
            minify: false,
            sourcemap: false,
            build_secret: "test-secret".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn sri_digest_matches_the_sha384_known_answer() {
        // Independent known-answer vector (python hashlib.sha384) — pins the
        // digest FORM (`sha384-` + standard padded base64), not just
        // self-consistency.
        assert_eq!(
            sri_digest(b"oxide-sri-known-answer"),
            "sha384-TaLTMFHfY6sTuqLLMg+Zm7Ev+NiiDsjI3P5a7fH6ztJDuNak4csE09KYeF784D3I"
        );
    }

    #[test]
    fn sri_digest_is_deterministic_and_content_sensitive() {
        assert_eq!(sri_digest(b"same"), sri_digest(b"same"));
        assert_ne!(sri_digest(b"same"), sri_digest(b"other"));
    }

    #[test]
    fn client_manifest_carries_an_integrity_entry_for_every_emitted_js_and_css_file() {
        // §V2-E04 acceptance: the manifest stores the digests keyed by file.
        let dir = tempfile::tempdir().unwrap();
        fixture(dir.path());
        let out = ChunkBundler::new(dir.path())
            .bundle(&opts(dir.path()))
            .unwrap();
        let manifest = out.client_manifest.expect("client manifest");
        assert!(
            !manifest.integrity.is_empty(),
            "the client manifest records integrity digests"
        );

        // Every emitted JS chunk and CSS file has an entry…
        for c in &out.chunks {
            assert!(
                manifest.integrity.contains_key(&c.file),
                "chunk {} has no integrity entry",
                c.file
            );
            for css in &c.css {
                assert!(
                    manifest.integrity.contains_key(css),
                    "css {css} has no integrity entry"
                );
            }
        }
        // …and only emitted JS/CSS files do (no `.map`, no stray keys).
        for key in manifest.integrity.keys() {
            let ext = std::path::Path::new(key)
                .extension()
                .and_then(|e| e.to_str());
            assert!(
                matches!(ext, Some("js" | "css")),
                "unexpected integrity key {key}"
            );
        }

        // The digests are the sha384 of the exact bytes on disk — recomputed
        // here independently of `integrity_map`'s file walk.
        for (file, digest) in &manifest.integrity {
            let body = std::fs::read(dir.path().join("dist/client").join(file)).unwrap();
            assert_eq!(&sri_digest(&body), digest, "digest of {file} matches disk");
        }

        // The manifest round-trips through serde with the field intact (the
        // CLI reads the manifest as a JSON `Value`).
        let json = serde_json::to_string(&manifest).unwrap();
        let back: crate::ClientManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.integrity, manifest.integrity);
    }

    #[test]
    fn integrity_map_warns_and_skips_a_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("real.js"), b"console.log(1)").unwrap();
        let map = integrity_map(dir.path(), ["real.js", "ghost.js"]);
        assert_eq!(map.len(), 1, "only the readable file is recorded");
        assert_eq!(map.get("real.js").unwrap(), &sri_digest(b"console.log(1)"));
    }
}
