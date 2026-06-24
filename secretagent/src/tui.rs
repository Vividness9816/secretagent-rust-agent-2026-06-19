//! `secretagent tui` — an interactive REPL (Phase 6f). A `reedline` line editor (multiline via a
//! backslash-continuation validator + in-session history + slash-command autocomplete) that runs
//! each input as an agentic task, reusing the 6a `assemble_agent` seam + `Agent::run_task` verbatim.
//! NOT a full-screen app — the §4.5 acceptance is a line editor (the ADR chose reedline over ratatui).
//!
//! The pure helpers (`slash_suggestions` / `is_input_complete` / `classify`) carry the logic and are
//! unit-tested; the `reedline` event loop is a thin TTY-only shell over them.

use anyhow::Result;
use sa_audit::Audit;
use sa_core::RunContext;
use sa_core_types::config;

/// Slash commands the REPL understands (also the autocomplete set).
const SLASH_COMMANDS: &[&str] = &["/help", "/exit", "/quit"];

/// What a submitted line means.
#[derive(Debug, PartialEq, Eq)]
pub enum TuiAction {
    Exit,
    Help,
    Empty,
    Task(String),
}

/// Classify a submitted (already newline-joined) line.
pub fn classify(input: &str) -> TuiAction {
    match input.trim() {
        "" => TuiAction::Empty,
        "/exit" | "/quit" => TuiAction::Exit,
        "/help" => TuiAction::Help,
        other => TuiAction::Task(other.to_string()),
    }
}

/// Autocomplete suggestions for a `/`-prefixed prefix (empty otherwise).
pub fn slash_suggestions(prefix: &str) -> Vec<String> {
    if !prefix.starts_with('/') {
        return Vec::new();
    }
    SLASH_COMMANDS
        .iter()
        .filter(|c| c.starts_with(prefix))
        .map(|c| c.to_string())
        .collect()
}

/// Multiline rule: a line ending in a backslash is a continuation (incomplete), so the editor keeps
/// reading. Everything else submits.
pub fn is_input_complete(line: &str) -> bool {
    !line.trim_end_matches([' ', '\t']).ends_with('\\')
}

const HELP: &str = "\
secretagent REPL — type a task and press Enter; the agent may call policy-gated tools.
  /help          show this help
  /exit, /quit   leave (Ctrl-D / Ctrl-C also exit)
  end a line with \\ to continue on the next line (multiline)";

#[cfg(feature = "tui")]
mod editor {
    use super::*;
    use reedline::{
        Completer, DefaultPrompt, DefaultPromptSegment, Reedline, Signal, Span, Suggestion,
        ValidationResult, Validator,
    };

    struct SlashCompleter;
    impl Completer for SlashCompleter {
        fn complete(&mut self, line: &str, pos: usize) -> Vec<Suggestion> {
            let prefix = &line[..pos.min(line.len())];
            super::slash_suggestions(prefix)
                .into_iter()
                .map(|value| Suggestion {
                    value,
                    description: None,
                    style: None,
                    extra: None,
                    span: Span::new(0, pos),
                    append_whitespace: true,
                })
                .collect()
        }
    }

    struct MultilineValidator;
    impl Validator for MultilineValidator {
        fn validate(&self, line: &str) -> ValidationResult {
            if super::is_input_complete(line) {
                ValidationResult::Complete
            } else {
                ValidationResult::Incomplete
            }
        }
    }

    /// Drive the REPL. Each task runs as the interactive operator (`auto_approve = false` — the TUI
    /// can't show an approval prompt yet, so side-effectful tools are denied, not auto-approved).
    pub async fn run(session: &str) -> Result<()> {
        let cfg = config::Config::load()?;
        let mut audit = Audit::open(&config::audit_path())?;
        let agent = crate::setup::build_agent(&cfg)?;
        let (registry, backend_label) = crate::setup::build_registry(&cfg, false).await?;
        crate::exec::audit_backend_armed(&mut audit, &backend_label)?;

        let mut line_editor = Reedline::create()
            .with_completer(Box::new(SlashCompleter))
            .with_validator(Box::new(MultilineValidator));
        let prompt = DefaultPrompt::new(
            DefaultPromptSegment::Basic("secretagent".into()),
            DefaultPromptSegment::Empty,
        );

        println!("secretagent REPL — /help for commands, /exit to leave.");
        loop {
            match line_editor.read_line(&prompt) {
                Ok(Signal::Success(buffer)) => match super::classify(&buffer) {
                    super::TuiAction::Exit => break,
                    super::TuiAction::Empty => continue,
                    super::TuiAction::Help => println!("{}", super::HELP),
                    super::TuiAction::Task(task) => {
                        match agent
                            .run_task(
                                session,
                                &task,
                                &registry,
                                &cfg.policy,
                                &mut audit,
                                &RunContext::operator(false),
                            )
                            .await
                        {
                            Ok(answer) => println!("{answer}"),
                            // A failed turn must not kill the REPL — report and keep going.
                            Err(e) => eprintln!("task failed: {e:#}"),
                        }
                    }
                },
                Ok(Signal::CtrlC) | Ok(Signal::CtrlD) => break,
                Err(e) => {
                    eprintln!("input error: {e}");
                    break;
                }
            }
        }
        Ok(())
    }
}

#[cfg(feature = "tui")]
pub use editor::run;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_maps_commands_empty_and_tasks() {
        assert_eq!(classify("/exit"), TuiAction::Exit);
        assert_eq!(classify("  /quit "), TuiAction::Exit);
        assert_eq!(classify("/help"), TuiAction::Help);
        assert_eq!(classify("   "), TuiAction::Empty);
        assert_eq!(
            classify("summarize my inbox"),
            TuiAction::Task("summarize my inbox".to_string())
        );
    }

    #[test]
    fn slash_suggestions_filters_by_prefix_and_ignores_non_slash() {
        assert_eq!(slash_suggestions("/e"), vec!["/exit".to_string()]);
        assert!(slash_suggestions("/").contains(&"/help".to_string()));
        assert!(slash_suggestions("hello").is_empty()); // not a slash command
        assert!(slash_suggestions("/zzz").is_empty()); // no match
    }

    #[test]
    fn is_input_complete_respects_backslash_continuation() {
        assert!(is_input_complete("a single line"));
        assert!(!is_input_complete("continue me \\"));
        assert!(!is_input_complete("trailing spaces after slash \\   "));
        assert!(is_input_complete("not a continuation \\x"));
    }
}
