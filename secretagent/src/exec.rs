//! Resolve the OPERATOR-FROZEN execution backend from config (ADR-20260623 BLOCKER #2: the backend
//! is config, NEVER a model tool argument). Shared by `run` and `gateway` so both arm `execute_code`
//! with the same operator-chosen backend.

use anyhow::{anyhow, bail, Result};
use sa_core_types::config::ExecConfig;
use sa_exec::Backend;

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
}
