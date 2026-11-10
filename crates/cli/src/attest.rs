//! The V2-F20 attested-reproducible-builds substrate (spec v0.2 §V2-F20).
//!
//! `oxide build --attest` writes `dist/attest/`: a CycloneDX 1.5 SBOM
//! (`sbom.cdx.json`), an in-toto Statement v1 provenance with the SLSA v1
//! predicate (`provenance.intoto.json`) whose subjects are the SHA-256 of
//! `dist/build-id` and every top-level artifact, and an ed25519 signature
//! over the canonical JSON (`provenance.sig`). `oxide verify <dist>` checks
//! the canonical form, the signature (with `--pubkey`) and the subject
//! digests; `--rebuild` re-runs the build in a scratch copy and compares
//! the dist digests.
//!
//! The layout is the F16/F18 substrate pattern: everything that decides a
//! byte is a pure function pinned by `tests/attest_tdd.rs` — the lockfile
//! parsers (own serde-struct-free line parsers, no external generator),
//! the canonical-JSON writer, the SBOM/statement builders, the digest
//! walks and the sign/verify round trip (a fixed-seed ed25519 key makes
//! the signatures reproducible). The process glue (env reads, `git
//! rev-parse`, the `--rebuild` scratch build) sits in the command layer of
//! `lib.rs` and is the only unpinned part, exactly like the F18
//! `BuildProbe`.
//!
//! Determinism is the contract: no wall-clock timestamps (the only
//! timestamp comes from `SOURCE_DATE_EPOCH`, omitted when unset), sorted
//! maps and file lists everywhere (`canonical_json` sorts recursively and
//! every walk collects + sorts), and root-relative, slash-normalized paths
//! in every digest — two builds in different directories carry identical
//! artifacts (pinned).
//!
//! Errors are diagnostics: every variant of [`AttestError`] carries a
//! stable `ATT-*` code and a fix line ([`AttestError::fix`]) so the CLI
//! surface renders the same code/cause/fix shape as the rest of the tool
//! (architecture invariant 3 — the miette-style help text).

use std::path::Path;

use base64::Engine as _;
use ring::signature::Ed25519KeyPair;
use serde::{Deserialize, Serialize};
use sha2::Digest as _;
use thiserror::Error;

use oxide_fw_diagnostics::{Diagnostic, OxideError};

/// The environment variable fixing the only timestamp an artifact carries
/// (spec Appendix V2 env table, the F20 row).
pub const SOURCE_DATE_EPOCH_VAR: &str = "SOURCE_DATE_EPOCH";
/// The environment variable holding the ed25519 signing key path.
pub const SIGNING_KEY_VAR: &str = "OXIDE_SIGNING_KEY";
/// The environment variable naming the builder for the provenance record.
pub const BUILDER_ID_VAR: &str = "OXIDE_BUILDER_ID";

/// The attestation directory under `dist/` (spec §V2-F20: `dist/attest/`).
pub const ATTEST_DIRNAME: &str = "attest";
/// The build-id file under `dist/`, written (and subject #1) by
/// `--attest`. The id itself is the A04 canonical `build_id_of_dist`.
pub const BUILD_ID_FILE: &str = "build-id";
/// The CycloneDX 1.5 SBOM file name.
pub const SBOM_FILENAME: &str = "sbom.cdx.json";
/// The in-toto Statement v1 provenance file name.
pub const PROVENANCE_FILENAME: &str = "provenance.intoto.json";
/// The ed25519 signature envelope file name.
pub const SIGNATURE_FILENAME: &str = "provenance.sig";

/// The `_type` of the in-toto statement this substrate writes.
pub const STATEMENT_TYPE: &str = "https://in-toto.io/Statement/v1";
/// The SLSA predicate type the statement carries.
pub const PREDICATE_TYPE: &str = "https://slsa.dev/provenance/v1";
/// The `buildDefinition.buildType` naming this builder's contract.
pub const BUILD_TYPE: &str = "https://github.com/elcoosp/oxide/build/v1";

/// ed25519 PKCS#8 is a fixed 16-byte prefix in front of the 32-byte seed
/// (`302e020100300506032b657004220420`). Exposed so the TDD pins and the
/// walkthrough example can mint a key from a fixed seed without a
/// dependency on ring's RNG.
const ED25519_PKCS8_PREFIX: [u8; 16] = [
    0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
];

/// The 12-byte SPKI prefix of an ed25519 `SubjectPublicKeyInfo` DER
/// (`302a300506032b6570032100`) in front of the 32-byte public key.
const ED25519_SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

// ---------------------------------------------------------------------------
// Errors (each variant is a diagnostic with a stable code and a fix line)
// ---------------------------------------------------------------------------

