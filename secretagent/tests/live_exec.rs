//! Live execute_code acceptance — IGNORED by default. Run on a Linux box with landlock:
//!   cargo test -p secretagent --test live_exec -- --ignored --nocapture
//! Requires a tools-capable local model (Ollama hermes3) configured in config.toml; the
//! full agent path is driven manually via `secretagent run "<task>" --yes`. This test
//! asserts the host capability precondition so a green "ignored" run is meaningful.
#![cfg(target_os = "linux")]

#[tokio::test]
#[ignore]
async fn execute_code_host_has_enforced_landlock() {
    assert!(
        matches!(
            sa_exec::landlock_status(),
            sa_exec::LandlockStatus::Enforced { .. }
        ),
        "this host has no enforced landlock; the live confined-exec path can't be exercised here"
    );
}
