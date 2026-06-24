//! `op_tool` — an operator-frozen external-command adapter (the 5d-voice argv template generalized
//! for vision / image-gen / TTS / a browser CLI). The program + every flag + any URL/host are FROZEN
//! in `cmd`; the model supplies only a final `input` data arg, appended last. Spawned via argv,
//! NEVER `sh -c`, so a flag-looking input stays data. Errors name argv[0] only (no secret leak — the
//! 5d ruling). A NARROW adapter, not a generic curl/bash escape hatch.

use crate::Tool;
use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use sa_core_types::policy::Policy;
use serde_json::{json, Value};
use std::process::Command;

pub struct OpTool {
    name: String,
    cmd: Vec<String>,
    description: String,
}

impl OpTool {
    /// Build from operator config. Errors on an empty name or empty `cmd` (a program is required).
    pub fn new(
        name: impl Into<String>,
        cmd: Vec<String>,
        description: Option<String>,
    ) -> Result<Self> {
        let name = name.into();
        if name.is_empty() {
            bail!("op_tool: name is required");
        }
        if cmd.is_empty() {
            bail!("op_tool '{name}': cmd must contain at least the program (argv[0])");
        }
        let description = description.unwrap_or_else(|| {
            format!(
                "Operator-configured external tool '{name}'. You provide only the input argument."
            )
        });
        Ok(Self {
            name,
            cmd,
            description,
        })
    }

    /// (program, args) with the model's `input` ALWAYS the last, separate argv element — it can never
    /// become the program or a flag (those are the frozen `cmd[..]`).
    fn build_argv<'a>(&'a self, input: &'a str) -> (&'a str, Vec<&'a str>) {
        let mut argv: Vec<&str> = self.cmd[1..].iter().map(String::as_str).collect();
        argv.push(input);
        (&self.cmd[0], argv)
    }
}

#[async_trait]
impl Tool for OpTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"input":{"type":"string"}},"required":["input"]})
    }
    // op_tool does not consult `policy`: the command (incl. any host/URL) is operator-frozen, so
    // there is no model-supplied target to allow-list. The operator vouches for the command.
    // ponytail: blocking spawn on the async path — fine for a single CLI task (the run_unconfined
    // precedent); wrap in spawn_blocking only if concurrent op_tool calls ever contend.
    async fn run(&self, args: Value, _policy: &Policy) -> Result<String> {
        let input = args
            .get("input")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("op_tool '{}': missing 'input'", self.name))?;
        let (program, argv) = self.build_argv(input);
        let out = Command::new(program).args(&argv).output().map_err(|e| {
            // argv[0] only — never the flags/input (no secret leak).
            anyhow!("op_tool '{}': failed to spawn '{program}': {e}", self.name)
        })?;
        let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
        let err = String::from_utf8_lossy(&out.stderr);
        if !err.is_empty() {
            s.push_str(&err);
        }
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_is_input_only_and_hides_the_frozen_command() {
        let t = OpTool::new("imagegen", vec!["gen".into(), "--out".into()], None).unwrap();
        let s = t.parameters().to_string();
        assert!(s.contains("input"));
        assert!(!s.contains("cmd") && !s.contains("gen") && !s.contains("program"));
    }

    #[test]
    fn new_rejects_empty_name_or_cmd() {
        assert!(OpTool::new("", vec!["x".into()], None).is_err());
        assert!(OpTool::new("t", vec![], None).is_err());
    }

    #[test]
    fn input_is_appended_as_the_last_separate_argv_element() {
        let t = OpTool::new(
            "gen",
            vec!["prog".into(), "--out".into(), "/o.png".into()],
            None,
        )
        .unwrap();
        // A flag-looking input is just the final data arg — never the program or a frozen flag.
        let (prog, argv) = t.build_argv("--evil");
        assert_eq!(prog, "prog");
        assert_eq!(argv, vec!["--out", "/o.png", "--evil"]);
    }

    #[tokio::test]
    async fn spawns_the_frozen_argv_and_returns_stdout() {
        #[cfg(unix)]
        let t = OpTool::new("echo", vec!["printf".into(), "%s".into()], None).unwrap();
        #[cfg(windows)]
        let t = OpTool::new("echo", vec!["cmd".into(), "/C".into(), "echo".into()], None).unwrap();
        let out = t
            .run(json!({"input":"HELLO_OPTOOL"}), &Policy::default())
            .await
            .unwrap();
        assert!(out.contains("HELLO_OPTOOL"), "got {out:?}");
    }
}