/// Everything that can refuse an attestation or a verification. Each
/// variant renders as an `OxideError(Diagnostic)` at the CLI boundary with
/// a stable `ATT-*` code and a fix line — the miette-style help text.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum AttestError {
    /// No signing key was provided (`--key` / `OXIDE_SIGNING_KEY`).
    #[error("no signing key: pass --key <file> or set {SIGNING_KEY_VAR}")]
    KeyMissing,
    /// The signing key file could not be read.
    #[error("cannot read the signing key `{path}`: {detail}")]
    KeyUnreadable {
        /// The key path that failed.
        path: String,
        /// The io error detail.
        detail: String,
    },
    /// The signing key is not an ed25519 PKCS#8 key.
    #[error("the signing key is not an ed25519 PKCS#8 private key: {detail}")]
    KeyMalformed {
        /// The parser's rejection detail.
        detail: String,
    },
    /// A lockfile could not be parsed into components.
    #[error("`{file}` is not a parseable {which} lockfile (line {line}): {detail}")]
    LockUnparseable {
        /// The lockfile path.
        file: String,
        /// Which lockfile grammar failed (`cargo` / `pnpm`).
        which: String,
        /// The 1-based line the parser stopped at.
        line: usize,
        /// What was wrong with the line.
        detail: String,
    },
    /// The `dist` directory does not exist.
    #[error("dist missing: {path}")]
    DistMissing {
        /// The dist path that does not exist.
        path: String,
    },
    /// `dist/build-id` is absent — the attestation flow never ran.
    #[error("`dist/{BUILD_ID_FILE}` is missing — run `oxide build --attest` first")]
    BuildIdMissing,
    /// A subject digest does not match the dist on disk.
    #[error("subject `{name}` does not match dist: {detail}")]
    SubjectMismatch {
        /// The subject name that mismatched.
        name: String,
        /// Why (missing file, digest differs, ...).
        detail: String,
    },
    /// The provenance file is not canonical JSON (whitespace tamper).
    #[error("provenance is not canonical JSON: {detail}")]
    NotCanonical {
        /// Why canonicalization refused the bytes.
        detail: String,
    },
    /// The ed25519 signature does not verify over the statement.
    #[error("the provenance signature does not verify")]
    SignatureMismatch,
    /// The `--pubkey` material is not an ed25519 public key.
    #[error("the public key is not ed25519 (SPKI PEM, 64-char hex or 32-byte base64): {detail}")]
    PubkeyMalformed {
        /// The parser's rejection detail.
        detail: String,
    },
    /// The filesystem or an epoch conversion failed underneath the flow.
    #[error("attestation io: {detail}")]
    Io {
        /// The underlying failure detail.
        detail: String,
    },
}

impl AttestError {
    /// The stable diagnostic code for this error.
    pub fn code(&self) -> &'static str {
        match self {
            AttestError::KeyMissing | AttestError::KeyUnreadable { .. } => "ATT-KEY",
            AttestError::KeyMalformed { .. } => "ATT-KEYFMT",
            AttestError::LockUnparseable { .. } => "ATT-LOCK",
            AttestError::DistMissing { .. }
            | AttestError::BuildIdMissing
            | AttestError::SubjectMismatch { .. } => "ATT-DIST",
            AttestError::NotCanonical { .. } => "ATT-CANON",
            AttestError::SignatureMismatch => "ATT-SIG",
            AttestError::PubkeyMalformed { .. } => "ATT-PUBKEY",
            AttestError::Io { .. } => "ATT-IO",
        }
    }

    /// The miette-style help text: what to do about this error.
    pub fn fix(&self) -> String {
        match self {
            AttestError::KeyMissing => format!(
                "mint an ed25519 key (`openssl genpkey -algorithm ed25519`) and \
                 pass it via --key or {SIGNING_KEY_VAR}"
            ),
            AttestError::KeyUnreadable { path, .. } => {
                format!("check the path and permissions of `{path}`")
            }
            AttestError::KeyMalformed { .. } => "export the key as PKCS#8 \
                 (`openssl pkcs8 -topk8 -nocrypt`); raw seeds are accepted only as 48-byte DER"
                .to_string(),
            AttestError::LockUnparseable { file, .. } => {
                format!(
                    "`{file}` must be a plain generated lockfile; re-run the \
                 package manager to regenerate it before attesting"
                )
            }
            AttestError::DistMissing { .. } => "run `oxide build --attest` — verification \
                 runs against the dist a build emitted"
                .to_string(),
            AttestError::BuildIdMissing => "run `oxide build --attest` first: the attestation \
                 flow writes dist/build-id and dist/attest/"
                .to_string(),
            AttestError::SubjectMismatch { name, .. } => {
                format!(
                    "rebuild with `oxide build --attest` — `{name}` changed since \
                 the provenance was signed"
                )
            }
            AttestError::NotCanonical { .. } => "the statement must be written by the same \
                 `oxide build --attest` that signed it; hand-edited or re-serialized \
                 files fail the canonical check"
                .to_string(),
            AttestError::SignatureMismatch => "check --pubkey matches the signing key \
                 (the key id is in provenance.sig), and confirm the statement bytes \
                 were not modified after signing"
                .to_string(),
            AttestError::PubkeyMalformed { .. } => "pass the verifier key as a PUBLIC KEY \
                 PEM, a 64-char hex string or a 32-byte base64 blob — the SPKI PEM \
                 `openssl pkey -pubout` prints works"
                .to_string(),
            AttestError::Io { .. } => "check the disk state of dist/ and retry the build; \
                 the attestation walk never writes outside dist/"
                .to_string(),
        }
    }

    /// The full diagnostic for the CLI boundary: code, message and fix.
    pub fn diagnostic(&self) -> OxideError {
        OxideError(Box::new(
            Diagnostic::error(self.code(), self.to_string()).with_fix(self.fix()),
        ))
    }
}

// ---------------------------------------------------------------------------
// Canonical JSON
// ---------------------------------------------------------------------------

