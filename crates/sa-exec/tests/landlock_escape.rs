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
    std::fs::write(&secret_file, "TOPSECRET").unwrap(); // 0644: perms allow; only landlock denies

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
fn confined_shell_cannot_read_the_agents_environment() {
    // A secret in the AGENT's environment must NOT be reachable by confined code — neither
    // by direct inheritance ($VAR) nor via /proc/self/environ. Landlock is path-FS only, so
    // this is enforced by run_confined's env_clear(). Without that, `echo $SECRET` would
    // exfiltrate the operator's secrets straight past the file boundary.
    std::env::set_var("SA_ENV_LEAK_CANARY", "TOPSECRET_ENV_VALUE");
    let allowed = tempfile::tempdir().unwrap();
    let p = policy(vec![allowed.path().to_path_buf()], vec![]);
    let sb = LandlockSandbox::new();

    let out = sb
        .run_confined(
            "echo VAR=$SA_ENV_LEAK_CANARY; cat /proc/self/environ 2>/dev/null | tr '\\0' '\\n'",
            &p,
        )
        .unwrap();
    std::env::remove_var("SA_ENV_LEAK_CANARY");
    assert!(
        !out.contains("TOPSECRET_ENV_VALUE"),
        "ENV LEAK: confined code read the agent's environment:\n{out}"
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
    assert!(
        w.contains("EXIT=0"),
        "allowed write into write_root failed: {w}"
    );
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
