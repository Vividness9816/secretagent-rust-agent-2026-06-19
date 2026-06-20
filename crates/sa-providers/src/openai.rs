use crate::{ChatChunk, ChatMsg, Provider};
use anyhow::Result;
use futures::stream::{BoxStream, StreamExt};
use serde_json::json;

/// OpenAI-compatible streaming chat. `base_url` selects the backend:
/// `http://localhost:11434/v1` (Ollama), `https://api.openai.com/v1`, etc.
pub struct OpenAiCompat {
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
}

/// Pure SSE-line parser: returns the delta content, or None for `[DONE]`,
/// comments, and blanks. Unit-testable without a network.
pub fn parse_sse_line(line: &str) -> Option<String> {
    let data = line.strip_prefix("data:")?.trim();
    if data == "[DONE]" || data.is_empty() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(data).ok()?;
    let c = v
        .get("choices")?
        .get(0)?
        .get("delta")?
        .get("content")?
        .as_str()?;
    Some(c.to_string())
}

#[async_trait::async_trait]
impl Provider for OpenAiCompat {
    async fn chat(&self, messages: Vec<ChatMsg>) -> Result<BoxStream<'static, Result<ChatChunk>>> {
        let msgs: Vec<_> = messages
            .iter()
            .map(|m| json!({"role": m.role, "content": m.content}))
            .collect();
        let mut req = reqwest::Client::new()
            .post(format!("{}/chat/completions", self.base_url))
            .json(&json!({"model": self.model, "messages": msgs, "stream": true}));
        if let Some(k) = &self.api_key {
            req = req.bearer_auth(k);
        }
        let resp = req.send().await?.error_for_status()?;

        let stream = async_stream::stream! {
            let mut buf = String::new();
            let mut bytes = resp.bytes_stream();
            while let Some(chunk) = bytes.next().await {
                let chunk = match chunk {
                    Ok(c) => c,
                    Err(e) => { yield Err(e.into()); break; }
                };
                buf.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(nl) = buf.find('\n') {
                    let line: String = buf.drain(..=nl).collect();
                    if let Some(delta) = parse_sse_line(line.trim_end()) {
                        yield Ok(ChatChunk(delta));
                    }
                }
            }
        };
        Ok(Box::pin(stream))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChatMsg, Provider};
    use futures::StreamExt;

    #[test]
    fn parse_sse_line_extracts_delta_and_ignores_done() {
        assert_eq!(
            parse_sse_line(r#"data: {"choices":[{"delta":{"content":"Mo"}}]}"#),
            Some("Mo".to_string())
        );
        assert_eq!(parse_sse_line("data: [DONE]"), None);
        assert_eq!(parse_sse_line(""), None);
        assert_eq!(parse_sse_line(": comment"), None);
    }

    #[tokio::test]
    async fn streams_from_an_openai_compatible_endpoint() {
        let server = wiremock::MockServer::start().await;
        let body = "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n\
                    data: {\"choices\":[{\"delta\":{\"content\":\" Mochi\"}}]}\n\n\
                    data: [DONE]\n\n";
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/chat/completions"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(body))
            .mount(&server)
            .await;

        let p = OpenAiCompat {
            base_url: server.uri(),
            model: "test".into(),
            api_key: None,
        };
        let mut s = p
            .chat(vec![ChatMsg {
                role: "user".into(),
                content: "hi".into(),
            }])
            .await
            .unwrap();
        let mut out = String::new();
        while let Some(c) = s.next().await {
            out.push_str(&c.unwrap().0);
        }
        assert_eq!(out, "Hello Mochi");
    }
}