fn sorted_value(v: &serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut out = serde_json::Map::new();
            for k in keys {
                out.insert(k.clone(), sorted_value(&map[k]));
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(sorted_value).collect())
        }
        other => other.clone(),
    }
}

/// The canonical JSON spelling of `value`: object keys sorted recursively,
/// arrays preserved in order, no whitespace. Sorting is explicit (not an
/// artifact of `serde_json`'s map type), so the bytes stay identical even
/// if a dependency ever unifies in `serde_json/preserve_order`.
pub fn canonical_json(value: &serde_json::Value) -> String {
    serde_json::to_string(&sorted_value(value)).expect("a Value always serializes")
}

/// True when `bytes` are exactly the canonical JSON spelling of the value
/// they encode. The verify flow refuses non-canonical statements so a
/// whitespace-only tamper cannot sneak past the signature check.
pub fn is_canonical(bytes: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return false;
    };
    canonical_json(&value).into_bytes() == bytes
}

// ---------------------------------------------------------------------------
// Timestamps
// ---------------------------------------------------------------------------

/// The RFC 3339 UTC (`Z`) spelling of a Unix epoch — the only timestamp an
/// artifact may carry, and only when `SOURCE_DATE_EPOCH` is set.
///
/// # Errors
/// [`AttestError::Io`] when the epoch does not fit an `i64` (the `chrono`
/// conversion range).
pub fn rfc3339_from_epoch(secs: u64) -> Result<String, AttestError> {
    let secs = i64::try_from(secs).map_err(|_| AttestError::Io {
        detail: "SOURCE_DATE_EPOCH is out of range".to_string(),
    })?;
    let dt = chrono::DateTime::from_timestamp(secs, 0).ok_or_else(|| AttestError::Io {
        detail: "SOURCE_DATE_EPOCH is out of range".to_string(),
    })?;
    Ok(dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

// ---------------------------------------------------------------------------
// Lockfile parsers (own line parsers; no external generator)
// ---------------------------------------------------------------------------

/// One lockfile component: the package identity an SBOM row carries.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct LockPackage {
    /// The package name (`serde`, `@oxide-fw/core`).
    pub name: String,
    /// The exact locked version.
    pub version: String,
}

impl LockPackage {
    /// A `(name, version)` pair, for fixtures and callers.
    pub fn new(name: &str, version: &str) -> Self {
        Self {
            name: name.to_string(),
            version: version.to_string(),
        }
    }
}

/// Parse a `Cargo.lock` (v3/v4 text) into its `[[package]]` components.
///
/// The parser is a strict line machine over the generated shape: a
/// `[[package]]` header opens a block; `name = "…"` and `version = "…"`
/// inside it are captured; every other line (dependencies, checksums,
/// `source = `, blank) is skipped. Output is sorted by `(name, version)`
/// and deduplicated — a lockfile never repeats a package, and the SBOM
/// wants a stable order regardless of the file's own grouping.
///
/// # Errors
/// [`AttestError::LockUnparseable`] when a `[[package]]` block ends
/// without a `name` line (the line number is the block's header).
pub fn parse_cargo_lock(text: &str) -> Result<Vec<LockPackage>, AttestError> {
    let mut packages: Vec<LockPackage> = Vec::new();
    let mut block_start: Option<usize> = None;
    let mut name: Option<String> = None;
    let mut version: Option<String> = None;
    for (idx, raw) in text.lines().enumerate() {
        let line = raw.trim();
        let line_no = idx + 1;
        if line == "[[package]]" {
            if let Some(start) = block_start {
                push_cargo_block(start, name.take(), version.take(), &mut packages)?;
            }
            block_start = Some(line_no);
            continue;
        }
        if block_start.is_none() {
            continue; // the file header (`version = 4`, comments) — not a block
        }
        if let Some(v) = line.strip_prefix("name = ") {
            name = Some(parse_toml_string(v, line_no)?);
        } else if let Some(v) = line.strip_prefix("version = ") {
            version = Some(parse_toml_string(v, line_no)?);
        }
    }
    if let Some(start) = block_start {
        push_cargo_block(start, name.take(), version.take(), &mut packages)?;
    }
    packages.sort();
    packages.dedup();
    Ok(packages)
}

fn push_cargo_block(
    start: usize,
    name: Option<String>,
    version: Option<String>,
    out: &mut Vec<LockPackage>,
) -> Result<(), AttestError> {
    let name = name.ok_or_else(|| AttestError::LockUnparseable {
        file: "Cargo.lock".to_string(),
        which: "cargo".to_string(),
        line: start,
        detail: "a [[package]] block without a name line".to_string(),
    })?;
    let version = version.unwrap_or_default();
    out.push(LockPackage { name, version });
    Ok(())
}

/// Parse a bare TOML string value (`"1.2.3"`) — the only value shape a
/// generated Cargo.lock writes for `name`/`version`.
fn parse_toml_string(value: &str, line_no: usize) -> Result<String, AttestError> {
    let value = value.trim();
    let unquoted = value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .ok_or_else(|| AttestError::LockUnparseable {
            file: "Cargo.lock".to_string(),
            which: "cargo".to_string(),
            line: line_no,
            detail: format!("expected a quoted string, got `{value}`"),
        })?;
    Ok(unquoted.to_string())
}

/// Parse the `packages:` section of a `pnpm-lock.yaml` (v9) into
/// components. The section holds one two-space-indented key per installed
/// package — `/name@version:` for plain names, `'@scope/name@version':`
/// for scoped ones (quoted), with an optional `(peer@x.y.z)` suffix the
/// parser strips. Deeper-indented metadata lines (`resolution: …`) are
/// skipped; the section ends at the first line with no leading whitespace.
/// Output is sorted and deduplicated like [`parse_cargo_lock`].
///
/// # Errors
/// [`AttestError::LockUnparseable`] when a package key carries no
/// `name@version` identity.
pub fn parse_pnpm_lock_packages(text: &str) -> Result<Vec<LockPackage>, AttestError> {
    let mut packages: Vec<LockPackage> = Vec::new();
    let mut in_packages = false;
    for (idx, raw) in text.lines().enumerate() {
        let line_no = idx + 1;
        if raw.trim().is_empty() {
            continue;
        }
        if !raw.starts_with(' ') {
            // A top-level key: `packages:` opens the section, anything else
            // (`snapshots:`, `importers:`) closes it.
            in_packages = raw.trim() == "packages:";
            continue;
        }
        if !in_packages {
            continue;
        }
        // Exactly two spaces + a non-space character is a package key.
        // Four (or more) leading spaces are the key's metadata — skipped.
        let key = raw.strip_prefix("  ").unwrap_or_default();
        if key.starts_with(' ') || key.trim().is_empty() {
            continue;
        }
        let key = key.trim_end();
        let key = key.strip_suffix(':').unwrap_or(key);
        let key = strip_yaml_quotes(key.trim());
        let key = key.strip_prefix('/').unwrap_or(key);
        let identity = key.split('(').next().unwrap_or(key);
        let at = identity
            .rfind('@')
            .ok_or_else(|| AttestError::LockUnparseable {
                file: "pnpm-lock.yaml".to_string(),
                which: "pnpm".to_string(),
                line: line_no,
                detail: format!("a package key without `name@version`: `{identity}`"),
            })?;
        let (name, version) = identity.split_at(at);
        let version = version.strip_prefix('@').unwrap_or(version);
        if name.is_empty() || version.is_empty() {
            return Err(AttestError::LockUnparseable {
                file: "pnpm-lock.yaml".to_string(),
                which: "pnpm".to_string(),
                line: line_no,
                detail: format!("an empty name or version in `{identity}`"),
            });
        }
        packages.push(LockPackage {
            name: name.to_string(),
            version: version.to_string(),
        });
    }
    packages.sort();
    packages.dedup();
    Ok(packages)
}

fn strip_yaml_quotes(key: &str) -> &str {
    let quoted = key
        .strip_prefix('\'')
        .and_then(|k| k.strip_suffix('\''))
        .or_else(|| key.strip_prefix('"').and_then(|k| k.strip_suffix('"')));
    quoted.unwrap_or(key)
}

// ---------------------------------------------------------------------------
// SBOM (CycloneDX 1.5)
// ---------------------------------------------------------------------------

/// The CycloneDX 1.5 SBOM over the parsed lockfiles: one `library`
/// component per locked package (`pkg:cargo/…` for the binary's crates,
/// `pkg:npm/…` for the packages that reached the bundle), the oxide CLI
/// itself under `metadata.tools.components`, and `metadata.timestamp` only
/// when `SOURCE_DATE_EPOCH` provided one. Components are sorted by `purl`;
/// two calls with the same inputs serialize to the same bytes (pinned).
#[must_use]
pub fn sbom_from_locks(
    cargo: &[LockPackage],
    pnpm: &[LockPackage],
    generated_at_epoch: Option<u64>,
) -> serde_json::Value {
    let mut components: Vec<serde_json::Value> = Vec::new();
    for pkg in cargo {
        components.push(component(pkg, "cargo", "library"));
    }
    for pkg in pnpm {
        components.push(component(pkg, "npm", "library"));
    }
    components.sort_by(|a, b| {
        a["purl"]
            .as_str()
            .unwrap_or_default()
            .cmp(b["purl"].as_str().unwrap_or_default())
    });

    let tool = serde_json::json!({
        "bom-ref": format!("pkg:cargo/oxide-fw-cli@{}", env!("CARGO_PKG_VERSION")),
        "name": "oxide-fw-cli",
        "type": "application",
        "version": env!("CARGO_PKG_VERSION"),
    });

    let mut sbom = serde_json::json!({
        "bomFormat": "CycloneDX",
        "components": components,
        "metadata": {
            "tools": { "components": [tool] },
        },
        "specVersion": "1.5",
        "version": 1,
    });
    if let Some(epoch) = generated_at_epoch
        && let Ok(ts) = rfc3339_from_epoch(epoch)
    {
        sbom["metadata"]["timestamp"] = serde_json::Value::String(ts);
    }
    sbom
}

fn component(pkg: &LockPackage, scheme: &str, kind: &str) -> serde_json::Value {
    let purl = format!("pkg:{scheme}/{}@{}", pkg.name, pkg.version);
    serde_json::json!({
        "bom-ref": purl.clone(),
        "name": pkg.name,
        "purl": purl,
        "type": kind,
        "version": pkg.version,
    })
}

// ---------------------------------------------------------------------------
// Digests + subjects
// ---------------------------------------------------------------------------

/// Lowercase hex of `bytes` (the digest spelling every subject carries).
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = sha2::Sha256::digest(bytes);
    hex_lower(&digest)
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// The SHA-256 over the sorted `relative/path<TAB>sha256-hex` manifest of
/// every file under `dir` — the directory-digest spelling the subjects use
/// for top-level artifact trees (`dist/client/`, `dist/server/`). Paths
/// are slash-normalized and root-relative, so the same bytes in different
/// parent directories digest identically (pinned).
///
/// # Errors
/// [`AttestError::Io`] when the walk or a read fails.
pub fn subtree_digest(dir: &Path) -> Result<String, AttestError> {
    let mut manifest: Vec<(String, String)> = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries = std::fs::read_dir(&current).map_err(|e| AttestError::Io {
            detail: e.to_string(),
        })?;
        for entry in entries {
            let entry = entry.map_err(|e| AttestError::Io {
                detail: e.to_string(),
            })?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if !path.is_file() {
                continue;
            }
            let rel = path
                .strip_prefix(dir)
                .map_err(|e| AttestError::Io {
                    detail: e.to_string(),
                })?
                .to_string_lossy()
                .replace('\\', "/");
            let bytes = std::fs::read(&path).map_err(|e| AttestError::Io {
                detail: e.to_string(),
            })?;
            manifest.push((rel, sha256_hex(&bytes)));
        }
    }
    manifest.sort();
    let lines: String = manifest
        .iter()
        .map(|(path, hex)| format!("{path}\t{hex}"))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(sha256_hex(lines.as_bytes()))
}

