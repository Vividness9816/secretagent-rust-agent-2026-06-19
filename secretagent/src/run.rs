use anyhow::Context;
use sa_audit::Audit;
use sa_core::Agent;
use sa_core_types::config;
use sa_memory::Store;
use sa_providers::openai::OpenAiCompat;
use sa_tools::Registry;

pub async fn run(session: &str, task: &str) -> anyhow::Result<()> {
    let cfg = config::Config::load()?;
    let store = Store::open(&config::db_path())?;
    let mut audit = Audit::open(&config::audit_path())?;

    // Provider key from the vault at call time only if the provider needs one.
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
    let agent = Agent::new(store, Box::new(provider));
    let registry = Registry::default_tools();

    let answer = agent
        .run_task(session, task, &registry, &cfg.policy, &mut audit)
        .await
        .context("agentic task failed — is the model endpoint reachable?")?;
    println!("{answer}");
    Ok(())
}
