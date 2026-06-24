mod chat;
mod doctor;
mod exec;
mod gateway;
mod model;
mod ops;
mod pref;
mod run;
mod schedule;
mod service;
mod setup;
mod skill;
mod summarize;
#[cfg(feature = "tui")]
mod tui;
#[cfg(feature = "voice")]
mod voice;

use clap::{Parser, Subcommand};
use sa_core_types::config;
use sa_vault::{age_file::AgeFileVault, SecretString, Vault};
use secrecy::ExposeSecret;

#[derive(Parser)]
#[command(name = "secretagent", version)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Health/self-diagnostic. Exits 0 when healthy; never fails on a missing keyring.
    Doctor,
    /// Credential vault.
    Vault {
        #[command(subcommand)]
        op: VaultOp,
    },
    /// Chat with the configured model (streams the reply; remembers across runs).
    Chat {
        message: String,
        #[arg(long, default_value = "default")]
        session: String,
    },
    /// Run an agentic task: the model may call policy-gated, audited tools.
    Run {
        task: String,
        #[arg(long, default_value = "default")]
        session: String,
        /// Auto-approve side-effectful tools (write_file, execute_code) instead of denying.
        #[arg(long)]
        yes: bool,
        /// DANGER: run execute_code with NO sandbox when landlock is unavailable. Per-
        /// invocation, never persisted, loudly audited. The operator's own-box escape valve.
        #[arg(long)]
        allow_unsandboxed_exec: bool,
    },
    /// Stated operator preferences (the user model). Written only here, always Trusted.
    Pref {
        #[command(subcommand)]
        op: PrefOp,
    },
    /// Learned skills (the procedural memory). Activation is approval-gated.
    Skill {
        #[command(subcommand)]
        op: SkillOp,
    },
    /// Compress a session's older context into a rolling LLM summary.
    Summarize {
        #[arg(long, default_value = "default")]
        session: String,
    },
    /// Switch the provider model: rewrites `[provider] model` in config.toml (operator-only — a
    /// Remote/cron principal can't invoke CLI subcommands). Takes effect on the next run/chat.
    Model {
        /// The model id, e.g. `claude-opus-4-8`, `claude-haiku-4-5`, or `llama3.2`.
        name: String,
    },
    /// Interactive REPL: a reedline line editor (multiline + history + slash-autocomplete) that runs
    /// each input as an agentic task (the interactive operator; side-effects denied, not auto-approved).
    #[cfg(feature = "tui")]
    Tui {
        #[arg(long, default_value = "default")]
        session: String,
    },
    /// Schedule NL jobs the gateway fires (cron, delivered to a connector).
    Schedule {
        #[command(subcommand)]
        op: ScheduleOp,
    },
    /// Back up the data dir (DB via the SQLite Online Backup API; the encrypted vault + audit
    /// copied) into a directory. The backup contains your private identity key — protect it. (6g)
    Backup {
        /// Destination directory for the backup (created if absent).
        dest: std::path::PathBuf,
    },
    /// Restore a backup directory into the data dir (chmod 600 the identity; verify the audit
    /// chain). Overwrites the live data dir — stop the daemon first. (6g)
    Restore {
        /// The backup directory produced by `backup`.
        src: std::path::PathBuf,
    },
    /// Export a session's trajectory (messages + audit) to JSONL, secret-free: recognizable
    /// secrets are redacted and the export fails closed if any survive. (6g)
    Export {
        #[arg(long, default_value = "default")]
        session: String,
        /// Output path (default: <data-dir>/trajectory-<session>.jsonl).
        #[arg(long)]
        out: Option<std::path::PathBuf>,
    },
    /// Run the always-on gateway daemon (messaging connectors + scheduler). Installed as a
    /// service by `service install` (4b). Stops cleanly on Ctrl-C / SIGTERM.
    Gateway,
    /// Install / uninstall / check the OS service (systemd on Linux, SCM on Windows).
    Service {
        #[command(subcommand)]
        op: ServiceOp,
    },
    /// INTERNAL: the entry the installed service launches. On Windows it joins the SCM
    /// dispatcher; elsewhere it runs the gateway loop. Not for interactive use.
    #[command(hide = true)]
    ServiceRun,
    /// Voice round-trip: transcribe <input.wav> → run the task → synthesize the reply to a wav.
    /// The transcript is treated as UNTRUSTED (no --yes; side-effects only via [voice] allow_tools).
    #[cfg(feature = "voice")]
    Voice {
        /// Path to the input audio file (passed to the configured stt_cmd as its final argument).
        input: std::path::PathBuf,
    },
}

