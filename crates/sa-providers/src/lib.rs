pub mod openai;

use anyhow::Result;
use futures::stream::{self, BoxStream};

#[derive(Debug, Clone)]
pub struct ChatMsg {
    pub role: String,
    pub content: String,
}

/// One streamed token/delta of an assistant reply.
#[derive(Debug, Clone)]
pub struct ChatChunk(pub String);

/// A model backend. One OpenAI-compatible impl (`openai::OpenAiCompat`) covers both
/// Ollama (via its `/v1` endpoint) and OpenAI; the trait keeps `sa-core` testable
/// against a `MockProvider` with no network.
#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    async fn chat(&self, messages: Vec<ChatMsg>) -> Result<BoxStream<'static, Result<ChatChunk>>>;
}

/// Deterministic in-memory provider for tests. Yields `reply` as a single chunk.
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

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

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
}
