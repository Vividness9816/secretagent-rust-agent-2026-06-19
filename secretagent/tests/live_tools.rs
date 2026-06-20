use assert_cmd::Command;
use predicates::prelude::*;

// Phase-2a live tool-use acceptance. Run with:
//   cargo test -p secretagent --test live_tools -- --ignored
// Requires local Ollama serving a tools-capable model (e.g. hermes3:latest).
// Proves the agent calls a policy-gated tool and the call is audited.
#[test]
#[ignore]
fn agent_reads_an_allowed_file_via_a_tool_and_audits_the_call() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    std::fs::create_dir_all(&root).unwrap();
    let secret_note = "the launch code is bluebird-7";
    std::fs::write(root.join("note.txt"), secret_note).unwrap();

    // Config: a tools-capable model + a read root the file lives in.
    let cfg = format!(
        "[provider]\nbase_url = \"http://localhost:11434/v1\"\nmodel = \"hermes3:latest\"\n\n\
         [policy]\nread_roots = [{root:?}]\n",
        root = root
    );
    std::fs::write(dir.path().join("config.toml"), cfg).unwrap();

    let out = Command::cargo_bin("secretagent")
        .unwrap()
        .env("SECRETAGENT_DATA_DIR", dir.path())
        .env("SECRETAGENT_CONFIG_DIR", dir.path())
        .args([
            "run",
            &format!(
                "Use the read_file tool to read {} and tell me exactly what it says.",
                root.join("note.txt").display()
            ),
        ])
        .assert()
        .success();
    out.stdout(predicate::str::contains("bluebird-7"));

    // The tool call must be in the audit log (by name), and the secret content must not.
    let log = std::fs::read_to_string(dir.path().join("audit.jsonl")).unwrap();
    assert!(
        log.contains("read_file"),
        "tool call must be audited; log:\n{log}"
    );
    assert!(
        !log.contains("bluebird-7"),
        "tool OUTPUT must never reach the audit log"
    );
}
