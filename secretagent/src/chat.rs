use anyhow::Context;
use futures::StreamExt;
use sa_core::Agent;
use sa_core_types::config;
use sa_memory::Store;
use sa_providers::openai::OpenAiCompat;
use std::io::Write;

pub async fn run(session: &str, message: &str) -> anyhow::Result<()> {
    let cfg = config::Config::load()?;
    let store = Store::open(&config::db_path())?;

    // Resolve an API key from the vault ONLY if the provider needs one (keyless = Ollama).
    // The secret is read at call time and never written to messages/config/logs.
    let api_key = match &cfg.provider.api_key_ref {
        Some(key_id) => {
            use sa_vault::{age_file::AgeFileVault, Vault};
            use secrecy::ExposeSecret;
            let v = AgeFileVault::open_or_init(&config::identity_path(), &config::store_path())?;
            v.get(key_id)?.map(|s| s.expose_secret().to_string())
        }
        None => None,
    };

    let provider = OpenAiCompat {
        base_url: cfg.provider.base_url.clone(),
        model: cfg.provider.model.clone(),
        api_key,
    };
    let agent = Agent::new(
        store,
        Box::new(provider),
        crate::pref::load_system_context(),
    );

    let mut stream = agent
        .turn(session, message)
        .await
        .context("provider request failed — is the model endpoint reachable?")?;

    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    while let Some(chunk) = stream.next().await {
        write!(lock, "{}", chunk?.0)?;
        lock.flush()?;
    }
    writeln!(lock)?;
    Ok(())
}
