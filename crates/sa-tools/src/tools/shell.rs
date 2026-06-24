//! `shell` — run a command on the operator-frozen sandboxed backend. A thin alias over the
//! `execute_code` path (same `sa_exec::Backend`), but STRICTLY fail-closed: it never takes the
//! `allow_unsandboxed` override (that escape hatch is execute_code's local-CLI-only). The name
//! `"shell"` is already in `policy::approval_required`, so the side-effect approval gate covers it.

use crate::{ExecuteCode, Tool};
use anyhow::Result;
use async_trait::async_trait;
use sa_core_types::policy::Policy;
use serde_json::{json, Value};

pub struct Shell {
    inner: ExecuteCode,
}

impl Shell {
    /// Wrap the operator-frozen backend (Local landlock / Docker / Ssh). Fail-closed: no override.
    pub fn with_backend(backend: sa_exec::Backend) -> Self {
        Self {
            inner: ExecuteCode::with_backend(backend, false),
        }
    }
}

#[async_trait]
impl Tool for Shell {
    fn name(&self) -> &str {
        "shell"
    }
    fn description(&self) -> &str {
        "Run a shell command on the operator-configured sandboxed backend (landlock/docker/ssh; \
         fail-closed). Same backend as execute_code; the backend is operator-set, never chosen here."
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"command":{"type":"string"}},"required":["command"]})
    }
    async fn run(&self, args: Value, policy: &Policy) -> Result<String> {
        let command = args
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("shell: missing 'command'"))?;
        // Delegate to the proven execute_code path (maps the `command` arg onto its `code` arg).
        self.inner.run(json!({ "code": command }), policy).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refusing() -> Shell {
        Shell::with_backend(sa_exec::Backend::Local(Box::new(sa_exec::RefuseSandbox)))
    }

    #[test]
    fn schema_is_command_only_and_hides_the_backend() {
        let s = refusing().parameters().to_string();
        assert!(s.contains("command"));
        assert!(!s.contains("\"code\"") && !s.contains("backend"));
    }

    #[test]
    fn name_is_shell_so_the_approval_gate_applies() {
        assert_eq!(refusing().name(), "shell");
        assert!(sa_core_types::policy::approval_required("shell"));
    }

    #[tokio::test]
    async fn fail_closed_without_an_enforced_sandbox() {
        let err = refusing()
            .run(json!({"command":"echo hi"}), &Policy::default())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("refused"), "got {err}");
    }
}
