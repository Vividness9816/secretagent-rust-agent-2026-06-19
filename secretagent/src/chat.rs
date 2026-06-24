use anyhow::Context;
use futures::StreamExt;
use sa_core_types::config;
use std::io::Write;

pub async fn run(session: &str, message: &str) -> anyhow::Result<()> {
    let cfg = config::Config::load()?;
    let agent = crate::setup::build_agent(&cfg)?;

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
