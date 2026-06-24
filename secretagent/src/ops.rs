//! `secretagent {backup,restore,export}` — operational data-dir backup/restore + a secret-free
//! trajectory export (Phase 6g, ADR-20260623 slice 6g).
//!
//! - **backup** snapshots `memory.db` via the SQLite **Online Backup API** (never `cp` a live WAL
//!   DB) and byte-copies the encrypted `store.age` / `identity.age` / `audit.jsonl` into a dir. The
//!   age vault stays ENCRYPTED (we copy ciphertext, never decrypt); the identity travels with it so
//!   the backup is decryptable — which makes the backup dir as sensitive as the data dir, so every
//!   backed-up artifact is `chmod 600` and the dir `chmod 700` (unix), and the operator is warned.
//! - **restore** copies those artifacts back into `data_dir()`, removes any stale WAL sidecars (so
//!   SQLite can't replay an old WAL onto the restored DB), `chmod 600`s the restored artifacts, and
//!   verifies the restored audit hash-chain (loud, never silent). It overwrites the live data dir.
//! - **export** writes a session's `messages` + the audit log as JSONL with recognizable secrets
//!   REDACTED (content AND provenance), and is fail-closed: it re-scans the assembled artifact and
//!   refuses to write if any recognizable secret survived (secret-free is an enforced postcondition).
//!
//! Both backup and restore REFUSE to target the live data dir itself (a self-copy truncates the
//! files to zero) and warn loudly on an incoherent {DB, vault} set. Scope = the four data-dir
//! artifacts (operational state); `config.toml`/`SOUL.md`/`context.md` are operator-authored config
//! in `config_dir()` — out of scope (documented in the 6g plan).
//!
//! Accepted residuals (6g review, documented): the per-file copy is not temp+rename atomic (an
//! interrupted restore is recoverable by re-running from the intact backup); `export --out`
//! overwrites its target (operator-trusted CLI, same class as `restore`); `verify_chain` cannot
//! detect a CLEAN tail truncation (a signed head anchor is the named upgrade — see sa-audit).

use anyhow::{bail, ensure, Context, Result};
use sa_core_types::config;
use sa_memory::{looks_like_secret, Store};
use std::path::{Path, PathBuf};

/// Whole-field replacement for a field that carries a recognizable secret (never partial — a
/// substring redaction could leak across the boundary). Chosen so it can't itself trip the detector.
const REDACTED: &str = "[redacted]";

/// The flat (whole-file) artifacts a backup/restore moves verbatim. `memory.db` is handled
/// separately via the Online Backup API (it is the only one that may be a live WAL DB).
const FLAT_FILES: [&str; 3] = ["identity.age", "store.age", "audit.jsonl"];

/// Refuse a backup/restore that targets the live data dir itself — `std::fs::copy` of a file onto
/// itself truncates it to zero (it would destroy the very identity/vault being protected).
fn ensure_not_data_dir(other: &Path, data: &Path) -> Result<()> {
    if let (Ok(a), Ok(b)) = (other.canonicalize(), data.canonicalize()) {
        ensure!(
            a != b,
            "the backup directory must differ from the live data dir {data:?}"
        );
    }
    Ok(())
}

/// Warn loudly (never silently) when the coupled set is incomplete: a DB without its vault (or a
/// vault without its DB) mixes epochs / yields an undecryptable or empty agent.
fn warn_if_incoherent(present: &[&str]) {
    let db = present.contains(&"memory.db");
    let vault = present.contains(&"store.age") || present.contains(&"identity.age");
    if db && !vault {
        println!(
            "WARNING: memory.db is present but the vault (store.age/identity.age) is NOT — \
             the DB and vault may be from different epochs (incoherent set)."
        );
    } else if vault && !db {
        println!("WARNING: the vault is present but memory.db is NOT — incomplete set.");
    }
}

