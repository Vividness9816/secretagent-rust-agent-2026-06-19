//! The tool-execution sandbox (ADR-20260620, Tier D: landlock-only). A single
//! `Sandbox` seam with two impls — `LandlockSandbox` (Linux kernel tier) and
//! `RefuseSandbox` (all platforms, fail-closed default). The injection guard is NOT
//! here: it lives in sa-core-types/sa-core. This crate is sandbox-only.
use anyhow::{Context, Result};
use sa_core_types::policy::Policy;
use std::io::Write;
use std::process::{Command, Stdio};

#[derive(Debug, Clone)]
pub enum LandlockStatus {
    /// Landlock is present AND a ruleset enforces; `abi` is the kernel ABI level.
    Enforced { abi: i32 },
    /// No enforced sandbox (wrong OS, old kernel, disabled). `execute_code` fail-closes.
    Unavailable { reason: String },
}

/// The one containment seam. `run_confined` runs `code` (a `sh -c` snippet) restricted
/// to the policy's file roots; it MUST return Err rather than run unconfined.
pub trait Sandbox: Send + Sync {
    fn run_confined(&self, code: &str, policy: &Policy) -> Result<String>;
    fn status(&self) -> LandlockStatus;
}

/// Refuses everything. The all-platforms fallback when no kernel sandbox exists.
pub struct RefuseSandbox;

impl Sandbox for RefuseSandbox {
    fn run_confined(&self, _code: &str, _policy: &Policy) -> Result<String> {
        anyhow::bail!("execute_code refused: no enforced sandbox on this platform")
    }
    fn status(&self) -> LandlockStatus {
        LandlockStatus::Unavailable {
            reason: "no sandbox backend on this platform".into(),
        }
    }
}

#[cfg(target_os = "linux")]
mod landlock_linux;
#[cfg(target_os = "linux")]
pub use landlock_linux::LandlockSandbox;

/// The platform's best sandbox: Landlock on Linux, Refuse everywhere else.
pub fn default_sandbox() -> Box<dyn Sandbox> {
    #[cfg(target_os = "linux")]
    {
        Box::new(LandlockSandbox::new())
    }
    #[cfg(not(target_os = "linux"))]
    {
        Box::new(RefuseSandbox)
    }
}

/// The capability the `doctor` line reports + the dispatch gate consults.
pub fn landlock_status() -> LandlockStatus {
    default_sandbox().status()
}

/// Honest per-backend confinement (ADR-20260623 BLOCKER #1). A remote/container backend NEVER
/// borrows the local landlock verdict — that lie is the exact fail-open-while-looking-closed the
/// ADR forbids.
#[derive(Debug, Clone)]
pub enum Confinement {
    /// Local process under the kernel sandbox; carries the real landlock status.
    LocalKernel(LandlockStatus),
    /// A Docker container — operator-vouched isolation (image + `--network=none`), not agent-proven.
    Container { image: String },
    /// A remote host over SSH — operator-vouched; confinement (and egress) is the host's, not ours.
    RemoteHost { host: String },
}

/// Where `execute_code` runs. CLOSED + operator-frozen (selected from config, NEVER a model arg —
/// ADR-20260623 BLOCKER #2). Docker/SSH shell out (zero new deps → the musl-static binary holds by
/// construction) and are runtime-optional (refuse if the CLI is absent). `Local` delegates to the
/// existing `Sandbox`/landlock path verbatim, so the proven 2b fail-closed contract is unchanged.
// ponytail: an enum (not a trait) because the backend set is closed + config-selected with one
// dispatch site; graduate to a trait only if out-of-tree backends are ever needed (ADR Revisit-when).
pub enum Backend {
    Local(Box<dyn Sandbox>),
    Docker { image: String },
    Ssh { host: String },
}

impl Backend {
    /// The default local backend (landlock on Linux, refuse elsewhere).
    pub fn local() -> Self {
        Backend::Local(default_sandbox())
    }

    pub fn is_local(&self) -> bool {
        matches!(self, Backend::Local(_))
    }

    /// Secret-free label for logs / audit / `doctor` (the host is operator-config, not a secret).
    pub fn label(&self) -> String {
        match self {
            Backend::Local(_) => "local".into(),
            Backend::Docker { image } => format!("docker:{image}"),
            Backend::Ssh { host } => format!("ssh:{host}"),
        }
    }

    /// The HONEST confinement story for this backend — never a borrowed local landlock verdict.
    pub fn confinement(&self) -> Confinement {
        match self {
            Backend::Local(sb) => Confinement::LocalKernel(sb.status()),
            Backend::Docker { image } => Confinement::Container {
                image: image.clone(),
            },
            Backend::Ssh { host } => Confinement::RemoteHost { host: host.clone() },
        }
    }

    /// Run `code` on this backend. `Local` keeps the existing fail-closed landlock contract;
    /// Docker/SSH shell out (snippet via STDIN, never argv) and fail-closed if the CLI is absent.
    pub fn run(&self, code: &str, policy: &Policy) -> Result<String> {
        match self {
            Backend::Local(sb) => sb.run_confined(code, policy),
            Backend::Docker { image } => run_docker(image, code, policy),
            Backend::Ssh { host } => run_ssh(host, code),
        }
    }
}

