use assert_cmd::Command;
use predicates::prelude::*;

// The Phase 1 live acceptance. Run with:
//   cargo test -p secretagent --test live_ollama -- --ignored
// Requires a local Ollama serving the configured model (default llama3.2).
#[test]
#[ignore]
fn fact_stated_in_session_one_is_recalled_in_session_two_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let env = |c: &mut Command| {
        c.env("SECRETAGENT_DATA_DIR", dir.path());
    };

    // Session 1: state the fact.
    let mut c1 = Command::cargo_bin("secretagent").unwrap();
    env(&mut c1);
    c1.args([
        "chat",
        "--session",
        "s1",
        "Remember: my cat is named Mochi.",
    ])
    .assert()
    .success();

    // New process = daemon restart. FTS5 context should carry the fact forward.
    let mut c2 = Command::cargo_bin("secretagent").unwrap();
    env(&mut c2);
    c2.args(["chat", "--session", "s1", "What is my cat's name?"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Mochi"));
}
