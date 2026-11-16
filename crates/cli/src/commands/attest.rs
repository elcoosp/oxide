//! Moved verbatim from lib.rs by scripts/split_cli.py (V2-A01).

use crate::*;

/// The `--attest` tail of `oxide build` (§V2-F20): collect the build facts
/// (config hash, git state, builder id, `SOURCE_DATE_EPOCH`), resolve the
/// signing key (`--key` overrides `OXIDE_SIGNING_KEY`), and hand the pure
/// [`attest::write_attestation`] composer the dist this build emitted.
pub(crate) fn cmd_attest_write(
    rootp: &Path,
    dist: &Path,
    key_path: Option<&str>,
    w: Writer<'_>,
) -> anyhow::Result<()> {
    let _span = tracing::info_span!("cli_attest_build", root = %rootp.display()).entered();
    // The key: --key wins, then OXIDE_SIGNING_KEY; attesting unsigned is
    // not a mode — a missing key fails the build.
    let resolved = key_path
        .map(PathBuf::from)
        .or_else(|| std::env::var_os(attest::SIGNING_KEY_VAR).map(PathBuf::from))
        .ok_or_else(|| anyhow::Error::new(attest::AttestError::KeyMissing.diagnostic()))?;
    let key_bytes = std::fs::read(&resolved).map_err(|e| {
        anyhow::Error::new(
            attest::AttestError::KeyUnreadable {
                path: resolved.to_string_lossy().into_owned(),
                detail: e.to_string(),
            }
            .diagnostic(),
        )
    })?;
    let pair = attest::load_signing_key_bytes(&key_bytes)
        .map_err(|e| anyhow::Error::new(e.diagnostic()))?;

    // The facts: everything process-shaped stays here; the statement
    // builder is pure and pinned.
    let config_hash = config_fingerprint(rootp);
    let git_commit = git_output(rootp, ["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let dirty = git_output(rootp, ["status", "--porcelain"]).is_some_and(|s| !s.is_empty());
    let facts = attest::BuildFacts {
        oxide_version: env!("CARGO_PKG_VERSION").to_string(),
        config_hash,
        git_commit,
        dirty,
        builder_id: attest::builder_id_from(std::env::var(attest::BUILDER_ID_VAR).ok().as_deref()),
        source_date_epoch: std::env::var(attest::SOURCE_DATE_EPOCH_VAR)
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok()),
    };

    let report = attest::write_attestation(rootp, dist, &facts, &pair)
        .map_err(|e| anyhow::Error::new(e.diagnostic()))?;
    outln!(w, "attest: build-id {}", report.build_id);
    outln!(
        w,
        "attest: {} subjects, {} sbom components -> dist/{}",
        report.subjects,
        report.components,
        attest::ATTEST_DIRNAME
    );
    outln!(w, "attest: signed (keyid {})", report.keyid);
    Ok(())
}

/// `oxide verify` (§V2-F20): the canonical-form gate, the optional
/// signature check (`--pubkey`; skipped loudly without one — the envelope
/// records the key id, not the key), the subject-digest walk, and with
/// `--rebuild` the scratch-copy rebuild whose dist digests must match.
pub(crate) async fn cmd_verify(
    root: &str,
    dist_path: Option<&str>,
    pubkey_path: Option<&str>,
    rebuild: bool,
    w: Writer<'_>,
) -> anyhow::Result<i32> {
    let _span = tracing::info_span!("cli_attest_verify", root = %root).entered();
    let rootp = std::fs::canonicalize(root).unwrap_or_else(|_| PathBuf::from(root));
    let dist = dist_path
        .map(PathBuf::from)
        .unwrap_or_else(|| rootp.join("dist"));
    let attest_dir = dist.join(attest::ATTEST_DIRNAME);
    let provenance_path = attest_dir.join(attest::PROVENANCE_FILENAME);

    // Canonical gate first: a whitespace-only tamper dies here, before
    // the signature check could be fooled by a re-serialization.
    let bytes = std::fs::read(&provenance_path)
        .map_err(|_| anyhow::Error::new(attest::AttestError::BuildIdMissing.diagnostic()))?;
    if !attest::is_canonical(&bytes) {
        return Err(anyhow::Error::new(
            attest::AttestError::NotCanonical {
                detail: "provenance.intoto.json is not the canonical spelling".to_string(),
            }
            .diagnostic(),
        ));
    }
    outln!(w, "canonical: ok");
    let statement: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| {
        anyhow::Error::new(
            attest::AttestError::NotCanonical {
                detail: e.to_string(),
            }
            .diagnostic(),
        )
    })?;

    // The signature: verified only when a verifier key is supplied.
    match pubkey_path {
        None => outln!(
            w,
            "signature: skipped (pass --pubkey <file> to check keyid {})",
            signature_keyid(&attest_dir)?
        ),
        Some(spec) => {
            let pubkey = resolve_pubkey(spec)?;
            let envelope: attest::SignatureEnvelope = serde_json::from_str(
                &std::fs::read_to_string(attest_dir.join(attest::SIGNATURE_FILENAME)).map_err(
                    |e| {
                        anyhow::Error::new(
                            attest::AttestError::Io {
                                detail: e.to_string(),
                            }
                            .diagnostic(),
                        )
                    },
                )?,
            )
            .map_err(|_| anyhow::Error::new(attest::AttestError::SignatureMismatch.diagnostic()))?;
            attest::verify_signature(&bytes, &envelope, &pubkey)
                .map_err(|e| anyhow::Error::new(e.diagnostic()))?;
            outln!(w, "signature: verified (keyid {})", envelope.keyid);
        }
    }

    // The subjects: every signed digest must still match the dist.
    attest::verify_subjects(&dist, &statement).map_err(|e| anyhow::Error::new(e.diagnostic()))?;
    let subject_count = statement["subject"].as_array().map_or(0, Vec::len);
    outln!(w, "subjects: {subject_count} verified against dist");

    // V2-E04 tail: the manifest integrity walk — the mismatch-case loop
    // the attestation subjects cannot cover (the subjects are signed at
    // attest time over the dist they saw; this walk re-digests the client
    // manifest's `integrity` map against whatever the dist carries at
    // verify time). A clean walk prints the verified count; a missing
    // manifest (a non-client build, a pre-E04 dist) is a loud skip; a
    // dirty walk prints each finding and fails with `VRF-INTEGRITY` —
    // the same fail-closed verdict the subjects carry.
    match integrity_verdict(&dist) {
        IntegrityVerdict::Skipped(reason) => {
            outln!(w, "integrity: skipped ({reason})");
        }
        IntegrityVerdict::Corrupt => {
            return Err(anyhow::Error::new(oxide_fw_diagnostics::OxideError::new(
                "VRF-INTEGRITY",
                "client manifest integrity verification failed: dist/client/client.manifest.json does not type-parse",
            )));
        }
        IntegrityVerdict::Verified(checked) => {
            outln!(w, "integrity: {checked} files verified (sha384)");
        }
        IntegrityVerdict::Failed(report) => {
            for file in &report.missing {
                outln!(w, "integrity: MISSING {file}");
            }
            for finding in &report.mismatched {
                outln!(
                    w,
                    "integrity: MISMATCH {} (expected {}, got {})",
                    finding.file,
                    finding.expected,
                    finding.actual.as_deref().unwrap_or("unreadable")
                );
            }
            return Err(anyhow::Error::new(oxide_fw_diagnostics::OxideError::new(
                "VRF-INTEGRITY",
                format!(
                    "client manifest integrity verification failed: {} missing, {} mismatched",
                    report.missing.len(),
                    report.mismatched.len()
                ),
            )));
        }
    }

    // The reproducibility check: rebuild in a scratch copy and compare.
    if rebuild {
        let (match_, detail) = rebuild_and_compare(&rootp).await?;
        outln!(
            w,
            "rebuild: {}",
            if match_ {
                "dist digests match"
            } else {
                "MISMATCH"
            }
        );
        if !match_ {
            outln!(w, "rebuild detail: {detail}");
            return Ok(1);
        }
    }
    Ok(0)
}

