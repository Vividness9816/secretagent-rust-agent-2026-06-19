use assert_cmd::Command;
use predicates::prelude::*;

fn cmd(dir: &std::path::Path) -> Command {
    let mut c = Command::cargo_bin("secretagent").unwrap();
    c.env("SECRETAGENT_DATA_DIR", dir)
        .env("SECRETAGENT_CONFIG_DIR", dir);
    c
}

#[test]
fn pref_set_then_list_persists_across_processes() {
    let dir = tempfile::tempdir().unwrap();
    cmd(dir.path())
        .args(["pref", "set", "tone", "concise"])
        .assert()
        .success();
    // A SEPARATE process (cold open of the same DB) must see it — cross-session proof.
    cmd(dir.path())
        .args(["pref", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("tone: concise"));
}
