pub mod anthropic;
pub mod openai;

use anyhow::Result;
use futures::stream::{self, BoxStream};
use futures::StreamExt;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct ChatMsg {
    pub role: String,
    pub content: String,
}

/// One streamed token/delta of an assistant reply (the plain `chat` path).
#[derive(Debug, Clone)]
pub struct ChatChunk(pub String);

/// A tool the model may call, as an OpenAI-style function spec.
#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value, // JSON schema
}

/// What the model decided to do this turn: emit a final answer, or call a tool.
#[derive(Debug, Clone)]
pub enum ProviderAction {
    Text(String),
    ToolCall {
        id: String,
        name: String,
        args: serde_json::Value,
    },
}

/// A model backend. `chat` is the plain streaming path (Phase 1). `act` is the
/// agentic tool-calling path (Phase 2): given OpenAI-format messages + tool specs,
/// it returns either final text or a tool call. `act` defaults to an error so
/// existing providers compile unchanged.
#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    async fn chat(&self, messages: Vec<ChatMsg>) -> Result<BoxStream<'static, Result<ChatChunk>>>;

    async fn act(
        &self,
        _messages: Vec<serde_json::Value>,
        _tools: &[ToolSpec],
    ) -> Result<ProviderAction> {
        anyhow::bail!("this provider does not support tool-calling (act)")
    }

    /// One-shot completion: collect the streaming `chat` into a single String. Default works
    /// for any provider that implements `chat` (used by memory summarization, Phase 3c).
    async fn complete(&self, messages: Vec<ChatMsg>) -> Result<String> {
        let mut stream = self.chat(messages).await?;
        let mut out = String::new();
        while let Some(chunk) = stream.next().await {
            out.push_str(&chunk?.0);
        }
        Ok(out)
    }
}

/// Deterministic single-chunk provider for the plain-chat tests.
pub struct MockProvider {
    pub reply: String,
}

#[async_trait::async_trait]
impl Provider for MockProvider {
    async fn chat(&self, _messages: Vec<ChatMsg>) -> Result<BoxStream<'static, Result<ChatChunk>>> {
        let reply = self.reply.clone();
        Ok(Box::pin(stream::once(async move { Ok(ChatChunk(reply)) })))
    }
}

/// Scripts a fixed sequence of `act` results and records the messages it was given
/// on each call — for hermetically testing the agentic loop + injection guard.
#[derive(Clone, Default)]
pub struct ScriptedProvider {
    actions: Arc<Mutex<VecDeque<ProviderAction>>>,
    pub seen: Arc<Mutex<Vec<Vec<serde_json::Value>>>>,
}

impl ScriptedProvider {
    pub fn new(actions: Vec<ProviderAction>) -> Self {
        Self {
            actions: Arc::new(Mutex::new(actions.into())),
            seen: Arc::new(Mutex::new(Vec::new())),
        }
    }
    /// The messages passed to the Nth `act` call (0-indexed).
    pub fn messages_on_call(&self, n: usize) -> Vec<serde_json::Value> {
        self.seen
            .lock()
            .unwrap()
            .get(n)
            .cloned()
            .unwrap_or_default()
    }
}

#[async_trait::async_trait]
impl Provider for ScriptedProvider {
    async fn chat(&self, _messages: Vec<ChatMsg>) -> Result<BoxStream<'static, Result<ChatChunk>>> {
        Ok(Box::pin(stream::empty()))
    }
    async fn act(
        &self,
        messages: Vec<serde_json::Value>,
        _tools: &[ToolSpec],
    ) -> Result<ProviderAction> {
        self.seen.lock().unwrap().push(messages);
        Ok(self
            .actions
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(ProviderAction::Text("(end)".into())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use serde_json::json;

    #[tokio::test]
    async fn mock_provider_streams_its_reply() {
        let p = MockProvider {
            reply: "hello world".into(),
        };
        let mut s = p
            .chat(vec![ChatMsg {
                role: "user".into(),
                content: "hi".into(),
            }])
            .await
            .unwrap();
        let mut out = String::new();
        while let Some(chunk) = s.next().await {
            out.push_str(&chunk.unwrap().0);
        }
        assert_eq!(out, "hello world");
    }

    #[tokio::test]
    async fn scripted_provider_returns_actions_in_order_and_records_messages() {
        let p = ScriptedProvider::new(vec![
            ProviderAction::ToolCall {
                id: "c0".into(),
                name: "fetch".into(),
                args: json!({"url": "http://example.com"}),
            },
            ProviderAction::Text("done".into()),
        ]);
        let a1 = p
            .act(vec![json!({"role": "user", "content": "go"})], &[])
            .await
            .unwrap();
        assert!(matches!(a1, ProviderAction::ToolCall { .. }));
        let a2 = p
            .act(vec![json!({"role": "tool", "content": "result"})], &[])
            .await
            .unwrap();
        assert!(matches!(a2, ProviderAction::Text(t) if t == "done"));
        assert_eq!(p.messages_on_call(1)[0]["role"], "tool");
    }

    #[tokio::test]
    async fn complete_collects_the_chat_stream() {
        let p = MockProvider {
            reply: "the summary".into(),
        };
        let out = p
            .complete(vec![ChatMsg {
                role: "user".into(),
                content: "summarize".into(),
            }])
            .await
            .unwrap();
        assert_eq!(out, "the summary");
    }
}
