//! TDD pins for the V2-F20 "packages that reached a bundle" SBOM filter
//! (`oxide_fw_cli::attest`, spec §V2-F20). The substrate's own
//! `crates/cli/src/attest.rs` comment names the deferral verbatim:
//!
//! > "The 'packages that reached a bundle' manifest filter is the
//! > substrate's documented deferral — every locked package is listed."
//!
//! And `docs/deviations.md` delta 5 names the seam the filter closes:
//!
//! > "Intersecting `pnpm-lock.yaml` with the client manifest's source list
//! > needs the bundler's module-to-package map on the attest path; until
//! > that lands, the SBOM over-reports (every locked package, not only the
//! > ones that reached the bundle)."
//!
//! [`filter_reached`] is the pure intersection: the lockfile's npm
//! packages narrowed to the set the bundler's module→package map
//! (`oxide_fw_bundler::packages_reached`) says reached the bundle. An
//! empty reached set is the pre-F20 over-reporting shape byte-for-byte
//! (a manifest-less or first-party-only build lists every locked package
//! — the safe direction for an SBOM). [`write_attestation`] wires the
//! filter to the client manifest on disk: `dist/client/client.manifest.json`
//! feeds the reached set, the pnpm lockfile feeds the candidates, and the
//! SBOM's npm components narrow to the intersection. Written FIRST (red):
//! these pins fail on unresolved-symbol errors before the seam exists.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use oxide_fw_bundler::{ClientManifest, RouteBundle};
use oxide_fw_cli::attest::{
    BuildFacts, LockPackage, SignatureEnvelope, filter_reached, is_canonical, load_signing_key,
    parse_pnpm_lock_packages, seed_to_pkcs8, verify_signature, verify_subjects, write_attestation,
};
use ring::signature::KeyPair as _;

// A fixed 32-byte seed so the ed25519 key (and therefore every signed
// artifact) is byte-reproducible across runs and machines.
const SEED: [u8; 32] = [0x42; 32];

/// Wrap a DER blob as a PEM body (the `BEGIN`/`END` fence + base64).
fn pem(label: &str, der: &[u8]) -> String {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    format!(
        "-----BEGIN {label}-----\n{}\n-----END {label}-----\n",
        STANDARD.encode(der)
    )
}

fn pem_private_key(der: &[u8]) -> String {
    pem("PRIVATE KEY", der)
}

fn facts() -> BuildFacts {
    BuildFacts {
        oxide_version: "0.9.0".to_string(),
        config_hash: "deadbeefdeadbeef".to_string(),
        git_commit: "779561f2b54a4dbe1ba598e6e6733511f9cf477c".to_string(),
        dirty: false,
        builder_id: "local".to_string(),
        source_date_epoch: Some(1_700_000_000),
    }
}

/// A pnpm lockfile with three locked packages — `react` (reached), and
/// `@oxide-fw/core` + `tinybench` (NOT reached by the manifest below) — so
/// the filter's narrowing is observable (two packages drop out).
const PNPM_LOCK: &str = "\
lockfileVersion: '9.0'

importers:
  .:
    dependencies:
      react:
        specifier: ^19.0.0
        version: 19.2.0

packages:

  react@19.2.0:
    resolution: {integrity: sha512-aaa}

  '@oxide-fw/core@0.6.1':
    resolution: {integrity: sha512-bbb}

  /tinybench@2.9.0(typescript@5.9.2):
    resolution: {integrity: sha512-ccc}

snapshots:

  react@19.2.0: {}
";

/// A fake dist tree: `build-id`, a `client/` dir, a `server/` dir, and the
/// `attest/` dir that must never become a subject. Optionally writes a
/// `client/client.manifest.json` carrying the routes the reached-packages
/// filter reads.
fn dist_fixture(root: &Path, manifest: Option<&ClientManifest>) -> std::path::PathBuf {
    let dist = root.join("dist");
    fs::create_dir_all(dist.join("client")).unwrap();
    fs::create_dir_all(dist.join("server/ssr")).unwrap();
    fs::create_dir_all(dist.join("attest")).unwrap();
    fs::write(dist.join("build-id"), b"0123456789abcdef").unwrap();
    fs::write(dist.join("server.manifest.json"), b"{\"routes\":{}}").unwrap();
    fs::write(dist.join("client/app.js"), b"export const app = 1;\n").unwrap();
    fs::write(
        dist.join("server/ssr/index.js"),
        b"export const routes = {};\n",
    )
    .unwrap();
    if let Some(m) = manifest {
        let json = serde_json::to_string_pretty(m).unwrap() + "\n";
        fs::write(dist.join("client/client.manifest.json"), json).unwrap();
    }
    dist
}