/// One provenance subject: a root-relative name and its SHA-256.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subject {
    /// The root-relative name: `dist/build-id`, `dist/client/`,
    /// `dist/server.manifest.json` (a trailing slash marks a directory
    /// digest over [`subtree_digest`]'s manifest).
    pub name: String,
    /// The lowercase SHA-256 hex digest.
    pub digest_sha256: String,
}

/// The provenance subjects for a built `dist`: `dist/build-id` (required —
/// the attestation flow writes it first) plus every top-level entry of
/// `dist/` — files hashed whole, directories hashed via
/// [`subtree_digest`] — sorted by name. The attestation directory itself
/// is excluded: a statement can never list (and therefore never
/// self-reference) its own output.
///
/// # Errors
/// [`AttestError::DistMissing`] when `dist` does not exist,
/// [`AttestError::BuildIdMissing`] when `dist/build-id` is absent,
/// [`AttestError::Io`] on walk/read failures.
pub fn dist_subjects(dist: &Path) -> Result<Vec<Subject>, AttestError> {
    if !dist.is_dir() {
        return Err(AttestError::DistMissing {
            path: dist.to_string_lossy().into_owned(),
        });
    }
    if !dist.join(BUILD_ID_FILE).is_file() {
        return Err(AttestError::BuildIdMissing);
    }
    let mut subjects = vec![Subject {
        name: format!("dist/{BUILD_ID_FILE}"),
        digest_sha256: sha256_hex(&std::fs::read(dist.join(BUILD_ID_FILE)).map_err(|e| {
            AttestError::Io {
                detail: e.to_string(),
            }
        })?),
    }];
    let mut entries: Vec<std::path::PathBuf> = std::fs::read_dir(dist)
        .map_err(|e| AttestError::Io {
            detail: e.to_string(),
        })?
        .collect::<std::result::Result<Vec<std::fs::DirEntry>, std::io::Error>>()
        .map_err(|e| AttestError::Io {
            detail: e.to_string(),
        })?
        .into_iter()
        .map(|e| e.path())
        .collect();
    entries.sort();
    for path in entries {
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if file_name == ATTEST_DIRNAME {
            continue;
        }
        if path.is_dir() {
            subjects.push(Subject {
                name: format!("dist/{file_name}/"),
                digest_sha256: subtree_digest(&path)?,
            });
        } else if path.is_file() {
            let bytes = std::fs::read(&path).map_err(|e| AttestError::Io {
                detail: e.to_string(),
            })?;
            subjects.push(Subject {
                name: format!("dist/{file_name}"),
                digest_sha256: sha256_hex(&bytes),
            });
        }
    }
    subjects.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(subjects)
}

