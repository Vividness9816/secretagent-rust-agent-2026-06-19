use assert_cmd::Command;
use predicates::prelude::*;

/// Hermetic: point the provider at a dead port so the result does not depend on
/// whether a local Ollama happens to be running. Asserts a clean (non-panic)
/// failure path. The real round-trip is the `#[ignore]` test in `live_ollama.rs`.
#[test]
fn chat_fails_cleanly_when_provider_unreachable() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.toml"),
        "[provider]\nbase_url = \"http://127.0.0.1:1/v1\"\nmodel = \"none\"\n",
    )
    .unwrap();

    Command::cargo_bin("secretagent")
        .unwrap()
        .env("SECRETAGENT_DATA_DIR", dir.path())
        .env("SECRETAGENT_CONFIG_DIR", dir.path())
        .args(["chat", "hello"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("provider").or(predicate::str::contains("connect")));
}
