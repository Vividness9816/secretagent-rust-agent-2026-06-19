//! Native Anthropic Messages API provider (Phase 6e). NOT an OpenAI-compat shim — the Messages API
//! differs: top-level `system`, content blocks, `tool_use`/`tool_result`, the tools JSON schema
//! under `input_schema` (NOT `parameters`), a REQUIRED `max_tokens`, and `x-api-key` (NOT a Bearer
//! `Authorization`). Wire contract verified 2026-06-24 against platform.claude.com.
//!
//! This is an OPERATOR-FROZEN client: it talks to a fixed first-party endpoint and is never
//! model-reachable, so it deliberately stays OUTSIDE the 6c egress seam (like the connectors).
//!
//! SECRET POLICY (invariant #4): `x-api-key` is set as a header and NEVER logged — no request
//! headers/body, no `json()` dump, no error context that could echo the key.

use crate::{ChatChunk, ChatMsg, Provider, ProviderAction, ToolSpec};
use anyhow::Result;
use futures::stream::{self, BoxStream};
use serde_json::{json, Value};

/// The only published stable version strings are `2023-01-01` and `2023-06-01`; the latter is
/// current. One named place so it's one-line tunable if a newer version is ever published.
const ANTHROPIC_API_VERSION: &str = "2023-06-01";
/// REQUIRED by the API (a request 400s without it); caps OUTPUT tokens for one `act`/`chat` turn.
const MAX_TOKENS_PER_CALL: usize = 4096;

pub struct Anthropic {
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
}

impl Anthropic {
    /// Construct against the first-party endpoint. `api_key` is `None`-constructible (it only fails
    /// at request time) so provider selection never has to open the vault just to build this.
    pub fn new(model: String, api_key: Option<String>) -> Self {
        Self {
            base_url: "https://api.anthropic.com".to_string(),
            model,
            api_key,
        }
    }

    fn build_body(&self, messages: &[Value], tools: &[ToolSpec]) -> Value {
        let (system, msgs) = translate(messages);
        let mut body = json!({
            "model": self.model,
            "max_tokens": MAX_TOKENS_PER_CALL,
            "messages": msgs,
        });
        if let Some(s) = system {
            body["system"] = json!(s);
        }
        // `input_schema`, NOT `parameters` — the #1 OpenAI-divergence bug. Omit `tools` when empty
        // (never send `[]`).
        if !tools.is_empty() {
            let specs: Vec<Value> = tools
                .iter()
                .map(|t| {
                    json!({"name": t.name, "description": t.description, "input_schema": t.parameters})
                })
                .collect();
            body["tools"] = json!(specs);
        }
        body
    }

    async fn post(&self, body: &Value) -> Result<Value> {
        let url = format!("{}/v1/messages", self.base_url.trim_end_matches('/'));
        let mut req = reqwest::Client::new()
            .post(url)
            .header("anthropic-version", ANTHROPIC_API_VERSION)
            .header("content-type", "application/json")
            .json(body);
        if let Some(k) = &self.api_key {
            req = req.header("x-api-key", k); // header only — never logged
        }
        // We surface only the HTTP status (error_for_status), never the response body — keeps any
        // echoed credential / prompt out of logs (the v1 diagnostic signal is the status code).
        Ok(req.send().await?.error_for_status()?.json().await?)
    }
}

#[async_trait::async_trait]
impl Provider for Anthropic {
    async fn chat(&self, messages: Vec<ChatMsg>) -> Result<BoxStream<'static, Result<ChatChunk>>> {
        let as_values: Vec<Value> = messages
            .iter()
            .map(|m| json!({"role": m.role, "content": m.content}))
            .collect();
        let body = self.build_body(&as_values, &[]);
        let v = self.post(&body).await?;
        let text = match parse_action(&v) {
            ProviderAction::Text(t) => t,
            ProviderAction::ToolCall { .. } => String::new(),
        };
        // ponytail: v1 Anthropic `chat` is non-streaming — yields the whole reply as ONE chunk. Real
        // SSE (content_block_delta/text_delta accumulation + the post-200 in-stream `error` event)
        // is deferred until a UX need proves it; the agentic `act` path (the task runner) is
        // non-streaming anyway, so task execution is unaffected.
        Ok(Box::pin(stream::once(async move { Ok(ChatChunk(text)) })))
    }

    async fn act(&self, messages: Vec<Value>, tools: &[ToolSpec]) -> Result<ProviderAction> {
        let body = self.build_body(&messages, tools);
        let v = self.post(&body).await?;
        Ok(parse_action(&v))
    }
}

