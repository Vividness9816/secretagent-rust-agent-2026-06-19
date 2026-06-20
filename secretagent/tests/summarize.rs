use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn summarize_short_session_is_a_clean_noop() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.toml"),
        "[provider]\nbase_url = \"http://127.0.0.1:1/v1\"\nmodel = \"none\"\n",
    )
    .unwrap();
    // Empty/short session: summarize_session returns false BEFORE any provider call, so this
    // succeeds even with an unreachable provider (proves the no-op short-circuit).
    Command::cargo_bin("secretagent")
        .unwrap()
        .env("SECRETAGENT_DATA_DIR", dir.path())
        .env("SECRETAGENT_CONFIG_DIR", dir.path())
        .args(["summarize", "--session", "empty"])
        .assert()
        .success()
        .stdout(predicate::str::contains("nothing to summarize"));
}
