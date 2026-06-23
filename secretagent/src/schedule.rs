//! `secretagent schedule` — arm/list/remove NL-scheduled jobs (Phase 4d). `add` asks the model
//! for a cron expression and GATES it through sa-core's deterministic validator before persisting
//! the FROZEN job (action + cron + allow-list — M4). The gateway fires due jobs and delivers to
//! the target connector.

use anyhow::{Context, Result};
use sa_core::schedule::{next_fire_unix, nl_to_cron};
use sa_core_types::config;
use sa_memory::Store;
use sa_providers::openai::OpenAiCompat;
use sa_vault::{age_file::AgeFileVault, Vault};
use secrecy::ExposeSecret;

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Arm a job. The model proposes a cron expression for `request`; the validator gates it; the
/// frozen job (NL spec + cron + the task text + the per-job tool grant) is persisted.
pub async fn add(request: &str, connector: &str, chat: &str, tools: &[String]) -> Result<()> {
    let cfg = config::Config::load()?;
    let vault = AgeFileVault::open_or_init(&config::identity_path(), &config::store_path())?;
    let api_key = match &cfg.provider.api_key_ref {
        Some(k) => vault.get(k)?.map(|s| s.expose_secret().to_string()),
        None => None,
    };
    let provider = OpenAiCompat {
        base_url: cfg.provider.base_url.clone(),
        model: cfg.provider.model.clone(),
        api_key,
    };
    let cron_expr = nl_to_cron(&provider, request)
        .await
        .context("the model did not propose a valid cron expression")?;
    let next = next_fire_unix(&cron_expr, now_unix())?;
    let allow_json = serde_json::to_string(tools)?;
    let store = Store::open(&config::db_path())?;
    let id = store.add_cron_job(
        request,
        &cron_expr,
        request,
        connector,
        chat,
        &allow_json,
        next,
    )?;
    println!("scheduled job {id}: `{cron_expr}` (UTC) -> {connector}; next fire at unix {next}");
    if tools.is_empty() {
        println!("  no tool grant (read-only run)");
    } else {
        println!("  frozen tool grant: {}", tools.join(", "));
    }
    Ok(())
}

pub fn list() -> Result<()> {
    let store = Store::open(&config::db_path())?;
    let jobs = store.list_cron_jobs()?;
    if jobs.is_empty() {
        println!("no scheduled jobs");
        return Ok(());
    }
    for j in jobs {
        let last = j
            .last_run
            .map(|t| t.to_string())
            .unwrap_or_else(|| "never".into());
        println!(
            "[{}] {} `{}` -> {} (next {}, last {}, {})",
            j.id,
            j.nl_spec,
            j.cron_expr,
            j.target_connector,
            j.next_run,
            last,
            if j.enabled { "enabled" } else { "disabled" }
        );
    }
    Ok(())
}

pub fn remove(id: i64) -> Result<()> {
    let store = Store::open(&config::db_path())?;
    if store.remove_cron_job(id)? == 0 {
        eprintln!("no such job: {id}");
        std::process::exit(2);
    }
    println!("removed job {id}");
    Ok(())
}
