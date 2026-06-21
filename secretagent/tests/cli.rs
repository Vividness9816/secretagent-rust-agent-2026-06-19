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
