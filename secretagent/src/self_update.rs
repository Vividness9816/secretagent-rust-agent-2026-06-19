//! `secretagent self-update` — replace the running binary with a newer signed release (Phase 6h).
//!
//! RCE-grade: the new binary is accepted ONLY after a complete, fail-closed chain. (1) A detached
//! minisign signature over the release manifest verifies against a public key PINNED in this binary
//! (a `const`, never fetched). (2) The manifest version is strictly newer (no-downgrade — the
//! version is read from the SIGNED payload, never the filename/tag/the untrusted binary). (3) The
//! downloaded binary's sha256 matches the signed manifest. (4) An atomic rename swaps it into place
//! and an audit event records the change. Nothing ever executes the downloaded bytes — they are
//! verified, placed on disk, and the operator restarts.
//!
//! The update client is operator-frozen (`[update] base_url`, never model-reachable) and lives
//! OUTSIDE the egress seam — it MAY follow redirects (a GitHub release → CDN) because trust comes
//! from the pinned signature + the manifest sha256, NOT from host-pinning. `self-update` is a CLI
//! subcommand, so a Remote/cron principal can never invoke it.

use anyhow::{ensure, Context, Result};
use sa_audit::{Audit, AuditEvent};
use sa_core_types::config;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

/// OPERATOR-GATED TRUST ANCHOR. Paste your minisign public key's base64 line here (the SAME key the
/// 6b release pipeline signs with — see docs/RELEASE.md). EMPTY = self-update is INERT / fail-closed
/// (the safe default for an RCE primitive). Compiled into the binary; NEVER fetched.
const PINNED_MINISIGN_PUBKEY_B64: &str = "";

/// The target triple this binary was built for (from build.rs) — picks the right artifact.
const TARGET: &str = env!("SA_TARGET");

/// A signed release manifest — the SIGNED payload. `version` drives no-downgrade; each artifact's
/// `sha256` binds the downloaded binary to the signature.
#[derive(Debug, Deserialize)]
struct Manifest {
    version: String,
    artifacts: BTreeMap<String, Artifact>,
}

#[derive(Debug, Deserialize)]
struct Artifact {
    url: String,
    sha256: String,
}

/// True once the operator has pinned a minisign key (doctor surfaces this).
pub fn is_pinned() -> bool {
    !PINNED_MINISIGN_PUBKEY_B64.trim().is_empty()
}

/// The pinned trust anchor, or a clear fail-closed error if the operator hasn't configured it.
fn pinned_pubkey_b64() -> Result<&'static str> {
    let k = PINNED_MINISIGN_PUBKEY_B64.trim();
    ensure!(
        !k.is_empty(),
        "self-update is not configured: pin your minisign public key in self_update.rs \
         (PINNED_MINISIGN_PUBKEY_B64) — the operator-gated finish, see docs/RELEASE.md"
    );
    Ok(k)
}

/// Verify the detached minisign signature over `bytes` against `pubkey_b64`, THEN parse the manifest.
/// The bytes are deserialized ONLY after the signature verifies — untrusted bytes are never parsed
/// as trusted. Fail-closed on any signature or parse error.
fn verify_manifest(bytes: &[u8], minisig: &str, pubkey_b64: &str) -> Result<Manifest> {
    use minisign_verify::{PublicKey, Signature};
    let pk =
        PublicKey::from_base64(pubkey_b64).map_err(|e| anyhow::anyhow!("bad pinned key: {e}"))?;
    let sig = Signature::decode(minisig).map_err(|e| anyhow::anyhow!("bad signature: {e}"))?;
    // allow_legacy=false: require a prehashed signature (what the minisign signer emits by default).
    pk.verify(bytes, &sig, false).map_err(|e| {
        anyhow::anyhow!("manifest signature did NOT verify against the pinned key: {e}")
    })?;
    let m: Manifest =
        serde_json::from_slice(bytes).context("parsing the (verified) manifest JSON")?;
    Ok(m)
}