/// The verify-time integrity verdict (the `VRF-INTEGRITY` step's shape).
/// `Skipped` names the reason loudly — a non-client build or a pre-E04
/// manifest (no `integrity` key) degrades to a skip rather than a failure,
/// mirroring the signature check's "skipped" verdict. `Corrupt` is the
/// fail-closed arm: a present-but-unparseable manifest is suspicious, not
/// absent (a tampered dist could corrupt the file to dodge the walk).
/// `Failed` carries the full finding list for the operator output.
pub(crate) enum IntegrityVerdict {
    Skipped(&'static str),
    Corrupt,
    Verified(usize),
    Failed(oxide_fw_bundler::sri::IntegrityReport),
}

impl std::fmt::Debug for IntegrityVerdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Skipped(reason) => f.debug_tuple("Skipped").field(reason).finish(),
            Self::Corrupt => f.write_str("Corrupt"),
            Self::Verified(checked) => f.debug_tuple("Verified").field(checked).finish(),
            Self::Failed(report) => f
                .debug_struct("Failed")
                .field("missing", &report.missing)
                .field("mismatched", &report.mismatched)
                .finish(),
        }
    }
}

/// The `oxide verify` integrity step's decision: read
/// `dist/client/client.manifest.json`, type-parse it, and walk its
/// `integrity` map against the bytes under `dist/client/`. Extracted as a
/// pure (fs-only) decision so the verdict mapping is pinned without
/// spawning the attestation flow around it.
pub(crate) fn integrity_verdict(dist: &Path) -> IntegrityVerdict {
    let manifest_path = dist.join("client/client.manifest.json");
    let Ok(text) = std::fs::read_to_string(&manifest_path) else {
        return IntegrityVerdict::Skipped("no client manifest");
    };
    let Ok(manifest) = serde_json::from_str::<oxide_fw_bundler::ClientManifest>(&text) else {
        return IntegrityVerdict::Corrupt;
    };
    if manifest.integrity.is_empty() {
        return IntegrityVerdict::Skipped("manifest carries no integrity map");
    }
    let report =
        oxide_fw_bundler::sri::verify_integrity_map(&dist.join("client"), &manifest.integrity);
    if report.is_clean() {
        IntegrityVerdict::Verified(report.checked)
    } else {
        IntegrityVerdict::Failed(report)
    }
}

