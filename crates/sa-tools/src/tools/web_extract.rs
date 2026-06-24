//! `web_extract` — GET an allow-listed URL through the egress seam and return its readable text.
//! The HTML→text pass is intentionally naive (dep-free).
// ponytail: naive tag-stripper, not a readability extractor — swap in a crate if extraction
// quality (boilerplate removal, main-content detection) ever matters.

use crate::{egress, Tool};
use anyhow::Result;
use async_trait::async_trait;
use sa_core_types::policy::Policy;
use serde_json::{json, Value};

pub struct WebExtract;

#[async_trait]
impl Tool for WebExtract {
    fn name(&self) -> &str {
        "web_extract"
    }
    fn description(&self) -> &str {
        "Fetch an allow-listed URL and return its readable text (HTML stripped; untrusted)."
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"url":{"type":"string"}},"required":["url"]})
    }
    async fn run(&self, args: Value, policy: &Policy) -> Result<String> {
        let url = args
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("web_extract: missing 'url'"))?;
        let html = egress::egress_get(policy, url)
            .await?
            .detaint("core re-taints tool output at the registry boundary");
        Ok(strip_html(&html))
    }
}

/// Drop `<script>`/`<style>` blocks, strip remaining tags, decode a handful of entities, and
/// collapse whitespace. ASCII-lowercase search keeps byte indices aligned (length-preserving).
fn strip_html(html: &str) -> String {
    let no_scripts = remove_block(html, "script");
    let cleaned = remove_block(&no_scripts, "style");

    let mut out = String::with_capacity(cleaned.len());
    let mut in_tag = false;
    for c in cleaned.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    // Decode `&amp;` LAST so `&amp;lt;` becomes `&lt;`, not `<`.
    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Remove `<tag ...> ... </tag>` spans (case-insensitive). `to_ascii_lowercase` is length-preserving
/// so the lowercased copy's byte offsets index the original safely.
fn remove_block(s: &str, tag: &str) -> String {
    let lower = s.to_ascii_lowercase();
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut out = String::new();
    let mut i = 0usize;
    while i < s.len() {
        match lower[i..].find(&open) {
            Some(rel) => {
                let start = i + rel;
                out.push_str(&s[i..start]);
                match lower[start..].find(&close) {
                    Some(crel) => i = start + crel + close.len(),
                    None => break, // unterminated block — drop the rest
                }
            }
            None => {
                out.push_str(&s[i..]);
                break;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_tags_scripts_styles_and_decodes_entities() {
        assert_eq!(
            strip_html("<style>.x{}</style><p>Hi <b>there</b></p>"),
            "Hi there"
        );
        assert_eq!(strip_html("<SCRIPT>alert(1)</SCRIPT>A &amp; B"), "A & B");
        assert_eq!(strip_html("a&lt;b&gt;c"), "a<b>c");
    }

    #[tokio::test]
    async fn denies_unlisted_host_through_the_seam() {
        let p = Policy {
            egress_allow: vec!["example.com".into()],
            ..Default::default()
        };
        let err = WebExtract
            .run(json!({"url":"http://evil.test/x"}), &p)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("denied"), "got {err}");
    }
}
