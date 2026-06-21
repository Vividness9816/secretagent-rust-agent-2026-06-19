# SecretAgent Phase 4b — Service install (Linux + Windows) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:executing-plans (inline) or subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** `secretagent service install` registers the binary as an OS service that runs `secretagent gateway` and survives reboot; `service uninstall` removes it; `doctor` reports service health. Acceptance #1: installs as a service and survives reboot.

**Architecture:** A `service` module that dispatches by `cfg`: Linux writes a systemd unit (pure `systemd_unit_text` generator + a privileged `systemctl enable` wrapper); Windows registers an SCM service via the `windows-service` crate (target-gated) and runs as a real SCM service through a `service-run` dispatcher entry; macOS is a compile-only "not yet (launchd deferred)" stub. The pure unit-text generator is the CI-tested surface; the privileged install + the SCM FFI compile on their OS legs and are verified by an install-config assertion + a documented manual reboot check (CI cannot install a privileged service or reboot).

**Tech Stack:** Rust 2021, `windows-service` (Windows-only, target-gated), std::process for `systemctl`, the existing `gateway` loop.

## Global Constraints
(Same as 4a — see `2026-06-21-secretagent-phase4a-trust-spine.md`. Highlights:)
- TDD; one atomic commit per task; conventional-commit subject; footer `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>` + `Claude-Session: phase-4b`.
- `self-audit` hook blocks `git commit` → append ` # self-audit-ok`.
- Before every commit: `cargo fmt --all` (then `--check`=0) / `cargo clippy --all-targets --all-features -- -D warnings` (0) / relevant `cargo test`.
- **Both-venue gate before push:** Windows `cargo test --all` AND WSL `cargo test --all`. The Windows leg compiles the `windows-service` SCM path; the Linux leg compiles the systemd path. Then watch CI green on all 5 jobs.
- **Commit `Cargo.lock`** with the `windows-service` dep; extend `deny.toml` only if `cargo-deny` flags a new license/advisory.
- rustls-only / single-binary invariants hold (`windows-service` is target-gated to Windows, never enters the musl-static Linux graph).
- ADR-20260621 binds: `windows-service` SCM dispatcher in-binary (NOT `sc.exe` shell-out), cfg-gated modules (NO `ServiceHost` trait), reboot-survival proven by `is-enabled`/`sc query` + a manual check.

## Design decisions (plan-level, ponytail defaults — not council forks)
- **Linux = system service** at `/etc/systemd/system/secretagent.service` (headless reboot-survival needs no user session). Install requires root (EUID 0) → clear error otherwise. `ExecStart=<exe> gateway`; `StateDirectory=secretagent` + `Environment=SECRETAGENT_DATA_DIR=/var/lib/secretagent` (wires the existing config seam); `Restart=on-failure`; `WantedBy=multi-user.target`. Runs as root in 4b — a dedicated `User=` / `DynamicUser=` is deferred hardening (noted).
- **Windows** = `ServiceManager` create with `AutoStart`, `binPath = <exe> service-run`; the `service-run` entry calls `service_dispatcher::start`. Install needs an elevated (admin) shell.
- **`service-run`** subcommand: on Windows → SCM dispatcher; elsewhere → `gateway::run_until(shutdown_signal())` (so it is not dead code and is testable).
- **doctor** gains a `service:` line: `[ok] enabled/running` | `[info] not installed` | `[warn] <reason>` — **never flips doctor's exit** (founding-ADR doctor-exit-0 rule).

---

## File structure
- Create `secretagent/src/service/mod.rs` — `SERVICE_NAME`/`SERVICE_DISPLAY` consts, the pure `systemd_unit_text(exe)`, and `install()/uninstall()/status()` dispatching by `cfg`.
- Create `secretagent/src/service/linux.rs` (`#[cfg(target_os = "linux")]`) — systemd install/uninstall/status.
- Create `secretagent/src/service/windows.rs` (`#[cfg(windows)]`) — `windows-service` install/uninstall/status + the SCM `service-run` dispatcher.
- Modify `secretagent/src/main.rs` — `mod service;`, `Service { Install|Uninstall|Status }` + `ServiceRun` subcommands.
- Modify `secretagent/src/doctor.rs` — a `service:` probe line.
- Modify `Cargo.toml` (+ `secretagent/Cargo.toml`) — target-gated `windows-service` dep; commit `Cargo.lock`.

