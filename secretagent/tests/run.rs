use assert_cmd::Command;
use predicates::prelude::*;

/// Hermetic: point the provider at a dead port so the result is deterministic
/// regardless of whether a local Ollama is running. Asserts a clean failure path.
#[test]
fn run_fails_cleanly_when_provider_unreachable() {
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
        .args(["run", "do something"])
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("task")
                .or(predicate::str::contains("model"))
                .or(predicate::str::contains("connect")),
        );
}
