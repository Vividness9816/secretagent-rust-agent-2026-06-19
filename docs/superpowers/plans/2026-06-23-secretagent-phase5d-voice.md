# SecretAgent Phase 5d — Voice Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:test-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** `secretagent voice <input.wav>` round-trips audio→transcript→`run_task`→answer→`output.wav`, proving acceptance (c) — the LAST Phase-5 slice.

**Architecture (ADR-20260623-secretagent-phase5d-voice):** A feature-gated `voice` bin module that **shells out** (`std::process::Command`, never `sh -c`) to operator-configured `[voice] stt_cmd`/`tts_cmd` argv templates. The transcript runs as an **untrusted, non-persisting `Remote`** principal (`RunContext::remote("voice", source, cfg.voice.allow_tools)`) — provenance `Untrusted`, `may_persist=false`, side-effects default-deny via the frozen `allow_tools`, depth-0, **no `--yes`**. Answer text → TTS via **stdin**; `output.wav` → a **fixed policy path**; doctor/audit report **argv[0] only**.

**Tech Stack:** Rust, `std::process::Command`, the existing `run_task` + `RunContext::remote` trust spine, `OpenAiCompat` provider. **Zero new deps. No audio C-lib. No in-binary HTTP client.**

## Global Constraints
- **No new dep, no new crate, no audio C-lib, no in-binary HTTP client** — shell-out only (the 4 invariants + the ADR shell-out ruling).
- **Trust:** voice = `RunContext::remote("voice", source, allow_tools)` → `Untrusted` + no-persist + no-auto-activate + default-deny side-effects + depth-0. **No `--yes` flag on `voice`.**
- **Security hardening:** STT/TTS spawned via argv (`Command::new(argv[0]).args(...)`), **NEVER `sh -c`**; the answer text reaches TTS via **stdin or a single non-interpolated argv element**, never shell-built; **cap the transcript** length; `output.wav` is a **fixed `data_dir()` path**, never transcript-influenced; doctor/audit log **argv[0]** (the binary name), never the full command.
- **Feature-gate** the `voice` module + `Cmd::Voice` + dispatch (`secretagent` package feature `voice`, default-enabled — mirrors discord/email/slack). `VoiceConfig` is unconditional (config parses in all builds).
- **TDD**; commit per task; footer `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>` + `Claude-Session: phase-5d`; append ` # self-audit-ok` to each `git commit`.
- **Gates:** fmt `--check` 0 / clippy `-D warnings` 0 / `cargo test --all` on **both** Windows + WSL; `cargo deny` unaffected (no dep). Live whisper/piper round-trip is **operator-gated** (the Telegram/Slack/SSH precedent).

## File Structure
- `crates/sa-core-types/src/config.rs` — `VoiceConfig { stt_cmd, tts_cmd, allow_tools }` + `pub voice` on `Config` (unconditional). + parse test.
- `secretagent/src/voice.rs` — NEW feature-gated module: pure helpers (`build_stt`, `build_tts`, `cap_transcript`, `voice_ctx`) + `spawn_capture` (I/O) + `run`. + unit tests.
- `secretagent/src/main.rs` — `Cmd::Voice { input }` + dispatch (both `#[cfg(feature="voice")]`).
- `secretagent/src/doctor.rs` — a `#[cfg(feature="voice")]` voice probe (argv[0] presence).
- `secretagent/Cargo.toml` — `[features] default = ["voice"]`, `voice = []`.

---

### Task 1: `VoiceConfig` (sa-core-types)

**Files:** Modify `crates/sa-core-types/src/config.rs`.

**Interfaces:** Produces `VoiceConfig { stt_cmd: Vec<String>, tts_cmd: Vec<String>, allow_tools: Vec<String> }` (all `#[serde(default)]`, empty) + `Config.voice`.

- [ ] **Step 1: Failing test** (append to config.rs `mod tests`):
```rust
    #[test]
    fn config_parses_voice_default_empty_and_explicit() {
        // Absent [voice] → all empty (voice unconfigured = unavailable).
        let c: Config = toml::from_str("").unwrap();
        assert!(c.voice.stt_cmd.is_empty() && c.voice.tts_cmd.is_empty());
        assert!(c.voice.allow_tools.is_empty());
        let toml = r#"
[voice]
stt_cmd = ["whisper", "--output-txt", "--stdout"]
tts_cmd = ["piper", "--model", "en.onnx", "--output_file"]
allow_tools = ["read_file"]
"#;
        let c2: Config = toml::from_str(toml).unwrap();
        assert_eq!(c2.voice.stt_cmd[0], "whisper");
        assert_eq!(c2.voice.tts_cmd[0], "piper");
        assert_eq!(c2.voice.allow_tools, vec!["read_file".to_string()]);
    }
```
- [ ] **Step 2: Run → FAIL** `cargo test -p sa-core-types config_parses_voice` (no field `voice`).
- [ ] **Step 3: Implement** — add to config.rs:
```rust
/// Operator-configured voice (Phase 5d). `stt_cmd`/`tts_cmd` are argv templates SHELLED OUT
/// (never `sh -c`): the STT gets the audio path as its final arg + prints the transcript to
/// stdout; the TTS gets the answer on STDIN + the output wav path as its final arg. `allow_tools`
/// is the **frozen default-deny** side-effect grant for the voice `Remote` run (empty = none).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct VoiceConfig {
    pub stt_cmd: Vec<String>,
    pub tts_cmd: Vec<String>,
    pub allow_tools: Vec<String>,
}
```
And `pub voice: VoiceConfig,` on `Config`.
- [ ] **Step 4: Run → PASS.**
- [ ] **Step 5: Commit** `feat(voice): VoiceConfig (stt_cmd/tts_cmd/allow_tools) (phase 5d)`.

