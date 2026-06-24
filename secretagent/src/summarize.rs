use anyhow::Context;
use sa_core::{Agent, SystemContext};
use sa_core_types::config;
use sa_memory::Store;

pub async fn run(session: &str) -> anyhow::Result<()> {
    let cfg = config::Config::load()?;
    let store = Store::open(&config::db_path())?;
    // Route through the single provider-selection seam (Phase 6e) — picks openai|anthropic and
    // resolves the key from the vault, exactly like run/chat/gateway. No SystemContext preamble:
    // summarization has its own prompt.
    let agent = Agent::new(
        store,
        crate::setup::build_provider(&cfg)?,
        SystemContext::default(),
    );
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
