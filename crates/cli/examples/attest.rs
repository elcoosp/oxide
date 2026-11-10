//! Consumer example for the V2-F20 attestation engine: the flow a
//! contributor (or an agent) drives — and the tests pin — when a build is
//! attested and its provenance verified.
//!
//! Shows the pieces `oxide build --attest` / `oxide verify` compose:
//!
//! 1. [`write_attestation`](oxide_fw_cli::attest::write_attestation) —
//!    the composer: A04 build id, CycloneDX SBOM, the in-toto statement
//!    (SLSA v1 predicate, subjects = `dist/build-id` + every top-level
//!    dist artifact) and the ed25519 signature envelope, all canonical
//!    bytes.
//! 2. [`verify_signature`](oxide_fw_cli::attest::verify_signature) +
//!    [`verify_subjects`](oxide_fw_cli::attest::verify_subjects) — what
//!    `oxide verify` runs: the signature over the canonical statement and
//!    every subject digest against the dist on disk.
//! 3. The tamper contract — one flipped byte in the dist fails the
//!    subject check, exactly as the spec's "tampering with one byte fails
//!    `verify`" acceptance requires.
//!
//! A fixed-seed ed25519 key (the `seed_to_pkcs8` + PEM loader helpers in
//! `oxide_fw_cli::attest`) makes the walkthrough reproducible; the real
//! command takes the key from `--key` / `OXIDE_SIGNING_KEY`. The fake dist
//! lives in a tempdir so the example runs in milliseconds with no build:
//!
//! ```text
//! cargo run -p oxide-fw-cli --example attest
//! ```

// ring exposes `Ed25519KeyPair::public_key` only through its
// `signature::KeyPair` trait.
use ring::signature::KeyPair as _;
use std::fs;

use base64::Engine as _;
use oxide_fw_cli::attest::{
    BuildFacts, ed25519_pubkey_from_pem, is_canonical, load_signing_key, seed_to_pkcs8,
    verify_signature, verify_subjects, write_attestation,
};

fn main() {
    let tmp = tempfile::tempdir().expect("a scratch dist to attest");
    let dist = tmp.path().join("dist");
    fs::create_dir_all(dist.join("client")).expect("fixture dirs");
    fs::create_dir_all(dist.join("server")).expect("fixture dirs");
    fs::write(dist.join("client/app.js"), b"export const app = 1;\n").expect("fixture");
    fs::write(dist.join("server/ssr.js"), b"export const routes = {};\n").expect("fixture");

    // The signing key: a fixed seed mints the same PKCS#8 every run (the
    // operator's real key comes from OXIDE_SIGNING_KEY / --key).
    let pair = load_signing_key(&pem(&seed_to_pkcs8(&[0x42; 32]))).expect("a fixed-seed key");
    let facts = BuildFacts {
        oxide_version: env!("CARGO_PKG_VERSION").to_string(),
        config_hash: "none".to_string(),
        git_commit: "779561f2b54a4dbe1ba598e6e6733511f9cf477c".to_string(),
        dirty: false,
        builder_id: "local".to_string(),
        // SOURCE_DATE_EPOCH: the only timestamp an artifact may carry.
        source_date_epoch: Some(1_700_000_000),
    };

    let report = write_attestation(tmp.path(), &dist, &facts, &pair).expect("the fixture attests");
    println!("build-id  : {}", report.build_id);
    println!(
        "subjects  : {} (build-id + client/ + server/ + ssr.js)",
        report.subjects
    );
    println!(
        "artifacts : {}",
        report
            .provenance_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    );
    println!("keyid     : {}", report.keyid);

    // What `oxide verify` checks first: the statement bytes on disk are
    // the canonical spelling — a re-serialized (pretty-printed) file is
    // rejected before the signature even runs.
    let bytes = fs::read(&report.provenance_path).expect("the statement was written");
    println!(
        "canonical : {}",
        if is_canonical(&bytes) {
            "ok"
        } else {
            "REFUSED"
        }
    );

    // The signature round trip with the verifier key derived from the
    // same pair (the SPKI PEM `openssl pkey -pubout` prints).
    let pubkey = ed25519_pubkey_from_pem(&pem_public(pair.public_key().as_ref())).expect("spki");
    verify_signature(&bytes, &envelope(&dist), &pubkey).expect("the signature verifies");
    println!("signature : verified");

    // The subjects, then the tamper: one flipped byte in the dist fails.
    let statement: serde_json::Value = serde_json::from_slice(&bytes).expect("canonical json");
    verify_subjects(&dist, &statement).expect("fresh subjects verify");
    println!("subjects  : verified");

    let tampered = dist.join("client/app.js");
    let mut contents = fs::read(&tampered).expect("the app chunk");
    let last = contents.len() - 1;
    contents[last] ^= 0x01;
    fs::write(&tampered, &contents).expect("tamper");
    match verify_subjects(&dist, &statement) {
        Ok(()) => panic!("a flipped byte cannot verify"),
        Err(e) => println!(
            "tamper    : refused ({} — {})",
            e.code(),
            one_line(&e.to_string())
        ),
    }
}

/// The signature envelope as `oxide verify` reads it back from
/// `dist/attest/provenance.sig`.
fn envelope(dist: &std::path::Path) -> oxide_fw_cli::attest::SignatureEnvelope {
    let raw = fs::read_to_string(dist.join("attest/provenance.sig")).expect("the envelope");
    serde_json::from_str(&raw).expect("the envelope shape")
}

/// Collapse the diagnostic's multi-line display to the example's one line.
fn one_line(s: &str) -> String {
    s.lines().next().unwrap_or_default().to_string()
}

/// Test-local PEM armor (the same shape the TDD pins build).
fn pem(der: &[u8]) -> String {
    use base64::engine::general_purpose::STANDARD;
    format!(
        "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
        STANDARD.encode(der)
    )
}

fn pem_public(key: &[u8]) -> String {
    use base64::engine::general_purpose::STANDARD;
    // SPKI DER = the 12-byte ed25519 prefix in front of the 32-byte key.
    let mut spki = vec![
        0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
    ];
    spki.extend_from_slice(key);
    format!(
        "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----\n",
        STANDARD.encode(spki)
    )
}
