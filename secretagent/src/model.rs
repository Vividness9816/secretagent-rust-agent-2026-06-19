//! The operator-only `model` switch (Phase 6e): rewrites `[provider] model` in `config.toml`
//! format-preservingly (toml_edit keeps comments + layout). **Operator-only BY CONSTRUCTION** —
//! this is a CLI subcommand, and a Remote/cron principal invokes only registry tools, never CLI
//! subcommands, so it can never repoint the model/endpoint. No model-switching tool is ever
//! registered (that would be the hole). Takes effect on the next `run`/`chat` load.

use anyhow::{Context, Result};
use sa_core_types::config;
use std::fs;
use toml_edit::{value, DocumentMut, Item, Table};

/// Set `[provider] model = <name>` in a config.toml string, creating the `[provider]` table if
/// absent. Preserves all other content, comments, and formatting. Pure + unit-testable.
pub fn set_model_in(doc_str: &str, name: &str) -> Result<String> {
    let mut doc: DocumentMut = doc_str.parse().context("config.toml is not valid TOML")?;
    let provider = doc
        .entry("provider")
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .context("[provider] exists but is not a table")?;
    provider["model"] = value(name);
    Ok(doc.to_string())
}

pub fn run(name: &str) -> Result<()> {
    let path = config::config_dir().join("config.toml");
    let current = if path.exists() {
        fs::read_to_string(&path)?
    } else {
        String::new()
    };
    let updated = set_model_in(&current, name)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, updated)?;
    println!("provider model set to '{name}' in {}", path.display());
    println!("(takes effect on the next run/chat; restart the gateway daemon to pick it up)");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_model_preserves_comments_and_other_keys() {
        let input =
            "# my config\n[provider]\nkind = \"anthropic\"  # the provider\nmodel = \"old\"\n";
        let out = set_model_in(input, "claude-opus-4-8").unwrap();
        assert!(out.contains("model = \"claude-opus-4-8\""));
        assert!(!out.contains("\"old\""));
        assert!(
            out.contains("# my config"),
            "leading comment preserved: {out}"
        );
        assert!(
            out.contains("kind = \"anthropic\""),
            "other key preserved: {out}"
        );
        assert!(
            out.contains("# the provider"),
            "inline comment preserved: {out}"
        );
    }

    #[test]
    fn set_model_creates_provider_table_when_absent_and_round_trips() {
        let out = set_model_in("", "claude-haiku-4-5").unwrap();
        // The rewritten document must re-parse as well-formed TOML with the model in [provider].
        let doc: DocumentMut = out.parse().unwrap();
        assert_eq!(doc["provider"]["model"].as_str(), Some("claude-haiku-4-5"));
    }
}
