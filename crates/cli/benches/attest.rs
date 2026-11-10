//! Criterion benches for the V2-F20 attestation substrate
//! (`bench-util` gated like the V2-A02/V2-B09/V2-E01/V2-F18 benches —
//! plain `cargo test` never pays the criterion build).
//!
//! The bench-relevant questions (the attest flow runs at the end of every
//! `--attest` build, so its cost must stay noise next to the build
//! itself):
//! - `canonical_json/{obj,arr}`: the recursive key sort over a realistic
//!   statement-shaped object and a large array of rows.
//! - `sbom/{100,1000}`: the component assembly + purl sort over full
//!   lockfiles.
//! - `subtree_digest/128`: the sorted walk + SHA-256 over a 128-file
//!   dist-shaped tree (the subject computation per build).
//! - `ed25519/sign_verify`: the fixed-key signature round trip over a
//!   ~60 KiB statement (the provenance a large route table yields).

use std::hint::black_box;
use std::path::Path;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_cli::attest::{
    LockPackage, canonical_json, load_signing_key, sbom_from_locks, seed_to_pkcs8, sign_statement,
    subtree_digest, verify_signature,
};

fn statement_shaped_object(rows: usize) -> serde_json::Value {
    let subject: Vec<serde_json::Value> = (0..rows)
        .map(|i| {
            serde_json::json!({
                "digest": { "sha256": format!("{i:064x}") },
                "name": format!("dist/artifact-{i}"),
            })
        })
        .collect();
    serde_json::json!({
        "_type": "https://in-toto.io/Statement/v1",
        "predicate": {
            "buildDefinition": {
                "buildType": "https://github.com/elcoosp/oxide/build/v1",
                "externalParameters": {
                    "configHash": "deadbeef",
                    "dirty": false,
                    "gitCommit": "779561f2b54a4dbe1ba598e6e6733511f9cf477c",
                    "oxideVersion": env!("CARGO_PKG_VERSION"),
                },
                "internalParameters": {},
            },
            "runDetails": { "builder": { "id": "local" } },
        },
        "predicateType": "https://slsa.dev/provenance/v1",
        "subject": subject,
    })
}

fn bench_canonical_json(c: &mut Criterion) {
    let mut group = c.benchmark_group("canonical_json");
    group.bench_function("obj_64_subjects", |b| {
        let v = statement_shaped_object(64);
        b.iter(|| canonical_json(black_box(&v)))
    });
    group.bench_function("obj_512_subjects", |b| {
        let v = statement_shaped_object(512);
        b.iter(|| canonical_json(black_box(&v)))
    });
    group.finish();
}

fn bench_sbom(c: &mut Criterion) {
    let mut group = c.benchmark_group("sbom");
    for n in [100_usize, 1000] {
        let cargo: Vec<LockPackage> = (0..n).map(|i| LockPackage::new(&format!("crate-{i}"), "1.0.0")).collect();
        let pnpm: Vec<LockPackage> = (0..n).map(|i| LockPackage::new(&format!("pkg-{i}"), "2.0.0")).collect();
        group.bench_with_input(BenchmarkId::from_parameter(n), &(cargo, pnpm), |b, (cargo, pnpm)| {
            b.iter(|| sbom_from_locks(black_box(cargo), black_box(pnpm), Some(1_700_000_000)))
        });
    }
    group.finish();
}

/// A 128-file dist-shaped tree, built once before the benchmark loop.
fn dist_tree(root: &Path) -> std::path::PathBuf {
    let dist = root.join("dist");
    for sub in ["client/chunks", "server/ssr"] {
        std::fs::create_dir_all(dist.join(sub)).expect("fixture dirs");
    }
    for i in 0..64 {
        std::fs::write(
            dist.join(format!("client/chunks/chunk-{i}.js")),
            format!("export const c{i} = {i};\n").repeat(8),
        )
        .expect("fixture files");
        std::fs::write(
            dist.join(format!("server/ssr/mod-{i}.js")),
            format!("export const m{i} = {i};\n").repeat(8),
        )
        .expect("fixture files");
    }
    dist
}

fn bench_subtree_digest(c: &mut Criterion) {
    let dir = tempfile::tempdir().expect("bench fixture dir");
    let dist = dist_tree(dir.path());
    let mut group = c.benchmark_group("subtree_digest");
    group.bench_function("dist_128_files", |b| {
        b.iter(|| subtree_digest(black_box(&dist)))
    });
    group.finish();
}

fn bench_ed25519(c: &mut Criterion) {
    let pair = load_signing_key(&pem(&seed_to_pkcs8(&[0x42; 32]))).expect("bench key");
    let statement = statement_shaped_object(512);
    let message = canonical_json(&statement);
    assert!(message.len() > 32 * 1024, "the bench statement is ~60 KiB");
    let mut group = c.benchmark_group("ed25519");
    group.bench_function("sign_60k", |b| {
        b.iter(|| sign_statement(black_box(&statement), &pair))
    });
    let envelope = sign_statement(&statement, &pair);
    let bytes = message.into_bytes();
    group.bench_function("verify_60k", |b| {
        b.iter(|| verify_signature(black_box(&bytes), black_box(&envelope), pair.public_key().as_ref()))
    });
    group.finish();
}

/// PEM armor around DER bytes (the same shape the TDD pins build).
fn pem(der: &[u8]) -> String {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD.encode(der);
    format!("-----BEGIN PRIVATE KEY-----\n{b64}\n-----END PRIVATE KEY-----\n")
}

criterion_group!(
    attest,
    bench_canonical_json,
    bench_sbom,
    bench_subtree_digest,
    bench_ed25519
);
criterion_main!(attest);