/// `docker run --rm -i --network=none [-v root[:ro]] <image> /bin/sh -s`, code on stdin.
/// `--network=none` confines egress (a remote/container's network is otherwise un-confinable by our
/// in-process `Policy` — the MCP-honesty gap); the policy file roots are mounted so the container's
/// FS scope matches the local policy.
fn run_docker(image: &str, code: &str, policy: &Policy) -> Result<String> {
    let mut cmd = Command::new("docker");
    cmd.args(["run", "--rm", "-i", "--network=none"]);
    for r in &policy.read_roots {
        if let Some(p) = r.to_str() {
            cmd.args(["-v", &format!("{p}:{p}:ro")]);
        }
    }
    for w in &policy.write_roots {
        if let Some(p) = w.to_str() {
            cmd.args(["-v", &format!("{p}:{p}")]);
        }
    }
    cmd.arg(image).args(["/bin/sh", "-s"]);
    pipe_code(cmd, code).context("docker backend (is the docker CLI on PATH + the daemon up?)")
}

/// `ssh <host> /bin/sh -s`, code on stdin. Remote confinement + egress are the host's, not ours
/// (documented residual — operator-vouched, not agent-proven).
fn run_ssh(host: &str, code: &str) -> Result<String> {
    let mut cmd = Command::new("ssh");
    cmd.arg(host).args(["/bin/sh", "-s"]);
    pipe_code(cmd, code).context("ssh backend (is the ssh CLI on PATH + the host reachable?)")
}

/// Spawn `cmd`, write `code` to its STDIN (NEVER argv-interpolated — the safe-passing mechanism),
/// capture stdout+stderr. A missing binary makes `spawn()` Err → fail-closed.
//
// ENV HYGIENE (5a adversarial-review LOW): unlike the local landlock backend (which env_clear()s
// the confined shell), we deliberately do NOT env_clear() the docker/ssh CLIENT — it needs the
// operator's env to function (HOME for ~/.ssh/config + known_hosts + ~/.docker/config.json,
// SSH_AUTH_SOCK for agent auth, DOCKER_HOST for a remote daemon). The UNTRUSTED snippet is still
// protected: `docker run` forwards NO host env into the container without `-e`/`--env-file`, and
// `ssh` forwards none to the remote without `SendEnv` — none of which we pass. LOAD-BEARING
// INVARIANT: never add `-e`/`--env-file` (docker) or `SendEnv` (ssh) carrying operator env, or the
// operator's secrets would reach the untrusted model-supplied code.
fn pipe_code(mut cmd: Command, code: &str) -> Result<String> {
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    child
        .stdin
        .take()
        .context("no stdin pipe")?
        .write_all(code.as_bytes())?;
    let out = child.wait_with_output()?;
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

    #[test]
    fn backend_local_confinement_reports_landlock_status() {
        let b = Backend::local();
        assert!(matches!(b.confinement(), Confinement::LocalKernel(_)));
        assert_eq!(b.label(), "local");
        assert!(b.is_local());
    }

    #[test]
    fn docker_and_ssh_never_report_landlock_enforced() {
        // THE honesty invariant (ADR-20260623 BLOCKER #1): a remote/container backend must NEVER
        // borrow the local landlock verdict. Its confinement is Container/RemoteHost, never
        // LocalKernel(Enforced) — that lie would be fail-open-while-looking-closed.
        let d = Backend::Docker {
            image: "alpine".into(),
        };
        assert!(matches!(d.confinement(), Confinement::Container { .. }));
        assert!(!d.is_local());
        assert_eq!(d.label(), "docker:alpine");
        let s = Backend::Ssh {
            host: "host".into(),
        };
        assert!(matches!(s.confinement(), Confinement::RemoteHost { .. }));
        assert_eq!(s.label(), "ssh:host");
        assert!(!s.is_local());
    }

    #[cfg(unix)]
    #[test]
    fn pipe_code_passes_snippet_via_stdin_not_argv() {
        // The safe-passing mechanism (Maintainer's finding): the model snippet goes to the child
        // via STDIN (`/bin/sh -s`), never interpolated into a host argv. Proven deterministically
        // with the local shell — no docker/ssh needed.
        let mut cmd = std::process::Command::new("/bin/sh");
        cmd.arg("-s");
        let out = pipe_code(cmd, "echo stdin-works; echo $((3+4))").unwrap();
        assert!(out.contains("stdin-works"), "stdin snippet must run: {out}");
        assert!(
            out.contains('7'),
            "the shell evaluated the stdin snippet: {out}"
        );
    }

    #[test]
    fn refuse_sandbox_always_denies_and_reports_unavailable() {
        let sb = RefuseSandbox;
        let err = sb.run_confined("echo hi", &Policy::default()).unwrap_err();
        assert!(err.to_string().contains("refused"), "got: {err}");
        assert!(matches!(sb.status(), LandlockStatus::Unavailable { .. }));
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn default_sandbox_refuses_on_non_linux() {
        // On a box with no landlock, execute_code MUST be fail-closed by default.
        let err = default_sandbox()
            .run_confined("echo hi", &Policy::default())
            .unwrap_err();
        assert!(err.to_string().contains("refused"), "got: {err}");
        assert!(matches!(
            landlock_status(),
            LandlockStatus::Unavailable { .. }
        ));
    }
}
