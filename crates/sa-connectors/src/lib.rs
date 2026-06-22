//! Messaging connectors (Phase 4c). The `Connector` trait + InboundMsg/OutboundMsg are the
//! seam between an untrusted external transport and the gateway. Each impl hides its transport
//! (Telegram long-poll, Discord gateway, IMAP poll); the gateway treats them uniformly and
//! stamps every inbound message as a `Remote` principal (ADR-20260621). Heavy transport deps
//! are feature-gated so they never bloat sa-core's or the musl build's graph unnecessarily.

use anyhow::Result;
use async_trait::async_trait;

pub mod telegram;

#[cfg(feature = "discord")]
pub mod discord;
#[cfg(feature = "email")]
pub mod email;

/// An untrusted inbound message from a connector. `sender` is the M3 identity; `chat` is the
/// conversation to reply to. NEVER trusted — the gateway stamps it `Untrusted{source}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundMsg {
    pub connector: String,
    pub sender: String,
    pub chat: String,
    pub text: String,
}

/// A reply to deliver back to a `chat` on the connector that produced the inbound message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundMsg {
    pub chat: String,
    pub text: String,
}

#[async_trait]
pub trait Connector: Send {
    /// Stable id for audit + the M3 allow-list keying.
    fn id(&self) -> &str;
    /// Next inbound message, or `None` on clean shutdown. Drains the transport internally.
    async fn recv(&mut self) -> Result<Option<InboundMsg>>;
    /// Deliver a reply on the same transport.
    async fn send(&mut self, reply: OutboundMsg) -> Result<()>;
}

/// In-memory test connector: yields scripted inbound messages, records what was sent. Lets the
/// gateway boundary be tested with NO network (the McpClient<R,W> testability discipline).
pub struct MockConnector {
    id: String,
    inbound: std::collections::VecDeque<InboundMsg>,
    pub sent: std::sync::Arc<std::sync::Mutex<Vec<OutboundMsg>>>,
}

impl MockConnector {
    pub fn new(id: &str, inbound: Vec<InboundMsg>) -> Self {
        Self {
            id: id.to_string(),
            inbound: inbound.into(),
            sent: Default::default(),
        }
    }
}

#[async_trait]
impl Connector for MockConnector {
    fn id(&self) -> &str {
        &self.id
    }
    async fn recv(&mut self) -> Result<Option<InboundMsg>> {
        Ok(self.inbound.pop_front())
    }
    async fn send(&mut self, reply: OutboundMsg) -> Result<()> {
        self.sent.lock().unwrap().push(reply);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_connector_yields_then_drains_and_records_sends() {
        let mut c = MockConnector::new(
            "telegram",
            vec![InboundMsg {
                connector: "telegram".into(),
                sender: "1".into(),
                chat: "c1".into(),
                text: "hi".into(),
            }],
        );
        let sent = c.sent.clone();
        let m = c.recv().await.unwrap().expect("one message");
        assert_eq!(m.sender, "1");
        assert!(c.recv().await.unwrap().is_none(), "drained → None");
        c.send(OutboundMsg {
            chat: "c1".into(),
            text: "yo".into(),
        })
        .await
        .unwrap();
        assert_eq!(sent.lock().unwrap()[0].text, "yo");
    }
}