/// (major, minor, patch), ignoring any `-prerelease`/`+build` suffix (pre-release ORDERING is not
/// handled — a documented v1 limit; a real release is x.y.z). Errors on a non-3-numeric core.
fn parse_semver(v: &str) -> Result<(u64, u64, u64)> {
    let core = v.split(['-', '+']).next().unwrap_or(v);
    let parts: Vec<&str> = core.split('.').collect();
    ensure!(parts.len() == 3, "not an x.y.z version: {v:?}");
    let n = |s: &str| {
        s.parse::<u64>()
            .map_err(|_| anyhow::anyhow!("non-numeric version field in {v:?}"))
    };
    Ok((n(parts[0])?, n(parts[1])?, n(parts[2])?))
}

/// No-downgrade: the candidate must be STRICTLY newer than current (equal = refused no-op).
fn ensure_upgrade(current: &str, candidate: &str) -> Result<()> {
    let (cur, cand) = (parse_semver(current)?, parse_semver(candidate)?);
    ensure!(
        cand > cur,
        "refusing a non-upgrade: candidate {candidate} is not newer than current {current}"
    );
    Ok(())
}

/// Bind the downloaded bytes to the signed manifest: their sha256 must equal `expected_hex`
/// (case-insensitive). Fail-closed on mismatch (a tampered/substituted binary).
fn ensure_sha256(bytes: &[u8], expected_hex: &str) -> Result<()> {
    use sha2::{Digest, Sha256};
    let got = format!("{:x}", Sha256::digest(bytes));
    ensure!(
        got.eq_ignore_ascii_case(expected_hex.trim()),
        "downloaded binary sha256 {got} != signed manifest {} — refusing",
        expected_hex.trim()
    );
    Ok(())
}

fn select_artifact<'a>(m: &'a Manifest, target: &str) -> Result<&'a Artifact> {
    m.artifacts
        .get(target)
        .with_context(|| format!("the signed manifest has no artifact for this target ({target})"))
}

/// Atomically replace `target` with `temp` (same filesystem). Unix: a single rename (the running
/// process keeps the old inode). Windows: a running exe can be RENAMED (not overwritten in place),
/// so move the current exe aside, then rename the new one into place.
fn atomic_replace(target: &Path, temp: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        let old = target.with_extension("old");
        let _ = std::fs::remove_file(&old);
        let moved_aside = target.exists();
        if moved_aside {
            std::fs::rename(target, &old).context("moving the current exe aside (windows)")?;
        }
        if let Err(e) = std::fs::rename(temp, target) {
            // Roll back so a failed swap is a no-op — never leave the install path empty.
            if moved_aside {
                let _ = std::fs::rename(&old, target);
            }
            return Err(e).context("renaming the new exe into place (windows)");
        }
        // Best-effort: a still-running exe.old can't be deleted until the process exits.
        let _ = std::fs::remove_file(&old);
    }
    #[cfg(not(windows))]
    {
        std::fs::rename(temp, target).context("atomic rename of the new binary")?;
    }
    Ok(())
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    Ok(())
}
#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

/// latest.json + its .minisig are tiny; a self-contained binary is large but bounded. The caps
/// protect against a memory/disk DoS from a compromised mirror/redirect hop BEFORE verification
/// (Content-Length can be absent or lie, so the streamed-byte counter is the real guard).
const MANIFEST_CAP: usize = 256 * 1024; // 256 KiB
const BINARY_CAP: usize = 256 * 1024 * 1024; // 256 MiB

/// The operator-frozen update client. Outside the egress seam: redirects ARE followed (release →
/// CDN) because trust comes from the pinned signature + the manifest sha256, not host-pinning. The
/// connect + overall timeout bounds a slow-loris stall; the per-call size cap bounds an endless body.
fn update_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .context("building the update client")
}

