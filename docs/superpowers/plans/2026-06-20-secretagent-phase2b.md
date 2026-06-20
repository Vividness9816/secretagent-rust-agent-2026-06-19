# SecretAgent Phase 2b (sa-exec + landlock kernel tier) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `execute_code` can run a shell snippet **confined by landlock to the policy's file roots on Linux**, and is **fail-closed** everywhere else — refused unless a kernel sandbox is runtime-enforced, with one per-invocation, never-persisted, screaming override. A CI/WSL deny-corpus proves the kernel boundary actually denies (not vacuously).

**Architecture:** The Linux kernel tier of ADR-20260620. A new `sa-exec` crate exposes a `Sandbox` trait with two impls: `LandlockSandbox` behind one `#[cfg(target_os = "linux")]` module (target-gated `landlock` dep — never enters the Windows/macOS build graph) and `RefuseSandbox` (all platforms, always denies). The `execute_code` tool owns a `Box<dyn Sandbox>` (so the shared `Tool` trait is unchanged) and refuses unless the sandbox is enforced. The agent loop gains an approval path (`--yes`) so an approved+confined exec can actually run; the screaming override (`--allow-unsandboxed-exec`) is a per-CLI-invocation escape valve.

**Tech Stack:** existing crates + new `sa-exec`; `landlock` 0.4 (Linux-target-gated); `libc` (already transitive) for the doctor probe. Cross-platform tasks gate on Windows + WSL; Linux-only tasks (2, 3) gate in **WSL Ubuntu (kernel 6.6, landlock ABI 3)** then CI `ubuntu-latest` confirms.

**Authority:** `~/.claude/second-brain/decisions/ADR-20260620-secretagent-phase2-sandbox.md` (+ founding ADR-20260619). On conflict, the ADR wins.

## Global Constraints

- **`execute_code` is fail-closed**: it runs ONLY when the sandbox is runtime-*enforced* (landlock FullyEnforced), not merely compiled-in. On Windows/macOS (no landlock) it is **refused**, never run unconfined — unless the per-invocation override is passed.
- **One `#[cfg(target_os = "linux")]` module** holds all landlock code; the `landlock` dep is target-gated so Windows/macOS never compile it.
- **Defer** (ADR named triggers — do NOT build): seccomp, namespaces, Docker, Firecracker, the `ExecutionBackend` trait, override TTL/expiry, aarch64 runtime sandbox verification.
- **`sa-exec` is sandbox-only** — the injection guard already lives in `sa-core-types`/`sa-core` (2a); do not duplicate it here.
- **Audit durability**: the audit record for an untrusted/irreversible dispatch is `append_synced` (fsync) **before** the tool runs. The override-enable event is the first thing fsync'd.
- **Override**: per-invocation CLI flag, **never persisted** (no config bool, no TTL), **screaming** — a loud `sa-audit` event + a stderr banner every use.
- **TDD; commit per task**; conventional commits ending with the Co-Authored-By + Claude-Session footer. Gate each task on `cargo fmt --all -- --check` (0) / `cargo clippy --all-targets --all-features -- -D warnings` (0) / `cargo test` (pass). The `self-audit` PreToolUse hook blocks `git commit` — append ` # self-audit-ok` to the commit command.
- **Local Linux venue:** WSL Ubuntu at `/mnt/c/Users/dnoye/ClaudeSecondBrain/SecretAgent`, building with `CARGO_TARGET_DIR=$HOME/sa-target` (keeps artifacts off the slow 9p mount and away from the Windows `target/`). Cargo path: `$HOME/.cargo/bin/cargo`.

## File Structure

```
crates/
  sa-exec/ (NEW crate)
    Cargo.toml                 trait deps + target-gated landlock; dev-deps tempfile
    src/lib.rs                 Sandbox trait + LandlockStatus + RefuseSandbox + default_sandbox + landlock_status + pure refuse tests
    src/landlock_linux.rs      #[cfg(linux)] LandlockSandbox + apply_landlock ruleset + probe
    tests/landlock_escape.rs   #![cfg(linux)] kernel deny-corpus (forbidden EACCES + positive control + no-op canary + write cases)
  sa-tools/src/
    lib.rs                     + ExecuteCode tool (owns Box<dyn Sandbox> + allow_unsandboxed); fail-closed refuse; screaming unconfined override path
    Cargo.toml                 + sa-exec dep
  sa-core/src/lib.rs           run_task gains `auto_approve: bool`; audit moves to before-dispatch (append_synced)
  sa-core/Cargo.toml           (unchanged — already deps sa-audit/sa-tools)
secretagent/
  Cargo.toml                   + sa-exec dep
  src/main.rs                  Run gains --yes and --allow-unsandboxed-exec flags
  src/run.rs                   wire ExecuteCode into the registry; screaming override-enable audit + banner; pass auto_approve
  src/doctor.rs                + landlock capability line (reports; never fails doctor's exit)
  tests/live_exec.rs (NEW)     #[ignore] live execute_code acceptance (Linux/landlock)
```

---

### Task 1: `sa-exec` crate — `Sandbox` trait, `RefuseSandbox`, status, factory (cross-platform)

**Files:**
- Create: `crates/sa-exec/Cargo.toml`, `crates/sa-exec/src/lib.rs`
- Modify: `Cargo.toml` (workspace members)

**Interfaces:**
- Consumes: `sa_core_types::policy::Policy`.
- Produces: `pub trait Sandbox: Send + Sync { fn run_confined(&self, code: &str, policy: &Policy) -> anyhow::Result<String>; fn status(&self) -> LandlockStatus; }`; `pub enum LandlockStatus { Enforced { abi: i32 }, Unavailable { reason: String } }`; `pub struct RefuseSandbox`; `pub fn default_sandbox() -> Box<dyn Sandbox>`; `pub fn landlock_status() -> LandlockStatus`. On non-Linux, `default_sandbox()` returns `RefuseSandbox`.