---

### Task 2: `voice.rs` module + wiring (bin)

**Files:** Create `secretagent/src/voice.rs`; modify `main.rs`, `doctor.rs`, `Cargo.toml`.

**Interfaces:**
- `pub(crate) fn build_stt(stt_cmd: &[String], input: &Path) -> (String, Vec<String>)` — (program, args) with input appended; pure.
- `pub(crate) fn build_tts(tts_cmd: &[String], output: &Path) -> (String, Vec<String>)` — output appended; pure; answer is NOT in args.
- `pub(crate) fn cap_transcript(s: String, max: usize) -> String` — pure.
- `pub(crate) fn voice_ctx(source: &str, allow_tools: Vec<String>) -> RunContext` — `RunContext::remote("voice", source, allow_tools)`.
- `pub async fn run(input: &Path) -> Result<()>`.

- [ ] **Step 1: Failing tests** (in voice.rs `mod tests`):
```rust
    use super::*;
    use std::path::Path;

    #[test]
    fn build_stt_appends_audio_path_as_final_argv() {
        let (prog, args) = build_stt(&["whisper".into(), "--stdout".into()], Path::new("/a/in.wav"));
        assert_eq!(prog, "whisper");
        assert_eq!(args, vec!["--stdout".to_string(), "/a/in.wav".to_string()]);
    }

    #[test]
    fn build_tts_appends_output_path_and_never_the_answer_text() {
        // The answer text goes to STDIN, NOT argv — argv-injection guard (Skeptic).
        let (prog, args) = build_tts(&["piper".into(), "--out".into()], Path::new("/d/voice-out.wav"));
        assert_eq!(prog, "piper");
        assert_eq!(args, vec!["--out".to_string(), "/d/voice-out.wav".to_string()]);
        assert!(!args.iter().any(|a| a.contains("answer")), "answer must never be in argv");
    }

    #[test]
    fn cap_transcript_bounds_length() {
        assert_eq!(cap_transcript("abcdef".into(), 3), "abc");
        assert_eq!(cap_transcript("hi".into(), 10), "hi");
    }

    #[test]
    fn voice_ctx_is_untrusted_nonpersisting_and_default_deny() {
        // The security crux (ADR fork B): a voice run can never persist, auto-activate, or run a
        // side-effect that the operator did not pre-arm in [voice] allow_tools.
        let ctx = voice_ctx("in.wav", vec![]);
        assert!(!ctx.is_operator());
        assert!(!ctx.may_persist());                       // no skill minted from a transcript
        assert!(!ctx.may_auto_activate_skill());
        assert!(!ctx.may_run_side_effect("execute_code")); // default-deny
        assert_eq!(ctx.depth, 0);                          // no subagent fan-out
        assert!(matches!(ctx.provenance(), sa_core_types::types::Provenance::Untrusted { .. }));
        assert_eq!(ctx.audit_label(), "remote:voice:in.wav");
        // an operator-armed grant is honored (frozen, default-deny otherwise)
        let armed = voice_ctx("in.wav", vec!["read_file".into()]);
        assert!(armed.may_run_side_effect("read_file"));
        assert!(!armed.may_run_side_effect("write_file"));
    }

    #[cfg(unix)]
    #[test]
    fn spawn_capture_runs_argv_and_returns_stdout() {
        let out = spawn_capture("printf", &["hi".to_string()], None).unwrap();
        assert_eq!(out.trim(), "hi");
    }
    #[cfg(windows)]
    #[test]
    fn spawn_capture_runs_argv_and_returns_stdout() {
        let out = spawn_capture("cmd", &["/C".into(), "echo hi".into()], None).unwrap();
        assert_eq!(out.trim(), "hi");
    }
```
- [ ] **Step 2: Run → FAIL** (`voice` module absent). First wire the empty module + Cargo feature so it compiles to the failing-assert state.
- [ ] **Step 3: Implement** `voice.rs`:
```rust
use anyhow::{bail, Context, Result};
use sa_audit::{Audit, AuditEvent};
use sa_core::{Agent, RunContext};
use sa_core_types::config;
use sa_memory::Store;
use sa_providers::openai::OpenAiCompat;
use sa_tools::Registry;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// Max transcript chars fed to run_task — bounds a wedged/expensive run on a huge clip.
const MAX_TRANSCRIPT: usize = 8192;

pub(crate) fn build_stt(stt_cmd: &[String], input: &Path) -> (String, Vec<String>) {
    let mut args: Vec<String> = stt_cmd[1..].to_vec();
    args.push(input.to_string_lossy().into_owned());
    (stt_cmd[0].clone(), args)
}

pub(crate) fn build_tts(tts_cmd: &[String], output: &Path) -> (String, Vec<String>) {
    let mut args: Vec<String> = tts_cmd[1..].to_vec();
    args.push(output.to_string_lossy().into_owned());
    (tts_cmd[0].clone(), args)
}

pub(crate) fn cap_transcript(s: String, max: usize) -> String {
    s.chars().take(max).collect()
}

pub(crate) fn voice_ctx(source: &str, allow_tools: Vec<String>) -> RunContext {
    // ADR fork B: a transcript is untrusted machine-generated content → the connector trust model
    // (Untrusted, no-persist, no-auto-activate, default-deny side-effects, depth-0). No --yes.
    RunContext::remote("voice", source, allow_tools)
}

/// Spawn argv (NEVER `sh -c`), optionally feed `stdin`, return captured stdout. Errors include
/// only the program NAME (argv[0]), never the full command (no secret leak — ADR fork A).
pub(crate) fn spawn_capture(program: &str, args: &[String], stdin: Option<&str>) -> Result<String> {
    let mut cmd = Command::new(program);
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    if stdin.is_some() {
        cmd.stdin(Stdio::piped());
    }
    let mut child = cmd
        .spawn()
        .with_context(|| format!("voice: failed to spawn '{program}'"))?;
    if let Some(text) = stdin {
        child
            .stdin
            .take()
            .context("voice: no stdin pipe")?
            .write_all(text.as_bytes())?;
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!("voice: '{program}' exited with {}", out.status);
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub async fn run(input: &Path) -> Result<()> {
    let cfg = config::Config::load()?;
    if cfg.voice.stt_cmd.is_empty() || cfg.voice.tts_cmd.is_empty() {
        bail!("voice: configure [voice] stt_cmd and tts_cmd in config.toml (see docs)");
    }
    let mut audit = Audit::open(&config::audit_path())?;
    let source = input
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "audio".into());
    let ctx = voice_ctx(&source, cfg.voice.allow_tools.clone());

    // STT: shell out, audio path as the final argv, transcript on stdout. Audit the run by the STT
    // BINARY NAME only (argv[0]) under the voice principal — forensic visibility, no secret leak.
    let (stt_prog, stt_args) = build_stt(&cfg.voice.stt_cmd, input);
    audit.append_synced(AuditEvent {
        action: "voice.transcribe".into(),
        key_id: stt_prog.clone(),
        principal: Some(ctx.audit_label()),
    })?;
    let transcript = cap_transcript(spawn_capture(&stt_prog, &stt_args, None)?, MAX_TRANSCRIPT);
    let transcript = transcript.trim();
    if transcript.is_empty() {
        bail!("voice: empty transcript from '{stt_prog}'");
    }

    // The transcript drives run_task as an Untrusted Remote turn (no persist, default-deny tools).
    let provider = OpenAiCompat {
        base_url: cfg.provider.base_url.clone(),
        model: cfg.provider.model.clone(),
        api_key: provider_key(&cfg)?,
    };
    let agent = Agent::new(
        Store::open(&config::db_path())?,
        Box::new(provider),
        crate::pref::load_system_context(),
    );
    // Same registry as `run` MINUS the unsandboxed override (voice never gets it). execute_code is
    // registered but denied unless the operator pre-armed it in [voice] allow_tools.
    let mut registry = Registry::default_tools();
    let backend = crate::exec::backend_from_config(&cfg.exec)?;
    registry.register(Box::new(sa_tools::ExecuteCode::with_backend(backend, false)));
    for tool in sa_tools::mcp::load_mcp_tools(&cfg.mcp).await {
        registry.register(tool);
    }
    let answer = agent
        .run_task("voice", transcript, &registry, &cfg.policy, &mut audit, &ctx)
        .await
        .context("voice: agentic run failed — is the model endpoint reachable?")?;

    // TTS: answer text on STDIN, output wav at a FIXED policy path (never transcript-influenced).
    let output = config::data_dir().join("voice-out.wav");
    let (tts_prog, tts_args) = build_tts(&cfg.voice.tts_cmd, &output);
    spawn_capture(&tts_prog, &tts_args, Some(&answer))?;
    println!("{answer}");
    println!("[voice] reply audio: {}", output.display());
    Ok(())
}

fn provider_key(cfg: &config::Config) -> Result<Option<String>> {
    match &cfg.provider.api_key_ref {
        Some(key_id) => {
            use sa_vault::{age_file::AgeFileVault, Vault};
            use secrecy::ExposeSecret;
            let v = AgeFileVault::open_or_init(&config::identity_path(), &config::store_path())?;
            Ok(v.get(key_id)?.map(|s| s.expose_secret().to_string()))
        }
        None => Ok(None),
    }
}
```
- [ ] **Step 3b: Wire `Cargo.toml`** — add:
```toml
[features]
default = ["voice"]
voice = []
```
- [ ] **Step 3c: Wire `main.rs`** — `mod voice;` becomes `#[cfg(feature = "voice")] mod voice;`; add to `Cmd`:
```rust
    /// Voice round-trip: transcribe <input.wav> → run the task → synthesize the reply to a wav.
    #[cfg(feature = "voice")]
    Voice {
        /// Path to the input audio file (passed to the configured stt_cmd).
        input: std::path::PathBuf,
    },
```
and the dispatch arm:
```rust
        #[cfg(feature = "voice")]
        Cmd::Voice { input } => voice::run(&input).await,
```
- [ ] **Step 3d: Wire `doctor.rs`** — add (cfg-gated) after the exec-backend block:
```rust
    #[cfg(feature = "voice")]
    {
        let v = &cfg.voice;
        if v.stt_cmd.is_empty() || v.tts_cmd.is_empty() {
            println!("[info] voice: not configured ([voice] stt_cmd/tts_cmd)");
        } else {
            for (which, argv) in [("stt", &v.stt_cmd), ("tts", &v.tts_cmd)] {
                if backend_cli_present(&argv[0]) {
                    println!("[ok]   voice {which}: {} on PATH", argv[0]);
                } else {
                    println!("[warn] voice {which}: {} not found on PATH", argv[0]);
                }
            }
        }
    }
```
- [ ] **Step 4: Run → PASS** `cargo test -p secretagent voice` + `cargo test -p sa-core-types`.
- [ ] **Step 5: fmt + clippy** (`--all-targets --all-features -- -D warnings`).
- [ ] **Step 6: Commit** `feat(voice): shell-out STT/TTS round-trip as an Untrusted Remote run (phase 5d)`.

