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
    fn soul_and_context_paths_honor_config_override() {
        std::env::set_var("SECRETAGENT_CONFIG_DIR", "/tmp/sa-cfg");
        let soul = soul_path();
        let ctx = context_path();
        std::env::remove_var("SECRETAGENT_CONFIG_DIR");
        assert!(soul.ends_with("SOUL.md") && soul.starts_with("/tmp/sa-cfg"));
        assert!(ctx.ends_with("context.md") && ctx.starts_with("/tmp/sa-cfg"));
    }
}
