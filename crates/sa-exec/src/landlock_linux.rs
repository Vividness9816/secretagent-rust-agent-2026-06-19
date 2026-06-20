//! The Linux kernel tier: landlock-confined execution. The ONLY cfg(linux) surface.
//!
//! `landlock` 0.4 also exports a `LandlockStatus` type — we deliberately do NOT import it;
//! `crate::LandlockStatus` (our reporting enum) is the one in scope here.
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
    "/bin",
    "/sbin",
    "/usr/bin",
    "/usr/sbin",
    "/usr/local/bin",
    "/lib",
    "/lib64",
    "/usr/lib",
    "/usr/lib64",
    "/usr/local/lib",
];

/// The ABI we target. WSL Ubuntu + CI `ubuntu-latest` are ABI 3. On a newer kernel the V3
/// rights still FullyEnforce; on an older one we refuse (fail-closed). Bump when a targeted
/// deployment needs a newer landlock right.
// ponytail: one fixed ABI floor, not a negotiated range — simplest thing that's correct on
// the only kernels we run (WSL/CI). Revisit per ADR-20260620 when a deployment needs more.
const TARGET_ABI: ABI = ABI::V3;

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
/// forked child (pre_exec) so it confines the child, never the agent. `BestEffort` + a
/// `FullyEnforced` check = fail-closed: on a kernel that can't fully honor the requested
/// rights we return Err and the spawn fails (we refuse rather than run loose).
fn apply_landlock(read_roots: &[PathBuf], write_roots: &[PathBuf]) -> Result<(), String> {
    let abi = TARGET_ABI;
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
        .add_rules(landlock::path_beneath_rules(
            &read_paths,
            AccessFs::from_read(abi),
        ))
        .map_err(|e| e.to_string())?
        .add_rules(landlock::path_beneath_rules(
            &write_paths,
            AccessFs::from_all(abi),
        ))
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
// ponytail: thread-probe avoids a raw fork; the throwaway thread restricts only itself and
// exits. Reports Enforced ONLY on FullyEnforced — matches run_confined's fail-closed gate.
fn probe() -> LandlockStatus {
    let result = std::thread::spawn(|| -> Option<i32> {
        let abi = TARGET_ABI;
        let status = Ruleset::default()
            .set_compatibility(CompatLevel::BestEffort)
            .handle_access(AccessFs::from_all(abi))
            .ok()?
            .create()
            .ok()?
            .restrict_self()
            .ok()?;
        match status.ruleset {
            RulesetStatus::FullyEnforced => Some(abi as i32),
            _ => None,
        }
    })
    .join();

    match result {
        Ok(Some(abi)) => LandlockStatus::Enforced { abi },
        Ok(None) => LandlockStatus::Unavailable {
            reason: "landlock present but ruleset not fully enforced".into(),
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
        // builds + enforces a landlock ruleset (syscalls + a small allocation), confining
        // the child. ponytail: build-in-child is simplest and the deny-corpus (single-
        // threaded) proves it; if the async agent path ever deadlocks on post-fork malloc,
        // move the ruleset build to the parent and only call restrict_self() here.
        unsafe {
            cmd.pre_exec(move || {
                apply_landlock(&read_roots, &write_roots).map_err(std::io::Error::other)?;
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