---

### Task 3: Docs + both-venue gate + self-audit + push

- [ ] **Step 1:** Update README (voice section + `[voice]` config example), PROGRESS.md (5d slice ledger → **Phase 5 COMPLETE**), ROADMAP.md (✅ 5d → Phase 5 done), HANDOFF (5d done).
- [ ] **Step 2: Both-venue gate** — Windows `cargo test --all`; WSL `cargo test --all` (CARGO_EXIT=0). Build venue: voice is pure std::process, no landlock, so both venues compile it.
- [ ] **Step 3: Self-audit** (ADR + handoff: the provenance call). One `self-audit` agent: confirm voice cannot (i) persist / auto-activate / run an unarmed side-effect / spawn a subagent; (ii) shell-inject via tts/stt (no `sh -c`, answer on stdin/argv only); (iii) leak a secret into doctor/audit (argv[0] only); (iv) write output.wav to a transcript-influenced path. Fix any real finding with a regression test.
- [ ] **Step 4:** `--no-default-features` build check (voice compiles out cleanly): `cargo build -p secretagent --no-default-features`.
- [ ] **Step 5: Commit docs + push** + watch CI green on all 5 jobs.

---

## Self-Review
- **Spec coverage:** acceptance (c) round-trip → Task 2 `run` (+ operator-gated live); no-auto-side-effect unit test → `voice_ctx_is_untrusted_nonpersisting_and_default_deny`; CI C-lib-free → no dep added (Task 1/2). ✓
- **ADR coverage:** A1 templates (Task 1/2), argv[0]-only audit/doctor (Task 2), B2 remote-trust + no `--yes` (`voice_ctx`), no `sh -c` + stdin answer + transcript cap + fixed out-path (Task 2 helpers/tests), feature-gate (Task 2 Cargo/main/doctor), voice audit event (Task 2 `run`). ✓
- **Placeholder scan:** none. **Type consistency:** `build_stt`/`build_tts`/`cap_transcript`/`voice_ctx`/`spawn_capture` used identically across Task 2. ✓
- **Deferred (ponytail):** `--play`/`play_cmd` (operator runs their own player); turnkey cloud arm (operator wraps a CLI); voice-driven skill learning via an explicit confirm seam (ADR "revisit when").