/// Snapshot the data dir into `dest` (created if absent).
pub fn backup(dest: &Path) -> Result<()> {
    let data = config::data_dir();
    std::fs::create_dir_all(dest).with_context(|| format!("creating backup dir {dest:?}"))?;
    ensure_not_data_dir(dest, &data)?;
    chmod_700_dir(dest)?; // the backup holds the private key — lock the dir down (unix)
    println!(
        "NOTE: for a fully cross-artifact-coherent backup, stop the gateway/daemon first; a live \
         backup is per-artifact-consistent but the DB snapshot and audit.jsonl may differ by ms."
    );
    let mut done: Vec<&str> = Vec::new();

    // memory.db — Online Backup API (a consistent snapshot even while the daemon holds the DB
    // open in WAL mode). NEVER a byte copy of a live WAL DB.
    if data.join("memory.db").exists() {
        let store = Store::open(&data.join("memory.db")).context("opening live DB for backup")?;
        store.backup_to(&dest.join("memory.db"))?;
        done.push("memory.db");
    }

    // The flat files are append-only / replace-whole, so a byte copy is consistent.
    for name in FLAT_FILES {
        let src = data.join(name);
        if src.exists() {
            std::fs::copy(&src, dest.join(name)).with_context(|| format!("copying {name}"))?;
            done.push(name);
        }
    }
    // Every backed-up artifact is as sensitive as the live data dir — lock each to 0600 (unix).
    for name in &done {
        chmod_600(&dest.join(name))?;
    }

    if done.is_empty() {
        println!("nothing to back up (no data dir artifacts at {data:?})");
    } else {
        warn_if_incoherent(&done);
        println!(
            "backed up to {dest:?}: {} (memory.db = online snapshot)",
            done.join(", ")
        );
        println!(
            "WARNING: this backup contains identity.age (your private key) + the encrypted vault — \
             protect it exactly like the live data dir."
        );
    }
    Ok(())
}

/// Restore a backup directory into `data_dir()`. Overwrites existing artifacts.
pub fn restore(src: &Path) -> Result<()> {
    ensure!(src.is_dir(), "backup source {src:?} is not a directory");
    let data = config::data_dir();
    std::fs::create_dir_all(&data)?;
    ensure_not_data_dir(src, &data)?;
    println!(
        "WARNING: restore overwrites the live data dir — stop the gateway/daemon first if it is running."
    );
    let mut done: Vec<&str> = Vec::new();
    // memory.db is restored like any flat file — the backup wrote a self-contained DB (no sidecar).
    for name in ["memory.db", "identity.age", "store.age", "audit.jsonl"] {
        let from = src.join(name);
        if from.exists() {
            std::fs::copy(&from, data.join(name)).with_context(|| format!("restoring {name}"))?;
            done.push(name);
        }
    }
    // Remove any stale WAL sidecars a prior live daemon left behind, so SQLite does NOT replay old
    // frames onto the freshly-restored self-contained DB (corruption / data loss — 6g review HIGH).
    for s in ["memory.db-wal", "memory.db-shm"] {
        let p = data.join(s);
        if p.exists() {
            std::fs::remove_file(&p).with_context(|| format!("removing stale {s}"))?;
        }
    }
    // Lock the restored artifacts to 0600 (unix) — never inherit the transport's mode bits.
    for name in &done {
        chmod_600(&data.join(name))?;
    }

    if done.is_empty() {
        println!("nothing restored (no recognized artifacts in {src:?})");
        return Ok(());
    }
    warn_if_incoherent(&done);
    println!("restored into {data:?}: {}", done.join(", "));

    // Verify the restored audit hash-chain — report loudly, never silently (and never fatal here).
    // Honest scope: a clean tail truncation is not detectable by a bare chain (see sa-audit).
    let ap = config::audit_path();
    if ap.exists() {
        match sa_audit::Audit::verify_chain(&ap) {
            Ok(true) => println!("audit chain: internally consistent"),
            Ok(false) => {
                println!("audit chain: UNVERIFIED (torn tail or tamper) — inspect {ap:?}")
            }
            Err(e) => println!("audit chain: could not verify: {e}"),
        }
    }
    Ok(())
}