- [ ] **Step 1: `crates/sa-exec/Cargo.toml`**

```toml
[package]
name = "sa-exec"
version = "0.1.0"
edition.workspace = true
license.workspace = true
repository.workspace = true
rust-version.workspace = true

[dependencies]
sa-core-types = { path = "../sa-core-types" }
anyhow.workspace = true

# Linux-only: the landlock kernel sandbox + libc for the doctor probe. Target-gated so
# the Windows/macOS build graph never sees them (ADR-20260620: one cfg(linux) surface).
[target.'cfg(target_os = "linux")'.dependencies]
landlock = "0.4"
libc = "0.2"

[dev-dependencies]
tempfile = "3"
```

Add `"crates/sa-exec"` to the workspace `members` array in the root `Cargo.toml`.

- [ ] **Step 2: Write the failing pure tests** in `crates/sa-exec/src/lib.rs` (bottom):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use sa_core_types::policy::Policy;

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
        assert!(matches!(landlock_status(), LandlockStatus::Unavailable { .. }));
    }
}
```

- [ ] **Step 3: Run — verify fail.** WSL: `cargo test -p sa-exec` → FAIL (types missing). (Also on Windows for the non-linux test.)

- [ ] **Step 4: Implement `crates/sa-exec/src/lib.rs`**

```rust
//! The tool-execution sandbox (ADR-20260620, Tier D: landlock-only). A single
//! `Sandbox` seam with two impls — `LandlockSandbox` (Linux kernel tier) and
//! `RefuseSandbox` (all platforms, fail-closed default). The injection guard is NOT
//! here: it lives in sa-core-types/sa-core. This crate is sandbox-only.
use anyhow::Result;
use sa_core_types::policy::Policy;

#[derive(Debug, Clone)]
pub enum LandlockStatus {
    /// Landlock is present AND a ruleset fully enforces; `abi` is the kernel ABI level.
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
```

- [ ] **Step 5: Run — verify pass.** WSL: `cargo test -p sa-exec` (the refuse test passes; the linux module is empty-stub-free because Task 2 fills it — until then `landlock_linux` is missing, so DO Task 2 in the same task boundary OR stub `LandlockSandbox::new()`). **NOTE:** because `pub use landlock_linux::LandlockSandbox` references the module, Tasks 1 and 2 must both land before `-p sa-exec` compiles on Linux. Keep Step 4 here, but run the Linux compile/test at the end of Task 2. On **Windows**, `cargo test -p sa-exec` compiles now (the linux module is cfg'd out) and the non-linux test passes — run it here.

- [ ] **Step 6: Commit** (after Task 2 lands on Linux, or now if validating only the Windows path):

```bash
git add crates/sa-exec/Cargo.toml crates/sa-exec/src/lib.rs Cargo.toml && git commit -m "feat(exec): Sandbox trait + RefuseSandbox + status/factory (sandbox seam)" # self-audit-ok
```

---

### Task 2: `LandlockSandbox` — Linux module (`apply_landlock` ruleset + probe + confined exec)

**Files:**
- Create: `crates/sa-exec/src/landlock_linux.rs`

**Interfaces:**
- Consumes: `crate::{LandlockStatus, Sandbox}`, `sa_core_types::policy::Policy`, the `landlock` + `libc` crates.
- Produces: `pub struct LandlockSandbox; impl LandlockSandbox { pub fn new() -> Self }`; `impl Sandbox for LandlockSandbox`. Internal: `fn apply_landlock(read_roots, write_roots) -> Result<(), String>` (builds + restricts a ruleset granting read+exec on standard system dirs, read on `read_roots`, read/write on `write_roots`); `fn probe() -> LandlockStatus` (reports the enforced ABI without confining the caller).

- [ ] **Step 0: Verify the `landlock` 0.4 API before coding.** Run `cargo doc -p landlock --open` is unavailable headless; instead read the installed source: WSL `ls ~/.cargo/registry/src/*/landlock-0.4*/src/` and confirm the names used below — `ABI`, `AccessFs::{from_read,from_all}`, `Ruleset`, `RulesetAttr::handle_access`, `RulesetCreatedAttr::add_rules`, `path_beneath_rules`, `restrict_self`, `RestrictionStatus`, `RulesetStatus::FullyEnforced`, `CompatLevel::BestEffort`, `Compatible::set_compatibility`. Adjust the code below to the real signatures (the compiler in WSL is the final arbiter).

- [ ] **Step 1: Implement `crates/sa-exec/src/landlock_linux.rs`**

```rust
//! The Linux kernel tier: landlock-confined execution. The ONLY cfg(linux) surface.
use crate::{LandlockStatus, Sandbox};
use anyhow::{Context, Result};
use landlock::{
    Access, AccessFs, CompatLevel, Compatible, Ruleset, RulesetAttr, RulesetCreatedAttr,
    RulesetStatus, ABI,
};
use sa_core_types::policy::Policy;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

/// Standard dirs a child needs read+exec on to run `/bin/sh` + coreutils. We grant the
/// ones that exist; everything else (notably /etc, $HOME, the vault dir) stays denied.
const SYSTEM_EXEC_DIRS: &[&str] = &[
    "/bin", "/sbin", "/usr/bin", "/usr/sbin", "/usr/local/bin", "/lib", "/lib64",
    "/usr/lib", "/usr/lib64", "/usr/local/lib",
];

pub struct LandlockSandbox;

impl LandlockSandbox {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LandlockSandbox {
    fn default() -> Self {
        Self::new()
    }
}

/// Build + enforce a landlock ruleset on the CALLING thread/process. Used inside the
/// forked child (pre_exec) so it confines the child, never the agent. `BestEffort` +
/// a `FullyEnforced` check = fail-closed: on a kernel that can't fully honor the
/// requested rights we return Err and the spawn fails (we refuse rather than run loose).
// ponytail: ABI::V3 is the floor we target (WSL + CI ubuntu-latest are ABI 3). On a
// newer kernel, V3 rights still FullyEnforce; on an older one we refuse. Bump when a
// targeted deployment needs a newer right.
fn apply_landlock(read_roots: &[PathBuf], write_roots: &[PathBuf]) -> Result<(), String> {
    let abi = ABI::V3;
    let read_paths: Vec<PathBuf> = SYSTEM_EXEC_DIRS
        .iter()
        .map(PathBuf::from)
        .chain(read_roots.iter().cloned())
        .filter(|p| p.exists())
        .collect();
    let write_paths: Vec<PathBuf> = write_roots.iter().filter(|p| p.exists()).cloned().collect();

    let status = Ruleset::default()
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(AccessFs::from_all(abi))
        .map_err(|e| e.to_string())?
        .create()
        .map_err(|e| e.to_string())?
        .add_rules(landlock::path_beneath_rules(&read_paths, AccessFs::from_read(abi)))
        .map_err(|e| e.to_string())?
        .add_rules(landlock::path_beneath_rules(&write_paths, AccessFs::from_all(abi)))
        .map_err(|e| e.to_string())?
        .restrict_self()
        .map_err(|e| e.to_string())?;

    match status.ruleset {
        RulesetStatus::FullyEnforced => Ok(()),
        other => Err(format!("landlock not fully enforced: {other:?}")),
    }
}

/// Report the enforced ABI WITHOUT confining the agent: restrict a throwaway thread
/// (landlock restrictions apply to the calling thread + its future children, never to
/// existing sibling threads), then read its result. The main thread stays unconfined.
// ponytail: thread-probe avoids a raw fork; the throwaway thread restricts only itself
// and exits. If a future kernel makes thread-local restriction unsound here, switch to a
// forked-child probe.
fn probe() -> LandlockStatus {
    let handle = std::thread::spawn(|| {
        let abi = ABI::V3;
        let status = Ruleset::default()
            .set_compatibility(CompatLevel::BestEffort)
            .handle_access(AccessFs::from_all(abi))
            .ok()?
            .create()
            .ok()?
            .restrict_self()
            .ok()?;
        match status.ruleset {
            RulesetStatus::FullyEnforced | RulesetStatus::PartiallyEnforced => Some(abi as i32),
            RulesetStatus::NotEnforced => None,
        }
    });
    match handle.join() {
        Ok(Some(abi)) => LandlockStatus::Enforced { abi },
        Ok(None) => LandlockStatus::Unavailable {
            reason: "landlock present but ruleset not enforced".into(),
        },
        Err(_) => LandlockStatus::Unavailable {
            reason: "landlock probe thread panicked".into(),
        },
    }
}

impl Sandbox for LandlockSandbox {
    fn status(&self) -> LandlockStatus {
        probe()
    }

