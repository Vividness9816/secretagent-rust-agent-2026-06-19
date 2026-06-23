# SecretAgent Phase 5a — Execution Backends Implementation Plan

> Executed inline (autonomous) per ADR-20260623. TDD, commit-per-task, both-venue gate, adversarial-review Workflow before push.

**Goal:** Add Docker + SSH execution backends for `execute_code` behind a closed `enum Backend { Local, Docker, Ssh }`, with **honest per-backend confinement status** (a remote backend never borrows the local landlock verdict) and **backend chosen by operator-frozen config, never a model tool argument** — the two non-negotiable blockers from ADR-20260623. Foundation + half of acceptance (a).

**Architecture:** `Local` delegates to the existing `Sandbox`/landlock path verbatim (proven 2b tests unchanged). `Docker`/`Ssh` shell out (`docker run`/`ssh`, the model snippet passed via **stdin** to `/bin/sh -s`, never argv-interpolated), runtime-optional (refuse if the binary is absent — no feature gate needed since shell-out has zero deps). `enum Confinement { LocalKernel(LandlockStatus), Container, RemoteHost }` is the honest status type. Backend resolved from `[exec]` config at tool-construction time; `execute_code`'s schema stays `{code}`-only.

**Tech stack:** Rust, `std::process::Command` (zero new deps), existing `sa-exec`/`sa-tools`/`sa-core-types`.

