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
