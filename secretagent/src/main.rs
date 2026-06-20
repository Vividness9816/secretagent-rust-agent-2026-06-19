mod chat;
mod doctor;

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