## Global constraints
- TDD; commit per task; conventional commits; footer `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>` + `Claude-Session: phase-5a`; append ` # self-audit-ok` to git commits (the hook).
- fmt --check 0 / clippy -D warnings 0 / tests pass before each commit.
- **No new deps** (shell-out only) → musl-static + rustls-clean hold by construction. Verify `cargo tree` purity unchanged.
- The model snippet is passed via **stdin** (`/bin/sh -s`), NEVER interpolated into a host argv (Maintainer's safe-passing finding).
- Backend MUST NOT appear in any tool's `parameters()` schema (locked by a test).
- Honest status: `Docker`/`Ssh` NEVER return `LandlockStatus::Enforced`.

---

### Task 1: `sa-exec` — `enum Backend` + `Confinement` + Docker/SSH shell-out

**Files:** Modify `crates/sa-exec/src/lib.rs` (add `Backend`, `Confinement`, shell-out fns, tests).

**Interfaces produced:**
- `pub enum Confinement { LocalKernel(LandlockStatus), Container { image: String }, RemoteHost { host: String } }`
- `pub enum Backend { Local(Box<dyn Sandbox>), Docker { image: String }, Ssh { host: String } }`
- `impl Backend`: `pub fn local() -> Self` (wraps `default_sandbox()`); `pub fn run(&self, code: &str, policy: &Policy) -> Result<String>`; `pub fn confinement(&self) -> Confinement`; `pub fn is_local(&self) -> bool`; `pub fn label(&self) -> String` (e.g. `"local"`, `"docker:img"`, `"ssh:host"`, secret-free for audit).

- [ ] **Step 1: RED tests** (append to `sa-exec/src/lib.rs` tests). Docker/SSH are tested by their REFUSAL when the binary is forced absent (PATH manipulation) + honest status — no live daemon needed (the McpClient/MockConnector discipline):

```rust
#[test]
fn backend_local_confinement_reports_landlock_status() {
    let b = Backend::local();
    assert!(matches!(b.confinement(), Confinement::LocalKernel(_)));
    assert_eq!(b.label(), "local");
    assert!(b.is_local());
}

#[test]
fn docker_and_ssh_never_report_landlock_enforced() {
    // The honesty invariant: a remote/container backend must NEVER borrow the local
    // landlock verdict. Its confinement is Container/RemoteHost, never LocalKernel(Enforced).
    let d = Backend::Docker { image: "alpine".into() };
    assert!(matches!(d.confinement(), Confinement::Container { .. }));
    assert!(!d.is_local());
    assert_eq!(d.label(), "docker:alpine");
    let s = Backend::Ssh { host: "host".into() };
    assert!(matches!(s.confinement(), Confinement::RemoteHost { .. }));
    assert_eq!(s.label(), "ssh:host");
}

#[test]
fn docker_backend_refuses_when_docker_binary_is_absent() {
    // Runtime-optional: with `docker` not resolvable, run() fail-closes (no panic, no silent run).
    let b = Backend::Docker { image: "alpine".into() };
    // Force an empty PATH so the `docker` binary can't be found.
    let prev = std::env::var_os("PATH");
    std::env::set_var("PATH", "");
    let res = b.run("echo hi", &Policy::default());
    if let Some(p) = prev { std::env::set_var("PATH", p); } else { std::env::remove_var("PATH"); }
    assert!(res.is_err(), "docker backend must refuse when the docker binary is absent");
}
```

- [ ] **Step 2: verify RED** — `cargo test -p sa-exec backend` (also run in WSL for the docker test) → fail (undefined).

- [ ] **Step 3: GREEN** — implement in `sa-exec/src/lib.rs`:

```rust
use std::io::Write;
use std::process::{Command, Stdio};

/// Honest per-backend confinement (ADR-20260623). A remote/container backend NEVER borrows the
/// local landlock verdict — that lie is the exact fail-open-while-looking-closed the ADR forbids.
#[derive(Debug, Clone)]
pub enum Confinement {
    /// Local process under the kernel sandbox; carries the real landlock status.
    LocalKernel(LandlockStatus),
    /// A Docker container — operator-vouched isolation (image + --network=none), not agent-proven.
    Container { image: String },
    /// A remote host over SSH — operator-vouched; confinement (and egress) is the host's, not ours.
    RemoteHost { host: String },
}

/// Where execute_code runs. CLOSED + operator-frozen (selected from config, never a model arg).
/// Docker/SSH shell out (zero deps → musl-static holds by construction) and are runtime-optional
/// (refuse if the CLI is absent). `Local` delegates to the existing Sandbox path verbatim.
pub enum Backend {
    Local(Box<dyn Sandbox>),
    Docker { image: String },
    Ssh { host: String },
}

impl Backend {
    pub fn local() -> Self {
        Backend::Local(default_sandbox())
    }
    pub fn is_local(&self) -> bool {
        matches!(self, Backend::Local(_))
    }
    /// Secret-free label for logs/audit/doctor.
    pub fn label(&self) -> String {
        match self {
            Backend::Local(_) => "local".into(),
            Backend::Docker { image } => format!("docker:{image}"),
            Backend::Ssh { host } => format!("ssh:{host}"),
        }
    }
    pub fn confinement(&self) -> Confinement {
        match self {
            Backend::Local(sb) => Confinement::LocalKernel(sb.status()),
            Backend::Docker { image } => Confinement::Container { image: image.clone() },
            Backend::Ssh { host } => Confinement::RemoteHost { host: host.clone() },
        }
    }
    /// Run `code` on this backend. `Local` keeps the existing fail-closed landlock contract;
    /// Docker/SSH shell out (snippet via stdin, never argv) and refuse if the CLI is absent.
    pub fn run(&self, code: &str, policy: &Policy) -> Result<String> {
        match self {
            Backend::Local(sb) => sb.run_confined(code, policy),
            Backend::Docker { image } => run_docker(image, code, policy),
            Backend::Ssh { host } => run_ssh(host, code),
        }
    }
}

/// `docker run --rm -i --network=none [-v root[:ro]] <image> /bin/sh -s`, code on stdin.
/// --network=none confines egress (the host's network is otherwise un-confinable by our Policy).
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
    pipe_code(cmd, code).context("docker backend (is the docker CLI on PATH + daemon up?)")
}

/// `ssh <host> /bin/sh -s`, code on stdin. Remote confinement + egress are the host's, not ours.
fn run_ssh(host: &str, code: &str) -> Result<String> {
    let mut cmd = Command::new("ssh");
    cmd.arg(host).args(["/bin/sh", "-s"]);
    pipe_code(cmd, code).context("ssh backend (is the ssh CLI on PATH + host reachable?)")
}

/// Spawn `cmd`, write `code` to its stdin (NEVER argv-interpolated), capture stdout+stderr.
fn pipe_code(mut cmd: Command, code: &str) -> Result<String> {
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn()?; // Err if the binary is absent → fail-closed
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
```

- [ ] **Step 4: verify GREEN** — `cargo test -p sa-exec` (Windows + WSL). fmt/clippy. Commit.

---

### Task 2: `sa-tools` — `ExecuteCode` dispatches via `Backend`

**Files:** Modify `crates/sa-tools/src/lib.rs` (`ExecuteCode` holds a `Backend`; `run` dispatches; override only for Local).

**Interfaces:** `ExecuteCode::with_backend(backend: sa_exec::Backend, allow_unsandboxed: bool)`; keep `new`/`with_sandbox` (wrap into `Backend::Local`) so the 2b tests are unchanged.

- [ ] **Step 1: RED** — a Docker-backend ExecuteCode reports it's not fail-closed-by-landlock (it runs in a container); the override applies only to Local. Minimal new test:

```rust
#[test]
fn execute_code_backend_label_is_exposed_and_schema_has_no_backend_arg() {
    let tool = ExecuteCode::with_backend(sa_exec::Backend::Docker { image: "alpine".into() }, false);
    assert_eq!(tool.backend_label(), "docker:alpine");
    // The model must NEVER be able to choose a backend/host: schema is {code} only.
    let schema = tool.parameters().to_string();
    for k in ["backend", "host", "image", "ssh", "docker"] {
        assert!(!schema.contains(k), "schema must not expose '{k}': {schema}");
    }
}
```

- [ ] **Step 2: RED verify** → fail.

- [ ] **Step 3: GREEN** — change `ExecuteCode`:

```rust
pub struct ExecuteCode {
    backend: sa_exec::Backend,
    allow_unsandboxed: bool,
}
impl ExecuteCode {
    pub fn new(allow_unsandboxed: bool) -> Self {
        Self { backend: sa_exec::Backend::local(), allow_unsandboxed }
    }
    pub fn with_backend(backend: sa_exec::Backend, allow_unsandboxed: bool) -> Self {
        Self { backend, allow_unsandboxed }
    }
    pub fn with_sandbox(sandbox: Box<dyn sa_exec::Sandbox>, allow_unsandboxed: bool) -> Self {
        Self { backend: sa_exec::Backend::Local(sandbox), allow_unsandboxed }
    }
    pub fn backend_label(&self) -> String {
        self.backend.label()
    }
}
```

`run`: dispatch via `self.backend.run(code, policy)`; the screaming-override fallback applies ONLY when `self.backend.is_local()` (you don't "unconfined-override" a missing docker — that's a config error):

```rust
match self.backend.run(code, policy) {
    Ok(out) => Ok(out),
    Err(e) if self.allow_unsandboxed && self.backend.is_local() => { /* scream */ run_unconfined(code) }
    Err(e) => Err(e),
}
```

Update the tool `description()` to mention the configured backend is operator-set.

- [ ] **Step 4: GREEN verify** — `cargo test -p sa-tools` (the existing `execute_code_is_fail_closed*` + `execute_code_override*` tests must stay green — they use `with_sandbox`, now wrapping `Backend::Local`). fmt/clippy. Commit.

---

### Task 3: config `[exec]` + wire backend into `run`/`gateway` (frozen, not a model arg) + `doctor`

**Files:** `crates/sa-core-types/src/config.rs` (add `ExecConfig`), `secretagent/src/run.rs` + `gateway.rs` (build `Backend` from config), `secretagent/src/doctor.rs` (report backend).

**Interfaces:** `pub struct ExecConfig { pub backend: String, pub image: Option<String>, pub host: Option<String> }` (default backend `"local"`); `Config.exec: ExecConfig`; a helper `fn backend_from_config(&ExecConfig) -> anyhow::Result<sa_exec::Backend>`.

- [ ] **Step 1: RED** — config parse test (in config.rs): `[exec] backend="docker" image="alpine"` parses; default is `local`. And `backend_from_config` maps kinds, erroring on docker-without-image / ssh-without-host.

```rust
#[test]
fn config_parses_exec_backend_default_local() {
    let c: Config = toml::from_str("").unwrap();
    assert_eq!(c.exec.backend, "local");
    let c2: Config = toml::from_str("[exec]\nbackend=\"docker\"\nimage=\"alpine\"\n").unwrap();
    assert_eq!(c2.exec.backend, "docker");
    assert_eq!(c2.exec.image.as_deref(), Some("alpine"));
}
```

- [ ] **Step 2: RED verify** → fail.

- [ ] **Step 3: GREEN** — add `ExecConfig` (serde default backend `"local"`) + `Config.exec`. Add `backend_from_config` (in `secretagent/src/exec.rs`, a tiny new module, shared by run/gateway): `"local"`→`Backend::local()`; `"docker"`→`Backend::Docker{image: image.ok_or(...)?}`; `"ssh"`→`Backend::Ssh{host: host.ok_or(...)?}`; else error. In `run.rs` + `gateway.rs`, replace `ExecuteCode::new(...)` with `ExecuteCode::with_backend(backend_from_config(&cfg.exec)?, allow_unsandboxed)`. In `doctor.rs`, add a line: `exec backend: <label> (<confinement>)`.

- [ ] **Step 4: GREEN verify** — `cargo test --all` (Windows + WSL). fmt/clippy. Build the bin; `secretagent doctor` shows the exec backend line. Commit.

---

### Task 4: adversarial-review Workflow + both-venue gate + live Docker check + push

- [ ] **Step 1:** Run a multi-lens adversarial-review Workflow on the 5a boundary (lenses: fail-closed-honesty — can any path make Docker/SSH report/behave as landlock-enforced?; backend-as-config — can the model influence backend selection?; snippet-injection — is `code` ever argv-interpolated?; egress — Docker `--network=none` present?; secret-leak — does the backend label/error leak a secret or a token-bearing host?). Fix findings (commit each).
- [ ] **Step 2:** Both-venue gate (Windows `cargo test --all` + WSL). rustls/C-lib purity unchanged (`cargo tree | grep` empty). `cargo deny` green.
- [ ] **Step 3:** Live Docker check (manual, like the Telegram E2E — CI can't run Docker-in-Docker): `[exec] backend="docker" image="alpine"`, `secretagent run "echo hello-from-container"` → output from the container; `secretagent doctor` shows `docker:alpine` available. SSH live check documented (needs a host).
- [ ] **Step 4:** Push; watch CI green on all 5 jobs (verify headSha == HEAD). Update PROGRESS/ROADMAP + memory.

---

## Self-review
- ADR §1 (enum Backend, Local-verbatim, shell-out, no deps) → Task 1 ✅
- ADR §2 (honest per-backend status, never borrow landlock) → Task 1 `Confinement` + the `docker_and_ssh_never_report_landlock_enforced` test ✅
- ADR §3 (backend = frozen config, no model arg) → Task 3 config + the `schema has no backend arg` test ✅
- ADR §7 (doctor reports backend; egress un-confinable → `--network=none`) → Task 1 docker `--network=none` + Task 3 doctor ✅
- Audit-records-backend: covered via tracing + doctor + the secret-free `label()`; the hash-chained AuditEvent backend FIELD is deferred to a follow-up (note for the adversarial review — escalate if it flags it as a blocker).
- Snippet via stdin never argv → Task 1 `pipe_code` ✅
- Existing 2b fail-closed/override tests unchanged (with_sandbox wraps Backend::Local) → Task 2 ✅