/// A manifest whose only route reaches `react` (one node_modules module).
fn react_only_manifest() -> ClientManifest {
    ClientManifest {
        routes: [(
            "home".to_string(),
            RouteBundle {
                entry_chunk: "home.js".to_string(),
                modules: vec![std::path::PathBuf::from(
                    "/proj/node_modules/react/index.js",
                )],
                ..RouteBundle::default()
            },
        )]
        .into_iter()
        .collect(),
        ..ClientManifest::default()
    }
}

// ---------------------------------------------------------------------------
// filter_reached — the pure lockfile ⇄ reached-set intersection
// ---------------------------------------------------------------------------

#[test]
fn filter_reached_keeps_only_packages_the_bundle_reached() {
    // The lockfile lists three npm packages; the bundle reached only react.
    // The filter narrows to react — @oxide-fw/core and tinybench drop out.
    let pnpm = [
        LockPackage::new("react", "19.2.0"),
        LockPackage::new("@oxide-fw/core", "0.6.1"),
        LockPackage::new("tinybench", "2.9.0"),
    ];
    let reached = BTreeSet::from(["react".to_string()]);
    let kept = filter_reached(&pnpm, &reached);
    let names: Vec<&str> = kept.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec!["react"]);
}

#[test]
fn filter_reached_lists_every_package_when_the_reached_set_is_empty() {
    // An empty reached set is the pre-F20 shape: every locked package is
    // listed (the safe over-reporting default — a manifest-less or
    // first-party-only build never narrows the SBOM).
    let pnpm = [
        LockPackage::new("react", "19.2.0"),
        LockPackage::new("vue", "3.4.0"),
    ];
    let kept = filter_reached(&pnpm, &BTreeSet::new());
    assert_eq!(
        kept.len(),
        2,
        "an empty reached set over-reports (the default)"
    );
}

#[test]
fn filter_reached_preserves_the_input_order() {
    // The SBOM sorts components by purl after the filter; the filter itself
    // keeps the lockfile's order so a caller that wants the lockfile order
    // gets it (no hidden re-sort).
    let pnpm = [
        LockPackage::new("vue", "3.4.0"),
        LockPackage::new("react", "19.2.0"),
        LockPackage::new("axios", "1.7.0"),
    ];
    let reached = BTreeSet::from(["react".to_string(), "axios".to_string()]);
    let kept = filter_reached(&pnpm, &reached);
    let names: Vec<&str> = kept.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["react", "axios"],
        "the lockfile order is preserved"
    );
}

#[test]
fn filter_reached_drops_a_reached_name_the_lockfile_does_not_carry() {
    // The manifest reached `vue` but the lockfile does not list it (a
    // hand-edited lockfile, or a package removed between build and attest).
    // The filter drops the orphan name silently on the SBOM side — the
    // lockfile is the source of truth for what was installed.
    let pnpm = [LockPackage::new("react", "19.2.0")];
    let reached = BTreeSet::from(["react".to_string(), "vue".to_string()]);
    let kept = filter_reached(&pnpm, &reached);
    let names: Vec<&str> = kept.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec!["react"]);
}

#[test]
fn filter_reached_empty_lockfile_is_empty() {
    let kept = filter_reached(&[], &BTreeSet::from(["react".to_string()]));
    assert!(kept.is_empty());
}

// ---------------------------------------------------------------------------
// write_attestation — the manifest feeds the filter end-to-end
// ---------------------------------------------------------------------------

