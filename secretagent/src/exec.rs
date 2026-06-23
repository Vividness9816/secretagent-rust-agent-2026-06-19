//! Resolve the OPERATOR-FROZEN execution backend from config (ADR-20260623 BLOCKER #2: the backend
//! is config, NEVER a model tool argument). Shared by `run` and `gateway` so both arm `execute_code`
//! with the same operator-chosen backend.

use anyhow::{anyhow, bail, Result};
use sa_audit::{Audit, AuditEvent};
use sa_core_types::config::ExecConfig;
use sa_exec::Backend;

/// Record the operator-armed execution backend in the tamper-evident audit log (ADR-20260623 5a
/// gate: "the audit records the backend"). Emitted once when `run`/`gateway` arms the backend; the
/// backend is frozen per process, so every subsequent `tool.execute_code` in this session ran on
/// it — answering "where did this code run?" for an incident responder (the doctor line reports
/// only the current config, not the historical forensic record).
pub fn audit_backend_armed(audit: &mut Audit, label: &str) -> Result<()> {
    audit.append_synced(AuditEvent {
        action: "exec.backend".into(),
        key_id: label.to_string(),
        principal: Some("operator".into()),
    })?;
    Ok(())
}

/// Map `[exec]` config to a `Backend`. Default/`"local"` → the landlock-or-refuse local sandbox.
pub fn backend_from_config(cfg: &ExecConfig) -> Result<Backend> {
    match cfg.backend.as_str() {
        "local" => Ok(Backend::local()),
        "docker" => Ok(Backend::Docker {
            image: cfg
                .image
                .clone()
                .ok_or_else(|| anyhow!("[exec] backend=\"docker\" needs an `image`"))?,
        }),
        "ssh" => Ok(Backend::Ssh {
            host: cfg
                .host
                .clone()
                .ok_or_else(|| anyhow!("[exec] backend=\"ssh\" needs a `host` (e.g. user@host)"))?,
        }),
        other => bail!("[exec] unknown backend '{other}' (want local|docker|ssh)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(backend: &str, image: Option<&str>, host: Option<&str>) -> ExecConfig {
        ExecConfig {
            backend: backend.into(),
            image: image.map(Into::into),
            host: host.map(Into::into),
        }
    }

    #[test]
    fn maps_kinds_and_errors_on_missing_fields() {
        assert!(backend_from_config(&cfg("local", None, None))
            .unwrap()
            .is_local());
        assert_eq!(
            backend_from_config(&cfg("docker", Some("alpine"), None))
                .unwrap()
                .label(),
            "docker:alpine"
        );
        assert_eq!(
            backend_from_config(&cfg("ssh", None, Some("u@h")))
                .unwrap()
                .label(),
            "ssh:u@h"
        );
        assert!(backend_from_config(&cfg("docker", None, None)).is_err());
        assert!(backend_from_config(&cfg("ssh", None, None)).is_err());
        assert!(backend_from_config(&cfg("bogus", None, None)).is_err());
    }

    #[test]
    fn audit_backend_armed_records_the_label_in_the_log() {
        // 5a gate "the audit records the backend": the armed backend is written to the tamper-
        // evident log, so a Docker/SSH run is distinguishable from a local one in audit.jsonl.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.jsonl");
        let mut audit = Audit::open(&path).unwrap();
        audit_backend_armed(&mut audit, "docker:alpine").unwrap();
        let log = std::fs::read_to_string(&path).unwrap();
        assert!(log.contains("exec.backend"), "log: {log}");
        assert!(log.contains("docker:alpine"), "log: {log}");
    }
}