// ---------------------------------------------------------------------------
// The in-toto statement
// ---------------------------------------------------------------------------

/// The build facts the provenance's `buildDefinition` carries — collected
/// by the command layer (env, git, config hash) and consumed purely here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildFacts {
    /// The oxide version that ran the build.
    pub oxide_version: String,
    /// A hash over the config the build loaded (blake3 hex, `none` when no
    /// config file exists).
    pub config_hash: String,
    /// The `git rev-parse HEAD` of the built tree (`unknown` outside git).
    pub git_commit: String,
    /// Whether `git status --porcelain` was non-empty at build time.
    pub dirty: bool,
    /// `OXIDE_BUILDER_ID` or `local` (see [`builder_id_from`]).
    pub builder_id: String,
    /// The parsed `SOURCE_DATE_EPOCH`, when set.
    pub source_date_epoch: Option<u64>,
}

/// The spec-verbatim builder-id resolution: `OXIDE_BUILDER_ID` when set to
/// a non-blank value, `local` otherwise.
#[must_use]
pub fn builder_id_from(env: Option<&str>) -> String {
    match env.map(str::trim) {
        Some(id) if !id.is_empty() => id.to_string(),
        _ => "local".to_string(),
    }
}

/// The in-toto Statement v1 with the SLSA v1 predicate: the subjects verbatim,
/// `buildDefinition.externalParameters` carrying the facts, and
/// `internalParameters.sourceDateEpoch` only when the epoch was set (an
/// absent field is the determinism contract, not a missing value).
#[must_use]
pub fn provenance_statement(subjects: &[Subject], facts: &BuildFacts) -> serde_json::Value {
    let subject: Vec<serde_json::Value> = subjects
        .iter()
        .map(|s| {
            serde_json::json!({
                "digest": { "sha256": s.digest_sha256 },
                "name": s.name,
            })
        })
        .collect();

    let mut internal = serde_json::Map::new();
    if let Some(epoch) = facts.source_date_epoch {
        internal.insert(
            "sourceDateEpoch".to_string(),
            serde_json::Value::Number(serde_json::Number::from(epoch)),
        );
    }

    serde_json::json!({
        "_type": STATEMENT_TYPE,
        "predicate": {
            "buildDefinition": {
                "buildType": BUILD_TYPE,
                "externalParameters": {
                    "configHash": facts.config_hash,
                    "dirty": facts.dirty,
                    "gitCommit": facts.git_commit,
                    "oxideVersion": facts.oxide_version,
                },
                "internalParameters": serde_json::Value::Object(internal),
            },
            "runDetails": {
                "builder": { "id": facts.builder_id },
            },
        },
        "predicateType": PREDICATE_TYPE,
        "subject": subject,
    })
}

