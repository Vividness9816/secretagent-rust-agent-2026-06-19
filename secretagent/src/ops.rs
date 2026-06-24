//! `secretagent {backup,restore,export}` — operational data-dir backup/restore + a secret-free
//! trajectory export (Phase 6g, ADR-20260623 slice 6g).
//!
//! - **backup** snapshots `memory.db` via the SQLite **Online Backup API** (never `cp` a live WAL
//!   DB) and byte-copies the encrypted `store.age` / `identity.age` / `audit.jsonl` into a dir. The
//!   age vault stays ENCRYPTED (we copy ciphertext, never decrypt); the identity travels with it so
//!   the backup is decryptable — which makes the backup dir as sensitive as the data dir (we
//!   `chmod 600` the copied identity and warn the operator).
//! - **restore** copies those artifacts back into `data_dir()`, `chmod 600`s the identity, and
//!   verifies the restored audit hash-chain (loud, never silent). It overwrites the live data dir.
//! - **export** writes a session's `messages` + the audit log as JSONL with recognizable secrets
//!   REDACTED, and is fail-closed: it re-scans the assembled artifact and refuses to write if any
//!   recognizable secret survived redaction (secret-free is an enforced postcondition, not a hope).
//!
//! Scope = the four data-dir artifacts (operational state). `config.toml`/`SOUL.md`/`context.md`
//! are operator-authored config in `config_dir()` — out of scope (documented in the 6g plan).

use anyhow::{bail, ensure, Context, Result};
use sa_core_types::config;
use sa_memory::{looks_like_secret, Store};
use std::path::{Path, PathBuf};

/// Whole-field replacement for a content field that carries a recognizable secret (never partial —
/// a substring redaction could leak across the boundary). Chosen so it can't itself trip the
/// detector (no `secret=`/token prefix inside).
const REDACTED: &str = "[redacted]";

/// The flat (whole-file) artifacts a backup/restore moves verbatim. `memory.db` is handled
/// separately via the Online Backup API (it is the only one that may be a live WAL DB).
const FLAT_FILES: [&str; 3] = ["identity.age", "store.age", "audit.jsonl"];

/// Snapshot the data dir into `dest` (created if absent).
pub fn backup(dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest).with_context(|| format!("creating backup dir {dest:?}"))?;
    let mut done: Vec<&str> = Vec::new();

    // memory.db — Online Backup API (a consistent snapshot even while the daemon holds the DB
    // open in WAL mode). NEVER a byte copy of a live WAL DB.
    let db = config::db_path();
    if db.exists() {
        let store = Store::open(&db).context("opening live DB for backup")?;
        store.backup_to(&dest.join("memory.db"))?;
        done.push("memory.db (online snapshot)");
    }

    // The flat files are append-only / replace-whole, so a byte copy is consistent.
    let data = config::data_dir();
    for name in FLAT_FILES {
        let src = data.join(name);
        if src.exists() {
            std::fs::copy(&src, dest.join(name)).with_context(|| format!("copying {name}"))?;
            done.push(name);
        }
    }
    // The identity is the private key wherever it lands — lock the copy down too.
    chmod_600(&dest.join("identity.age"))?;

    if done.is_empty() {
        println!("nothing to back up (no data dir artifacts at {data:?})");
    } else {
        println!("backed up to {dest:?}: {}", done.join(", "));
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
    println!(
        "WARNING: restore overwrites the live data dir — stop the gateway/daemon first if it is running."
    );
    let data = config::data_dir();
    std::fs::create_dir_all(&data)?;
    let mut done: Vec<&str> = Vec::new();
    // memory.db is restored like any flat file — the backup wrote a self-contained DB (no sidecar).
    for name in ["memory.db", "identity.age", "store.age", "audit.jsonl"] {
        let from = src.join(name);
        if from.exists() {
            std::fs::copy(&from, data.join(name)).with_context(|| format!("restoring {name}"))?;
            done.push(name);
        }
    }
    // The identity must be 0600 after restore (handoff requirement).
    chmod_600(&config::identity_path())?;

    if done.is_empty() {
        println!("nothing restored (no recognized artifacts in {src:?})");
        return Ok(());
    }
    println!("restored into {data:?}: {}", done.join(", "));

    // Verify the restored audit hash-chain — report loudly, never silently (and never fatal here).
    let ap = config::audit_path();
    if ap.exists() {
        match sa_audit::Audit::verify_chain(&ap) {
            Ok(true) => println!("audit chain: VERIFIED"),
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
        let rec = serde_json::json!({
            "type": "message",
            "role": m.role,
            "provenance": prov,
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
    // redaction above removes recognizable secrets from message content; this catches anything that
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