/// Export `session`'s trajectory (messages + audit) to JSONL, secret-free. Writes to `out` or
/// `data_dir()/trajectory-<session>.jsonl`.
pub fn export(session: &str, out: Option<PathBuf>) -> Result<()> {
    let store = Store::open(&config::db_path())?;
    let msgs = store.all_messages(session)?;
    let provs = store.message_provenances(session)?;
    // Both query `ORDER BY id` for the same session → aligned 1:1.
    ensure!(
        msgs.len() == provs.len(),
        "message/provenance count mismatch ({} vs {})",
        msgs.len(),
        provs.len()
    );

    let mut lines: Vec<String> = Vec::new();
    for (m, prov) in msgs.iter().zip(provs.iter()) {
        // Redact BOTH content and provenance (provenance is connector/sender-derived for a remote
        // turn — the more attacker-influenced field; symmetric whole-field redaction avoids both a
        // leak and the fail-closed-DoS edge if a recognizable secret ever lands in it).
        let rec = serde_json::json!({
            "type": "message",
            "role": m.role,
            "provenance": redact(prov),
            "content": redact(&m.content),
        });
        lines.push(rec.to_string());
    }
    // Audit events are global (the log is not session-tagged — a session column is the named
    // upgrade) and secret-free by construction (key NAMES + principal only).
    for ev in sa_audit::Audit::read_events(&config::audit_path())? {
        let rec = serde_json::json!({
            "type": "audit",
            "action": ev.action,
            "key_id": ev.key_id,
            "principal": ev.principal,
        });
        lines.push(rec.to_string());
    }

    // Fail-closed BEFORE touching disk: a recognizable secret must never reach the artifact. (The
    // redaction above removes recognizable secrets from message fields; this catches anything that
    // slipped through in any field. Bounded by `looks_like_secret` — see its doc for the limit.)
    for line in &lines {
        if !scan_secret_free(line) {
            bail!(
                "refusing to export: a recognizable secret survived redaction — nothing was written"
            );
        }
    }

    let out = out.unwrap_or_else(|| config::data_dir().join(format!("trajectory-{session}.jsonl")));
    if let Some(p) = out.parent() {
        std::fs::create_dir_all(p)?;
    }
    let body = if lines.is_empty() {
        String::new()
    } else {
        lines.join("\n") + "\n"
    };
    std::fs::write(&out, &body).with_context(|| format!("writing {out:?}"))?;
    println!(
        "exported {} message(s) + audit events for session '{session}' to {out:?} (secret-free)",
        msgs.len()
    );
    Ok(())
}

/// Whole-field redaction. Returns the content verbatim unless it carries a recognizable secret, in
/// which case the WHOLE field becomes `[redacted]`. Bounded by `sa_memory::looks_like_secret`.
fn redact(content: &str) -> String {
    if looks_like_secret(content) {
        REDACTED.to_string()
    } else {
        content.to_string()
    }
}

/// A line is secret-free iff the detector finds nothing — the export's enforced postcondition.
fn scan_secret_free(line: &str) -> bool {
    !looks_like_secret(line)
}

#[cfg(unix)]
fn chmod_600(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if path.exists() {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("chmod 600 {path:?}"))?;
    }
    Ok(())
}

#[cfg(windows)]
fn chmod_600(_path: &Path) -> Result<()> {
    // Windows: rely on per-user %APPDATA% ACL inheritance (the sa-vault `write_0600` precedent). A
    // backup placed OUTSIDE %APPDATA% inherits that dir's ACL — the "protect the backup" warning
    // covers it; an explicit icacls pass is the named upgrade if a shared-box threat is real.
    Ok(())
}

#[cfg(unix)]
fn chmod_700_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .with_context(|| format!("chmod 700 {path:?}"))?;
    Ok(())
}

#[cfg(windows)]
fn chmod_700_dir(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_hides_recognizable_secrets_keeps_clean_text() {
        assert_eq!(
            redact("a normal note about cats"),
            "a normal note about cats"
        );
        assert_eq!(redact("my key is sk-abcd1234efgh"), REDACTED);
        assert_eq!(redact("SECRET=hunter2"), REDACTED);
        // The marker itself must never trip the detector (else the rescan would loop on it).
        assert!(scan_secret_free(REDACTED));
    }

    #[test]
    fn scan_secret_free_flags_a_line_carrying_a_secret() {
        assert!(scan_secret_free(r#"{"type":"message","content":"hello"}"#));
        assert!(!scan_secret_free(r#"{"content":"ghp_ABCDEFGHJKLM"}"#));
    }
}
