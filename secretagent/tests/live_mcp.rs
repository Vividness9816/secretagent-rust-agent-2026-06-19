//! Live MCP acceptance — IGNORED by default. Point config.toml at a real MCP server, then:
//!   cargo test -p secretagent --test live_mcp -- --ignored --nocapture
//! Asserts that every tool loaded from the configured servers is namespaced.
#[tokio::test]
#[ignore]
async fn configured_mcp_servers_load_namespaced_tools() {
    let cfg = sa_core_types::config::Config::load().unwrap();
    let tools = sa_tools::mcp::load_mcp_tools(&cfg.mcp).await;
    assert!(
        tools.iter().all(|t| t.name().contains("::")),
        "every loaded MCP tool must be namespaced (server::tool)"
    );
}