#[derive(Subcommand)]
enum ScheduleOp {
    /// Arm a job: `schedule add "<request>" --connector <name> --chat <id> [--tool write_file ...]`.
    Add {
        request: String,
        #[arg(long)]
        connector: String,
        #[arg(long)]
        chat: String,
        /// FROZEN per-job side-effect grant (repeatable). Default: none (read-only run).
        #[arg(long = "tool")]
        tools: Vec<String>,
    },
    /// List scheduled jobs.
    List,
    /// Remove a job by id.
    Remove { id: i64 },
}

#[derive(Subcommand)]
enum ServiceOp {
    /// Install + enable the service so it starts on boot (needs root/admin).
    Install,
    /// Stop + remove the service.
    Uninstall,
    /// Print the service's install/run state (never fails).
    Status,
}

#[derive(Subcommand)]
enum PrefOp {
    /// Remember a stated preference: `pref set <dimension> <value>`.
    Set { dimension: String, value: String },
    /// List stated preferences.
    List,
}

#[derive(Subcommand)]
enum SkillOp {
    /// List learned skills (name, status, runs, score).
    List,
    /// Activate a draft skill (operator approval → Trusted + active).
    Activate { name: String },
}

#[derive(Subcommand)]
enum VaultOp {
    /// Create the age identity + empty store if absent.
    Init,
    /// Store a secret under a key.
    Set { key: String, value: String },
    /// Print a secret by key (exits 2 if absent).
    Get { key: String },
}

fn open_vault() -> anyhow::Result<AgeFileVault> {
    AgeFileVault::open_or_init(&config::identity_path(), &config::store_path())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Doctor => doctor::run(),
        Cmd::Chat { message, session } => chat::run(&session, &message).await,
        Cmd::Run {
            task,
            session,
            yes,
            allow_unsandboxed_exec,
        } => run::run(&session, &task, yes, allow_unsandboxed_exec).await,
        Cmd::Pref { op } => match op {
            PrefOp::Set { dimension, value } => pref::set(&dimension, &value),
            PrefOp::List => pref::list(),
        },
        Cmd::Skill { op } => match op {
            SkillOp::List => skill::list(),
            SkillOp::Activate { name } => skill::activate(&name),
        },
        Cmd::Summarize { session } => summarize::run(&session).await,
        Cmd::Model { name } => model::run(&name),
        #[cfg(feature = "tui")]
        Cmd::Tui { session } => tui::run(&session).await,
        Cmd::Schedule { op } => match op {
            ScheduleOp::Add {
                request,
                connector,
                chat,
                tools,
            } => schedule::add(&request, &connector, &chat, &tools).await,
            ScheduleOp::List => schedule::list(),
            ScheduleOp::Remove { id } => schedule::remove(id),
        },
        Cmd::Backup { dest } => ops::backup(&dest),
        Cmd::Restore { src } => ops::restore(&src),
        Cmd::Export { session, out } => ops::export(&session, out),
        Cmd::Gateway => gateway::run_until(gateway::shutdown_signal()).await,
        Cmd::Service { op } => match op {
            ServiceOp::Install => service::install(),
            ServiceOp::Uninstall => service::uninstall(),
            ServiceOp::Status => {
                println!("{}", service::status());
                Ok(())
            }
        },
        Cmd::ServiceRun => {
            #[cfg(windows)]
            {
                service::windows::run_service_dispatch()
            }
            #[cfg(not(windows))]
            {
                gateway::run_until(gateway::shutdown_signal()).await
            }
        }
        #[cfg(feature = "voice")]
        Cmd::Voice { input } => voice::run(&input).await,
        Cmd::Vault { op } => match op {
            VaultOp::Init => {
                open_vault()?;
                println!("vault initialized at {:?}", config::store_path());
                Ok(())
            }
            VaultOp::Set { key, value } => {
                let mut v = open_vault()?;
                v.set(&key, SecretString::new(value))?;
                println!("set {key}"); // key name only — never the value
                Ok(())
            }
            VaultOp::Get { key } => {
                let v = open_vault()?;
                match v.get(&key)? {
                    Some(s) => {
                        println!("{}", s.expose_secret());
                        Ok(())
                    }
                    None => {
                        eprintln!("no such key: {key}");
                        std::process::exit(2);
                    }
                }
            }
        },
    }
}
