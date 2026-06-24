//! Phase 5d — voice round-trip. Shells out (NEVER `sh -c`) to operator-configured `[voice]`
//! STT/TTS argv templates; the transcript runs as an Untrusted, non-persisting `Remote` principal
//! (ADR-20260623-secretagent-phase5d-voice). No audio C-lib, no in-binary HTTP client, no `--yes`.
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

/// (program, args) for the STT spawn: the audio path is the FINAL argv element (never shell).
pub(crate) fn build_stt(stt_cmd: &[String], input: &Path) -> (String, Vec<String>) {
    let mut args: Vec<String> = stt_cmd[1..].to_vec();
    args.push(input.to_string_lossy().into_owned());
    (stt_cmd[0].clone(), args)
}

/// (program, args) for the TTS spawn: the output wav path is the FINAL argv element. The answer
/// text is NOT here — it goes to stdin (argv-injection guard).
pub(crate) fn build_tts(tts_cmd: &[String], output: &Path) -> (String, Vec<String>) {
    let mut args: Vec<String> = tts_cmd[1..].to_vec();
    args.push(output.to_string_lossy().into_owned());
    (tts_cmd[0].clone(), args)
}

pub(crate) fn cap_transcript(s: String, max: usize) -> String {
    s.chars().take(max).collect()
}

/// The voice run's trust context: Untrusted + no-persist + default-deny side-effects + depth-0.
/// ADR fork B: a transcript is untrusted machine-generated content → the connector trust model
/// verbatim (no `--yes` path exists on `Remote`).
pub(crate) fn voice_ctx(source: &str, allow_tools: Vec<String>) -> RunContext {
    RunContext::remote("voice", source, allow_tools)
}

/// Spawn argv (NEVER `sh -c`), optionally feed `stdin`, return captured stdout. Errors carry only
/// the program NAME (argv[0]), never the full command (no secret leak — ADR fork A).
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
        bail!("voice: configure [voice] stt_cmd and tts_cmd in config.toml (see README)");
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
    // Same registry as `run` MINUS the unsandboxed override (voice NEVER gets it). execute_code is
    // registered but denied unless the operator pre-armed it in [voice] allow_tools.
    let mut registry = Registry::default_tools();
    let backend = crate::exec::backend_from_config(&cfg.exec)?;
    registry.register(Box::new(sa_tools::ExecuteCode::with_backend(
        backend, false,
    )));
    for tool in sa_tools::mcp::load_mcp_tools(&cfg.mcp).await {
        registry.register(tool);
    }
    let answer = agent
        .run_task(
            "voice",
            transcript,
            &registry,
            &cfg.policy,
            &mut audit,
            &ctx,
        )
        .await
        .context("voice: agentic run failed — is the model endpoint reachable?")?;

    // Print the text answer FIRST so a broken/missing TTS never swallows the reply (self-audit).
    println!("{answer}");
    // TTS: answer text on STDIN, output wav at a FIXED policy path (never transcript-influenced).
    let output = config::data_dir().join("voice-out.wav");
    let (tts_prog, tts_args) = build_tts(&cfg.voice.tts_cmd, &output);
    spawn_capture(&tts_prog, &tts_args, Some(&answer))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn build_stt_appends_audio_path_as_final_argv() {
        let (prog, args) = build_stt(
            &["whisper".into(), "--stdout".into()],
            Path::new("/a/in.wav"),
        );
        assert_eq!(prog, "whisper");
        assert_eq!(args, vec!["--stdout".to_string(), "/a/in.wav".to_string()]);
    }

    #[test]
    fn build_tts_appends_output_path_and_never_the_answer_text() {
        // The answer text goes to STDIN, NOT argv — argv-injection guard (Skeptic).
        let (prog, args) = build_tts(
            &["piper".into(), "--out".into()],
            Path::new("/d/voice-out.wav"),
        );
        assert_eq!(prog, "piper");
        assert_eq!(
            args,
            vec!["--out".to_string(), "/d/voice-out.wav".to_string()]
        );
        assert!(
            !args.iter().any(|a| a.contains("answer")),
            "answer must never be in argv"
        );
    }

    #[test]
    fn cap_transcript_bounds_length() {
        assert_eq!(cap_transcript("abcdef".into(), 3), "abc");
        assert_eq!(cap_transcript("hi".into(), 10), "hi");
    }

    #[test]
    fn voice_ctx_is_untrusted_nonpersisting_and_default_deny() {
        // The security crux (ADR fork B): a voice run can never persist, auto-activate, or run a
        // side-effect the operator did not pre-arm in [voice] allow_tools.
        let ctx = voice_ctx("in.wav", vec![]);
        assert!(!ctx.is_operator());
        assert!(!ctx.may_persist()); // no skill minted from a transcript
        assert!(!ctx.may_auto_activate_skill());
        assert!(!ctx.may_run_side_effect("execute_code")); // default-deny
        assert_eq!(ctx.depth, 0); // no subagent fan-out
        assert!(matches!(
            ctx.provenance(),
            sa_core_types::types::Provenance::Untrusted { .. }
        ));
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
}
