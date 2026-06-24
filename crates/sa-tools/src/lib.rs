pub mod egress;
pub mod mcp;

use anyhow::{bail, Result};
use async_trait::async_trait;
use sa_core_types::policy::{egress_allowed, path_allowed, Policy};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// A first-party tool. Each enforces its own slice of the `Policy` (fetch → egress,
/// read/write → path roots) and returns a raw `String` that the caller taints.
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    /// JSON-schema of the tool's args, sent to the model so it fills the right fields.
    fn parameters(&self) -> Value {
        json!({"type": "object"})
    }
    async fn run(&self, args: Value, policy: &Policy) -> Result<String>;
}

#[derive(Default)]
pub struct Registry {
    tools: BTreeMap<String, Box<dyn Tool>>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, t: Box<dyn Tool>) {
        self.tools.insert(t.name().to_string(), t);
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|b| b.as_ref())
    }

    pub fn names(&self) -> Vec<&str> {
        self.tools.keys().map(|s| s.as_str()).collect()
    }

    /// The Phase-2a tool set: fetch, read_file, write_file. (No execute_code — it needs
    /// the landlock sandbox and lands in slice 2b.)
    pub fn default_tools() -> Self {
        let mut r = Self::new();
        r.register(Box::new(Fetch));
        r.register(Box::new(ReadFile));
        r.register(Box::new(WriteFile));
        r
    }
}

pub struct Fetch;

#[async_trait]
impl Tool for Fetch {
    fn name(&self) -> &str {
        "fetch"
    }
    fn description(&self) -> &str {
        "HTTP GET an allow-listed URL; returns the body (untrusted)."
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"url":{"type":"string"}},"required":["url"]})
    }
    async fn run(&self, args: Value, policy: &Policy) -> Result<String> {
        let url = args
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("fetch: missing 'url'"))?;
        let host = url_host(url).ok_or_else(|| anyhow::anyhow!("fetch: bad url"))?;
        if !egress_allowed(policy, &host) {
            bail!("egress denied: {host}");
        }
        let body = reqwest::Client::new()
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        Ok(body)
    }
}

pub struct ReadFile;

#[async_trait]
impl Tool for ReadFile {
    fn name(&self) -> &str {
        "read_file"
    }
    fn description(&self) -> &str {
        "Read a file within an allowed read root (untrusted)."
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]})
    }
    async fn run(&self, args: Value, policy: &Policy) -> Result<String> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("read_file: missing 'path'"))?;
        let pb = std::path::PathBuf::from(path);
        if !path_allowed(policy, &pb, false) {
            bail!("path denied: {path}");
        }
        Ok(std::fs::read_to_string(&pb)?)
    }
}

pub struct WriteFile;

#[async_trait]
impl Tool for WriteFile {
    fn name(&self) -> &str {
        "write_file"
    }
    fn description(&self) -> &str {
        "Write a file within an allowed write root (requires approval)."
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]})
    }
    async fn run(&self, args: Value, policy: &Policy) -> Result<String> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("write_file: missing 'path'"))?;
        let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
        let pb = std::path::PathBuf::from(path);
        if !path_allowed(policy, &pb, true) {
            bail!("path denied: {path}");
        }
        if let Some(parent) = pb.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&pb, content)?;
        Ok(format!("wrote {} bytes to {path}", content.len()))
    }
}

/// Extract the host from a URL without a url-parsing dependency.
fn url_host(url: &str) -> Option<String> {
    let after = url.split("://").nth(1).unwrap_or(url);
    let hostport = after.split('/').next().unwrap_or(after);
    let host = hostport.split(':').next().unwrap_or(hostport);
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

/// Run a shell snippet confined by the platform sandbox. FAIL-CLOSED: if the sandbox
/// can't be enforced, refuse — UNLESS `allow_unsandboxed` (the per-invocation, never-
/// persisted, screaming override), which runs the code with NO sandbox and a loud banner.
pub struct ExecuteCode {
    backend: sa_exec::Backend,
    allow_unsandboxed: bool,
}

impl ExecuteCode {
    /// Default: the local backend (landlock on Linux, refuse elsewhere).
    pub fn new(allow_unsandboxed: bool) -> Self {
        Self {
            backend: sa_exec::Backend::local(),
            allow_unsandboxed,
        }
    }
    /// Construct with an operator-frozen backend (Local/Docker/Ssh) resolved from config.
    pub fn with_backend(backend: sa_exec::Backend, allow_unsandboxed: bool) -> Self {
        Self {
            backend,
            allow_unsandboxed,
        }
    }
    /// Test/seam constructor with an explicit local sandbox (wraps it in `Backend::Local`).
    pub fn with_sandbox(sandbox: Box<dyn sa_exec::Sandbox>, allow_unsandboxed: bool) -> Self {
        Self {
            backend: sa_exec::Backend::Local(sandbox),
            allow_unsandboxed,
        }
    }
    /// Secret-free backend label (for the audit/doctor record).
    pub fn backend_label(&self) -> String {
        self.backend.label()
    }
}

#[async_trait]
impl Tool for ExecuteCode {
    fn name(&self) -> &str {
        "execute_code"
    }
    fn description(&self) -> &str {
        "Run a shell snippet on the operator-configured execution backend, confined to the policy's \
         file roots. The backend (local landlock / docker / ssh) is operator-set, never chosen here."
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"code":{"type":"string"}},"required":["code"]})
    }
    async fn run(&self, args: Value, policy: &Policy) -> Result<String> {
        let code = args
            .get("code")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("execute_code: missing 'code'"))?;
        match self.backend.run(code, policy) {
            Ok(out) => Ok(out),
            // The screaming override applies ONLY to the local backend (a missing docker/ssh CLI is
            // an operator config error, not a sandbox refusal to override).
            Err(e) if self.allow_unsandboxed && self.backend.is_local() => {
                eprintln!(
                    "\n!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!\n\
                     !!! UNSANDBOXED EXECUTION — landlock not enforced ({e}).\n\
                     !!! Running code with NO sandbox because --allow-unsandboxed-exec.\n\
                     !!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!"
                );
                run_unconfined(code)
            }
            Err(e) => Err(e), // fail-closed
        }
    }
}

