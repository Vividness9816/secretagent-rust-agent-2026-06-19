//! Telegram connector — hand-rolled `getUpdates` long-poll + `sendMessage` on raw `reqwest`
//! (rustls; zero framework). The offset is in-memory: Telegram confirms updates server-side
//! once we poll with `offset = last+1`, so a restart does not reprocess confirmed updates. The
//! bot token is injected at construction (from the vault) — never logged.

use crate::{Connector, InboundMsg, OutboundMsg};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::VecDeque;
use std::time::Duration;

pub struct TelegramConnector {
    id: String,
    token: String,
    base: String,
    client: reqwest::Client,
    offset: i64,
    buf: VecDeque<InboundMsg>,
}

impl TelegramConnector {
    pub fn new(id: impl Into<String>, token: impl Into<String>) -> Self {
        Self::with_base(id, token, "https://api.telegram.org")
    }

    /// Construct against a custom base URL (for tests against a mock HTTP server).
    pub fn with_base(
        id: impl Into<String>,
        token: impl Into<String>,
        base: impl Into<String>,
    ) -> Self {
        // Timeout > the 25s long-poll so a network hang can't block recv forever.
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(35))
            .build()
            .expect("reqwest client");
        Self {
            id: id.into(),
            token: token.into(),
            base: base.into(),
            client,
            offset: 0,
            buf: VecDeque::new(),
        }
    }

    fn api(&self, method: &str) -> String {
        format!("{}/bot{}/{}", self.base, self.token, method)
    }
}

/// PURE parse of a `getUpdates` response: extract text messages into `InboundMsg` + the max
/// `update_id` seen (the next offset is `max + 1`). Non-message / non-text updates are skipped
/// (we only drive on text). Content is NEVER trusted — the gateway stamps it `Untrusted`.
pub fn parse_updates(json: &Value, connector: &str) -> (Vec<InboundMsg>, i64) {
    let mut out = Vec::new();
    let mut max_id = 0i64;
    if let Some(arr) = json.get("result").and_then(|r| r.as_array()) {
        for u in arr {
            if let Some(uid) = u.get("update_id").and_then(|v| v.as_i64()) {
                if uid > max_id {
                    max_id = uid;
                }
            }
            let msg = match u.get("message") {
                Some(m) => m,
                None => continue,
            };
            let text = match msg.get("text").and_then(|v| v.as_str()) {
                Some(t) => t.to_string(),
                None => continue,
            };
            let sender = msg
                .get("from")
                .and_then(|f| f.get("id"))
                .and_then(|v| v.as_i64())
                .map(|n| n.to_string())
                .unwrap_or_default();
            let chat = msg
                .get("chat")
                .and_then(|c| c.get("id"))
                .and_then(|v| v.as_i64())
                .map(|n| n.to_string())
                .unwrap_or_default();
            if sender.is_empty() || chat.is_empty() {
                continue;
            }
            out.push(InboundMsg {
                connector: connector.to_string(),
                sender,
                chat,
                text,
            });
        }
    }
    (out, max_id)
}

#[async_trait]
impl Connector for TelegramConnector {
    fn id(&self) -> &str {
        &self.id
    }

    async fn recv(&mut self) -> Result<Option<InboundMsg>> {
        // Return buffered messages first; otherwise long-poll until something arrives. An empty
        // poll (25s timeout, no updates) loops — recv only returns once there IS a message, so
        // it never spuriously signals shutdown (Ok(None) is reserved for a connector that ends).
        loop {
            if let Some(m) = self.buf.pop_front() {
                return Ok(Some(m));
            }
            let resp = self
                .client
                .get(self.api("getUpdates"))
                .query(&[
                    ("offset", (self.offset + 1).to_string()),
                    ("timeout", "25".to_string()),
                ])
                .send()
                .await
                .context("telegram getUpdates")?;
            let json: Value = resp.json().await.context("telegram getUpdates body")?;
            let (msgs, max_id) = parse_updates(&json, &self.id);
            if max_id > self.offset {
                self.offset = max_id;
            }
            self.buf.extend(msgs);
        }
    }

    async fn send(&mut self, reply: OutboundMsg) -> Result<()> {
        self.client
            .post(self.api("sendMessage"))
            .json(&serde_json::json!({"chat_id": reply.chat, "text": reply.text}))
            .send()
            .await
            .context("telegram sendMessage")?
            .error_for_status()
            .context("telegram sendMessage status")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_updates_extracts_text_messages_and_max_offset() {
        let json = serde_json::json!({
            "ok": true,
            "result": [
                {"update_id": 10, "message": {"from": {"id": 42}, "chat": {"id": 99}, "text": "hello"}},
                {"update_id": 11, "edited_message": {"text": "ignored"}},          // not a `message`
                {"update_id": 12, "message": {"from": {"id": 7}, "chat": {"id": 8}}}, // no text
                {"update_id": 13, "message": {"from": {"id": 5}, "chat": {"id": 6}, "text": "world"}}
            ]
        });
        let (msgs, max_id) = parse_updates(&json, "telegram");
        assert_eq!(
            max_id, 13,
            "offset advances past every update seen, even skipped ones"
        );
        assert_eq!(msgs.len(), 2, "only text messages become InboundMsg");
        assert_eq!(
            msgs[0],
            InboundMsg {
                connector: "telegram".into(),
                sender: "42".into(),
                chat: "99".into(),
                text: "hello".into(),
            }
        );
        assert_eq!(msgs[1].text, "world");
    }

    #[test]
    fn parse_updates_empty_result_is_no_messages() {
        let (msgs, max_id) =
            parse_updates(&serde_json::json!({"ok": true, "result": []}), "telegram");
        assert!(msgs.is_empty());
        assert_eq!(max_id, 0);
    }
}
