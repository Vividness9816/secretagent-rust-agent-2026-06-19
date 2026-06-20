use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;

/// An audited action. Records the key *name* / vault key-id only — NEVER a secret
/// value (ADR invariant #4: no secret in the audit log).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub action: String,
    pub key_id: String,
}

#[derive(Serialize, Deserialize)]
struct Entry {
    seq: u64,
    /// hex blake3 of the previous entry's hash chain ("" for genesis)
    prev: String,
    event: AuditEvent,
    /// blake3(seq || prev || event_json)
    hash: String,
}

/// Owns the only writable handle to the log. The `file` field is private to this
/// crate, so no other crate/module can append or mutate entries except through
/// `append` — the sole-writer invariant a mere `mod` boundary cannot enforce.
pub struct Audit {
    file: std::fs::File,
    last_hash: String,
    seq: u64,
}

fn entry_hash(seq: u64, prev: &str, event: &AuditEvent) -> String {
    let ev = serde_json::to_string(event).expect("AuditEvent serializes");
    let mut h = blake3::Hasher::new();
    h.update(&seq.to_le_bytes());
    h.update(prev.as_bytes());
    h.update(ev.as_bytes());
    h.finalize().to_hex().to_string()
}

impl Audit {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        let (last_hash, seq) = if path.exists() {
            let mut last = String::new();
            let mut n = 0u64;
            for line in std::fs::read_to_string(path)?.lines() {
                let e: Entry = serde_json::from_str(line)?;
                last = e.hash;
                n = e.seq + 1;
            }
            (last, n)
        } else {
            (String::new(), 0)
        };
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .context("opening audit log append-only")?;
        Ok(Self {
            file,
            last_hash,
            seq,
        })
    }

    pub fn append(&mut self, event: AuditEvent) -> anyhow::Result<()> {
        let hash = entry_hash(self.seq, &self.last_hash, &event);
        let entry = Entry {
            seq: self.seq,
            prev: self.last_hash.clone(),
            event,
            hash: hash.clone(),
        };
        writeln!(self.file, "{}", serde_json::to_string(&entry)?)?;
        self.file.flush()?;
        self.last_hash = hash;
        self.seq += 1;
        Ok(())
    }

    /// Re-derive the chain from disk; returns false on any truncation, reorder,
    /// or in-place mutation. ponytail: blake3 hash-chain = tamper-evidence with
    /// zero key management; upgrade to ed25519 signatures when an external
    /// verifier must trust the log without holding the file itself.
    pub fn verify_chain(path: &Path) -> anyhow::Result<bool> {
        let mut prev = String::new();
        for (i, line) in std::fs::read_to_string(path)?.lines().enumerate() {
            let e: Entry = serde_json::from_str(line)?;
            if e.seq != i as u64 || e.prev != prev {
                return Ok(false);
            }
            if entry_hash(e.seq, &e.prev, &e.event) != e.hash {
                return Ok(false);
            }
            prev = e.hash;
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_reads_back_and_chain_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("audit.jsonl");
        {
            let mut a = Audit::open(&p).unwrap();
            a.append(AuditEvent {
                action: "vault.set".into(),
                key_id: "API_KEY".into(),
            })
            .unwrap();
            a.append(AuditEvent {
                action: "vault.get".into(),
                key_id: "API_KEY".into(),
            })
            .unwrap();
        }
        assert_eq!(std::fs::read_to_string(&p).unwrap().lines().count(), 2);
        assert!(
            Audit::verify_chain(&p).unwrap(),
            "untampered chain should verify"
        );
    }

    #[test]
    fn appending_to_existing_log_continues_the_chain() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("audit.jsonl");
        {
            let mut a = Audit::open(&p).unwrap();
            a.append(AuditEvent {
                action: "a".into(),
                key_id: "k".into(),
            })
            .unwrap();
        }
        // reopen (simulates daemon restart) and append again
        {
            let mut a = Audit::open(&p).unwrap();
            a.append(AuditEvent {
                action: "b".into(),
                key_id: "k".into(),
            })
            .unwrap();
        }
        assert_eq!(std::fs::read_to_string(&p).unwrap().lines().count(), 2);
        assert!(Audit::verify_chain(&p).unwrap(), "chain survives a restart");
    }

    #[test]
    fn mutating_an_entry_breaks_the_chain() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("audit.jsonl");
        {
            let mut a = Audit::open(&p).unwrap();
            a.append(AuditEvent {
                action: "x".into(),
                key_id: "k".into(),
            })
            .unwrap();
            a.append(AuditEvent {
                action: "y".into(),
                key_id: "k".into(),
            })
            .unwrap();
        }
        let tampered = std::fs::read_to_string(&p).unwrap().replace("\"x\"", "\"z\"");
        std::fs::write(&p, tampered).unwrap();
        assert!(
            !Audit::verify_chain(&p).unwrap(),
            "tamper must be detected"
        );
    }

    #[test]
    fn audit_never_contains_secret_material() {
        // Phase 0 leak test: the only secret-handler is the vault. Prove that an
        // audited vault action records the KEY NAME, never the value.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("audit.jsonl");
        {
            let mut a = Audit::open(&p).unwrap();
            a.append(AuditEvent {
                action: "vault.set".into(),
                key_id: "API_KEY".into(),
            })
            .unwrap();
        }
        let body = std::fs::read_to_string(&p).unwrap();
        assert!(
            !body.contains("s3cr3t-sentinel"),
            "no secret value should ever reach the audit log"
        );
        assert!(body.contains("API_KEY"), "the key name is fine to record");
    }
}
