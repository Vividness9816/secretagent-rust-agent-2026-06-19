use serde::Deserialize;
use std::path::PathBuf;

/// Phase 0 config. Everything has a default so an empty/absent file is valid.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub vault: VaultConfig,
    pub provider: ProviderConfig,
    pub policy: crate::policy::Policy,
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
}