/// The key id recorded in the signature envelope (printed when the
/// signature check is skipped, so the operator knows whose key to supply).
pub(crate) fn signature_keyid(attest_dir: &Path) -> anyhow::Result<String> {
    let raw =
        std::fs::read_to_string(attest_dir.join(attest::SIGNATURE_FILENAME)).map_err(|e| {
            anyhow::Error::new(
                attest::AttestError::Io {
                    detail: e.to_string(),
                }
                .diagnostic(),
            )
        })?;
    let envelope: attest::SignatureEnvelope = serde_json::from_str(&raw)
        .map_err(|_| anyhow::Error::new(attest::AttestError::SignatureMismatch.diagnostic()))?;
    Ok(envelope.keyid)
}

/// Resolve `--pubkey`: a path that exists is read; anything else is the
/// key material itself (SPKI PEM, 64-char hex or 32-byte base64).
pub(crate) fn resolve_pubkey(spec: &str) -> anyhow::Result<Vec<u8>> {
    let material = match std::fs::read_to_string(spec) {
        Ok(text) => text,
        Err(_) => spec.to_string(),
    };
    attest::ed25519_pubkey_from_pem(&material).map_err(|e| anyhow::Error::new(e.diagnostic()))
}

/// The `--rebuild` check: copy the project (minus `dist/`, `target/`,
/// `node_modules/`, `.git/`, `.oxide/`) into a scratch dir under the
/// system temp, build it with the shared cache and `OXIDE_FORCE_BUILD=1`,
/// and compare every statement subject (except `dist/build-id` — the
/// scratch build runs without `--attest`, and the id is derived from the
/// same content the other subjects pin) against the rebuilt tree.
/// Returns `(match, detail)`.
pub(crate) async fn rebuild_and_compare(rootp: &Path) -> anyhow::Result<(bool, String)> {
    // std-only scratch: a per-pid directory, swept whatever the outcome.
    let scratch = std::env::temp_dir().join(format!("oxide-verify-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let outcome = rebuild_in(&scratch, rootp).await;
    let _ = std::fs::remove_dir_all(&scratch);
    outcome
}

pub(crate) async fn rebuild_in(scratch: &Path, rootp: &Path) -> anyhow::Result<(bool, String)> {
    copy_project(rootp, scratch)?;
    let exe = std::env::current_exe()?;
    let status = tokio::process::Command::new(&exe)
        .args(["build", "--root"])
        .arg(scratch)
        .env("OXIDE_FORCE_BUILD", "1")
        .env(
            perf_bisect::CACHE_DIR_VAR,
            perf_bisect::cache_dir_for(rootp),
        )
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map_err(|e| anyhow::anyhow!("the scratch build did not spawn: {e}"))?;
    if !status.success() {
        return Ok((false, "the scratch rebuild failed to build".to_string()));
    }
    let statement: serde_json::Value = serde_json::from_slice(&std::fs::read(
        rootp
            .join("dist")
            .join(attest::ATTEST_DIRNAME)
            .join(attest::PROVENANCE_FILENAME),
    )?)?;
    let rebuilt_dist = scratch.join("dist");
    let subjects = statement["subject"].as_array().cloned().unwrap_or_default();
    if subjects.is_empty() {
        return Ok((false, "the statement carries no subjects".to_string()));
    }
    for row in &subjects {
        let name = row["name"].as_str().unwrap_or_default();
        let expected = row["digest"]["sha256"].as_str().unwrap_or_default();
        if name == format!("dist/{}", attest::BUILD_ID_FILE) || name.is_empty() {
            continue;
        }
        let rel = name.strip_prefix("dist/").unwrap_or(name);
        let target = rebuilt_dist.join(rel);
        let actual = if rel.ends_with('/') {
            if !target.is_dir() {
                return Ok((false, format!("{name} is missing from the rebuild")));
            }
            match attest::subtree_digest(&target) {
                Ok(d) => d,
                Err(e) => return Ok((false, e.to_string())),
            }
        } else {
            match std::fs::read(&target) {
                Ok(bytes) => attest::sha256_hex(&bytes),
                Err(_) => return Ok((false, format!("{name} is missing from the rebuild"))),
            }
        };
        if actual != expected {
            return Ok((false, format!("{name} digests {actual} != {expected}")));
        }
    }
    Ok((true, String::new()))
}

/// Copy the project sources into `dst`, skipping the directories the build
/// regenerates or fetches (dist, target, node_modules, .git, .oxide).
pub(crate) fn copy_project(rootp: &Path, dst: &Path) -> anyhow::Result<()> {
    const SKIP: [&str; 5] = ["dist", "target", "node_modules", ".git", ".oxide"];
    fn walk(src: &Path, dst: &Path) -> anyhow::Result<()> {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if SKIP.contains(&name.as_ref()) {
                continue;
            }
            let from = entry.path();
            let to = dst.join(&*name);
            if from.is_dir() {
                walk(&from, &to)?;
            } else {
                std::fs::copy(&from, &to)?;
            }
        }
        Ok(())
    }
    walk(rootp, dst)
}

/// blake3 hex over the project's config file bytes (`oxide.config.ts`, then
/// `.json`); `none` when the project ships neither.
pub(crate) fn config_fingerprint(rootp: &Path) -> String {
    for name in ["oxide.config.ts", "oxide.config.json"] {
        let path = rootp.join(name);
        if path.is_file()
            && let Ok(bytes) = std::fs::read(&path)
        {
            let mut h = blake3::Hasher::new();
            h.update(&bytes);
            return h.finalize().to_hex().to_string();
        }
    }
    "none".to_string()
}

/// `git -C <dir> <args>` stdout, trimmed — `None` when git is absent or
/// the tree is not a repository (the fact then says `unknown`/clean).
pub(crate) fn git_output(
    dir: &Path,
    args: impl IntoIterator<Item = &'static str>,
) -> Option<String> {
    let mut cmd = std::process::Command::new("git");
    cmd.arg("-C").arg(dir).args(args);
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}