/// Translate OpenAI-format loop messages → (top-level `system`, Anthropic `messages`).
/// - The FIRST `{role:"system"}` content becomes `system`; later system messages are SKIPPED
///   (the loop guarantees exactly one at position 0 — a later one would be unexpected/injection).
/// - `{role:"assistant", tool_calls:[{id,function:{name,arguments}}]}` → an assistant `tool_use`
///   block (arguments is a JSON *string* → parsed to an object, `{}` on malformed JSON).
/// - `{role:"assistant", content}` (no tool_calls) → an assistant `text` block.
/// - `{role:"tool", tool_call_id, content}` → a USER message with a `tool_result` block
///   (`tool_call_id` → `tool_use_id`).
/// - anything else (`user`) → a user `text` block.
///
/// Consecutive same-role messages are MERGED (`push_block`) so no two adjacent same-role messages
/// exist and each `tool_result` lands as the first block of the user message right after its
/// `tool_use` assistant message.
fn translate(messages: &[Value]) -> (Option<String>, Vec<Value>) {
    let mut system: Option<String> = None;
    let mut out: Vec<Value> = Vec::new();
    for m in messages {
        let role = m.get("role").and_then(|v| v.as_str()).unwrap_or("");
        match role {
            "system" => {
                if system.is_none() {
                    system = m
                        .get("content")
                        .and_then(|v| v.as_str())
                        .map(str::to_string);
                }
            }
            "assistant" => {
                let block = if let Some(tc) = m
                    .get("tool_calls")
                    .and_then(|v| v.as_array())
                    .and_then(|a| a.first())
                {
                    let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("toolu_0");
                    let f = tc.get("function").cloned().unwrap_or_else(|| json!({}));
                    let name = f.get("name").and_then(|v| v.as_str()).unwrap_or_default();
                    let input = match f.get("arguments").and_then(|v| v.as_str()) {
                        Some(s) => serde_json::from_str(s).unwrap_or_else(|_| json!({})),
                        None => json!({}),
                    };
                    json!({"type": "tool_use", "id": id, "name": name, "input": input})
                } else {
                    let text = m
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default();
                    json!({"type": "text", "text": text})
                };
                push_block(&mut out, "assistant", block);
            }
            "tool" => {
                let id = m
                    .get("tool_call_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let content = m
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                push_block(
                    &mut out,
                    "user",
                    json!({"type": "tool_result", "tool_use_id": id, "content": content}),
                );
            }
            _ => {
                let text = m
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                push_block(&mut out, "user", json!({"type": "text", "text": text}));
            }
        }
    }
    (system, out)
}

/// Append `block` to the last message if it already has `role`, else start a new message. Guarantees
/// no two adjacent same-role messages (the API's tool_use→tool_result adjacency rule) and preserves
/// block insertion order.
fn push_block(out: &mut Vec<Value>, role: &str, block: Value) {
    if let Some(last) = out.last_mut() {
        if last.get("role").and_then(|v| v.as_str()) == Some(role) {
            if let Some(arr) = last.get_mut("content").and_then(|c| c.as_array_mut()) {
                arr.push(block);
                return;
            }
        }
    }
    out.push(json!({"role": role, "content": [block]}));
}

/// Read an Anthropic response into a `ProviderAction`. Iterates `content[]` BY ORDER (a response may
/// mix `text` and `tool_use`): the FIRST `tool_use` block wins (max one tool/turn), else all `text`
/// blocks are concatenated. No content → empty text.
fn parse_action(v: &Value) -> ProviderAction {
    let Some(blocks) = v.get("content").and_then(|c| c.as_array()) else {
        return ProviderAction::Text(String::new());
    };
    for b in blocks {
        if b.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
            let id = b
                .get("id")
                .and_then(|x| x.as_str())
                .unwrap_or("toolu_0")
                .to_string();
            let name = b
                .get("name")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string();
            let args = b.get("input").cloned().unwrap_or_else(|| json!({}));
            return ProviderAction::ToolCall { id, name, args };
        }
    }
    let text: String = blocks
        .iter()
        .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
        .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
        .collect::<Vec<_>>()
        .join("");
    ProviderAction::Text(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[test]
    fn translate_extracts_first_system_only_and_alternates_roles() {
        let msgs = vec![
            json!({"role": "system", "content": "SYS-A"}),
            json!({"role": "user", "content": "do it"}),
            json!({"role": "assistant", "tool_calls": [{"id": "toolu_1", "type": "function",
                "function": {"name": "fetch", "arguments": "{\"url\":\"http://x\"}"}}]}),
            json!({"role": "tool", "tool_call_id": "toolu_1", "content": "RESULT"}),
            // a SECOND system message must be ignored (injection guard)
            json!({"role": "system", "content": "SYS-B-INJECTED"}),
        ];
        let (system, out) = translate(&msgs);
        assert_eq!(system.as_deref(), Some("SYS-A"));
        // roles alternate user, assistant, user
        let roles: Vec<&str> = out.iter().map(|m| m["role"].as_str().unwrap()).collect();
        assert_eq!(roles, vec!["user", "assistant", "user"]);
        // assistant block is a tool_use with the arguments JSON-string parsed to an object
        assert_eq!(out[1]["content"][0]["type"], "tool_use");
        assert_eq!(out[1]["content"][0]["id"], "toolu_1");
        assert_eq!(out[1]["content"][0]["input"]["url"], "http://x");
        // the tool result is the FIRST block of the following user message, keyed tool_use_id
        assert_eq!(out[2]["content"][0]["type"], "tool_result");
        assert_eq!(out[2]["content"][0]["tool_use_id"], "toolu_1");
        assert_eq!(out[2]["content"][0]["content"], "RESULT");
    }

    #[test]
    fn translate_merges_consecutive_same_role_messages() {
        // Two user messages in a row must merge into one (no adjacent same-role messages).
        let msgs = vec![
            json!({"role": "user", "content": "one"}),
            json!({"role": "user", "content": "two"}),
        ];
        let (_s, out) = translate(&msgs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["content"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn translate_falls_back_to_empty_object_on_malformed_arguments() {
        let msgs = vec![
            json!({"role": "assistant", "tool_calls": [{"id": "t", "type": "function",
            "function": {"name": "fetch", "arguments": "NOT JSON"}}]}),
        ];
        let (_s, out) = translate(&msgs);
        assert_eq!(out[0]["content"][0]["input"], json!({}));
    }

    #[test]
    fn build_body_uses_input_schema_and_omits_empty_tools() {
        let a = Anthropic::new("claude".into(), None);
        // empty tools → no `tools` key
        let b0 = a.build_body(&[json!({"role": "user", "content": "hi"})], &[]);
        assert!(b0.get("tools").is_none());
        assert_eq!(b0["max_tokens"], MAX_TOKENS_PER_CALL);
        // a tool serializes its schema under `input_schema`, NOT `parameters`
        let b1 = a.build_body(
            &[json!({"role": "user", "content": "hi"})],
            &[ToolSpec {
                name: "fetch".into(),
                description: "get".into(),
                parameters: json!({"type": "object", "properties": {"url": {"type": "string"}}}),
            }],
        );
        assert!(b1["tools"][0].get("input_schema").is_some());
        assert!(b1["tools"][0].get("parameters").is_none());
        assert_eq!(b1["tools"][0]["name"], "fetch");
    }

    #[test]
    fn parse_action_prefers_the_first_tool_use_on_mixed_content() {
        let v = json!({"content": [
            {"type": "text", "text": "let me check"},
            {"type": "tool_use", "id": "toolu_9", "name": "fetch", "input": {"url": "http://y"}}
        ], "stop_reason": "tool_use"});
        match parse_action(&v) {
            ProviderAction::ToolCall { id, name, args } => {
                assert_eq!(id, "toolu_9");
                assert_eq!(name, "fetch");
                assert_eq!(args["url"], "http://y");
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }

    #[test]
    fn parse_action_concatenates_text_blocks_and_handles_empty() {
        let v = json!({"content": [{"type": "text", "text": "hello "}, {"type": "text", "text": "world"}]});
        assert!(matches!(parse_action(&v), ProviderAction::Text(t) if t == "hello world"));
        assert!(matches!(parse_action(&json!({})), ProviderAction::Text(t) if t.is_empty()));
    }

    #[tokio::test]
    async fn act_round_trips_a_tool_use_response_and_sends_the_required_headers() {
        let server = wiremock::MockServer::start().await;
        let body = json!({"id": "msg_1", "type": "message", "role": "assistant",
            "content": [{"type": "tool_use", "id": "toolu_1", "name": "fetch",
                "input": {"url": "http://example.com"}}],
            "stop_reason": "tool_use"});
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/messages"))
            .and(wiremock::matchers::header("x-api-key", "sekret"))
            .and(wiremock::matchers::header(
                "anthropic-version",
                ANTHROPIC_API_VERSION,
            ))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;

        let a = Anthropic {
            base_url: server.uri(),
            model: "claude".into(),
            api_key: Some("sekret".into()),
        };
        let action = a
            .act(
                vec![json!({"role": "user", "content": "go"})],
                &[ToolSpec {
                    name: "fetch".into(),
                    description: String::new(),
                    parameters: json!({}),
                }],
            )
            .await
            .unwrap();
        match action {
            ProviderAction::ToolCall { name, args, .. } => {
                assert_eq!(name, "fetch");
                assert_eq!(args["url"], "http://example.com");
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn act_returns_text_on_an_end_turn_response() {
        let server = wiremock::MockServer::start().await;
        let body =
            json!({"content": [{"type": "text", "text": "the answer"}], "stop_reason": "end_turn"});
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/messages"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
        let a = Anthropic {
            base_url: server.uri(),
            model: "claude".into(),
            api_key: None,
        };
        let action = a
            .act(vec![json!({"role": "user", "content": "hi"})], &[])
            .await
            .unwrap();
        assert!(matches!(action, ProviderAction::Text(t) if t == "the answer"));
    }

    #[tokio::test]
    async fn chat_yields_the_reply_as_a_single_chunk() {
        let server = wiremock::MockServer::start().await;
        let body = json!({"content": [{"type": "text", "text": "streamed-as-one"}]});
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/v1/messages"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
        let a = Anthropic {
            base_url: server.uri(),
            model: "claude".into(),
            api_key: None,
        };
        let mut s = a
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
        assert_eq!(out, "streamed-as-one");
    }
}