// ---------------------------------------------------------------------------
// Keys + signatures (ring ed25519)
// ---------------------------------------------------------------------------

/// Mint the 48-byte ed25519 PKCS#8 DER for a fixed 32-byte seed — how the
/// pins and the walkthrough example get a deterministic key without an
/// RNG. Operators keep using `openssl genpkey -algorithm ed25519`.
#[must_use]
pub fn seed_to_pkcs8(seed: &[u8; 32]) -> [u8; 48] {
    let mut der = [0_u8; 48];
    der[..16].copy_from_slice(&ED25519_PKCS8_PREFIX);
    der[16..].copy_from_slice(seed);
    der
}

/// Load an ed25519 private key from PEM PKCS#8 armor or a base64 DER blob.
/// (`from_pkcs8_maybe_unchecked` accepts the encoders OpenSSL and ring
/// produce, including the historical non-checksum variant.)
///
/// # Errors
/// [`AttestError::KeyMalformed`] when the armor or the DER is not an
/// ed25519 PKCS#8 private key.
pub fn load_signing_key(pem_or_base64: &str) -> Result<Ed25519KeyPair, AttestError> {
    let text = pem_or_base64.trim();
    let der: Vec<u8> = if let Some(body) = pem_section(text, "PRIVATE KEY") {
        decode_base64(&body)?
    } else {
        decode_base64(text)?
    };
    Ed25519KeyPair::from_pkcs8_maybe_unchecked(&der).map_err(|e| AttestError::KeyMalformed {
        detail: e.to_string(),
    })
}

/// Load an ed25519 private key from raw file bytes: PEM armor when the
/// file starts with the `-----BEGIN` marker, else raw 48-byte PKCS#8 DER,
/// else base64 text wrapping the DER — the command layer hands the key
/// file straight over.
///
/// # Errors
/// [`AttestError::KeyMalformed`] as for [`load_signing_key`].
pub fn load_signing_key_bytes(bytes: &[u8]) -> Result<Ed25519KeyPair, AttestError> {
    let text = String::from_utf8_lossy(bytes);
    if text.trim_start().starts_with("-----BEGIN") {
        return load_signing_key(&text);
    }
    if let Ok(pair) = Ed25519KeyPair::from_pkcs8_maybe_unchecked(bytes) {
        return Ok(pair);
    }
    let der = decode_base64(&text)?;
    Ed25519KeyPair::from_pkcs8_maybe_unchecked(&der).map_err(|e| AttestError::KeyMalformed {
        detail: e.to_string(),
    })
}

/// Resolve an ed25519 public key from SPKI PEM (`openssl pkey -pubout`),
/// a 64-char hex string or a 32-byte base64 blob.
///
/// # Errors
/// [`AttestError::PubkeyMalformed`] when none of the three shapes fit.
pub fn ed25519_pubkey_from_pem(text: &str) -> Result<Vec<u8>, AttestError> {
    let trimmed = text.trim();
    let bytes: Vec<u8> = if let Some(body) = pem_section(trimmed, "PUBLIC KEY") {
        let der = decode_base64(&body)?;
        if der.len() != 12 + 32 || der[..12] != ED25519_SPKI_PREFIX {
            return Err(AttestError::PubkeyMalformed {
                detail: "the SPKI DER is not an ed25519 key".to_string(),
            });
        }
        der[12..].to_vec()
    } else if trimmed.len() == 64 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        (0..32)
            .map(|i| {
                u8::from_str_radix(&trimmed[i * 2..i * 2 + 2], 16).map_err(|e| {
                    AttestError::PubkeyMalformed {
                        detail: e.to_string(),
                    }
                })
            })
            .collect::<Result<_, _>>()?
    } else {
        let der = decode_base64(trimmed)?;
        if der.len() != 32 {
            return Err(AttestError::PubkeyMalformed {
                detail: format!("expected 32 raw key bytes, got {}", der.len()),
            });
        }
        der
    };
    Ok(bytes)
}

