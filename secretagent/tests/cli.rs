use assert_cmd::Command;
use predicates::prelude::*;

fn cmd(data_dir: &std::path::Path) -> Command {
    let mut c = Command::cargo_bin("secretagent").unwrap();
    c.env("SECRETAGENT_DATA_DIR", data_dir);
    c
}

#[test]
fn doctor_exits_zero_headless() {
    let dir = tempfile::tempdir().unwrap();
    cmd(dir.path()).args(["vault", "init"]).assert().success();
    // No TTY, no D-Bus, no keyring in a CI/test runner — doctor must still be green.
    cmd(dir.path()).arg("doctor").assert().success();
}

#[test]
fn vault_round_trips_via_cli() {
    let dir = tempfile::tempdir().unwrap();
    cmd(dir.path()).args(["vault", "init"]).assert().success();
    cmd(dir.path())
        .args(["vault", "set", "API_KEY", "s3cr3t-sentinel"])
        .assert()
        .success();
    cmd(dir.path())
        .args(["vault", "get", "API_KEY"])
        .assert()
        .success()
        .stdout(predicate::str::contains("s3cr3t-sentinel"));
}

#[test]
fn service_status_is_green_and_prints_a_state() {
    let dir = tempfile::tempdir().unwrap();
    // `service status` must never fail (like doctor) — it reports an install/run state.
    cmd(dir.path())
        .args(["service", "status"])
        .assert()
        .success()
        .stdout(predicate::str::is_empty().not());
}

#[test]
fn backup_then_restore_round_trips_db_and_vault() {
    use sa_memory::Store;
    let a = tempfile::tempdir().unwrap(); // source data dir
    cmd(a.path()).args(["vault", "init"]).assert().success();
    cmd(a.path())
        .args(["vault", "set", "API_KEY", "s3cr3t-sentinel"])
        .assert()
        .success();
    // Seed a message directly into the live DB (scoped so the connection drops before backup).
    {
        let s = Store::open(&a.path().join("memory.db")).unwrap();
        s.add_message("default", "user", "round-trip-me", "{}")
            .unwrap();
    }
    let bk = tempfile::tempdir().unwrap(); // backup destination
    cmd(a.path())
        .args(["backup", bk.path().to_str().unwrap()])
        .assert()
        .success();

    // Restore into a FRESH data dir — proves the backup is self-contained.
    let b = tempfile::tempdir().unwrap();
    cmd(b.path())
        .args(["restore", bk.path().to_str().unwrap()])
        .assert()
        .success();

    // The vault secret survived (identity.age + store.age round-tripped, still encrypted).
    cmd(b.path())
        .args(["vault", "get", "API_KEY"])
        .assert()
        .success()
        .stdout(predicate::str::contains("s3cr3t-sentinel"));
    // The DB message survived (the Online Backup API snapshot round-tripped).
    let s = Store::open(&b.path().join("memory.db")).unwrap();
    assert!(
        s.recent("default", 10)
            .unwrap()
            .iter()
            .any(|m| m.content == "round-trip-me"),
        "the seeded message must survive backup→restore"
    );
}

#[test]
fn export_is_secret_free() {
    use sa_memory::Store;
    let a = tempfile::tempdir().unwrap();
    {
        let s = Store::open(&a.path().join("memory.db")).unwrap();
        s.add_message("default", "user", "my key is sk-abcdef12345678", "{}")
            .unwrap();
        s.add_message(
            "default",
            "assistant",
            "here is a clean answer about cats",
            "{}",
        )
        .unwrap();
    }
    let out = a.path().join("traj.jsonl");
    cmd(a.path())
        .args([
            "export",
            "--session",
            "default",
            "--out",
            out.to_str().unwrap(),
        ])
        .assert()
        .success();
    let body = std::fs::read_to_string(&out).unwrap();
    assert!(
        !body.contains("sk-abcdef12345678"),
        "the secret must be redacted out of the export"
    );
    assert!(
        body.contains("[redacted]"),
        "the redaction marker must be present"
    );
    assert!(
        body.contains("clean answer about cats"),
        "clean content must be preserved"
    );
}

#[test]
fn restore_removes_stale_wal_sidecars() {
    // 6g review HIGH: a stale memory.db-wal/-shm in the live data dir would make SQLite replay old
    // frames onto the freshly-restored DB. restore must delete them.
    use sa_memory::Store;
    let a = tempfile::tempdir().unwrap();
    {
        let s = Store::open(&a.path().join("memory.db")).unwrap();
        s.add_message("default", "user", "restored-row", "{}")
            .unwrap();
    }
    let bk = tempfile::tempdir().unwrap();
    cmd(a.path())
        .args(["backup", bk.path().to_str().unwrap()])
        .assert()
        .success();

    // Restore into a data dir that already holds stale WAL sidecars (the live-daemon-crash case).
    let b = tempfile::tempdir().unwrap();
    std::fs::write(b.path().join("memory.db-wal"), b"stale-wal").unwrap();
    std::fs::write(b.path().join("memory.db-shm"), b"stale-shm").unwrap();
    cmd(b.path())
        .args(["restore", bk.path().to_str().unwrap()])
        .assert()
        .success();
    assert!(
        !b.path().join("memory.db-wal").exists() && !b.path().join("memory.db-shm").exists(),
        "stale WAL sidecars must be removed on restore"
    );
    let s = Store::open(&b.path().join("memory.db")).unwrap();
    assert!(s
        .recent("default", 10)
        .unwrap()
        .iter()
        .any(|m| m.content == "restored-row"));
}

#[test]
fn backup_refuses_to_target_the_data_dir() {
    // 6g review LOW (severe blast radius): backup <data_dir> would self-copy and truncate the vault.
    let a = tempfile::tempdir().unwrap();
    cmd(a.path()).args(["vault", "init"]).assert().success();
    cmd(a.path())
        .args(["backup", a.path().to_str().unwrap()])
        .assert()
        .failure();
    // The identity must NOT have been truncated by an aborted self-copy.
    assert!(
        std::fs::metadata(a.path().join("identity.age"))
            .unwrap()
            .len()
            > 0,
        "identity.age must survive a refused self-target backup"
    );
}

#[cfg(unix)]
#[test]
fn backed_up_artifacts_are_locked_down() {
    use std::os::unix::fs::PermissionsExt;
    let a = tempfile::tempdir().unwrap();
    cmd(a.path()).args(["vault", "init"]).assert().success();
    cmd(a.path())
        .args(["vault", "set", "K", "v"])
        .assert()
        .success();
    let bk = tempfile::tempdir().unwrap();
    let dest = bk.path().join("out");
    cmd(a.path())
        .args(["backup", dest.to_str().unwrap()])
        .assert()
        .success();
    let mode = |p: std::path::PathBuf| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        mode(dest.join("identity.age")),
        0o600,
        "identity must be 0600"
    );
    assert_eq!(mode(dest.join("store.age")), 0o600, "vault must be 0600");
    assert_eq!(mode(dest.clone()), 0o700, "backup dir must be 0700");
}