/// The override's escape hatch: run via the OS shell with NO confinement. Cross-platform.
// ponytail: blocking spawn on the async path — fine for a single CLI task; wrap in
// spawn_blocking only if concurrent execs ever contend.
fn run_unconfined(code: &str) -> Result<String> {
    let out = if cfg!(windows) {
        std::process::Command::new("cmd")
            .arg("/C")
            .arg(code)
            .output()?
    } else {
        std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(code)
            .output()?
    };
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    let err = String::from_utf8_lossy(&out.stderr);
    if !err.is_empty() {
        s.push_str(&err);
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sa_core_types::policy::Policy;
    use serde_json::json;

    #[tokio::test]
    async fn fetch_denies_unlisted_host_without_making_a_request() {
        let p = Policy {
            egress_allow: vec!["example.com".into()],
            ..Default::default()
        };
        let err = Fetch
            .run(json!({"url": "http://evil.test/x"}), &p)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("egress"), "got {err}");
    }

    #[tokio::test]
    async fn read_file_allows_within_root_and_denies_outside() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("ok.txt"), "hello").unwrap();
        let p = Policy {
            read_roots: vec![dir.path().to_path_buf()],
            ..Default::default()
        };
        let ok = ReadFile
            .run(json!({"path": dir.path().join("ok.txt")}), &p)
            .await
            .unwrap();
        assert_eq!(ok, "hello");
        let err = ReadFile
            .run(json!({"path": "/etc/passwd"}), &p)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("path"), "got {err}");
    }

    #[tokio::test]
    async fn write_file_denies_outside_write_root() {
        let dir = tempfile::tempdir().unwrap();
        let p = Policy {
            write_roots: vec![dir.path().join("out")],
            ..Default::default()
        };
        let ok = WriteFile
            .run(
                json!({"path": dir.path().join("out").join("r.txt"), "content": "hi"}),
                &p,
            )
            .await
            .unwrap();
        assert!(ok.contains("wrote 2 bytes"));
        let err = WriteFile
            .run(
                json!({"path": dir.path().join("escape.txt"), "content": "x"}),
                &p,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("path"), "got {err}");
    }

    #[test]
    fn registry_lists_three_tools() {
        let r = Registry::default_tools();
        assert_eq!(r.names().len(), 3);
        assert!(r.get("fetch").is_some());
        assert!(r.get("read_file").is_some());
        assert!(r.get("write_file").is_some());
    }

    #[test]
    fn execute_code_backend_label_is_exposed_and_schema_has_no_backend_arg() {
        let tool = ExecuteCode::with_backend(
            sa_exec::Backend::Docker {
                image: "alpine".into(),
            },
            false,
        );
        assert_eq!(tool.backend_label(), "docker:alpine");
        // ADR-20260623 BLOCKER #2: the model must NEVER choose a backend/host — schema is {code}.
        let schema = tool.parameters().to_string();
        for k in ["backend", "host", "image", "ssh", "docker"] {
            assert!(
                !schema.contains(k),
                "schema must not expose '{k}': {schema}"
            );
        }
    }

    #[tokio::test]
    async fn execute_code_is_fail_closed_without_an_enforced_sandbox() {
        // Force the refusing sandbox (the non-Linux default / no-landlock case).
        let tool = ExecuteCode::with_sandbox(Box::new(sa_exec::RefuseSandbox), false);
        let err = tool
            .run(json!({"code": "echo pwned"}), &Policy::default())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("refused"), "got: {err}");
    }

    #[tokio::test]
    async fn execute_code_override_runs_unconfined_and_screams() {
        // The per-invocation escape valve: refusing sandbox + allow_unsandboxed = it runs.
        let tool = ExecuteCode::with_sandbox(Box::new(sa_exec::RefuseSandbox), true);
        let out = tool
            .run(json!({"code": "echo OVERRIDE_RAN"}), &Policy::default())
            .await
            .unwrap();
        assert!(
            out.contains("OVERRIDE_RAN"),
            "override should run the code: {out:?}"
        );
    }
}
