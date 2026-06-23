use serde::Deserialize;
use std::path::PathBuf;

/// Phase 0 config. Everything has a default so an empty/absent file is valid.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub vault: VaultConfig,
    pub provider: ProviderConfig,
    pub policy: crate::policy::Policy,
    pub mcp: Vec<McpServerConfig>,
    pub connectors: Vec<ConnectorConfig>,
    pub exec: ExecConfig,
}

/// The OPERATOR-FROZEN execution backend for `execute_code` (ADR-20260623 BLOCKER #2: backend is
/// config, NEVER a model tool argument). `backend` = "local" | "docker" | "ssh"; `image` is
/// required for docker, `host` (e.g. "user@host") for ssh. Default = local (landlock on Linux).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ExecConfig {
    pub backend: String,
    pub image: Option<String>,
    pub host: Option<String>,
}

impl Default for ExecConfig {
    fn default() -> Self {
        Self {
            backend: "local".into(),
            image: None,
            host: None,
        }
    }
}

/// A configured messaging connector binding (Phase 4c). `allow_senders` is **default-deny**
/// (empty = a connector that accepts NO one — M3); `allow_tools` is the **frozen** per-binding
/// side-effect grant a `Remote` run may use (never ad-hoc). `token_ref` is a vault key-id —
/// never a plaintext secret (invariant #4).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ConnectorConfig {
    pub name: String,
    /// "telegram" | "discord" | "email"
    pub kind: String,
    pub token_ref: Option<String>,
    pub allow_senders: Vec<String>,
    pub allow_tools: Vec<String>,
    // Email-only transport addresses (non-secret operator config). `token_ref` stays the vault
    // key-id for the IMAP/SMTP password. All optional so telegram/discord bindings ignore them.
    pub imap_host: Option<String>,
    pub imap_port: Option<u16>,
    pub smtp_host: Option<String>,
    pub smtp_port: Option<u16>,
    pub username: Option<String>,
    pub from: Option<String>,
}

/// A configured MCP server. The operator lists each server + which of its tools are
/// allow-listed; an empty `allow_tools` loads nothing (default-deny).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub allow_tools: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct VaultConfig {
    /// "age-file" in Phase 0. Keyring/TPM backends are added later behind the same trait.
    pub backend: String,
}

impl Default for VaultConfig {
    fn default() -> Self {
        Self {
            backend: "age-file".into(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    /// OpenAI-compatible base URL. Default = local Ollama.
    pub base_url: String,
    pub model: String,
    /// Vault key-id for the API key; `None` for keyless backends (Ollama).
    pub api_key_ref: Option<String>,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            base_url: "http://localhost:11434/v1".into(),
            model: "llama3.2".into(),
            api_key_ref: None,
        }
    }
}

impl Config {
    pub fn load() -> anyhow::Result<Config> {
        let path = config_dir().join("config.toml");
        if path.exists() {
            Ok(toml::from_str(&std::fs::read_to_string(path)?)?)
        } else {
            Ok(Config::default())
        }
    }
}

/// Per-OS data dir, overridable by `SECRETAGENT_DATA_DIR` (set by the systemd
/// unit's `StateDirectory=` on Linux). Falls back to the platform data dir.
pub fn data_dir() -> PathBuf {
    if let Ok(d) = std::env::var("SECRETAGENT_DATA_DIR") {
        return PathBuf::from(d);
    }
    directories::ProjectDirs::from("dev", "secretagent", "secretagent")
        .map(|p| p.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".secretagent"))
}

pub fn config_dir() -> PathBuf {
    if let Ok(d) = std::env::var("SECRETAGENT_CONFIG_DIR") {
        return PathBuf::from(d);
    }
    directories::ProjectDirs::from("dev", "secretagent", "secretagent")
        .map(|p| p.config_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".secretagent"))
}

pub fn identity_path() -> PathBuf {
    data_dir().join("identity.age")
}

pub fn store_path() -> PathBuf {
    data_dir().join("store.age")
}

