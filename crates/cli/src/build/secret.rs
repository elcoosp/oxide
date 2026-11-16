//! Moved verbatim from lib.rs by scripts/split_cli.py (V2-A01).

/// §9.4 / BUG-05: read (or create) the per-project build secret.
///
/// Stored at `<root>/.oxide/secret` as 64 hex chars (32 bytes of entropy,
/// blake3-folded). A missing file is created once from the OS CSPRNG; every
/// subsequent build reads the same value, so server-fn ids and manifests are
/// byte-stable across builds. Deleting the file (or `oxide secrets rotate`)
/// rotates the secret — server-fn ids then change, which is the intended
/// effect of a rotation.
///
/// A malformed file (wrong length, non-hex) is a hard error: silently
/// regenerating would churn every fn id without telling anyone.
pub(crate) fn build_secret(rootp: &std::path::Path) -> anyhow::Result<String> {
    let key_file = rootp.join(".oxide/secret");
    if let Ok(existing) = std::fs::read_to_string(&key_file) {
        let s = existing.trim();
        if s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Ok(s.to_string());
        }
        anyhow::bail!(
            "{} is malformed (expected 64 hex chars); delete it or run `oxide secrets rotate`",
            key_file.display()
        );
    }
    // Cross-platform random: ring::rand::SystemRandom uses /dev/urandom
    // on Linux/macOS and BCryptGenRandom on Windows. The prior code
    // opened "/dev/urandom" directly, which does not exist on Windows —
    // every build_secret() call failed there with
    // "cannot read /dev/urandom for the build secret: The system cannot
    // find the path specified. (os error 3)".
    use ring::rand::SecureRandom as _;
    let mut rnd = [0u8; 32];
    ring::rand::SystemRandom::new()
        .fill(&mut rnd)
        .map_err(|_| anyhow::anyhow!("cannot generate random bytes for the build secret"))?;
    let s = blake3::hash(&rnd).to_hex().to_string();
    std::fs::create_dir_all(rootp.join(".oxide"))
        .map_err(|e| anyhow::anyhow!("cannot create .oxide/: {e}"))?;
    std::fs::write(&key_file, &s)
        .map_err(|e| anyhow::anyhow!("cannot write {}: {e}", key_file.display()))?;
    Ok(s)
}
