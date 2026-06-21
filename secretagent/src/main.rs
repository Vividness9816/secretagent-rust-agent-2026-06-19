mod chat;
mod doctor;
mod gateway;
mod pref;
mod run;
mod service;
mod skill;
mod summarize;

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