pub fn db_path() -> PathBuf {
    data_dir().join("memory.db")
}

pub fn audit_path() -> PathBuf {
    data_dir().join("audit.jsonl")
}

/// Operator-authored personality file (global), read into the system preamble.
pub fn soul_path() -> PathBuf {
    config_dir().join("SOUL.md")
}

/// Operator-authored project/context file, read into the system preamble.
pub fn context_path() -> PathBuf {
    config_dir().join("context.md")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_path_honors_env_override() {
        std::env::set_var("SECRETAGENT_DATA_DIR", "/tmp/sa-test");
        let p = identity_path();
        std::env::remove_var("SECRETAGENT_DATA_DIR");
        assert!(p.ends_with("identity.age"), "got {p:?}");
        assert!(p.starts_with("/tmp/sa-test"), "env override ignored: {p:?}");
    }

    #[test]
    fn config_parses_minimal_toml() {
        let c: Config = toml::from_str("").unwrap();
        assert_eq!(c.vault.backend, "age-file");
    }

    #[test]
    fn config_parses_exec_backend_default_local() {
        // Absent [exec] → local (backward compatible; the default execution backend).
        let c: Config = toml::from_str("").unwrap();
        assert_eq!(c.exec.backend, "local");
        assert!(c.exec.image.is_none() && c.exec.host.is_none());
        let c2: Config = toml::from_str("[exec]\nbackend=\"docker\"\nimage=\"alpine\"\n").unwrap();
        assert_eq!(c2.exec.backend, "docker");
        assert_eq!(c2.exec.image.as_deref(), Some("alpine"));
        let c3: Config = toml::from_str("[exec]\nbackend=\"ssh\"\nhost=\"build@h\"\n").unwrap();
        assert_eq!(c3.exec.backend, "ssh");
        assert_eq!(c3.exec.host.as_deref(), Some("build@h"));
    }

    #[test]
    fn config_parses_mcp_servers() {
        let toml = r#"
[[mcp]]
name = "rose"
command = "rose-glass-mcp"
args = ["--db", "/x/index.db"]
allow_tools = ["search"]
"#;
        let c: Config = toml::from_str(toml).unwrap();
        assert_eq!(c.mcp.len(), 1);
        assert_eq!(c.mcp[0].name, "rose");
        assert_eq!(c.mcp[0].command, "rose-glass-mcp");
        assert_eq!(c.mcp[0].allow_tools, vec!["search".to_string()]);
        // empty/absent mcp is valid (default-deny: no servers)
        assert!(toml::from_str::<Config>("").unwrap().mcp.is_empty());
    }

    #[test]
    fn config_parses_connectors_default_deny() {
        let toml = r#"
[[connectors]]
name = "telegram-main"
kind = "telegram"
token_ref = "TELEGRAM_BOT_TOKEN"
allow_senders = ["123456789"]
allow_tools = []
"#;
        let c: Config = toml::from_str(toml).unwrap();
        assert_eq!(c.connectors.len(), 1);
        assert_eq!(c.connectors[0].kind, "telegram");
        assert_eq!(
            c.connectors[0].token_ref.as_deref(),
            Some("TELEGRAM_BOT_TOKEN")
        );
        assert_eq!(c.connectors[0].allow_senders, vec!["123456789".to_string()]);
        // empty/absent connectors is valid (default-deny: no connectors load)
        assert!(toml::from_str::<Config>("").unwrap().connectors.is_empty());
    }

    #[test]
    fn soul_and_context_paths_honor_config_override() {
        std::env::set_var("SECRETAGENT_CONFIG_DIR", "/tmp/sa-cfg");
        let soul = soul_path();
        let ctx = context_path();
        std::env::remove_var("SECRETAGENT_CONFIG_DIR");
        assert!(soul.ends_with("SOUL.md") && soul.starts_with("/tmp/sa-cfg"));
        assert!(ctx.ends_with("context.md") && ctx.starts_with("/tmp/sa-cfg"));
    }
}
