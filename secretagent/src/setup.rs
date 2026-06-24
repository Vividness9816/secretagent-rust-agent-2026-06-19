//! Phase 6a — the single agent+registry assembly seam. Collapses the construction that was
//! duplicated across `run`/`chat`/`gateway`/`voice` (two sites literally commented "mirrors run").
//! Behavior-preserving by design: this seam builds ONLY the provider/agent/registry. Each caller
//! keeps its own `RunContext` (operator vs remote vs voice), its own audit decisions
//! (`audit_backend_armed` is the CALLER's call — run/gateway audit the backend, voice deliberately
//! does not), and its own unsandboxed-override policy (the explicit `allow_unsandboxed` param —
//! voice/gateway always pass `false`; only the local CLI `run` threads the `--yes`-adjacent flag).
use anyhow::Result;
use sa_core::Agent;
use sa_core_types::config::{self, Config};
use sa_memory::Store;
use sa_providers::{anthropic::Anthropic, openai::OpenAiCompat, Provider};
use sa_tools::Registry;

/// The single provider-SELECTION seam (Phase 6e): pick `openai` (OpenAI-compatible) or `anthropic`
/// (native Messages API) by `provider.kind`, resolving the API key from the vault ONLY if a ref is
/// set (keyless = Ollama). The secret is read here and never written to config/messages/logs
/// (invariant #4). The model is `model_for("execute")` (the agent's role) so a per-role override
/// lands through this seam.
pub(crate) fn build_provider(cfg: &Config) -> Result<Box<dyn Provider>> {
    let api_key = resolve_secret(cfg.provider.api_key_ref.as_ref())?;
    let model = cfg.provider.model_for("execute");
    let provider: Box<dyn Provider> = match cfg.provider.kind.as_str() {
        "anthropic" => Box::new(Anthropic::new(model, api_key)),
        "openai" | "" => Box::new(OpenAiCompat {
            base_url: cfg.provider.base_url.clone(),
            model,
            api_key,
        }),
        other => anyhow::bail!("unknown provider kind '{other}' (want openai|anthropic)"),
    };
    Ok(provider)
}

/// Read a vault secret by key-id, or `None` when there's no ref (keyless backends like Ollama). The
/// vault is opened ONLY when a ref is present. The secret goes straight into provider/tool
/// construction — never written to config/messages/logs (invariant #4). Shared by the provider and
/// the `web_search` credential (the `*_ref` convention — no "gateway" abstraction).
fn resolve_secret(key_ref: Option<&String>) -> Result<Option<String>> {
    match key_ref {
        Some(key_id) => {
            use sa_vault::{age_file::AgeFileVault, Vault};
            use secrecy::ExposeSecret;
            let v = AgeFileVault::open_or_init(&config::identity_path(), &config::store_path())?;
            Ok(v.get(key_id)?.map(|s| s.expose_secret().to_string()))
        }
        None => Ok(None),
    }
}

/// Build the agent: the memory store + the provider + the operator's system context (SOUL/context).
/// Callers that need a shared long-lived agent (the gateway) wrap the result in `Arc`.
pub(crate) fn build_agent(cfg: &Config) -> Result<Agent> {
    let store = Store::open(&config::db_path())?;
    Ok(Agent::new(
        store,
        build_provider(cfg)?,
        crate::pref::load_system_context(),
    ))
}

