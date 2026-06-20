use assert_cmd::Command;
use predicates::prelude::*;

fn cmd(dir: &std::path::Path) -> Command {
    let mut c = Command::cargo_bin("secretagent").unwrap();
    c.env("SECRETAGENT_DATA_DIR", dir)
        .env("SECRETAGENT_CONFIG_DIR", dir);
    c
}

// Skills are agent-authored (no CLI to create one directly), so this asserts the command
// surface: `skill list` is clean on an empty store, and activating a missing skill reports
// it with a non-zero exit. The full create->activate->reuse round trip is exercised by the
// sa-core acceptance test (novel_task_creates_a_skill_then_reuses_and_scores_it_next_session).
#[test]
fn skill_list_is_clean_and_activate_missing_fails() {
    let dir = tempfile::tempdir().unwrap();
    cmd(dir.path()).args(["skill", "list"]).assert().success();
    cmd(dir.path())
        .args(["skill", "activate", "no-such-skill"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no such skill"));
}