async fn fetch(client: &reqwest::Client, url: &str, max: usize) -> Result<Vec<u8>> {
    let mut resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?
        .error_for_status()
        .with_context(|| format!("GET {url} status"))?;
    if let Some(len) = resp.content_length() {
        ensure!(
            len as usize <= max,
            "GET {url}: declared {len} bytes exceeds the {max}-byte cap"
        );
    }
    let mut buf = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .with_context(|| format!("reading {url}"))?
    {
        ensure!(
            buf.len() + chunk.len() <= max,
            "GET {url}: body exceeds the {max}-byte cap"
        );
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

/// Run self-update. `check_only` stops after the no-downgrade check (no download, no swap).
pub async fn run(check_only: bool) -> Result<()> {
    let cfg = config::Config::load()?;
    let base = cfg
        .update
        .base_url
        .as_deref()
        .filter(|s| !s.is_empty())
        .context("self-update is not configured: set [update] base_url in config.toml")?
        .trim_end_matches('/')
        .to_string();
    let pubkey = pinned_pubkey_b64()?;
    let client = update_client()?;

    // 1. Fetch + verify the signed manifest (capped; signature THEN parse).
    let manifest_bytes = fetch(&client, &format!("{base}/latest.json"), MANIFEST_CAP).await?;
    let minisig = String::from_utf8(
        fetch(
            &client,
            &format!("{base}/latest.json.minisig"),
            MANIFEST_CAP,
        )
        .await?,
    )
    .context("manifest signature is not UTF-8")?;
    let manifest = verify_manifest(&manifest_bytes, &minisig, pubkey)?;

    // 2. No-downgrade (version from the SIGNED manifest).
    let current = env!("CARGO_PKG_VERSION");
    ensure_upgrade(current, &manifest.version)?;
    println!(
        "update available: {current} -> {} ({TARGET})",
        manifest.version
    );
    if check_only {
        return Ok(());
    }

    // 3. Download the binary (capped) + bind it to the signed sha256.
    let art = select_artifact(&manifest, TARGET)?;
    let bin = fetch(&client, &art.url, BINARY_CAP).await?;
    ensure_sha256(&bin, &art.sha256)?;

    // 4. Stage the verified bytes with an UNPREDICTABLE name + O_EXCL in the exe's dir (same fs →
    // atomic rename), so a pre-planted file/symlink there can't be followed or raced (the bytes are
    // already sha256-bound, but we still never write THROUGH an attacker-planted path).
    let exe = std::env::current_exe().context("locating the running binary")?;
    let dir = exe
        .parent()
        .context("the running binary has no parent dir")?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let temp = dir.join(format!(
        ".secretagent-update-{}-{stamp}",
        std::process::id()
    ));
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true) // O_EXCL: never follow/clobber a pre-existing path or symlink
            .open(&temp)
            .with_context(|| format!("creating staging file {temp:?}"))?;
        f.write_all(&bin)
            .with_context(|| format!("writing {temp:?}"))?;
    }
    make_executable(&temp)?;

    // 5. Audit the operator's initiation BEFORE the irreversible swap (fsync'd so the record
    // survives a crash of the swap itself — the append_synced-before-dispatch rule, ADR-20260620).
    {
        let mut audit = Audit::open(&config::audit_path())?;
        if let Err(e) = audit.append_synced(AuditEvent {
            action: "self_update".into(),
            key_id: manifest.version.clone(),
            principal: Some("operator".into()),
        }) {
            let _ = std::fs::remove_file(&temp);
            return Err(e);
        }
    }

    // 6. Atomic swap (the Windows arm rolls back if the second rename fails).
    if let Err(e) = atomic_replace(&exe, &temp) {
        let _ = std::fs::remove_file(&temp);
        return Err(e);
    }
    println!(
        "updated to {} — restart secretagent to run the new version",
        manifest.version
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"{"version":"1.2.3","artifacts":{"x86_64-unknown-linux-musl":{"url":"https://example.invalid/bin","sha256":"deadbeef"}}}"#;

    /// Sign `bytes` with a fresh throwaway minisign keypair (dev-only; the signer prehashes).
    fn test_sign(bytes: &[u8]) -> (String, String) {
        let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let sig = minisign::sign(None, &kp.sk, bytes, Some("trusted"), Some("untrusted")).unwrap();
        (kp.pk.to_base64(), sig.to_string())
    }

    #[test]
    fn signed_manifest_verifies_and_parses() {
        let (pk, sig) = test_sign(MANIFEST.as_bytes());
        let m = verify_manifest(MANIFEST.as_bytes(), &sig, &pk).unwrap();
        assert_eq!(m.version, "1.2.3");
        assert!(m.artifacts.contains_key("x86_64-unknown-linux-musl"));
    }

    #[test]
    fn tampered_manifest_is_rejected() {
        let (pk, sig) = test_sign(MANIFEST.as_bytes());
        let mut tampered = MANIFEST.as_bytes().to_vec();
        let i = MANIFEST.find("1.2.3").unwrap();
        tampered[i] = b'9'; // 1.2.3 -> 9.2.3
        assert!(
            verify_manifest(&tampered, &sig, &pk).is_err(),
            "a tampered manifest must NOT verify"
        );
    }

    #[test]
    fn wrong_key_is_rejected() {
        let (_pk_a, sig_a) = test_sign(MANIFEST.as_bytes());
        let (pk_b, _sig_b) = test_sign(b"unrelated"); // a DIFFERENT keypair's pubkey
        assert!(
            verify_manifest(MANIFEST.as_bytes(), &sig_a, &pk_b).is_err(),
            "a signature from key A must not verify under key B's pubkey"
        );
    }

    #[test]
    fn downgrade_and_equal_are_refused() {
        assert!(ensure_upgrade("0.2.0", "0.3.0").is_ok());
        assert!(ensure_upgrade("1.0.0", "1.0.1").is_ok());
        assert!(
            ensure_upgrade("0.2.0", "0.2.0").is_err(),
            "an equal version is not an upgrade"
        );
        assert!(
            ensure_upgrade("0.2.0", "0.1.9").is_err(),
            "a lower version is a downgrade"
        );
    }

    #[test]
    fn sha256_mismatch_is_refused() {
        let bytes = b"the new binary";
        let good = {
            use sha2::{Digest, Sha256};
            format!("{:x}", Sha256::digest(bytes))
        };
        assert!(ensure_sha256(bytes, &good).is_ok());
        assert!(
            ensure_sha256(bytes, &good.to_uppercase()).is_ok(),
            "case-insensitive"
        );
        assert!(
            ensure_sha256(bytes, "deadbeef").is_err(),
            "a wrong hash (tampered/substituted binary) must be refused"
        );
    }

    #[test]
    fn select_artifact_errors_on_missing_target() {
        let (pk, sig) = test_sign(MANIFEST.as_bytes());
        let m = verify_manifest(MANIFEST.as_bytes(), &sig, &pk).unwrap();
        assert!(select_artifact(&m, "x86_64-unknown-linux-musl").is_ok());
        assert!(select_artifact(&m, "no-such-target").is_err());
    }

    #[test]
    fn self_update_is_inert_until_a_key_is_pinned() {
        // Ships EMPTY → fail-closed (the safe default for an RCE primitive).
        assert!(!is_pinned());
        assert!(pinned_pubkey_b64().is_err());
    }

    #[test]
    fn atomic_replace_swaps_file_contents() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("bin");
        let temp = dir.path().join("bin.tmp");
        std::fs::write(&target, b"OLD").unwrap();
        std::fs::write(&temp, b"NEW").unwrap();
        atomic_replace(&target, &temp).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"NEW");
        assert!(!temp.exists(), "the temp file is consumed by the rename");
    }
}
