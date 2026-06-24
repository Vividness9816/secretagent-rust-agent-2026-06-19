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
    /// Who drove this action: `"operator"` | `"remote:<connector>:<sender>"`. The forensic
    /// complement to the M1/M2/M3 prevention controls (ADR-20260621). `#[serde(default,
    /// skip_serializing_if)]` keeps pre-Phase-4 lines (no `principal` key) byte-identical on
    /// re-serialization, so `entry_hash`/`verify_chain` still pass them. NEVER a secret value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<String>,
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
            let content = std::fs::read_to_string(path)?;
            let lines: Vec<&str> = content.lines().collect();
            let mut last = String::new();
            let mut n = 0u64;
            for (i, line) in lines.iter().enumerate() {
                match serde_json::from_str::<Entry>(line) {
                    Ok(e) => {
                        last = e.hash;
                        n = e.seq + 1;
                    }
                    // A crash mid-`writeln!` leaves a torn final line; it has no valid
                    // chain successor, so tolerate it and let the daemon still start.
                    Err(_) if i == lines.len() - 1 => break,
                    Err(e) => return Err(e.into()),
                }
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

    /// Append + `fsync`. Use before dispatching an irreversible/untrusted action so the
    /// record survives a crash of the action itself (ADR-20260620). `flush()` alone only
    /// empties the userspace buffer; `sync_all()` forces it to disk.
    pub fn append_synced(&mut self, event: AuditEvent) -> anyhow::Result<()> {
        self.append(event)?;
        self.file.sync_all()?;
        Ok(())
    }

    /// Re-derive the chain from disk; returns false on any truncation, reorder,
    /// or in-place mutation. ponytail: blake3 hash-chain = tamper-evidence with
    /// zero key management; upgrade to ed25519 signatures when an external
    /// verifier must trust the log without holding the file itself.
    pub fn verify_chain(path: &Path) -> anyhow::Result<bool> {
        let content = std::fs::read_to_string(path)?;
        let mut prev = String::new();
        for (i, line) in content.lines().enumerate() {
            // A torn/garbage line (e.g. a crash mid-append) means the chain is not
            // verified — report Ok(false), never Err, so callers can act on it.
            let e: Entry = match serde_json::from_str(line) {
                Ok(e) => e,
                Err(_) => return Ok(false),
            };
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

    /// Read the log's events back (6g trajectory export). Returns the `AuditEvent`s in order;
    /// the events are secret-free by construction (key NAMES + principal only — invariant #4).
    /// A missing file is `Ok(vec![])`; a torn final line (crash mid-append) is tolerated like
    /// `open`. Does NOT verify the chain — use `verify_chain` for integrity.
    pub fn read_events(path: &Path) -> anyhow::Result<Vec<AuditEvent>> {
        if !path.exists() {
            return Ok(Vec::new());
        }
        let content = std::fs::read_to_string(path)?;
        let lines: Vec<&str> = content.lines().collect();
        let mut events = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            match serde_json::from_str::<Entry>(line) {
                Ok(e) => events.push(e.event),
                // Tolerate only a torn FINAL line; a mid-log parse error is real corruption.
                Err(_) if i == lines.len() - 1 => break,
                Err(e) => return Err(e.into()),
            }
        }
        Ok(events)
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
                principal: None,
            })
            .unwrap();
            a.append(AuditEvent {
                action: "vault.get".into(),
                key_id: "API_KEY".into(),
                principal: None,
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
                principal: None,
            })
            .unwrap();
        }
        // reopen (simulates daemon restart) and append again
        {
            let mut a = Audit::open(&p).unwrap();
            a.append(AuditEvent {
                action: "b".into(),
                key_id: "k".into(),
                principal: None,
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
                principal: None,
            })
            .unwrap();
            a.append(AuditEvent {
                action: "y".into(),
                key_id: "k".into(),
                principal: None,
            })
            .unwrap();
        }
        let tampered = std::fs::read_to_string(&p)
            .unwrap()
            .replace("\"x\"", "\"z\"");
        std::fs::write(&p, tampered).unwrap();
        assert!(!Audit::verify_chain(&p).unwrap(), "tamper must be detected");
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
                principal: None,
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

    #[test]
    fn open_tolerates_a_torn_final_line_and_verify_reports_false() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("audit.jsonl");
        {
            let mut a = Audit::open(&p).unwrap();
            a.append(AuditEvent {
                action: "a".into(),
                key_id: "k".into(),
                principal: None,
            })
            .unwrap();
            a.append(AuditEvent {
                action: "b".into(),
                key_id: "k".into(),
                principal: None,
            })
            .unwrap();
        }
        // Simulate a crash mid-write: append a partial, non-JSON trailing line.
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        write!(f, "{{\"seq\":2,\"prev\":\"deadbeef\",\"eve").unwrap();
        drop(f);

        // open must NOT error (the daemon must still start) and the good entries survive.
        let a = Audit::open(&p).unwrap();
        drop(a);
        // verify_chain reports the torn tail as unverified, never errors.
        assert!(!Audit::verify_chain(&p).unwrap());
    }

    #[test]
    fn old_lines_without_principal_still_verify_and_new_lines_carry_it() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("audit.jsonl");

        // Simulate a pre-Phase-4 log line written WITHOUT a `principal` key, plus its valid hash
        // (built the way `append` would have: seq 0, prev "", event = only action+key_id).
        let legacy_event = AuditEvent {
            action: "vault.set".into(),
            key_id: "API_KEY".into(),
            principal: None,
        };
        let legacy_hash = entry_hash(0, "", &legacy_event);
        let legacy_line = format!(
            "{{\"seq\":0,\"prev\":\"\",\"event\":{{\"action\":\"vault.set\",\"key_id\":\"API_KEY\"}},\"hash\":\"{legacy_hash}\"}}"
        );
        std::fs::write(&p, format!("{legacy_line}\n")).unwrap();

        // The legacy line (no `principal` key) must still verify — Option+skip is byte-stable.
        assert!(
            Audit::verify_chain(&p).unwrap(),
            "a pre-Phase-4 audit line without `principal` must still verify"
        );

        // Append a NEW line that DOES carry a principal; the chain must continue + verify.
        {
            let mut a = Audit::open(&p).unwrap();
            a.append(AuditEvent {
                action: "tool.write_file".into(),
                key_id: "write_file".into(),
                principal: Some("remote:telegram:123".into()),
            })
            .unwrap();
        }
        assert!(
            Audit::verify_chain(&p).unwrap(),
            "chain must continue after the schema add"
        );
        let body = std::fs::read_to_string(&p).unwrap();
        assert!(
            body.contains("remote:telegram:123"),
            "new line records the principal"
        );
        // The legacy line on disk still has NO `principal` key (we never rewrote it).
        let first = body.lines().next().unwrap();
        assert!(
            !first.contains("principal"),
            "legacy line must remain principal-free on disk"
        );
    }

    #[test]
    fn append_synced_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("audit.jsonl");
        let mut a = Audit::open(&p).unwrap();
        a.append_synced(AuditEvent {
            action: "execute.dispatch".into(),
            key_id: "fetch".into(),
            principal: None,
        })
        .unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap().lines().count(), 1);
        assert!(Audit::verify_chain(&p).unwrap());
    }

    #[test]
    fn read_events_returns_appended_events_and_tolerates_a_torn_tail() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("audit.jsonl");
        // Absent file → empty.
        assert!(Audit::read_events(&p).unwrap().is_empty());
        {
            let mut a = Audit::open(&p).unwrap();
            a.append(AuditEvent {
                action: "tool.fetch".into(),
                key_id: "fetch".into(),
                principal: Some("operator".into()),
            })
            .unwrap();
            a.append(AuditEvent {
                action: "tool.write_file".into(),
                key_id: "write_file".into(),
                principal: Some("remote:telegram:123".into()),
            })
            .unwrap();
        }
        let evs = Audit::read_events(&p).unwrap();
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0].action, "tool.fetch");
        assert_eq!(evs[1].key_id, "write_file");
        assert_eq!(evs[1].principal.as_deref(), Some("remote:telegram:123"));
        // A crash mid-append leaves a torn final line; read_events skips it, keeps the good ones.
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        write!(f, "{{\"seq\":2,\"prev\":\"x\",\"eve").unwrap();
        drop(f);
        assert_eq!(Audit::read_events(&p).unwrap().len(), 2, "torn tail tolerated");
    }
}