/// The key id the signature envelope records: the hex of the 32-byte
/// ed25519 public key. ring exposes the public key through the
/// `signature::KeyPair` trait — brought into scope here, not at the call
/// sites (ring 0.17: `Ed25519KeyPair` has no inherent `public_key`).
#[must_use]
pub fn key_id(pair: &Ed25519KeyPair) -> String {
    use ring::signature::KeyPair as _;
    hex_lower(pair.public_key().as_ref())
}

/// The `provenance.sig` envelope: the signing key's id and the base64
/// ed25519 signature over the canonical statement bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureEnvelope {
    /// The hex public key id (see [`key_id`]).
    pub keyid: String,
    /// The base64 64-byte ed25519 signature.
    pub sig: String,
}

/// Sign the canonical JSON of `statement` — the exact bytes
/// [`canonical_json`] writes are the bytes the signature covers.
#[must_use]
pub fn sign_statement(statement: &serde_json::Value, pair: &Ed25519KeyPair) -> SignatureEnvelope {
    let message = canonical_json(statement);
    // ring 0.17's `Ed25519KeyPair::sign` returns the signature (an
    // `ed25519::Signature`, exactly 64 bytes per RFC 8032 §5.1 — ring does
    // not export a length constant, so the signature is carried as its
    // own bytes rather than through a fixed-size buffer).
    let signature = pair.sign(message.as_bytes());
    SignatureEnvelope {
        keyid: key_id(pair),
        sig: base64::engine::general_purpose::STANDARD.encode(signature.as_ref()),
    }
}

/// Verify a signature envelope over exact statement bytes. The caller
/// enforces canonicality first ([`is_canonical`]) so a whitespace-only
/// tamper is rejected before the crypto even runs.
///
/// # Errors
/// [`AttestError::SignatureMismatch`] when the envelope does not verify
/// (bad base64, wrong key, any flipped byte).
pub fn verify_signature(
    statement_bytes: &[u8],
    envelope: &SignatureEnvelope,
    pubkey: &[u8],
) -> Result<(), AttestError> {
    let signature = base64::engine::general_purpose::STANDARD
        .decode(envelope.sig.trim())
        .map_err(|_| AttestError::SignatureMismatch)?;
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, pubkey)
        .verify(statement_bytes, &signature)
        .map_err(|_| AttestError::SignatureMismatch)
}

