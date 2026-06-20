use anyhow::Context;
use sa_audit::Audit;
use sa_core::Agent;
use sa_core_types::config;
use sa_memory::Store;
use sa_providers::openai::OpenAiCompat;
use sa_tools::Registry;

pub async fn run(
    session: &str,
    task: &str,
    auto_approve: bool,
    allow_unsandboxed_exec: bool,
) -> anyhow::Result<()> {
    let cfg = config::Config::load()?;
    let store = Store::open(&config::db_path())?;
    let mut audit = Audit::open(&config::audit_path())?;

    // The screaming override is per-invocation + never persisted. Announce it LOUDLY and
    // record it (fsync) before anything runs, so the grant is the first durable event.
    if allow_unsandboxed_exec {
        audit.append_synced(sa_audit::AuditEvent {
            action: "exec.override.UNSANDBOXED".into(),
            key_id: "run".into(),
        })?;
        eprintln!(
            "!!! --allow-unsandboxed-exec ENABLED: execute_code may run with NO sandbox this run !!!"
        );
    }

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

    // execute_code is always registered for `run`; it refuses at dispatch unless landlock
    // is enforced (or the per-invocation override is on). The 3 safe tools come default.
    let mut registry = Registry::default_tools();
    registry.register(Box::new(sa_tools::ExecuteCode::new(allow_unsandboxed_exec)));

    let answer = agent
        .run_task(
            session,
            task,
            &registry,
            &cfg.policy,
            &mut audit,
            auto_approve,
        )
        .await
        .context("agentic task failed — is the model endpoint reachable?")?;
    println!("{answer}");
    Ok(())
}
