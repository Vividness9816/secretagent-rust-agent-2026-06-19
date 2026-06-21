//! The always-on gateway daemon (Phase 4). This slice (4a) is the SHELL: it stands up the
//! `GatewayState` and runs a `tokio` loop that idles until a shutdown signal, then returns
//! cleanly. Messaging connectors (4c) and the scheduler tick (4d) plug into this loop later;
//! `service install` (4b) registers `secretagent gateway` to run on boot.

use anyhow::Result;
use std::collections::HashMap;
use std::future::Future;

/// Runtime status of the daemon's connectors. Empty in 4a (no connectors yet); the seam that
/// 4c's connectors and `doctor`/`status` read. Liveness is recorded here, not in a second
/// representation.
#[derive(Debug, Default)]
pub struct GatewayState {
    /// connector id -> last-known status line (e.g. "polling", "down: <reason>").
    pub connectors: HashMap<String, String>,
}

impl GatewayState {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Run the gateway until `shutdown` resolves, then return cleanly. The CLI passes a real signal
/// future (Ctrl-C / SIGTERM); tests pass `async {}` for an immediate clean exit.
pub async fn run_until(shutdown: impl Future<Output = ()>) -> Result<()> {
    let state = GatewayState::new();
    tracing::info!(
        "gateway: started ({} connectors configured)",
        state.connectors.len()
    );

    shutdown.await;
    tracing::info!("gateway: shutdown requested, stopping");
    Ok(())
}

/// The signal future the CLI uses: resolve on Ctrl-C, or (Unix) SIGTERM from systemd `stop`.
pub async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The gateway loop must build its state and return cleanly when the shutdown future
    // resolves — proving the daemon shell starts and stops without hanging.
    #[tokio::test]
    async fn gateway_runs_and_shuts_down_cleanly() {
        let res = run_until(async {}).await;
        assert!(res.is_ok(), "gateway must shut down cleanly: {res:?}");
    }
}