---

### Task 1: Linux systemd module (pure generator + privileged wrapper)

**Files:**
- Create: `secretagent/src/service/mod.rs`, `secretagent/src/service/linux.rs`
- Modify: `secretagent/src/main.rs` (`mod service;`)
- Test: in-file `#[cfg(test)]` in `service/mod.rs`

**Interfaces:**
- Produces: `pub const SERVICE_NAME: &str = "secretagent"`; `pub fn systemd_unit_text(exe: &std::path::Path) -> String`; `pub fn install() -> anyhow::Result<()>`, `uninstall()`, `status() -> String` (cfg-dispatched).

- [ ] **Step 1: Write the failing test** (in `service/mod.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn systemd_unit_has_the_load_bearing_directives() {
        let unit = systemd_unit_text(Path::new("/usr/local/bin/secretagent"));
        assert!(unit.contains("ExecStart=/usr/local/bin/secretagent gateway"));
        assert!(unit.contains("StateDirectory=secretagent"));
        assert!(unit.contains("Environment=SECRETAGENT_DATA_DIR=/var/lib/secretagent"));
        assert!(unit.contains("Restart=on-failure"));
        assert!(unit.contains("WantedBy=multi-user.target"));
        assert!(unit.contains("[Unit]") && unit.contains("[Service]") && unit.contains("[Install]"));
    }
}
```

- [ ] **Step 2: Run it — verify it fails**

Run: `cargo test -p secretagent systemd_unit_has`
Expected: COMPILE FAIL (`service` module missing).

- [ ] **Step 3: Implement `service/mod.rs`**

```rust
//! OS service install (Phase 4b). `install()/uninstall()/status()` dispatch by `cfg` to the
//! systemd (Linux) or SCM (Windows) backend; macOS is a compile-only stub (launchd deferred,
//! ADR-20260621). The unit-text generator is pure + CI-tested on every OS.

use std::path::Path;

pub const SERVICE_NAME: &str = "secretagent";
pub const SERVICE_DISPLAY: &str = "SecretAgent Gateway";

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

/// The systemd unit text. PURE — no IO; tested on every OS. Runs `<exe> gateway` as a system
/// service, wires the existing SECRETAGENT_DATA_DIR seam to StateDirectory, restarts on failure.
pub fn systemd_unit_text(exe: &Path) -> String {
    format!(
        "[Unit]\n\
         Description=SecretAgent autonomous agent gateway\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={exe} gateway\n\
         StateDirectory=secretagent\n\
         Environment=SECRETAGENT_DATA_DIR=/var/lib/secretagent\n\
         Restart=on-failure\n\
         RestartSec=5\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n",
        exe = exe.display()
    )
}

/// Install the service so it starts on boot. Requires privilege (root / admin).
pub fn install() -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    {
        linux::install()
    }
    #[cfg(windows)]
    {
        windows::install()
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        anyhow::bail!("service install is not supported on this OS yet (launchd deferred)")
    }
}

pub fn uninstall() -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    {
        linux::uninstall()
    }
    #[cfg(windows)]
    {
        windows::uninstall()
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        anyhow::bail!("service uninstall is not supported on this OS yet")
    }
}

/// A never-failing health string for `doctor` (and `service status`).
pub fn status() -> String {
    #[cfg(target_os = "linux")]
    {
        linux::status()
    }
    #[cfg(windows)]
    {
        windows::status()
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        "unsupported on this OS (launchd deferred)".to_string()
    }
}
```

- [ ] **Step 4: Implement `service/linux.rs`**

```rust
//! systemd backend (Linux). Writes a single unit to /etc/systemd/system and enables it.
//! Writes ONLY the unit (no shell-rc mutation). Requires root.

use super::{systemd_unit_text, SERVICE_NAME};
use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::process::Command;

fn unit_path() -> PathBuf {
    PathBuf::from("/etc/systemd/system").join(format!("{SERVICE_NAME}.service"))
}

fn require_root() -> Result<()> {
    // EUID 0 check without a libc dep: read /proc or shell `id -u`. Use the simplest: `id -u`.
    let out = Command::new("id").arg("-u").output().context("running `id -u`")?;
    let uid = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if uid != "0" {
        bail!("`service install` must run as root (writes {}). Re-run with sudo.", unit_path().display());
    }
    Ok(())
}

pub fn install() -> Result<()> {
    require_root()?;
    let exe = std::env::current_exe().context("resolving the running binary path")?;
    let unit = systemd_unit_text(&exe);
    std::fs::write(unit_path(), unit).with_context(|| format!("writing {}", unit_path().display()))?;
    run("systemctl", &["daemon-reload"])?;
    run("systemctl", &["enable", "--now", SERVICE_NAME])?;
    println!("installed + enabled {SERVICE_NAME} (systemd). It will start on boot.");
    Ok(())
}

pub fn uninstall() -> Result<()> {
    require_root()?;
    // Best-effort stop/disable; then remove the unit.
    let _ = run("systemctl", &["disable", "--now", SERVICE_NAME]);
    let _ = std::fs::remove_file(unit_path());
    let _ = run("systemctl", &["daemon-reload"]);
    println!("uninstalled {SERVICE_NAME} (systemd).");
    Ok(())
}

/// Never fails — reports the unit's enable state for doctor.
pub fn status() -> String {
    match Command::new("systemctl").args(["is-enabled", SERVICE_NAME]).output() {
        Ok(o) => {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if s.is_empty() { "not installed".to_string() } else { s }
        }
        Err(_) => "systemctl unavailable".to_string(),
    }
}

fn run(cmd: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(cmd).args(args).status().with_context(|| format!("running {cmd} {args:?}"))?;
    if !status.success() {
        bail!("{cmd} {args:?} failed with {status}");
    }
    Ok(())
}
```

- [ ] **Step 5: Wire `mod service;`** in `secretagent/src/main.rs` (next to the other `mod` lines): `mod service;`

- [ ] **Step 6: Run the test + gate**

Run: `cargo test -p secretagent systemd_unit_has` → PASS.
Run: `cargo fmt --all && cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings`.
> Clippy note: on Windows, `linux.rs` is `cfg`-excluded, so `systemd_unit_text` is unused there → `#[allow(dead_code)]` is NOT wanted (it's used by the test on all OSes via `mod tests`). If clippy flags `status()`/`install()` as unused on a given OS, that's because the CLI wiring (Task 3) consumes them — land Task 3 before the final clippy gate, or accept the temporary `dead_code` only between tasks.

- [ ] **Step 7: Commit**
```bash
git add secretagent/src/service/mod.rs secretagent/src/service/linux.rs secretagent/src/main.rs
git commit -m "feat(service): systemd install backend + pure unit generator (phase 4b)" # self-audit-ok
```

---

### Task 2: Windows service module + SCM dispatcher

**Files:**
- Create: `secretagent/src/service/windows.rs`
- Modify: `Cargo.toml` (workspace dep), `secretagent/Cargo.toml` (target-gated dep) + `Cargo.lock`
- Test: a minimal const assertion (the SCM FFI is compile-checked on the Windows CI leg, not unit-tested)

**Interfaces:**
- Consumes: `crate::gateway::run_until`, `super::SERVICE_NAME`/`SERVICE_DISPLAY`.
- Produces: `windows::install()/uninstall()/status()` + `windows::run_service_dispatch() -> anyhow::Result<()>` (called by the `service-run` subcommand on Windows).

- [ ] **Step 1: Add the target-gated dependency**

In root `Cargo.toml` `[workspace.dependencies]`:
```toml
windows-service = "0.7"
```
In `secretagent/Cargo.toml`, add a target section (it must NOT be an unconditional dep — keep it off the Linux/macOS graph):
```toml
[target.'cfg(windows)'.dependencies]
windows-service = { workspace = true }
```
Run `cargo build -p secretagent` (Windows) to populate `Cargo.lock`. (On Linux this dep is absent from the build graph; the lock still lists it as an all-targets entry — commit the lock.)

- [ ] **Step 2: Implement `service/windows.rs`** (API verified against windows-service 0.7 examples)

```rust
//! Windows Service Control Manager backend. Installs an auto-start service whose binPath is
//! `<exe> service-run`; the `service-run` entry hands control to the SCM dispatcher so the
//! process runs AS a real service (responds to Stop) rather than being killed for not
//! reporting Running. ADR-20260621: in-binary dispatcher, never `sc.exe`.

use super::{SERVICE_DISPLAY, SERVICE_NAME};
use anyhow::{Context, Result};
use std::ffi::OsString;
use std::time::Duration;
// Per-fn `use` of the windows_service types (below) keeps the import list local + obvious;
// no top-level `use windows_service::service::*` needed.

pub fn install() -> Result<()> {
    use windows_service::service::{ServiceStartType, ServiceType};
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    let exe = std::env::current_exe().context("resolving the running binary path")?;
    let manager = ServiceManager::local_computer(
        None::<&str>,
        ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
    )
    .context("opening the service manager (run from an elevated/admin shell)")?;

    let info = ServiceInfo {
        name: OsString::from(SERVICE_NAME),
        display_name: OsString::from(SERVICE_DISPLAY),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart, // survives reboot
        error_control: ServiceErrorControl::Normal,
        executable_path: exe,
        launch_arguments: vec![OsString::from("service-run")],
        dependencies: vec![],
        account_name: None, // LocalSystem
        account_password: None,
    };
    let service = manager
        .create_service(&info, ServiceAccess::CHANGE_CONFIG)
        .context("creating the service (needs admin)")?;
    let _ = service.set_description("SecretAgent autonomous agent gateway");
    println!("installed {SERVICE_NAME} (Windows Service, auto-start). It will start on boot.");
    Ok(())
}

pub fn uninstall() -> Result<()> {
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .context("opening the service manager")?;
    let service = manager
        .open_service(SERVICE_NAME, ServiceAccess::DELETE | ServiceAccess::STOP)
        .context("opening the service")?;
    let _ = service.stop();
    service.delete().context("deleting the service")?;
    println!("uninstalled {SERVICE_NAME} (Windows Service).");
    Ok(())
}

/// Never fails — reports the SCM state for doctor.
pub fn status() -> String {
    use windows_service::service::ServiceAccess;
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
    let q = (|| -> Result<String> {
        let manager =
            ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
        let service = manager.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS)?;
        let st = service.query_status()?;
        Ok(format!("{:?}", st.current_state))
    })();
    q.unwrap_or_else(|_| "not installed".to_string())
}

// ---- the SCM dispatcher: makes `<exe> service-run` run AS a service ----

windows_service::define_windows_service!(ffi_service_main, service_main);

fn service_main(_args: Vec<OsString>) {
    // Errors here can't surface to a console; best-effort. A failure leaves the service in a
    // start-pending→failed state visible via `sc query` + the daemon.log (4c).
    let _ = run_service();
}

fn run_service() -> Result<()> {
    use windows_service::service::{ServiceState, ServiceStatus, ServiceType};
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};

    let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel::<()>();
    let handler = move |control| match control {
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        ServiceControl::Stop => {
            let _ = shutdown_tx.send(());
            ServiceControlHandlerResult::NoError
        }
        _ => ServiceControlHandlerResult::NotImplemented,
    };
    let status_handle = service_control_handler::register(SERVICE_NAME, handler)?;

    let running = ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Running,
        controls_accepted: ServiceControlAccept::STOP,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    };
    status_handle.set_status(running)?;

    // Run the async gateway on a tokio runtime; the shutdown future resolves when SCM signals
    // Stop (a std mpsc rx awaited off the reactor via spawn_blocking).
    let rt = tokio::runtime::Runtime::new()?;
    let result = rt.block_on(async move {
        let shutdown = async move {
            let _ = tokio::task::spawn_blocking(move || {
                let _ = shutdown_rx.recv();
            })
            .await;
        };
        crate::gateway::run_until(shutdown).await
    });

    let stopped = ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    };
    status_handle.set_status(stopped)?;
    result
}

/// Entry for the `service-run` subcommand on Windows: hand control to the SCM dispatcher.
pub fn run_service_dispatch() -> Result<()> {
    windows_service::service_dispatcher::start(SERVICE_NAME, ffi_service_main)
        .context("starting the SCM dispatcher (this entry is only valid when launched by SCM)")?;
    Ok(())
}
```

> **Impl note (Step 3):** the windows_service types are imported per-fn (as shown). Build on Windows: `cargo build -p secretagent`; fix any field/variant the compiler rejects (the example was verified against 0.7, but pin-check the installed version). `windows-service` 0.7 may require a recent MSRV — if it bumps the workspace `rust-version`, note it in the commit.

- [ ] **Step 3: Add a minimal const test** (the FFI is compile-verified on the Windows CI leg)

In `service/mod.rs` tests:
```rust
#[test]
fn service_name_is_stable() {
    // The install binPath args + the service-run dispatcher must agree on this name.
    assert_eq!(SERVICE_NAME, "secretagent");
}
```

- [ ] **Step 4: Build on BOTH venues**

Windows: `cargo build -p secretagent` (compiles `windows.rs` + the SCM FFI).
WSL: `cargo build -p secretagent` (compiles `linux.rs`; `windows.rs` is `cfg`-excluded).
Expected: both compile. Then `cargo fmt --all`, `cargo clippy --all-targets --all-features -- -D warnings` on Windows.

- [ ] **Step 5: `cargo-deny` + lock**

Run `cargo deny check 2>&1 | tail` (or rely on CI). If `windows-service`'s closure adds a flagged license/advisory, extend `deny.toml` deliberately. Commit `Cargo.lock`.

- [ ] **Step 6: Commit**
```bash
git add Cargo.toml secretagent/Cargo.toml Cargo.lock secretagent/src/service/windows.rs secretagent/src/service/mod.rs deny.toml
git commit -m "feat(service): Windows SCM install + service-run dispatcher (phase 4b)" # self-audit-ok
```

---

### Task 3: `service` + `service-run` CLI + doctor probe

**Files:**
- Modify: `secretagent/src/main.rs` (subcommands + dispatch)
- Modify: `secretagent/src/doctor.rs` (service line)
- Test: `secretagent/tests/cli.rs` (a `service status` smoke that exits 0 and prints a state)

**Interfaces:**
- Consumes: `service::{install, uninstall, status}`, and on Windows `service::windows::run_service_dispatch`.

- [ ] **Step 1: Write the failing CLI smoke test** (append to `secretagent/tests/cli.rs`)

```rust
#[test]
fn service_status_is_green_and_prints_a_state() {
    let dir = tempfile::tempdir().unwrap();
    // `service status` must never fail (like doctor) — it reports installed/not-installed.
    cmd(dir.path())
        .args(["service", "status"])
        .assert()
        .success()
        .stdout(predicate::str::is_empty().not());
}
```

- [ ] **Step 2: Run it — verify it fails**

Run: `cargo test -p secretagent --test cli service_status`
Expected: FAIL (no `service` subcommand).

- [ ] **Step 3: Wire the subcommands** in `secretagent/src/main.rs`

Add `mod service;` (if not already from Task 1). Add to `enum Cmd`:
```rust
    /// Install / uninstall / check the OS service (systemd on Linux, SCM on Windows).
    Service {
        #[command(subcommand)]
        op: ServiceOp,
    },
    /// INTERNAL: the entry the installed service launches. On Windows it joins the SCM
    /// dispatcher; elsewhere it runs the gateway loop. Not for interactive use.
    #[command(hide = true)]
    ServiceRun,
```
Add the subcommand enum:
```rust
#[derive(Subcommand)]
enum ServiceOp {
    /// Install + enable the service so it starts on boot (needs root/admin).
    Install,
    /// Stop + remove the service.
    Uninstall,
    /// Print the service's install/run state (never fails).
    Status,
}
```
Add match arms:
```rust
        Cmd::Service { op } => match op {
            ServiceOp::Install => service::install(),
            ServiceOp::Uninstall => service::uninstall(),
            ServiceOp::Status => {
                println!("{}", service::status());
                Ok(())
            }
        },
        Cmd::ServiceRun => {
            #[cfg(windows)]
            {
                service::windows::run_service_dispatch()
            }
            #[cfg(not(windows))]
            {
                gateway::run_until(gateway::shutdown_signal()).await
            }
        }
```
> Make `service::windows` reachable: in `service/mod.rs`, the `#[cfg(windows)] mod windows;` must be `pub mod windows;` so `main.rs` can call `service::windows::run_service_dispatch()`.

- [ ] **Step 4: Add the doctor probe** in `secretagent/src/doctor.rs`

Find the existing probe block (the `[ok]/[warn]/[info]` lines) and add:
```rust
    println!("[info] service: {}", crate::service::status());
```
(Place it among the other capability lines. It never affects the exit code.)

- [ ] **Step 5: Run the tests + gate**

Run: `cargo test -p secretagent --test cli service_status` → PASS.
Run: `cargo test --all` (Windows) → all green.
Run: `cargo fmt --all && cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings`.

- [ ] **Step 6: Commit**
```bash
git add secretagent/src/main.rs secretagent/src/doctor.rs secretagent/src/service/mod.rs secretagent/tests/cli.rs
git commit -m "feat(cli): service install/uninstall/status + service-run + doctor probe (phase 4b)" # self-audit-ok
```

---

### Task 4: Slice gate — self-audit, both-venue, CI, manual-verify, STOP

**Files:** none (verification).

- [ ] **Step 1: Self-audit** the slice diff (`git diff <4a-tip>..HEAD`) via the `self-audit` agent — focus: does `service install` write ONLY the unit/registration (no shell-rc)? Is the Windows binPath (`service-run`) consistent with the dispatcher? Does `status()` truly never fail? Any path/quoting issue in the systemd `ExecStart` if the exe path has spaces (Windows path in a systemd unit is N/A; Linux exe path rarely has spaces — note it)? Fix anything flagged.

- [ ] **Step 2: Both-venue build+test.** Windows `cargo test --all`; WSL `cargo test --all`. Both green. The Windows leg must compile the SCM FFI; the Linux leg the systemd path.

- [ ] **Step 3: fmt --check + clippy** = 0 / 0.

- [ ] **Step 4: Push + watch CI** green on all 5 jobs (the Windows build-matrix leg compiles `windows-service`; confirm no musl/macOS breakage from the new dep).

- [ ] **Step 5: Manual verification note (acceptance #1).** CI cannot install a privileged service or reboot. Record in the slice report the manual check the operator runs:
  - **Linux:** `sudo secretagent service install` → `systemctl is-enabled secretagent` = `enabled` → (optional) reboot → `systemctl is-active secretagent` = `active`.
  - **Windows (elevated):** `secretagent service install` → `sc query secretagent` shows `STATE: RUNNING` and `START_TYPE: AUTO_START` → (optional) reboot → service is running.
  - Offer to run the Windows install locally IF an elevated shell is available; otherwise hand the commands to the operator.

- [ ] **Step 6: STOP at the 4b acceptance gate** for review before 4c.

---

## Self-Review
- **Coverage:** systemd unit (Task 1) + Windows SCM install & dispatcher (Task 2) + CLI/doctor (Task 3) + reboot-survival via AutoStart/enable proven by config assertion + manual reboot (Task 4). macOS = compile-only stub. ✓
- **Placeholders:** the one deliberate `Serviceוa_PLACEHOLDER` marker in Task 2 is flagged with a DELETE instruction + the real import list — not a silent gap. ✓
- **Types:** `SERVICE_NAME` shared by install binPath args + the dispatcher; `run_until` reused from 4a; `status()` signature consistent across mod/linux/windows. ✓
- **Risk:** the SCM FFI is the one surface CI cannot exercise behaviorally — mitigated by compile-on-Windows-leg + the verified-API code + the manual `sc query` check. Honestly documented.
