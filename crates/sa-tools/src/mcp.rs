//! MCP (Model Context Protocol) client. Connects to a configured MCP server over stdio
//! (newline-delimited JSON-RPC 2.0), loads its tools NAMESPACED + ALLOW-LISTED, and wraps
//! each as a `Tool`. Remote tools are untrusted: the agent loop taints their output. The
//! server is a separate process — its OWN egress is NOT governed by our Policy (honest gap).
use anyhow::{bail, Result};
use serde_json::{json, Value};

/// MCP protocol version we negotiate. (MCP 2025-06-18.)
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// A tool advertised by an MCP server (the subset we use).
#[derive(Debug, Clone)]
pub struct McpToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// The namespaced registry key for a remote tool. The `server::` prefix is a security
/// boundary: first-party tool names contain no `::`, so a remote tool can never shadow one.
pub fn namespaced(server: &str, tool: &str) -> String {
    format!("{server}::{tool}")
}

/// Parse a `tools/list` result into tool defs, tolerating missing description/schema.
pub fn parse_tools_list(result: &Value) -> Vec<McpToolDef> {
    result
        .get("tools")
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|t| {
                    let name = t.get("name").and_then(|n| n.as_str())?.to_string();
                    Some(McpToolDef {
                        name,
                        description: t
                            .get("description")
                            .and_then(|d| d.as_str())
                            .unwrap_or("")
                            .to_string(),
                        input_schema: t
                            .get("inputSchema")
                            .cloned()
                            .unwrap_or_else(|| json!({"type": "object"})),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Default-deny exact-name allow-list. Only tools whose name is in `allow` survive.
pub fn filter_allowed(defs: Vec<McpToolDef>, allow: &[String]) -> Vec<McpToolDef> {
    defs.into_iter()
        .filter(|d| allow.iter().any(|a| a == &d.name))
        .collect()
}

/// Extract a `tools/call` result's text. `isError:true` (a tool-level error) and a
/// JSON-RPC `error` object both become Err — the caller renders them as tool errors.
pub fn parse_call_result(response: &Value) -> Result<String> {
    if let Some(e) = response.get("error") {
        bail!(
            "mcp error: {}",
            e.get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown")
        );
    }
    let text = response
        .get("content")
        .and_then(|c| c.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();
    if response
        .get("isError")
        .and_then(|e| e.as_bool())
        .unwrap_or(false)
    {
        bail!("mcp tool error: {text}");
    }
    Ok(text)
}

/// Build a JSON-RPC 2.0 request.
pub fn req(id: u64, method: &str, params: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
}

/// The `notifications/initialized` notification (no id → no response expected).
pub fn initialized_notification() -> Value {
    json!({"jsonrpc":"2.0","method":"notifications/initialized"})
}

use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

/// A hung server must not hang the agent; a flooding server must not OOM it. These bound
/// every request. ponytail: fixed values — make them per-server config only if a real
/// server needs longer than 30s or lines bigger than 8 MiB.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;

/// A JSON-RPC-over-stdio MCP client, generic over its streams so it tests in-memory.
pub struct McpClient<R, W> {
    reader: BufReader<R>,
    writer: W,
    next_id: u64,
}

impl<R: AsyncRead + Unpin, W: AsyncWrite + Unpin> McpClient<R, W> {
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            reader: BufReader::new(reader),
            writer,
            next_id: 1,
        }
    }

    async fn write_message(&mut self, msg: &Value) -> Result<()> {
        let line = format!("{}\n", serde_json::to_string(msg)?);
        self.writer.write_all(line.as_bytes()).await?;
        self.writer.flush().await?;
        Ok(())
    }

    /// Read one newline-delimited message, byte-capped at MAX_LINE_BYTES (hard OOM guard).
    async fn read_message(&mut self) -> Result<Option<Value>> {
        let mut buf: Vec<u8> = Vec::new();
        loop {
            let mut byte = [0u8; 1];
            let n = self.reader.read(&mut byte).await?;
            if n == 0 {
                return Ok(None); // server closed
            }
            if byte[0] == b'\n' {
                break;
            }
            buf.push(byte[0]);
            if buf.len() > MAX_LINE_BYTES {
                bail!("mcp: server response line exceeds {MAX_LINE_BYTES} bytes");
            }
        }
        let line = String::from_utf8_lossy(&buf);
        if line.trim().is_empty() {
            return Ok(Some(Value::Null));
        }
        Ok(Some(serde_json::from_str(line.trim())?))
    }

    /// Send a request and read responses (skipping unrelated notifications) until the one
    /// whose id matches; bounded by REQUEST_TIMEOUT so a silent server can't hang us.
    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.write_message(&req(id, method, params)).await?;
        let fut = async {
            loop {
                match self.read_message().await? {
                    None => bail!("mcp: server closed the connection during {method}"),
                    Some(Value::Null) => continue,
                    Some(v) => {
                        if v.get("id").and_then(|i| i.as_u64()) == Some(id) {
                            if let Some(e) = v.get("error") {
                                bail!(
                                    "mcp error in {method}: {}",
                                    e.get("message")
                                        .and_then(|m| m.as_str())
                                        .unwrap_or("unknown")
                                );
                            }
                            return Ok(v.get("result").cloned().unwrap_or(Value::Null));
                        }
                        // a notification or a stray id → ignore and keep reading
                    }
                }
            }
        };
        tokio::time::timeout(REQUEST_TIMEOUT, fut)
            .await
            .map_err(|_| anyhow::anyhow!("mcp: {method} timed out after {REQUEST_TIMEOUT:?}"))?
    }

    /// Handshake: initialize, then send the `initialized` notification.
    pub async fn initialize(&mut self) -> Result<()> {
        let params = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "secretagent", "version": env!("CARGO_PKG_VERSION")}
        });
        self.request("initialize", params).await?;
        self.write_message(&initialized_notification()).await?;
        Ok(())
    }

    pub async fn list_tools(&mut self) -> Result<Vec<McpToolDef>> {
        let result = self.request("tools/list", json!({})).await?;
        Ok(parse_tools_list(&result))
    }

    pub async fn call_tool(&mut self, name: &str, arguments: Value) -> Result<String> {
        let result = self
            .request("tools/call", json!({"name": name, "arguments": arguments}))
            .await?;
        parse_call_result(&result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn namespacing_prevents_shadowing_first_party_tools() {
        // A malicious server advertising "execute_code" must NOT collide with the real one.
        let ns = namespaced("evil", "execute_code");
        assert_eq!(ns, "evil::execute_code");
        assert!(
            ns.contains("::"),
            "namespaced names carry the server prefix"
        );
        // first-party names never contain "::", so the keyspaces are disjoint.
        for builtin in ["fetch", "read_file", "write_file", "execute_code"] {
            assert!(!builtin.contains("::"));
            assert_ne!(ns, builtin);
        }
    }

    #[test]
    fn allow_list_is_default_deny_and_exact_match() {
        let defs = vec![
            McpToolDef {
                name: "search".into(),
                description: "".into(),
                input_schema: json!({}),
            },
            McpToolDef {
                name: "delete_everything".into(),
                description: "".into(),
                input_schema: json!({}),
            },
        ];
        // empty allow-list → nothing loads
        assert!(filter_allowed(defs.clone(), &[]).is_empty());
        // only the exact allow-listed name loads; the dangerous one is dropped
        let allowed = filter_allowed(defs, &["search".to_string()]);
        assert_eq!(allowed.len(), 1);
        assert_eq!(allowed[0].name, "search");
    }

    #[test]
    fn parses_a_tools_list_response() {
        let resp = json!({"tools":[
            {"name":"search","description":"find","inputSchema":{"type":"object"}},
            {"name":"noschema"}
        ]});
        let defs = parse_tools_list(&resp);
        assert_eq!(defs.len(), 2);
        assert_eq!(defs[0].name, "search");
        assert_eq!(defs[0].description, "find");
        assert_eq!(defs[1].name, "noschema"); // missing fields tolerated
    }

    #[test]
    fn parses_a_tool_call_result_and_surfaces_errors() {
        let ok = json!({"content":[{"type":"text","text":"hello"},{"type":"text","text":" world"}],"isError":false});
        assert_eq!(parse_call_result(&ok).unwrap(), "hello world");
        let err = json!({"content":[{"type":"text","text":"boom"}],"isError":true});
        assert!(parse_call_result(&err)
            .unwrap_err()
            .to_string()
            .contains("boom"));
        // a JSON-RPC error object (passed as the whole response) is surfaced too
        let rpcerr = json!({"error":{"code":-32602,"message":"bad args"}});
        assert!(parse_call_result(&rpcerr)
            .unwrap_err()
            .to_string()
            .contains("bad args"));
    }

    use tokio::io::AsyncBufReadExt;

    // A minimal in-memory MCP server: reads newline JSON-RPC requests, writes canned
    // responses. Exercises the real client framing without a subprocess.
    async fn mock_server(io: tokio::io::DuplexStream) {
        let (r, mut w) = tokio::io::split(io);
        let mut lines = tokio::io::BufReader::new(r).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let v: serde_json::Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let id = v.get("id").cloned();
            let method = v.get("method").and_then(|m| m.as_str()).unwrap_or("");
            if id.is_none() {
                continue; // a notification (e.g. initialized) → no reply
            }
            let result = match method {
                "initialize" => {
                    json!({"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"mock","version":"0"}})
                }
                "tools/list" => {
                    json!({"tools":[{"name":"echo","description":"echoes","inputSchema":{"type":"object"}}]})
                }
                "tools/call" => {
                    let args = v
                        .get("params")
                        .and_then(|p| p.get("arguments"))
                        .cloned()
                        .unwrap_or(json!({}));
                    let msg = args.get("msg").and_then(|m| m.as_str()).unwrap_or("");
                    json!({"content":[{"type":"text","text":format!("echoed:{msg}")}],"isError":false})
                }
                _ => json!({}),
            };
            let resp = json!({"jsonrpc":"2.0","id":id,"result":result});
            let _ = w.write_all(format!("{resp}\n").as_bytes()).await;
            let _ = w.flush().await;
        }
    }

    #[tokio::test]
    async fn client_handshakes_lists_and_calls_over_a_mock_server() {
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        tokio::spawn(mock_server(server_io));
        let (r, w) = tokio::io::split(client_io);
        let mut client = McpClient::new(r, w);

        client.initialize().await.unwrap();
        let tools = client.list_tools().await.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "echo");
        let out = client
            .call_tool("echo", json!({"msg": "hi"}))
            .await
            .unwrap();
        assert_eq!(out, "echoed:hi");
    }
}
