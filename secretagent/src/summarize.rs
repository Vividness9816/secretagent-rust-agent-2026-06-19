use anyhow::Context;
use sa_core::{Agent, SystemContext};
use sa_core_types::config;
use sa_memory::Store;
use sa_providers::openai::OpenAiCompat;

pub async fn run(session: &str) -> anyhow::Result<()> {
    let cfg = config::Config::load()?;
    let store = Store::open(&config::db_path())?;
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
    let agent = Agent::new(store, Box::new(provider), SystemContext::default());
    if agent
        .summarize_session(session)
        .await
        .context("summarization failed — is the model endpoint reachable?")?
    {
        println!("summarized older context for session {session}");
    } else {
        println!("nothing to summarize for session {session}");
    }
    Ok(())
}
