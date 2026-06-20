use crate::{ChatChunk, ChatMsg, Provider, ProviderAction, ToolSpec};
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

    async fn act(
        &self,
        messages: Vec<serde_json::Value>,
        tools: &[ToolSpec],
    ) -> Result<ProviderAction> {
        let tool_specs: Vec<_> = tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    }
                })
            })
            .collect();
        let mut body = json!({"model": self.model, "messages": messages, "stream": false});
        if !tool_specs.is_empty() {
            body["tools"] = json!(tool_specs);
        }
        let mut req = reqwest::Client::new()
            .post(format!("{}/chat/completions", self.base_url))
            .json(&body);
        if let Some(k) = &self.api_key {
            req = req.bearer_auth(k);
        }
        let v: serde_json::Value = req.send().await?.error_for_status()?.json().await?;
        let msg = &v["choices"][0]["message"];

        if let Some(tc) = msg["tool_calls"].as_array().and_then(|a| a.first()) {
            let id = tc["id"].as_str().unwrap_or("call_0").to_string();
            let name = tc["function"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            // OpenAI sends arguments as a JSON *string*; Ollama as an object. Accept both.
            let raw = &tc["function"]["arguments"];
            let args = match raw.as_str() {
                Some(s) => serde_json::from_str(s).unwrap_or_else(|_| json!({})),
                None => raw.clone(),
            };
            return Ok(ProviderAction::ToolCall { id, name, args });
        }
        let text = msg["content"].as_str().unwrap_or_default().to_string();
        Ok(ProviderAction::Text(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChatMsg, Provider, ProviderAction, ToolSpec};
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

    #[tokio::test]
    async fn act_parses_a_tool_call_from_the_response() {
        let server = wiremock::MockServer::start().await;
        let body = serde_json::json!({
            "choices": [{"message": {"tool_calls": [{
                "id": "call_1",
                "function": {"name": "fetch", "arguments": "{\"url\":\"http://example.com\"}"}
            }]}}]
        });
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/chat/completions"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;

        let p = OpenAiCompat {
            base_url: server.uri(),
            model: "test".into(),
            api_key: None,
        };
        let action = p
            .act(
                vec![serde_json::json!({"role": "user", "content": "go"})],
                &[ToolSpec {
                    name: "fetch".into(),
                    description: String::new(),
                    parameters: serde_json::json!({}),
                }],
            )
            .await
            .unwrap();
        match action {
            ProviderAction::ToolCall { name, args, .. } => {
                assert_eq!(name, "fetch");
                assert_eq!(args["url"], "http://example.com");
            }
            other => panic!("expected tool call, got {other:?}"),
        }
    }
}