/// Recompute every subject digest against the `dist` on disk. Extra files
/// under `dist/` that no subject names are not a failure — the check is
/// "each signed subject still matches", not "dist is byte-frozen".
///
/// # Errors
/// [`AttestError::SubjectMismatch`] for the first subject that is missing
/// or digests differently; [`AttestError::Io`] on read failures.
pub fn verify_subjects(dist: &Path, statement: &serde_json::Value) -> Result<(), AttestError> {
    let Some(subjects) = statement["subject"].as_array() else {
        return Err(AttestError::NotCanonical {
            detail: "the statement carries no subject array".to_string(),
        });
    };
    for subject in subjects {
        let name = subject["name"].as_str().unwrap_or_default();
        let expected = subject["digest"]["sha256"].as_str().unwrap_or_default();
        let rel = name.strip_prefix("dist/").unwrap_or(name);
        if rel.is_empty() {
            return Err(AttestError::SubjectMismatch {
                name: name.to_string(),
                detail: "the subject name is not dist-relative".to_string(),
            });
        }
        let actual = if rel.ends_with('/') {
            let dir = dist.join(rel);
            if !dir.is_dir() {
                return Err(AttestError::SubjectMismatch {
                    name: name.to_string(),
                    detail: "the directory is missing".to_string(),
                });
            }
            subtree_digest(&dir)?
        } else {
            match std::fs::read(dist.join(rel)) {
                Ok(bytes) => sha256_hex(&bytes),
                Err(e) => {
                    return Err(AttestError::SubjectMismatch {
                        name: name.to_string(),
                        detail: e.to_string(),
                    });
                }
            }
        };
        if actual != expected {
            return Err(AttestError::SubjectMismatch {
                name: name.to_string(),
                detail: "digest differs".to_string(),
            });
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The composer: write dist/build-id + dist/attest/
// ---------------------------------------------------------------------------

/// What `write_attestation` did — the fields `oxide build --attest` prints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttestReport {
    /// The A04 build id written to `dist/build-id` (16-hex).
    pub build_id: String,
    /// The SBOM path (absolute, under `dist/attest/`).
    pub sbom_path: std::path::PathBuf,
    /// The provenance path.
    pub provenance_path: std::path::PathBuf,
    /// The signature envelope path.
    pub signature_path: std::path::PathBuf,
    /// How many subjects the statement carries.
    pub subjects: usize,
    /// How many SBOM components the lockfiles yielded.
    pub components: usize,
    /// The signing key id recorded in the envelope.
    pub keyid: String,
}

/// The whole `--attest` flow over an already-built `dist`: compute and
/// write `dist/build-id` (the A04 canonical id via
/// `oxide_fw_cache::build_id_of_dist`), parse the project's lockfiles,
/// build the SBOM and the statement, sign, and write the three artifacts.
/// Every write is canonical bytes, so `oxide verify` re-reads them
/// verbatim.
///
/// # Errors
/// [`AttestError`] — the dist/build-id contract (`ATT-DIST`), lockfile
/// parsing (`ATT-LOCK`), or io (`ATT-IO`).
pub fn write_attestation(
    project_root: &Path,
    dist: &Path,
    facts: &BuildFacts,
    key: &Ed25519KeyPair,
) -> Result<AttestReport, AttestError> {
    let _span = tracing::info_span!(
        "attest_sbom",
        dist = %dist.display(),
        builder = %facts.builder_id
    )
    .entered();
    if !dist.is_dir() {
        return Err(AttestError::DistMissing {
            path: dist.to_string_lossy().into_owned(),
        });
    }

    // The A04 id closes the deferral the substrate needs: the statement's
    // first subject is the id file itself.
    let build_id = oxide_fw_cache::build_id_of_dist(dist).map_err(|e| AttestError::Io {
        detail: e.to_string(),
    })?;
    std::fs::write(dist.join(BUILD_ID_FILE), &build_id).map_err(|e| AttestError::Io {
        detail: e.to_string(),
    })?;

    // The binary's crates: the project's Cargo.lock when it exists (the
    // release-binaries CI builds inside the oxide workspace, where the
    // known-crates acceptance runs); a plain user app without one attests
    // with an empty cargo section (the deviation record documents it).
    let cargo_lock = project_root.join("Cargo.lock");
    let cargo = match std::fs::read_to_string(&cargo_lock) {
        Ok(text) => parse_cargo_lock(&text)?,
        Err(_) => Vec::new(),
    };
    // The bundle's npm packages: the project's pnpm-lock.yaml when it
    // exists. The "packages that reached a bundle" manifest filter is the
    // substrate's documented deferral — every locked package is listed.
    let pnpm_lock = project_root.join("pnpm-lock.yaml");
    let pnpm = match std::fs::read_to_string(&pnpm_lock) {
        Ok(text) => parse_pnpm_lock_packages(&text)?,
        Err(_) => Vec::new(),
    };

    let sbom = sbom_from_locks(&cargo, &pnpm, facts.source_date_epoch);
    let components = sbom["components"].as_array().map_or(0, Vec::len);
    let subjects = dist_subjects(dist)?;
    let statement = provenance_statement(&subjects, facts);
    let envelope = sign_statement(&statement, key);

    let attest_dir = dist.join(ATTEST_DIRNAME);
    std::fs::create_dir_all(&attest_dir).map_err(|e| AttestError::Io {
        detail: e.to_string(),
    })?;
    let sbom_path = attest_dir.join(SBOM_FILENAME);
    let provenance_path = attest_dir.join(PROVENANCE_FILENAME);
    let signature_path = attest_dir.join(SIGNATURE_FILENAME);
    std::fs::write(&sbom_path, canonical_json(&sbom).into_bytes()).map_err(|e| {
        AttestError::Io {
            detail: e.to_string(),
        }
    })?;
    let statement_bytes = canonical_json(&statement).into_bytes();
    std::fs::write(&provenance_path, &statement_bytes).map_err(|e| AttestError::Io {
        detail: e.to_string(),
    })?;
    let envelope_json = serde_json::to_string(&envelope).map_err(|e| AttestError::Io {
        detail: e.to_string(),
    })?;
    std::fs::write(&signature_path, envelope_json).map_err(|e| AttestError::Io {
        detail: e.to_string(),
    })?;
    tracing::debug!(
        subjects = subjects.len(),
        components,
        "attestation artifacts written"
    );

    Ok(AttestReport {
        build_id,
        sbom_path,
        provenance_path,
        signature_path,
        subjects: subjects.len(),
        components,
        keyid: envelope.keyid,
    })
}

// ---------------------------------------------------------------------------
// PEM helpers
// ---------------------------------------------------------------------------

/// Extract and concatenate the base64 body of a `-----BEGIN/END tag-----`
/// section (the body may wrap over any number of lines).
fn pem_section(text: &str, tag: &str) -> Option<String> {
    let begin = format!("-----BEGIN {tag}-----");
    let end = format!("-----END {tag}-----");
    let start = text.find(&begin)? + begin.len();
    let stop = text[start..].find(&end)? + start;
    let body: String = text[start..stop]
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if body.is_empty() { None } else { Some(body) }
}

fn decode_base64(text: &str) -> Result<Vec<u8>, AttestError> {
    base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .map_err(|e| AttestError::KeyMalformed {
            detail: format!("base64: {e}"),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The engines' own smoke pin lives in-module like the F18 bench-util
    /// modules; the full contract is `tests/attest_tdd.rs`.
    #[test]
    fn canonical_json_is_stable_under_reverse_insertion() {
        let mut map = serde_json::Map::new();
        map.insert("z".to_string(), serde_json::json!(1));
        map.insert("a".to_string(), serde_json::json!(2));
        assert_eq!(
            canonical_json(&serde_json::Value::Object(map)),
            r#"{"a":2,"z":1}"#
        );
    }

    #[test]
    fn the_pkcs8_prefix_is_the_standard_ed25519_wrapper() {
        let der = seed_to_pkcs8(&[7_u8; 32]);
        assert_eq!(der.len(), 48);
        assert_eq!(&der[..16], &ED25519_PKCS8_PREFIX);
    }
}
