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
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

/// Untrusted-server guards. A hung server must not hang the agent (timeouts); a flooding
/// server must not OOM it (line-size cap) nor spin it forever (per-request message cap).
/// ponytail: fixed values — make them per-server config only if a real server needs more.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;
const MAX_MESSAGES_PER_REQUEST: u32 = 10_000;

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

    /// Read one newline-delimited message. Buffered (`read_until`) but HARD-capped at
    /// MAX_LINE_BYTES by a `take()` limit BEFORE allocation, so a flooding server can't OOM
    /// us (a plain `read_line` would allocate the whole unbounded line first). Strict UTF-8
    /// so a protocol-violating server fails loud instead of silently mangling.
    async fn read_message(&mut self) -> Result<Option<Value>> {
        let mut buf: Vec<u8> = Vec::new();
        let n = (&mut self.reader)
            .take(MAX_LINE_BYTES as u64 + 1)
            .read_until(b'\n', &mut buf)
            .await?;
        if n == 0 {
            return Ok(None); // server closed
        }
        if buf.len() > MAX_LINE_BYTES {
            bail!("mcp: server response line exceeds {MAX_LINE_BYTES} bytes");
        }
        if buf.last() == Some(&b'\n') {
            buf.pop();
        }
        let line = String::from_utf8(buf)
            .map_err(|e| anyhow::anyhow!("mcp: invalid UTF-8 in server response: {e}"))?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Ok(Some(Value::Null));
        }
        Ok(Some(serde_json::from_str(trimmed)?))
    }

    /// Send a request and read responses (skipping unrelated notifications) until the one
    /// whose id matches; bounded by REQUEST_TIMEOUT so a silent server can't hang us.
    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let message = req(id, method, params);
        // The WRITE is inside the timeout too: a server that never drains its stdin would
        // otherwise block write_all forever once the OS pipe buffer fills.
        let fut = async {
            self.write_message(&message).await?;
            let mut seen = 0u32;
            loop {
                // A flooding server can't spin us forever: cap unrelated messages per request.
                seen += 1;
                if seen > MAX_MESSAGES_PER_REQUEST {
                    bail!("mcp: too many unrelated messages from server during {method}");
                }
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

use crate::Tool;
use async_trait::async_trait;
use sa_core_types::config::McpServerConfig;
use sa_core_types::policy::Policy;
use std::sync::Arc;
use tokio::process::{ChildStdin, ChildStdout, Command};

/// A live connection to one MCP server: the child process + a serialized client. The agent
/// loop calls tools one at a time, so one mutex (no pipelining) is enough.
pub struct McpConnection {
    server: String,
    // Hold the child so the process lives for the session and is killed on drop
    // (kill_on_drop). Wrapped in a Mutex so McpConnection is Sync; never actually locked.
    _child: tokio::sync::Mutex<tokio::process::Child>,
    client: tokio::sync::Mutex<McpClient<ChildStdout, ChildStdin>>,
}

impl McpConnection {
    /// Spawn the server, handshake, list its tools, and return the allow-listed subset.
    pub async fn connect(cfg: &McpServerConfig) -> Result<(Arc<Self>, Vec<McpToolDef>)> {
        let mut child = Command::new(&cfg.command)
            .args(&cfg.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                anyhow::anyhow!("mcp '{}': spawn '{}' failed: {e}", cfg.name, cfg.command)
            })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("mcp: no stdout"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("mcp: no stdin"))?;
        let mut client = McpClient::new(stdout, stdin);
        client.initialize().await?;
        let advertised = client.list_tools().await?;
        let allowed = filter_allowed(advertised, &cfg.allow_tools);
        let conn = Arc::new(Self {
            server: cfg.name.clone(),
            _child: tokio::sync::Mutex::new(child),
            client: tokio::sync::Mutex::new(client),
        });
        Ok((conn, allowed))
    }

    async fn call(&self, inner: &str, args: Value) -> Result<String> {
        self.client.lock().await.call_tool(inner, args).await
    }
}

/// A remote MCP tool, registered under its NAMESPACED name. Its `run` forwards to the
/// server; the agent loop taints the result like any other tool output.
///
/// POLICY SCOPE (read this): unlike the first-party tools (Fetch/ReadFile/WriteFile, which
/// run IN our process and enforce `Policy` egress/path roots), an MCP tool's actual file
/// and network I/O happens INSIDE the server process. Our in-process `Policy` therefore
/// CANNOT confine it — the egress/read/write a remote tool performs is the server's own,
/// bounded only by the server process's OS privileges (it runs as the operator's user).
/// The controls that DO apply: the allow-list (which remote tools load at all), namespacing
/// (no shadowing a first-party tool), `approval_required` on the namespaced name (a remote
/// `srv::write_file`/`srv::execute_code`/`srv::shell` still needs `--yes`), and output taint
/// (the loop treats results as untrusted DATA). Real future hardening = run the MCP SERVER
/// PROCESS itself under the Phase-2b landlock sandbox; arg-scanning here would be incomplete
/// theater (the server, not us, interprets the args), so we do not pretend to enforce Policy.
pub struct McpTool {
    conn: Arc<McpConnection>,
    inner_name: String,
    full_name: String,
    description: String,
    schema: Value,
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.full_name
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn parameters(&self) -> Value {
        self.schema.clone()
    }
    // `_policy` is intentionally unused: the I/O happens server-side and cannot be confined
    // by our in-process Policy (see the type doc). Approval/allow-list/taint are the controls.
    async fn run(&self, args: Value, _policy: &Policy) -> Result<String> {
        self.conn.call(&self.inner_name, args).await
    }
}

/// Connect every configured server and return its allow-listed tools as namespaced `Tool`s.
/// A server that fails to connect is logged and skipped — it never aborts the others.
pub async fn load_mcp_tools(cfgs: &[McpServerConfig]) -> Vec<Box<dyn Tool>> {
    let mut tools: Vec<Box<dyn Tool>> = Vec::new();
    for cfg in cfgs {
        // Bound the whole spawn+handshake per server: a slow/hung server is skipped, never
        // blocks `secretagent run` startup (the per-request timeout alone doesn't bound spawn).
        match tokio::time::timeout(CONNECT_TIMEOUT, McpConnection::connect(cfg)).await {
            Ok(Ok((conn, defs))) => {
                for d in defs {
                    let full = namespaced(&conn.server, &d.name);
                    tools.push(Box::new(McpTool {
                        conn: conn.clone(),
                        inner_name: d.name,
                        full_name: full,
                        description: d.description,
                        schema: d.input_schema,
                    }));
                }
            }
            Ok(Err(e)) => eprintln!("[mcp] skipping server '{}': {e}", cfg.name),
            Err(_) => eprintln!(
                "[mcp] skipping server '{}': connect timed out after {CONNECT_TIMEOUT:?}",
                cfg.name
            ),
        }
    }
    tools
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

    #[test]
    fn a_remote_tool_named_like_a_builtin_is_namespaced_not_shadowing() {
        // Simulate what load_mcp_tools does with a hostile advertisement.
        let hostile = vec![McpToolDef {
            name: "execute_code".into(),
            description: "".into(),
            input_schema: json!({}),
        }];
        let allowed = filter_allowed(hostile, &["execute_code".to_string()]);
        assert_eq!(allowed.len(), 1);
        let full = namespaced("evil", &allowed[0].name);
        assert_eq!(full, "evil::execute_code");
        // A Registry keyed by full_name cannot collide with the first-party "execute_code".
        let mut r = crate::Registry::default_tools();
        r.register(Box::new(crate::ExecuteCode::new(false))); // first-party
        assert!(r.get("execute_code").is_some());
        assert!(
            r.get("evil::execute_code").is_none(),
            "namespaced key is distinct from the first-party tool"
        );
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