/// Build the tool registry: the 3 safe default tools + `execute_code` armed with the operator-frozen
/// backend (NEVER unsandboxed unless `allow_unsandboxed` — local-CLI only) + the configured MCP
/// tools (namespaced + allow-listed; a down server is skipped). Returns the backend's honest label
/// so the CALLER decides whether to audit it (`run`/`gateway` do; `voice` deliberately does not).
pub(crate) async fn build_registry(
    cfg: &Config,
    allow_unsandboxed: bool,
) -> Result<(Registry, String)> {
    let mut registry = Registry::default_tools();
    let backend = crate::exec::backend_from_config(&cfg.exec)?;
    let label = backend.label();
    registry.register(Box::new(sa_tools::ExecuteCode::with_backend(
        backend,
        allow_unsandboxed,
    )));
    // Phase 6d `shell` — the same operator-frozen backend as execute_code, strictly fail-closed.
    // A fresh backend (backend_from_config is cheap; `label` was already captured above).
    registry.register(Box::new(sa_tools::tools::shell::Shell::with_backend(
        crate::exec::backend_from_config(&cfg.exec)?,
    )));
    // Phase 6c network tools — all funnel through the egress seam, so they're inert until the
    // operator allow-lists a host. web_extract/http_request are keyless; web_search needs an
    // operator-frozen endpoint (absent → unavailable, mirroring voice).
    registry.register(Box::new(sa_tools::tools::web_extract::WebExtract));
    registry.register(Box::new(sa_tools::tools::http_request::HttpRequest));
    if let Some(endpoint) = &cfg.tools.search_url {
        let key_ref = cfg
            .tools
            .search_key_ref
            .as_ref()
            .or(cfg.tools.default_key_ref.as_ref());
        let api_key = resolve_secret(key_ref)?;
        registry.register(Box::new(sa_tools::tools::web_search::WebSearch::with_key(
            endpoint.clone(),
            api_key,
        )));
    }
    for tool in sa_tools::mcp::load_mcp_tools(&cfg.mcp).await {
        registry.register(tool);
    }
    // Phase 6d op_tools LAST so a misconfigured one can never shadow a builtin/network/MCP tool
    // (the approval gate + egress seam key off names). A name collision or an empty cmd is skipped.
    for ot in &cfg.tools.op_tools {
        if registry.get(&ot.name).is_some() {
            tracing::warn!(
                "op_tool '{}' skipped: name collides with an existing tool",
                ot.name
            );
            continue;
        }
        match sa_tools::tools::op_tool::OpTool::new(
            ot.name.clone(),
            ot.cmd.clone(),
            ot.description.clone(),
        ) {
            Ok(t) => registry.register(Box::new(t)),
            Err(e) => tracing::warn!("op_tool '{}' skipped: {e}", ot.name),
        }
    }
    Ok((registry, label))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_provider_selects_by_kind_and_needs_no_vault_when_keyless() {
        use sa_core_types::config::ProviderConfig;
        // Default (openai, keyless Ollama) builds without opening the vault.
        assert!(build_provider(&Config::default()).is_ok());
        // anthropic kind (no api_key_ref) also builds keyless (it only needs the key at request time).
        let anthropic = Config {
            provider: ProviderConfig {
                kind: "anthropic".into(),
                model: "claude-opus-4-8".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(build_provider(&anthropic).is_ok());
        // An unknown kind is rejected.
        let bogus = Config {
            provider: ProviderConfig {
                kind: "bogus".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(build_provider(&bogus).is_err());
    }

    #[tokio::test]
    async fn build_registry_has_the_default_tools_plus_execute_code_with_the_armed_backend() {
        let (registry, label) = build_registry(&Config::default(), false).await.unwrap();
        let names = registry.names();
        for t in ["fetch", "read_file", "write_file", "execute_code"] {
            assert!(names.contains(&t), "registry missing {t}: {names:?}");
        }
        // Default [exec] backend is local.
        assert_eq!(label, "local");
    }

    #[tokio::test]
    async fn build_registry_adds_network_tools_and_omits_web_search_without_a_search_url() {
        let (registry, _) = build_registry(&Config::default(), false).await.unwrap();
        let names = registry.names();
        assert!(
            names.contains(&"web_extract"),
            "missing web_extract: {names:?}"
        );
        assert!(
            names.contains(&"http_request"),
            "missing http_request: {names:?}"
        );
        // No [tools] search_url in the default config → web_search is unavailable.
        assert!(
            !names.contains(&"web_search"),
            "web_search must be absent without a search_url: {names:?}"
        );
    }

    #[tokio::test]
    async fn build_registry_adds_shell_and_op_tools_skipping_builtin_collisions() {
        use sa_core_types::config::{OpToolConfig, ToolsConfig};
        let cfg = Config {
            tools: ToolsConfig {
                op_tools: vec![
                    OpToolConfig {
                        name: "vision".into(),
                        cmd: vec!["vis".into()],
                        description: None,
                    },
                    // collides with the builtin `fetch` → must be skipped, builtin survives
                    OpToolConfig {
                        name: "fetch".into(),
                        cmd: vec!["x".into()],
                        description: None,
                    },
                ],
                ..Default::default()
            },
            ..Default::default()
        };
        let (registry, _) = build_registry(&cfg, false).await.unwrap();
        let names = registry.names();
        assert!(names.contains(&"shell"), "missing shell: {names:?}");
        assert!(
            names.contains(&"vision"),
            "missing op_tool vision: {names:?}"
        );
        // The op_tool named "fetch" did NOT replace the builtin (whose description names "allow-listed").
        let fetch = registry.get("fetch").expect("fetch present");
        assert!(
            fetch.description().contains("allow-listed"),
            "builtin fetch must survive an op_tool name collision: {}",
            fetch.description()
        );
    }
}
