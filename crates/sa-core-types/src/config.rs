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
    pub voice: VoiceConfig,
    pub tools: ToolsConfig,
}

/// Operator-frozen network-tool config (Phase 6c). `search_url` = the FROZEN search endpoint the
/// model only fills a `q=` query into (`None` = `web_search` unavailable, mirroring voice). The
/// credential follows the existing `*_ref` convention (NO new "gateway" abstraction): `search_key_ref`
/// is the vault key-id for that endpoint's API key; `default_key_ref` is a shared fallback key-id a
/// tool uses when it has no own ref. Secrets are read from the vault and injected at tool
/// CONSTRUCTION (the `ExecuteCode::with_backend` precedent), never stored here as plaintext.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ToolsConfig {
    pub search_url: Option<String>,
    pub search_key_ref: Option<String>,
    pub default_key_ref: Option<String>,
}

/// Operator-configured voice (Phase 5d, ADR-20260623-phase5d-voice). `stt_cmd`/`tts_cmd` are argv
/// templates SHELLED OUT (never `sh -c`): the STT gets the audio path as its final arg and prints
/// the transcript to stdout; the TTS gets the answer on STDIN and the output wav path as its final
/// arg. `allow_tools` is the **frozen default-deny** side-effect grant for the voice `Remote` run
/// (empty = none). Empty `stt_cmd`/`tts_cmd` = voice unavailable.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct VoiceConfig {
    pub stt_cmd: Vec<String>,
    pub tts_cmd: Vec<String>,
    pub allow_tools: Vec<String>,
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
    /// "telegram" | "discord" | "email" | "slack"
    pub kind: String,
    pub token_ref: Option<String>,
    /// Slack Socket Mode only: the vault key-id for the `xapp-` app-level token (Socket Mode).
    /// `token_ref` stays the `xoxb-` bot token (chat.postMessage). `None` for other kinds.
    pub app_token_ref: Option<String>,
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
    fn config_parses_slack_with_app_token_ref() {
        let toml = r#"
[[connectors]]
name = "slack-main"
kind = "slack"
token_ref = "SLACK_BOT_TOKEN"
app_token_ref = "SLACK_APP_TOKEN"
allow_senders = ["T01ABCD:U05WXYZ"]
allow_tools = ["execute_code"]
"#;
        let c: Config = toml::from_str(toml).unwrap();
        assert_eq!(c.connectors[0].kind, "slack");
        assert_eq!(
            c.connectors[0].token_ref.as_deref(),
            Some("SLACK_BOT_TOKEN")
        );
        assert_eq!(
            c.connectors[0].app_token_ref.as_deref(),
            Some("SLACK_APP_TOKEN")
        );
        // The M3 identity is the (team, user) tuple, never a bare user id.
        assert_eq!(
            c.connectors[0].allow_senders,
            vec!["T01ABCD:U05WXYZ".to_string()]
        );
        // A binding without app_token_ref still parses (telegram/discord/email ignore it).
        let c2: Config =
            toml::from_str("[[connectors]]\nname=\"t\"\nkind=\"telegram\"\ntoken_ref=\"X\"\n")
                .unwrap();
        assert!(c2.connectors[0].app_token_ref.is_none());
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
    fn config_parses_voice_default_empty_and_explicit() {
        // Absent [voice] → all empty (voice unconfigured = unavailable).
        let c: Config = toml::from_str("").unwrap();
        assert!(c.voice.stt_cmd.is_empty() && c.voice.tts_cmd.is_empty());
        assert!(c.voice.allow_tools.is_empty());
        let toml = r#"
[voice]
stt_cmd = ["whisper", "--output-txt", "--stdout"]
tts_cmd = ["piper", "--model", "en.onnx", "--output_file"]
allow_tools = ["read_file"]
"#;
        let c2: Config = toml::from_str(toml).unwrap();
        assert_eq!(c2.voice.stt_cmd[0], "whisper");
        assert_eq!(c2.voice.tts_cmd[0], "piper");
        assert_eq!(c2.voice.allow_tools, vec!["read_file".to_string()]);
    }

    #[test]
    fn config_parses_tools_default_none_and_explicit() {
        // Absent [tools] → all None (web_search unavailable).
        let c: Config = toml::from_str("").unwrap();
        assert!(c.tools.search_url.is_none());
        assert!(c.tools.search_key_ref.is_none() && c.tools.default_key_ref.is_none());
        let toml = r#"
[tools]
search_url = "https://search.example/api"
search_key_ref = "SEARCH_KEY"
default_key_ref = "DEFAULT_KEY"
"#;
        let c2: Config = toml::from_str(toml).unwrap();
        assert_eq!(
            c2.tools.search_url.as_deref(),
            Some("https://search.example/api")
        );
        assert_eq!(c2.tools.search_key_ref.as_deref(), Some("SEARCH_KEY"));
        assert_eq!(c2.tools.default_key_ref.as_deref(), Some("DEFAULT_KEY"));
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