fn npm_purls(sbom_bytes: &[u8]) -> Vec<String> {
    let sbom: serde_json::Value = serde_json::from_slice(sbom_bytes).unwrap();
    sbom["components"]
        .as_array()
        .expect("components array")
        .iter()
        .filter(|c| {
            c["purl"]
                .as_str()
                .is_some_and(|p| p.starts_with("pkg:npm/"))
        })
        .map(|c| c["purl"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn write_attestation_narrows_the_sbom_to_packages_the_bundle_reached() {
    // The F20 acceptance: the client manifest's module→package map feeds
    // the attest path, and the SBOM's npm components narrow to the
    // intersection. The manifest reaches `react`; the lockfile lists react,
    // @oxide-fw/core, and tinybench — only react lands in the SBOM.
    let tmp = tempfile::tempdir().unwrap();
    let project_root = tmp.path().join("project");
    fs::create_dir_all(&project_root).unwrap();
    fs::write(project_root.join("pnpm-lock.yaml"), PNPM_LOCK).unwrap();

    let dist = dist_fixture(&project_root, Some(&react_only_manifest()));
    let pair = load_signing_key(&pem_private_key(&seed_to_pkcs8(&SEED))).unwrap();
    let report = write_attestation(&project_root, &dist, &facts(), &pair)
        .expect("the attest runs over the manifest-bearing dist");

    let sbom_bytes = fs::read(dist.join("attest/sbom.cdx.json")).unwrap();
    assert!(is_canonical(&sbom_bytes), "the SBOM is written canonically");
    let purls = npm_purls(&sbom_bytes);
    assert_eq!(
        purls,
        vec!["pkg:npm/react@19.2.0"],
        "only the reached package lands in the SBOM"
    );
    // The report's component count is the filtered count (one npm package;
    // no Cargo.lock in this fixture → zero cargo components).
    assert_eq!(report.components, 1);
}

#[test]
fn write_attestation_over_reports_when_no_client_manifest_is_present() {
    // The safe default: a dist WITHOUT a client manifest (a non-bundle
    // build, or a manifest from before the seam) lists every locked package
    // — byte-identical to the pre-F20 shape. The SBOM never narrows on a
    // missing manifest.
    let tmp = tempfile::tempdir().unwrap();
    let project_root = tmp.path().join("project");
    fs::create_dir_all(&project_root).unwrap();
    fs::write(project_root.join("pnpm-lock.yaml"), PNPM_LOCK).unwrap();

    let dist = dist_fixture(&project_root, None);
    let pair = load_signing_key(&pem_private_key(&seed_to_pkcs8(&SEED))).unwrap();
    let report = write_attestation(&project_root, &dist, &facts(), &pair)
        .expect("the attest runs over the manifest-less dist");

    let sbom_bytes = fs::read(dist.join("attest/sbom.cdx.json")).unwrap();
    let purls = npm_purls(&sbom_bytes);
    assert_eq!(
        purls.len(),
        3,
        "a manifest-less dist over-reports (every locked package): {purls:?}"
    );
    assert_eq!(report.components, 3);
}

#[test]
fn write_attestation_over_reports_when_the_manifest_reached_no_package() {
    // A manifest whose routes are first-party-only (no node_modules) reaches
    // no package — the reached set is empty and the SBOM over-reports (the
    // same safe default as a missing manifest).
    let tmp = tempfile::tempdir().unwrap();
    let project_root = tmp.path().join("project");
    fs::create_dir_all(&project_root).unwrap();
    fs::write(project_root.join("pnpm-lock.yaml"), PNPM_LOCK).unwrap();

    let first_party_only = ClientManifest {
        routes: [(
            "home".to_string(),
            RouteBundle {
                entry_chunk: "home.js".to_string(),
                modules: vec![std::path::PathBuf::from("/proj/src/app.tsx")],
                ..RouteBundle::default()
            },
        )]
        .into_iter()
        .collect(),
        ..ClientManifest::default()
    };
    let dist = dist_fixture(&project_root, Some(&first_party_only));
    let pair = load_signing_key(&pem_private_key(&seed_to_pkcs8(&SEED))).unwrap();
    let report = write_attestation(&project_root, &dist, &facts(), &pair)
        .expect("the attest runs over the first-party-only manifest");

    let sbom_bytes = fs::read(dist.join("attest/sbom.cdx.json")).unwrap();
    let purls = npm_purls(&sbom_bytes);
    assert_eq!(purls.len(), 3, "a first-party-only manifest over-reports");
    assert_eq!(report.components, 3);
}

#[test]
fn write_attestation_over_reports_when_the_manifest_is_corrupt() {
    // A corrupt client manifest is best-effort skipped (a warn, not a
    // refusal): over-reporting is the safe direction for an SBOM, and a
    // broken manifest must never block the attest.
    let tmp = tempfile::tempdir().unwrap();
    let project_root = tmp.path().join("project");
    fs::create_dir_all(&project_root).unwrap();
    fs::write(project_root.join("pnpm-lock.yaml"), PNPM_LOCK).unwrap();

    let dist = dist_fixture(&project_root, None);
    fs::write(
        dist.join("client/client.manifest.json"),
        b"{ this is not valid json ",
    )
    .unwrap();
    let pair = load_signing_key(&pem_private_key(&seed_to_pkcs8(&SEED))).unwrap();
    let report = write_attestation(&project_root, &dist, &facts(), &pair)
        .expect("a corrupt manifest never blocks the attest");

    let sbom_bytes = fs::read(dist.join("attest/sbom.cdx.json")).unwrap();
    let purls = npm_purls(&sbom_bytes);
    assert_eq!(
        purls.len(),
        3,
        "a corrupt manifest over-reports (best-effort)"
    );
    assert_eq!(report.components, 3);
}

#[test]
fn write_attestation_keeps_the_signature_round_trip_after_the_filter() {
    // The filter narrows the SBOM; the provenance signature still verifies
    // over the statement (the subjects are the dist, unchanged; only the
    // SBOM's npm components narrow).
    let tmp = tempfile::tempdir().unwrap();
    let project_root = tmp.path().join("project");
    fs::create_dir_all(&project_root).unwrap();
    fs::write(project_root.join("pnpm-lock.yaml"), PNPM_LOCK).unwrap();

    let dist = dist_fixture(&project_root, Some(&react_only_manifest()));
    let pair = load_signing_key(&pem_private_key(&seed_to_pkcs8(&SEED))).unwrap();
    let _report = write_attestation(&project_root, &dist, &facts(), &pair).unwrap();

    let statement_bytes = fs::read(dist.join("attest/provenance.intoto.json")).unwrap();
    assert!(is_canonical(&statement_bytes));
    let st: serde_json::Value = serde_json::from_slice(&statement_bytes).unwrap();
    let sig: SignatureEnvelope =
        serde_json::from_str(&fs::read_to_string(dist.join("attest/provenance.sig")).unwrap())
            .unwrap();
    verify_signature(&statement_bytes, &sig, pair.public_key().as_ref()).expect("sig verifies");
    verify_subjects(&dist, &st).expect("subjects verify against the dist");
}

// ---------------------------------------------------------------------------
// parse_pnpm_lock_packages parity — the scoped name the filter intersects
// ---------------------------------------------------------------------------

#[test]
fn pnpm_lock_scoped_name_matches_the_bundler_scoped_classification() {
    // The bundler classifies `node_modules/@scope/pkg/...` as `@scope/pkg`;
    // the pnpm parser yields `@oxide-fw/core` for the scoped lockfile key.
    // The two spellings agree so the intersection is non-empty for a
    // reached scoped package (the filter's correctness rests on this).
    let pkgs = parse_pnpm_lock_packages(PNPM_LOCK).expect("the fixture parses");
    let names: Vec<&str> = pkgs.iter().map(|p| p.name.as_str()).collect();
    assert!(
        names.contains(&"@oxide-fw/core"),
        "the scoped name parses verbatim"
    );
    // And the filter keeps it when reached.
    let reached = BTreeSet::from(["@oxide-fw/core".to_string()]);
    let kept = filter_reached(&pkgs, &reached);
    let kept_names: Vec<&str> = kept.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(kept_names, vec!["@oxide-fw/core"]);
}