    fn run_confined(&self, code: &str, policy: &Policy) -> Result<String> {
        // Fail-closed: if we can't even prove enforcement, refuse before spawning.
        if let LandlockStatus::Unavailable { reason } = probe() {
            anyhow::bail!("execute_code refused: landlock not enforced ({reason})");
        }
        let read_roots = policy.read_roots.clone();
        let write_roots = policy.write_roots.clone();

        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg(code);
        // SAFETY: pre_exec runs in the forked child, after fork, before execve. It only
        // calls landlock syscalls (build ruleset + restrict_self). Confines the child.
        unsafe {
            cmd.pre_exec(move || {
                apply_landlock(&read_roots, &write_roots)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
                Ok(())
            });
        }
        let out = cmd.output().context("spawning landlock-confined /bin/sh")?;
        let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
        let err = String::from_utf8_lossy(&out.stderr);
        if !err.is_empty() {
            s.push_str(&err);
        }
        Ok(s)
    }
}
```

- [ ] **Step 2: Run — compile + the Task-1 refuse test on Linux.** WSL: `cargo test -p sa-exec`. Expected: compiles, the refuse test passes (the kernel corpus is Task 3). If the `landlock` API names differ, fix per Step 0 until it compiles.

- [ ] **Step 3: fmt + clippy.** WSL: `cargo fmt -p sa-exec` then `cargo clippy -p sa-exec --all-targets -- -D warnings`. Fix warnings (e.g. `as_data` not relevant here; `#[allow(clippy::...)]` only with a reason).

- [ ] **Step 4: Commit** (folds Task 1's lib.rs since they co-compile on Linux):

```bash
git add crates/sa-exec/ Cargo.toml && git commit -m "feat(exec): LandlockSandbox — apply ruleset (read/exec sys + policy roots) + thread-probe + confined sh" # self-audit-ok
```

---

### Task 3: Kernel deny-corpus — landlock actually denies (the crown jewel)

**Files:**
- Create: `crates/sa-exec/tests/landlock_escape.rs`

**Interfaces:**
- Consumes: `sa_exec::{LandlockSandbox, Sandbox}`, `sa_core_types::policy::Policy`.
- Produces: nothing (integration test). Proves the boundary with a **positive control** (allowed path reachable), a **forbidden case** (outside-roots read → blocked), and a **no-op canary** (same read WITHOUT the sandbox → succeeds, so the forbidden case isn't vacuous), plus a write adjacent-deny.

- [ ] **Step 1: Write the corpus.** `crates/sa-exec/tests/landlock_escape.rs`:

```rust
//! Kernel-enforcement deny-corpus (ADR-20260620). Runs ONLY on Linux, where landlock
//! actually enforces (WSL Ubuntu ABI 3 / CI ubuntu-latest). Each escape case carries a
//! positive control (an allowed op succeeds — catches a deny-everything ruleset) and a
//! no-op canary (the SAME op succeeds without the sandbox — proves the test isn't vacuous).
#![cfg(target_os = "linux")]

use sa_core_types::policy::Policy;
use sa_exec::{LandlockSandbox, Sandbox};
use std::path::PathBuf;

fn policy(read: Vec<PathBuf>, write: Vec<PathBuf>) -> Policy {
    Policy {
        egress_allow: vec![],
        read_roots: read,
        write_roots: write,
    }
}

#[test]
fn confined_shell_reads_allowed_but_not_outside_roots() {
    let allowed = tempfile::tempdir().unwrap();
    std::fs::write(allowed.path().join("ok.txt"), "ALLOWED_CONTENT").unwrap();
    let secret = tempfile::tempdir().unwrap(); // deliberately NOT in any root
    let secret_file = secret.path().join("secret.txt");
    std::fs::write(&secret_file, "TOPSECRET").unwrap(); // 0644 → perms allow; only landlock denies

    let p = policy(vec![allowed.path().to_path_buf()], vec![]);
    let sb = LandlockSandbox::new();

    // POSITIVE CONTROL: an allowed read succeeds (proves the ruleset isn't deny-all AND
    // that /bin/sh + cat can run under it — i.e. the system-dir grants are correct).
    let ok = sb
        .run_confined(
            &format!("cat {}", allowed.path().join("ok.txt").display()),
            &p,
        )
        .unwrap();
    assert!(
        ok.contains("ALLOWED_CONTENT"),
        "positive control failed — sandbox denied an ALLOWED path or sh couldn't run: {ok:?}"
    );

    // FORBIDDEN: reading outside the roots is blocked by landlock.
    let denied = sb
        .run_confined(
            &format!("cat {} 2>&1; echo EXIT=$?", secret_file.display()),
            &p,
        )
        .unwrap();
    assert!(
        !denied.contains("TOPSECRET"),
        "SANDBOX ESCAPE: confined shell read a file OUTSIDE its roots:\n{denied}"
    );
    assert!(
        denied.contains("EXIT=") && !denied.contains("EXIT=0"),
        "forbidden read should fail non-zero (EACCES): {denied}"
    );

    // NO-OP CANARY: the identical read WITHOUT the sandbox SUCCEEDS — proving the file is
    // readable and the forbidden case above is non-vacuous (the sandbox is the only diff).
    let canary = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(format!("cat {}", secret_file.display()))
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&canary.stdout).contains("TOPSECRET"),
        "canary: secret must be readable WITHOUT the sandbox, else the forbidden case is vacuous"
    );
}

#[test]
fn confined_shell_writes_into_write_root_but_not_read_only_root() {
    let workdir = tempfile::tempdir().unwrap(); // read root only
    let outdir = tempfile::tempdir().unwrap(); // write root
    let p = policy(
        vec![workdir.path().to_path_buf()],
        vec![outdir.path().to_path_buf()],
    );
    let sb = LandlockSandbox::new();

    // POSITIVE CONTROL: write into a write_root succeeds.
    let w = sb
        .run_confined(
            &format!(
                "echo hi > {} 2>&1; echo EXIT=$?",
                outdir.path().join("w.txt").display()
            ),
            &p,
        )
        .unwrap();
    assert!(w.contains("EXIT=0"), "allowed write into write_root failed: {w}");
    assert_eq!(
        std::fs::read_to_string(outdir.path().join("w.txt"))
            .unwrap()
            .trim(),
        "hi"
    );

    // ADJACENT-DENY: writing into a read-only root is blocked (no MakeReg/WriteFile there).
    let evil = workdir.path().join("evil.txt");
    let d = sb
        .run_confined(
            &format!("echo nope > {} 2>&1; echo EXIT=$?", evil.display()),
            &p,
        )
        .unwrap();
    assert!(
        !d.contains("EXIT=0"),
        "write into a read-only root must FAIL: {d}"
    );
    assert!(
        !evil.exists(),
        "SANDBOX ESCAPE: created a file outside the write_roots"
    );
}
```

- [ ] **Step 2: Run — verify pass on a real landlock kernel.** WSL: `cargo test -p sa-exec --test landlock_escape -- --nocapture`. Expected: both tests PASS (positive controls succeed, forbidden ops blocked, canary confirms non-vacuity). If the positive control fails, the system-dir grant list in `apply_landlock` is wrong — fix it (the failure tells you sh/cat couldn't load libs). If a forbidden op leaks, the ruleset is wrong — STOP, do not commit.

- [ ] **Step 3: fmt + clippy + full crate test.** WSL: `cargo fmt -p sa-exec && cargo clippy -p sa-exec --all-targets -- -D warnings && cargo test -p sa-exec`.

- [ ] **Step 4: Commit**

```bash
git add crates/sa-exec/tests/landlock_escape.rs && git commit -m "test(exec): kernel deny-corpus — landlock blocks out-of-root read/write (positive-control + no-op canary)" # self-audit-ok
```

---

### Task 4: `execute_code` tool — fail-closed, with the screaming override (sa-tools)

**Files:**
- Modify: `crates/sa-tools/Cargo.toml` (+ `sa-exec` dep), `crates/sa-tools/src/lib.rs`

**Interfaces:**
- Consumes: `sa_exec::{Sandbox, default_sandbox}`, `sa_core_types::policy::Policy`, the `Tool` trait.
- Produces: `pub struct ExecuteCode { sandbox: Box<dyn Sandbox>, allow_unsandboxed: bool }`; `impl ExecuteCode { pub fn new(allow_unsandboxed: bool) -> Self }`; `impl Tool for ExecuteCode` (name `"execute_code"`, param `{code:string}`). On a refusing sandbox: `run` errs (fail-closed) UNLESS `allow_unsandboxed`, in which case it prints a screaming stderr banner and runs the code unconfined via the OS shell.

- [ ] **Step 1: `crates/sa-tools/Cargo.toml`** — add under `[dependencies]`:

```toml
sa-exec = { path = "../sa-exec" }
```

- [ ] **Step 2: Write the failing tests** (append to `crates/sa-tools/src/lib.rs` `mod tests`):

```rust
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
        assert!(out.contains("OVERRIDE_RAN"), "override should run the code: {out:?}");
    }
```

- [ ] **Step 3: Implement** — append to `crates/sa-tools/src/lib.rs`:

```rust
/// Run a shell snippet confined by the platform sandbox. FAIL-CLOSED: if the sandbox
/// can't be enforced, refuse — UNLESS `allow_unsandboxed` (the per-invocation, never-
/// persisted, screaming override), which runs the code with NO sandbox and a loud banner.
pub struct ExecuteCode {
    sandbox: Box<dyn sa_exec::Sandbox>,
    allow_unsandboxed: bool,
}

impl ExecuteCode {
    pub fn new(allow_unsandboxed: bool) -> Self {
        Self {
            sandbox: sa_exec::default_sandbox(),
            allow_unsandboxed,
        }
    }
    /// Test/seam constructor with an explicit sandbox.
    pub fn with_sandbox(sandbox: Box<dyn sa_exec::Sandbox>, allow_unsandboxed: bool) -> Self {
        Self {
            sandbox,
            allow_unsandboxed,
        }
    }
}

#[async_trait]
impl Tool for ExecuteCode {
    fn name(&self) -> &'static str {
        "execute_code"
    }
    fn description(&self) -> &'static str {
        "Run a shell snippet confined to the policy's file roots (Linux/landlock only; refused elsewhere)."
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"code":{"type":"string"}},"required":["code"]})
    }
    async fn run(&self, args: Value, policy: &Policy) -> Result<String> {
        let code = args
            .get("code")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("execute_code: missing 'code'"))?;
        match self.sandbox.run_confined(code, policy) {
            Ok(out) => Ok(out),
            Err(e) if self.allow_unsandboxed => {
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
        std::process::Command::new("cmd").arg("/C").arg(code).output()?
    } else {
        std::process::Command::new("/bin/sh").arg("-c").arg(code).output()?
    };
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    let err = String::from_utf8_lossy(&out.stderr);
    if !err.is_empty() {
        s.push_str(&err);
    }
    Ok(s)
}
```

- [ ] **Step 4: Run — verify pass.** WSL + Windows: `cargo test -p sa-tools`. Expected: both new tests pass on both. (On Linux the fail-closed test uses `RefuseSandbox` explicitly so it's deterministic regardless of host landlock.)

- [ ] **Step 5: fmt + clippy + commit.** `cargo fmt -p sa-tools && cargo clippy -p sa-tools --all-targets -- -D warnings`, then:

```bash
git add crates/sa-tools/ && git commit -m "feat(tools): execute_code — fail-closed sandboxed exec + screaming per-invocation override" # self-audit-ok
```

---

### Task 5: `sa-core` — approval path (`--yes`) + audit-before-dispatch

**Files:**
- Modify: `crates/sa-core/src/lib.rs` (`run_task` signature + loop)

**Interfaces:**
- Consumes: existing `run_task` deps.
- Produces: `run_task(&self, session_id, user_input, registry, policy, audit, auto_approve: bool) -> Result<String>`. When `auto_approve`, approval-required tools (`write_file`, `execute_code`) run instead of being headless-denied. The `append_synced` audit moves to **before** the tool runs (survives a crash of the tool itself).

- [ ] **Step 1: Write the failing test** (append to `crates/sa-core/src/lib.rs` `mod tests`):

```rust
    #[tokio::test]
    async fn approval_required_tool_runs_only_when_auto_approved() {
        use sa_audit::Audit;
        use sa_providers::ScriptedProvider;
        use sa_tools::Registry;

        let dir = tempfile::tempdir().unwrap();
        // A scripted model that calls an approval-required tool ("write_file"), then answers.
        let make_provider = || {
            ScriptedProvider::new(vec![
                ProviderAction::ToolCall {
                    id: "c0".into(),
                    name: "write_file".into(),
                    args: serde_json::json!({"path": "x", "content": "y"}),
                },
                ProviderAction::Text("done".into()),
            ])
        };
        let mut registry = Registry::new();
        registry.register(Box::new(MockTool {
            name: "write_file",
            output: "WROTE".into(),
        }));
        let policy = Policy::default();

        // auto_approve = false → denied (headless strict default).
        {
            let store = Store::open(&dir.path().join("a.db")).unwrap();
            let mut audit = Audit::open(&dir.path().join("a.jsonl")).unwrap();
            let agent = Agent::new(store, Box::new(make_provider()));
            agent
                .run_task("s", "go", &registry, &policy, &mut audit, false)
                .await
                .unwrap();
            let log = std::fs::read_to_string(dir.path().join("a.jsonl")).unwrap();
            assert!(log.contains("tool.denied"), "must deny without approval: {log}");
            assert!(!log.contains("WROTE")); // tool never ran (and output never logged anyway)
        }
        // auto_approve = true → the tool runs (audited by name before dispatch).
        {
            let store = Store::open(&dir.path().join("b.db")).unwrap();
            let mut audit = Audit::open(&dir.path().join("b.jsonl")).unwrap();
            let agent = Agent::new(store, Box::new(make_provider()));
            agent
                .run_task("s", "go", &registry, &policy, &mut audit, true)
                .await
                .unwrap();
            let log = std::fs::read_to_string(dir.path().join("b.jsonl")).unwrap();
            assert!(log.contains("tool.write_file"), "approved tool must be audited: {log}");
            assert!(!log.contains("tool.denied"), "approved tool must not be denied: {log}");
        }
    }
```

- [ ] **Step 2: Update the EXISTING test call site** — the 2a injection test calls `run_task("s1", ..., &mut audit)` with 5 args; add the 6th: change it to `run_task("s1", "summarize http://example.com", &registry, &policy, &mut audit, false)`.

- [ ] **Step 3: Run — verify fail.** WSL: `cargo test -p sa-core` → FAIL (arity mismatch / new test).

- [ ] **Step 4: Implement** — in `crates/sa-core/src/lib.rs`, change the `run_task` signature to add `auto_approve: bool`, and edit the loop body:

```rust
    pub async fn run_task(
        &self,
        session_id: &str,
        user_input: &str,
        registry: &Registry,
        policy: &Policy,
        audit: &mut Audit,
        auto_approve: bool,
    ) -> Result<String> {
```

Replace the approval gate + dispatch block so (a) approval respects `auto_approve`, and (b) the audit is fsync'd BEFORE the tool runs:

```rust
                ProviderAction::ToolCall { id, name, args } => {
                    let call_echo = json!({
                        "role": "assistant",
                        "tool_calls": [{
                            "id": id, "type": "function",
                            "function": {"name": name, "arguments": args.to_string()}
                        }]
                    });

                    // Strict-by-default: side-effectful tools require approval. Headless
                    // without --yes = deny; the denial is audited and fed back as data.
                    if approval_required(&name) && !auto_approve {
                        audit.append_synced(AuditEvent {
                            action: "tool.denied".into(),
                            key_id: name.clone(),
                        })?;
                        messages.push(call_echo);
                        messages.push(json!({"role": "tool", "tool_call_id": id,
                            "content": format!("[denied: {name} requires approval; re-run with --yes]")}));
                        continue;
                    }

                    // Audit the dispatch BEFORE running — fsync'd so the record survives a
                    // crash of the tool itself (ADR-20260620). NAME only, never the output.
                    audit.append_synced(AuditEvent {
                        action: format!("tool.{name}"),
                        key_id: name.clone(),
                    })?;

                    let output = match registry.get(&name) {
                        Some(tool) => match tool.run(args.clone(), policy).await {
                            Ok(o) => o,
                            Err(e) => format!("[tool error: {e}]"),
                        },
                        None => format!("[unknown tool: {name}]"),
                    };
                    // Untrusted by construction; rendered as DATA, never an instruction.
                    let tainted = Tainted::untrusted(output, name.clone());
                    messages.push(call_echo);
                    messages.push(json!({"role": "tool", "tool_call_id": id,
                        "content": tainted.as_data()}));
                }
```

- [ ] **Step 5: Run — verify pass.** WSL: `cargo test -p sa-core`. Expected: all pass (new approval test + the updated injection test).

- [ ] **Step 6: fmt + clippy + commit.** `cargo fmt -p sa-core && cargo clippy -p sa-core --all-targets -- -D warnings`, then:

```bash
git add crates/sa-core/src/lib.rs && git commit -m "feat(core): --yes approval path + audit-before-dispatch (fsync record survives tool crash)" # self-audit-ok
```

---

### Task 6: `doctor` — landlock capability line (reports; never fails the run)

**Files:**
- Modify: `secretagent/Cargo.toml` (+ `sa-exec` dep), `secretagent/src/doctor.rs`

**Interfaces:**
- Consumes: `sa_exec::{landlock_status, LandlockStatus}`.
- Produces: a doctor line: `[ok] landlock: enforced (ABI N) — execute_code available` / `[warn] landlock: unavailable (...) — execute_code disabled` on Linux / `[info] landlock: not applicable on this OS — execute_code disabled (expected)` elsewhere. It does **not** flip the doctor exit code (founding ADR: doctor exits 0 when otherwise healthy; the true fail-closed gate is dispatch-refuse, not doctor's exit).

- [ ] **Step 1: `secretagent/Cargo.toml`** — add under `[dependencies]`:

```toml
sa-exec = { path = "../crates/sa-exec" }
```

- [ ] **Step 2: Implement** — in `secretagent/src/doctor.rs`, add before the final `if ok { ... }` block:

```rust
    // Landlock capability (ADR-20260620). Reported, never gating doctor's exit — the
    // real fail-closed gate is execute_code's dispatch-refuse, proven by the deny-corpus.
    match sa_exec::landlock_status() {
        sa_exec::LandlockStatus::Enforced { abi } => {
            println!("[ok]   landlock: enforced (ABI {abi}) — execute_code available")
        }
        sa_exec::LandlockStatus::Unavailable { reason } => {
            if cfg!(target_os = "linux") {
                println!("[warn] landlock: unavailable ({reason}) — execute_code disabled");
            } else {
                println!("[info] landlock: not applicable on this OS — execute_code disabled (expected)");
            }
        }
    }
```

- [ ] **Step 3: Run.** WSL: `cargo run -p secretagent -- doctor` → shows `[ok] landlock: enforced (ABI 3) ...`, exits 0. Windows: `cargo run -p secretagent -- doctor` → shows the `[info] ... not applicable` line, exits 0.

- [ ] **Step 4: fmt + clippy + commit.** `cargo fmt -p secretagent && cargo clippy -p secretagent --all-targets -- -D warnings`, then:

```bash
git add secretagent/Cargo.toml secretagent/src/doctor.rs && git commit -m "feat(doctor): landlock capability line (reports enforced ABI; never gates exit)" # self-audit-ok
```

---

### Task 7: `secretagent run` — flags, override audit/banner, registry wiring + acceptance

**Files:**
- Modify: `secretagent/src/main.rs` (Run flags), `secretagent/src/run.rs` (wiring)
- Create: `secretagent/tests/live_exec.rs` (`#[ignore]`)

**Interfaces:**
- Consumes: `ExecuteCode`, `Registry`, `Audit`, the new `run_task` arity.
- Produces: `secretagent run "<task>" [--yes] [--allow-unsandboxed-exec]`. `--yes` auto-approves side-effectful tools; `--allow-unsandboxed-exec` registers `ExecuteCode` with the override AND emits a screaming, fsync'd `exec.override.UNSANDBOXED` audit event + a stderr banner once at startup. `execute_code` is always registered for `run` (refused at dispatch unless enforced/overridden).

- [ ] **Step 1: `secretagent/src/main.rs`** — extend the `Run` variant:

```rust
    /// Run an agentic task: the model may call policy-gated, audited tools.
    Run {
        task: String,
        #[arg(long, default_value = "default")]
        session: String,
        /// Auto-approve side-effectful tools (write_file, execute_code) instead of denying.
        #[arg(long)]
        yes: bool,
        /// DANGER: run execute_code with NO sandbox when landlock is unavailable. Per-
        /// invocation, never persisted, loudly audited. The operator's own-box escape valve.
        #[arg(long)]
        allow_unsandboxed_exec: bool,
    },
```

And update the dispatch arm:

```rust
        Cmd::Run {
            task,
            session,
            yes,
            allow_unsandboxed_exec,
        } => run::run(&session, &task, yes, allow_unsandboxed_exec).await,
```

- [ ] **Step 2: `secretagent/src/run.rs`** — update the signature + wiring:

```rust
pub async fn run(
    session: &str,
    task: &str,
    auto_approve: bool,
    allow_unsandboxed_exec: bool,
) -> anyhow::Result<()> {
    let cfg = config::Config::load()?;
    let store = Store::open(&config::db_path())?;
    let mut audit = Audit::open(&config::audit_path())?;

    // The screaming override is per-invocation + never persisted. Announce it LOUDLY and
    // record it (fsync) before anything runs, so the grant is the first durable event.
    if allow_unsandboxed_exec {
        audit.append_synced(sa_audit::AuditEvent {
            action: "exec.override.UNSANDBOXED".into(),
            key_id: "run".into(),
        })?;
        eprintln!(
            "!!! --allow-unsandboxed-exec ENABLED: execute_code may run with NO sandbox this run !!!"
        );
    }

    let api_key = match &cfg.provider.api_key_ref {
        Some(key_id) => {
            use sa_vault::{age_file::AgeFileVault, Vault};
            use secrecy::ExposeSecret;
            let v = AgeFileVault::open_or_init(&config::identity_path(), &config::store_path())?;
            v.get(key_id)?.map(|s| s.expose_secret().to_string())
        }
        None => None,
    };
    let provider = OpenAiCompat {
        base_url: cfg.provider.base_url.clone(),
        model: cfg.provider.model.clone(),
        api_key,
    };
    let agent = Agent::new(store, Box::new(provider));

    let mut registry = Registry::default_tools();
    registry.register(Box::new(sa_tools::ExecuteCode::new(allow_unsandboxed_exec)));

    let answer = agent
        .run_task(session, task, &registry, &cfg.policy, &mut audit, auto_approve)
        .await
        .context("agentic task failed — is the model endpoint reachable?")?;
    println!("{answer}");
    Ok(())
}
```

(Add `use sa_audit::AuditEvent;`? No — fully-qualified `sa_audit::AuditEvent` above avoids an extra import churn. Keep the existing `use sa_audit::Audit;`.)

- [ ] **Step 3: Live acceptance test** — `secretagent/tests/live_exec.rs` (ignored by default; run manually against local Ollama with landlock):

```rust
//! Live execute_code acceptance — IGNORED by default. Run on a Linux box with landlock:
//!   cargo test -p secretagent --test live_exec -- --ignored --nocapture
//! Requires a tools-capable local model (Ollama hermes3) configured in config.toml.
#![cfg(target_os = "linux")]

#[tokio::test]
#[ignore]
async fn execute_code_runs_confined_end_to_end() {
    // Sanity: the sandbox enforces on this host (else the test is meaningless).
    assert!(
        matches!(sa_exec::landlock_status(), sa_exec::LandlockStatus::Enforced { .. }),
        "this host has no enforced landlock; can't run the live confined-exec test"
    );
    // The full agent path is exercised by `secretagent run "<task>" --yes` manually; this
    // ignored test documents the entry point + asserts the host capability precondition.
}
```

- [ ] **Step 4: Run hermetic suites.** WSL + Windows: `cargo test -p secretagent` (the `run` hermetic path + doctor). Expected: pass on both. Then WSL: `cargo build -p secretagent` and a manual smoke:

```bash
# WSL, with a Linux landlock kernel:
cargo run -p secretagent -- doctor   # shows [ok] landlock enforced ABI 3
```

- [ ] **Step 5: fmt + clippy + commit.**

```bash
git add secretagent/ && git commit -m "feat(bin): secretagent run --yes/--allow-unsandboxed-exec + execute_code wiring + override audit" # self-audit-ok
```

---

### Task 8: Whole-workspace gates + push + CI to green

- [ ] **Step 1: Full workspace gate in WSL** (the Linux superset — compiles every crate incl. the landlock module + kernel corpus):

```bash
# WSL:
cd /mnt/c/Users/dnoye/ClaudeSecondBrain/SecretAgent
export CARGO_TARGET_DIR=$HOME/sa-target PATH=$HOME/.cargo/bin:$PATH
cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
```

Expected: fmt 0, clippy 0, all tests pass (including `landlock_escape`).

- [ ] **Step 2: Windows cross-platform gate** (confirms the non-Linux path: RefuseSandbox default, execute_code refused, no landlock dep in the graph):

```powershell
cargo fmt --all -- --check; cargo clippy --all-targets --all-features -- -D warnings; cargo test --all
```

Expected: pass (the `landlock_escape` test + linux module are cfg'd out; `default_sandbox_refuses_on_non_linux` + the fail-closed `execute_code` test pass).

- [ ] **Step 3: cargo-deny** (new `landlock`/`libc` deps must pass the licenses/bans/advisories closure):

```bash
# WSL:
cargo deny check 2>&1 | tail -20   # for inspection only; do NOT gate on a tail'd exit code
cargo deny check                    # the real gate
```

If `landlock`/`landlock-sys`/`enumflags2` licenses aren't allowed, add them to `deny.toml`'s allow-list with a one-line note (they are MIT/Apache-2.0 / BSD-family). Commit any `deny.toml` change separately: `chore(ci): allow-list landlock crate licenses`.

- [ ] **Step 4: Push + watch CI.**

```bash
git push origin master
# then watch the run to green (re-attach if it returns early):
gh run watch <id> --exit-status --interval 25
```

Fix any red (most likely: a `landlock` API name mismatch the WSL build already caught, or a deny license). The kernel corpus runs on the `check` leg (`cargo test --all` on `ubuntu-latest`, glibc, landlock ABI 3-4) — it must pass there exactly as in WSL.

- [ ] **Step 5: STOP at the acceptance gate.** Summarize for review before slice 2c (MCP):
  - `execute_code` fail-closed (refused without enforced landlock) — proven on Windows + by the RefuseSandbox test.
  - landlock actually denies out-of-root read/write — proven by the kernel deny-corpus (positive control + no-op canary make it non-vacuous) in WSL **and** CI.
  - the override is per-invocation, never persisted, screaming (audit event + banner) — proven by the tool test + run wiring.
  - audit fsync'd before dispatch; doctor reports the capability without breaking exit-0.

---

## Self-Review

**ADR coverage (ADR-20260620):**
- *`sa-exec` crate, `Sandbox` trait, `LandlockSandbox` behind one cfg(linux) module + target-gated dep, `RefuseSandbox` all-platforms* → Tasks 1, 2. ✓
- *Defer `ExecutionBackend` trait* → not built (one concrete sandbox seam only). ✓
- *`execute_code` refuses unless landlock runtime-enforced; fail-closed dispatch* → Task 4 (tool refuses on non-enforced sandbox) + Task 2 (`run_confined` probes + refuses). ✓
- *doctor landlock-ABI probe* → Task 6. ✓ (reports; the gating is at dispatch — deviation from the ADR's literal "doctor probe is gating" noted below.)
- *Deny-corpus: pure (Windows-runnable) + cfg(linux) kernel tier; positive control + no-op canary; paired adjacent-deny* → Task 1 (pure refuse tests, run on Windows) + Task 3 (kernel corpus: read forbidden+EACCES, allowed positive control, no-op canary, write adjacent-deny). ✓
- *`append_synced` before untrusted exec dispatch* → Task 5 (audit moved before `tool.run`) + Task 7 (override-enable fsync'd first). ✓
- *Per-invocation, never-persisted, screaming override; no TTL* → Task 4 (banner every use) + Task 7 (CLI flag, fsync'd audit event, never written to config). ✓
- *Deferred: seccomp/namespaces/Docker/Firecracker/ExecutionBackend/override-TTL/aarch64-runtime-verify* → none built. ✓

**Deliberate deviation (flagged for the gate):** the ADR says the doctor probe is "gating (fail-closed)." Interpreted as: the *probe result gates dispatch* (execute_code refuses), NOT that `doctor` exits 1. Making `doctor` exit non-zero on every Windows/macOS box (and on a Linux box with an old kernel) would violate the **founding ADR invariant** that `doctor` exits 0 when otherwise healthy. So doctor *reports* (`[ok]`/`[warn]`/`[info]`) and the true fail-closed enforcement is the dispatch-refuse, which the deny-corpus proves. One-line change to flip Linux-`[warn]`→`[fail]` if the user wants hard-gating there.

**Placeholder scan:** every code step has real code; the only "verify the exact API" step (Task 2 Step 0) is a genuine pre-code check against the installed `landlock` 0.4 source, with the compiler (in WSL) as the final arbiter — not a placeholder.

**Type consistency:** `Sandbox::{run_confined,status}`, `LandlockStatus::{Enforced{abi},Unavailable{reason}}`, `RefuseSandbox`, `default_sandbox`, `landlock_status`, `ExecuteCode::{new,with_sandbox}`, `run_task(.., auto_approve: bool)`, `AuditEvent{action,key_id}`, CLI `--yes`/`--allow-unsandboxed-exec` → consistent across Tasks 1–7.

**Ponytail decisions (named ceilings):** `Tool` trait unchanged — the tool owns its `Box<dyn Sandbox>` (no trait surgery); `ABI::V3` floor (WSL+CI are ABI 3); blocking spawn on the async path (single CLI task); thread-probe for `status()` (no raw fork); per-CLI-invocation override = one screaming enable event + per-use banner (a per-call distinct audit *action* would need a tool→loop return-channel — deferred). WSL is the local Linux venue so every Linux-only assertion is verified before push.
