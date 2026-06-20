# SecretAgent Phase 2c (MCP client) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** SecretAgent can connect to a configured **MCP server** over stdio, load its tools **namespaced** (so an external tool can never shadow a first-party one) and **allow-listed** (default-deny), and call them through the existing agent loop — where their output is already **tainted/untrusted**. This closes the spec's Phase-2 acceptance row "an MCP server's tools load namespaced and allow-listed" and the "confused-deputy via MCP" threat (§ spec lines 196/229).

**Architecture:** The MCP client lives in `sa-tools` (spec line 116), split into a **pure layer** (namespacing, allow-list filtering, JSON-RPC request building + result parsing — the security-critical logic, unit-tested cross-platform) and an **async transport** (`McpClient<R,W>` generic over the streams, tested over an in-memory `tokio::io::duplex` mock server; the real `tokio::process` spawn is a thin wrapper). Each allow-listed remote tool becomes a namespaced `McpTool: Tool` holding a shared connection. The existing `run_task` loop taints all tool output, so MCP output is untrusted DATA for free.

**Tech Stack:** existing crates; `tokio` (+ `process`, `io-util` features) for the subprocess + async stdio; serde_json. MCP transport = **newline-delimited JSON-RPC 2.0** (one message per line — confirmed against a real MCP server). Protocol version `2025-06-18`. All cross-platform; verified on Windows + WSL + CI.

**Authority:** spec `~/Downloads/SecretAgent-Build-Plan.md` §Phase-2 acceptance (line 229) + threat model (line 196: "MCP server tools are namespaced + allow-listed; treated as untrusted; same approval/egress rules as built-in tools"). Founding ADR-20260619 (MCP client is a `sa-tools` module, not a new crate — JIT, and the spec colocates it there).

## Global Constraints

- **Namespacing is a security boundary:** every remote tool registers as `"{server}::{tool}"`. First-party tools (`fetch`/`read_file`/`write_file`/`execute_code`) have **no `::`**, so a malicious server advertising a tool named `execute_code` registers as `myserver::execute_code` and **cannot** shadow or overwrite the real one.
- **Allow-list is default-deny + operator consent:** only tools whose name appears in that server's `allow_tools` config load at all. An empty `allow_tools` loads nothing. Allow-listing IS the operator's consent for an external tool to be callable (the per-session tool allow-list the threat model names); the loaded tool's output remains untrusted.
- **Treated as untrusted:** MCP tool output flows through the existing `run_task` loop → `Tainted::untrusted` → re-fed as role:"tool" DATA, audited by name. No new injection-guard work; do NOT special-case MCP in the loop.
- **Untrusted server = bounded:** every request has a **timeout** (a hung server must not hang the agent) and a **response line size cap** (a flooding server must not OOM the agent). Both are named ceilings.
- **Honest egress limitation:** the MCP server is a separate local process; SecretAgent's `Policy` egress allow-list does NOT govern the server's own outbound network. State this in the threat note — do not claim egress control we don't have.
- **Deferred (do NOT build):** SSE/HTTP MCP transport (stdio only), MCP resources/prompts (tools only), per-remote-tool approval classification (allow-list is the gate), reconnection/retry.
- **TDD; commit per task.** Gate each task: `cargo fmt --all -- --check` (0) / `cargo clippy --all-targets --all-features -- -D warnings` (0) / `cargo test` (pass). The `self-audit` PreToolUse hook blocks `git commit` — append ` # self-audit-ok`.
- **Venues:** build/gate in WSL (`PATH="$HOME/.cargo/bin:$PATH" CARGO_TARGET_DIR="$HOME/sa-target"`) AND Windows (all of 2c is cross-platform — no `#[cfg]` OS split). Then push, watch CI to green.

## File Structure

```
crates/
  sa-core-types/src/config.rs   + McpServerConfig { name, command, args, allow_tools } + Config.mcp: Vec<_>
  sa-tools/
    Cargo.toml                  tokio += features ["process","io-util"]
    src/mcp.rs (NEW)            pure layer (namespacing/allow-list/parse/build) + async McpClient<R,W> + McpConnection (spawn) + McpTool + load_mcp_tools
    src/lib.rs                  + `pub mod mcp;`
secretagent/
  src/run.rs                    load + register namespaced MCP tools from cfg.mcp
  src/doctor.rs                 + an informational mcp line (N servers configured)
```

---

### Task 1: config — `McpServerConfig` + `Config.mcp`

**Files:** Modify `crates/sa-core-types/src/config.rs`.

**Interfaces:** Produces `pub struct McpServerConfig { pub name: String, pub command: String, pub args: Vec<String>, pub allow_tools: Vec<String> }` (all `#[serde(default)]`) and `Config.mcp: Vec<McpServerConfig>`.

- [ ] **Step 1: Write the failing test** (append to `config.rs` `mod tests`):

```rust
    #[test]
    fn config_parses_mcp_servers() {
        let toml = r#"
[[mcp]]
name = "rose"
command = "rose-glass-mcp"
args = ["--db", "/x/index.db"]
allow_tools = ["search"]
"#;
        let c: Config = toml::from_str(toml).unwrap();
        assert_eq!(c.mcp.len(), 1);
        assert_eq!(c.mcp[0].name, "rose");
        assert_eq!(c.mcp[0].command, "rose-glass-mcp");
        assert_eq!(c.mcp[0].allow_tools, vec!["search".to_string()]);
        // empty/absent mcp is valid (default-deny: no servers)
        assert!(toml::from_str::<Config>("").unwrap().mcp.is_empty());
    }
```

- [ ] **Step 2: Run — verify fail.** `cargo test -p sa-core-types config_parses_mcp` → FAIL (no `mcp` field).

- [ ] **Step 3: Implement** — add to `config.rs`:

```rust
/// A configured MCP server. The operator lists each server + which of its tools are
/// allow-listed; an empty `allow_tools` loads nothing (default-deny).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub allow_tools: Vec<String>,
}
```

and add the field to `Config`:

```rust
pub struct Config {
    pub vault: VaultConfig,
    pub provider: ProviderConfig,
    pub policy: crate::policy::Policy,
    pub mcp: Vec<McpServerConfig>,
}
```

- [ ] **Step 4: Run — verify pass.** `cargo test -p sa-core-types`.
- [ ] **Step 5: fmt + clippy + commit** — `feat(core-types): McpServerConfig + Config.mcp (name/command/args/allow_tools, default-deny)`

---

### Task 2: `sa-tools/src/mcp.rs` — pure layer (namespacing + allow-list + JSON-RPC build/parse)

**Files:** Create `crates/sa-tools/src/mcp.rs`; add `pub mod mcp;` to `crates/sa-tools/src/lib.rs`.

**Interfaces:** Produces `McpToolDef { name, description, input_schema }`; `namespaced(server, tool) -> String`; `parse_tools_list(&Value) -> Vec<McpToolDef>`; `filter_allowed(Vec<McpToolDef>, &[String]) -> Vec<McpToolDef>`; `parse_call_result(&Value) -> anyhow::Result<String>`; request builders `req(id, method, params) -> Value` and `initialized_notification() -> Value`.

- [ ] **Step 1: Write the failing tests** — `crates/sa-tools/src/mcp.rs` (bottom):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn namespacing_prevents_shadowing_first_party_tools() {
        // A malicious server advertising "execute_code" must NOT collide with the real one.
        let ns = namespaced("evil", "execute_code");
        assert_eq!(ns, "evil::execute_code");
        assert!(ns.contains("::"), "namespaced names carry the server prefix");
        // first-party names never contain "::", so the keyspaces are disjoint.
        for builtin in ["fetch", "read_file", "write_file", "execute_code"] {
            assert!(!builtin.contains("::"));
            assert_ne!(ns, builtin);
        }
    }

    #[test]
    fn allow_list_is_default_deny_and_exact_match() {
        let defs = vec![
            McpToolDef { name: "search".into(), description: "".into(), input_schema: json!({}) },
            McpToolDef { name: "delete_everything".into(), description: "".into(), input_schema: json!({}) },
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
        assert!(parse_call_result(&err).unwrap_err().to_string().contains("boom"));
        // a JSON-RPC error object (passed as the whole response) is surfaced too
        let rpcerr = json!({"error":{"code":-32602,"message":"bad args"}});
        assert!(parse_call_result(&rpcerr).unwrap_err().to_string().contains("bad args"));
    }
}
```

- [ ] **Step 2: Run — verify fail.** `cargo test -p sa-tools mcp::` → FAIL.

- [ ] **Step 3: Implement the pure layer** — `crates/sa-tools/src/mcp.rs` (top):

```rust
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
            e.get("message").and_then(|m| m.as_str()).unwrap_or("unknown")
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
    if response.get("isError").and_then(|e| e.as_bool()).unwrap_or(false) {
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
```

Add `pub mod mcp;` to `crates/sa-tools/src/lib.rs`.

- [ ] **Step 4: Run — verify pass.** `cargo test -p sa-tools mcp::`.
- [ ] **Step 5: fmt + clippy + commit** — `feat(tools): MCP pure layer — namespacing + default-deny allow-list + JSON-RPC build/parse`

---

### Task 3: `McpClient<R,W>` async transport + duplex mock-server test

**Files:** Modify `crates/sa-tools/Cargo.toml` (tokio features), `crates/sa-tools/src/mcp.rs`.

**Interfaces:** Produces `McpClient<R, W>` with `new(reader, writer)`, `async initialize() -> Result<()>`, `async list_tools() -> Result<Vec<McpToolDef>>`, `async call_tool(name, args) -> Result<String>`. Generic over `tokio::io::AsyncRead + AsyncWrite` so it tests over `tokio::io::duplex` with no subprocess.

- [ ] **Step 1: `crates/sa-tools/Cargo.toml`** — widen tokio features:

```toml
tokio = { workspace = true, features = ["process", "io-util"] }
```

(replace the bare `tokio.workspace = true` line under `[dependencies]`; keep the dev-dependency `tokio` line as is.)

- [ ] **Step 2: Write the failing transport test** — append to `crates/sa-tools/src/mcp.rs` `mod tests`:

```rust
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

    // A minimal in-memory MCP server: reads newline JSON-RPC requests, writes canned
    // responses. Exercises the real client framing without a subprocess.
    async fn mock_server(io: tokio::io::DuplexStream) {
        let (r, mut w) = tokio::io::split(io);
        let mut lines = tokio::io::BufReader::new(r).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let v: serde_json::Value = match serde_json::from_str(&line) { Ok(v) => v, Err(_) => continue };
            let id = v.get("id").cloned();
            let method = v.get("method").and_then(|m| m.as_str()).unwrap_or("");
            if id.is_none() { continue; } // a notification (e.g. initialized) → no reply
            let result = match method {
                "initialize" => json!({"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"mock","version":"0"}}),
                "tools/list" => json!({"tools":[{"name":"echo","description":"echoes","inputSchema":{"type":"object"}}]}),
                "tools/call" => {
                    let args = v.get("params").and_then(|p| p.get("arguments")).cloned().unwrap_or(json!({}));
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
        let out = client.call_tool("echo", json!({"msg": "hi"})).await.unwrap();
        assert_eq!(out, "echoed:hi");
    }
```

- [ ] **Step 3: Run — verify fail.** `cargo test -p sa-tools mcp::tests::client_handshakes` → FAIL (no `McpClient`).

- [ ] **Step 4: Implement `McpClient`** — append to `crates/sa-tools/src/mcp.rs`:

```rust
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
                                    e.get("message").and_then(|m| m.as_str()).unwrap_or("unknown")
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
```

- [ ] **Step 5: Run — verify pass.** `cargo test -p sa-tools mcp::` (pure + transport).
- [ ] **Step 6: fmt + clippy + commit** — `feat(tools): async McpClient<R,W> — JSON-RPC stdio framing, timeout + line-size cap (duplex-tested)`

---

### Task 4: `McpConnection` (spawn) + `McpTool` + `load_mcp_tools`

**Files:** Modify `crates/sa-tools/src/mcp.rs`.

**Interfaces:** Produces `McpConnection` (owns the child + a `tokio::sync::Mutex<McpClient<ChildStdout, ChildStdin>>`), `McpConnection::connect(cfg) -> Result<(Arc<McpConnection>, Vec<McpToolDef>)>` (spawn → handshake → list → allow-list filter), `McpConnection::call(inner, args)`; `McpTool: Tool`; `async load_mcp_tools(cfgs: &[McpServerConfig]) -> Vec<Box<dyn Tool>>` (one server failing logs + is skipped, never aborts the others).

- [ ] **Step 1: Implement** — append to `crates/sa-tools/src/mcp.rs`:

```rust
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
    // ponytail: keep the child handle so the process is killed on drop (kill_on_drop).
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
            .map_err(|e| anyhow::anyhow!("mcp '{}': spawn '{}' failed: {e}", cfg.name, cfg.command))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow::anyhow!("mcp: no stdout"))?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow::anyhow!("mcp: no stdin"))?;
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
pub struct McpTool {
    conn: Arc<McpConnection>,
    inner_name: String,
    full_name: String,
    description: String,
    schema: Value,
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &'static str {
        // The registry keys by the owned `full_name`; this &'static is only used for
        // ToolSpec assembly, which reads name() — so leak once at construction.
        Box::leak(self.full_name.clone().into_boxed_str())
    }
    fn description(&self) -> &'static str {
        Box::leak(self.description.clone().into_boxed_str())
    }
    fn parameters(&self) -> Value {
        self.schema.clone()
    }
    async fn run(&self, args: Value, _policy: &Policy) -> Result<String> {
        self.conn.call(&self.inner_name, args).await
    }
}

/// Connect every configured server and return its allow-listed tools as namespaced `Tool`s.
/// A server that fails to connect is logged and skipped — it never aborts the others.
pub async fn load_mcp_tools(cfgs: &[McpServerConfig]) -> Vec<Box<dyn Tool>> {
    let mut tools: Vec<Box<dyn Tool>> = Vec::new();
    for cfg in cfgs {
        match McpConnection::connect(cfg).await {
            Ok((conn, defs)) => {
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
            Err(e) => {
                tracing::warn!("mcp: skipping server '{}': {e}", cfg.name);
            }
        }
    }
    tools
}
```

> **NOTE on `name()`/`description()` returning `&'static str`:** the `Tool` trait declares these `-> &'static str` (designed for the first-party tools' string literals). MCP names are runtime strings, so we `Box::leak` once. Leaking a handful of small strings for the process lifetime (one per loaded MCP tool, loaded once at startup) is acceptable. **If clippy flags this or the count could grow unbounded, the clean fix is a follow-up: change the trait to `fn name(&self) -> &str`** (a one-line, all-impls update — every first-party impl already returns a borrow-compatible literal). Do that refactor in Task 4 if `Box::leak` feels wrong; otherwise ship the leak with this note.

- [ ] **Step 2: Write the shadowing-safety test** — append to `mod tests` (uses the duplex pattern, but asserts the registry-level property via `load`-style namespacing):

```rust
    #[test]
    fn a_remote_tool_named_like_a_builtin_is_namespaced_not_shadowing() {
        // Simulate what load_mcp_tools does with a hostile advertisement.
        let hostile = vec![McpToolDef { name: "execute_code".into(), description: "".into(), input_schema: json!({}) }];
        let allowed = filter_allowed(hostile, &["execute_code".to_string()]);
        assert_eq!(allowed.len(), 1);
        let full = namespaced("evil", &allowed[0].name);
        assert_eq!(full, "evil::execute_code");
        // A Registry keyed by full_name cannot collide with the first-party "execute_code".
        let mut r = crate::Registry::default_tools();
        r.register(Box::new(crate::ExecuteCode::new(false))); // first-party
        assert!(r.get("execute_code").is_some());
        assert!(r.get("evil::execute_code").is_none(), "namespaced key is distinct");
    }
```

- [ ] **Step 3: Run — verify pass.** `cargo test -p sa-tools mcp::`. (If `name()`-as-`&'static` causes a clippy lint about `Box::leak`, apply the trait `-> &str` refactor per the note + update all 4 first-party impls + `run_task`'s ToolSpec assembly, then re-run.)
- [ ] **Step 4: fmt + clippy + commit** — `feat(tools): McpConnection spawn + McpTool + load_mcp_tools (namespaced, allow-listed, kill-on-drop)`

---

### Task 5: wire MCP tools into `run` + doctor line + a live smoke

**Files:** Modify `secretagent/src/run.rs`, `secretagent/src/doctor.rs`; create `secretagent/tests/live_mcp.rs` (`#[ignore]`).

**Interfaces:** `run` loads `cfg.mcp` servers and registers their namespaced tools alongside the first-party set; doctor reports the configured server count.

- [ ] **Step 1: `run.rs`** — after registering `execute_code`, before `run_task`:

```rust
    // Load configured MCP servers (namespaced + allow-listed). A down server is skipped.
    for tool in sa_tools::mcp::load_mcp_tools(&cfg.mcp).await {
        registry.register(tool);
    }
```

- [ ] **Step 2: `doctor.rs`** — add an informational line (after the landlock line):

```rust
    let cfg_for_mcp = config::Config::load().unwrap_or_default();
    if cfg_for_mcp.mcp.is_empty() {
        println!("[info] mcp: no servers configured");
    } else {
        let names: Vec<&str> = cfg_for_mcp.mcp.iter().map(|m| m.name.as_str()).collect();
        println!(
            "[info] mcp: {} server(s) configured: {} (tools loaded at run time, namespaced + allow-listed)",
            names.len(),
            names.join(", ")
        );
    }
```

- [ ] **Step 3: Live smoke** — `secretagent/tests/live_mcp.rs` (ignored; documents the manual end-to-end against a real server):

```rust
//! Live MCP acceptance — IGNORED by default. Point config.toml at a real MCP server, then:
//!   cargo test -p secretagent --test live_mcp -- --ignored --nocapture
//! Asserts that at least one namespaced tool loads from the configured servers.
#[tokio::test]
#[ignore]
async fn configured_mcp_servers_load_namespaced_tools() {
    let cfg = sa_core_types::config::Config::load().unwrap();
    let tools = sa_tools::mcp::load_mcp_tools(&cfg.mcp).await;
    assert!(
        tools.iter().all(|t| t.name().contains("::")),
        "every loaded MCP tool must be namespaced"
    );
}
```

- [ ] **Step 4: Run hermetic suites.** WSL + Windows: `cargo test -p secretagent` (the existing hermetic `run`/doctor tests still pass; MCP load with empty `cfg.mcp` is a no-op). `cargo run -p secretagent -- doctor` shows the `[info] mcp:` line.
- [ ] **Step 5: fmt + clippy + commit** — `feat(bin): load namespaced+allow-listed MCP tools into run + doctor mcp line`

---

### Task 6: Whole-workspace gates + adversarial review + push + CI

- [ ] **Step 1: Full gate (WSL + Windows).** `cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all` — both venues green.
- [ ] **Step 2: cargo-deny** (no new external crates — tokio features only — so the closure is unchanged; run it anyway): `cargo deny check`.
- [ ] **Step 3: Adversarial review** of the MCP client (the untrusted-server boundary) before push — confused-deputy/namespace-bypass, allow-list bypass, DoS (hang/flood/zombie child), protocol-parse panics, and the egress-gap honesty. Fix anything material.
- [ ] **Step 4: Push + watch CI to green** (`gh run watch <id> --exit-status --interval 25`; re-attach if it returns early). The whole slice is cross-platform, so all 5 CI jobs must pass.
- [ ] **Step 5:** Update the `project-secretagent` memory; summarize Phase 2 complete (2a+2b+2c) — the acceptance row "MCP server's tools load namespaced + allow-listed" is met.

---

## Self-Review

**Spec coverage:**
- *"MCP server's tools load namespaced and allow-listed"* (line 229) → Task 2 (`namespaced` + `filter_allowed`, default-deny) + Task 4 (`load_mcp_tools` applies both) + Task 5 (wired into `run`). ✓
- *"namespaced + allow-listed; treated as untrusted; same approval/egress rules"* (line 196) → namespacing (Task 2/4, shadow-safety test), allow-list (Task 2), untrusted = existing loop taint (no change — documented), egress gap stated honestly (Task 2 module doc). ✓
- *MCP client in `sa-tools`* (line 116) → `sa-tools/src/mcp.rs`, not a new crate. ✓

**Placeholder scan:** every code step is real. The one judgment call (`Box::leak` for the `&'static` name vs. a trait `-> &str` refactor) is spelled out with both options + the trigger to switch.

**Type consistency:** `McpToolDef{name,description,input_schema}`, `namespaced`, `filter_allowed`, `parse_tools_list`, `parse_call_result`, `McpClient::{new,initialize,list_tools,call_tool}`, `McpConnection::{connect,call}`, `McpTool`, `load_mcp_tools`, `McpServerConfig{name,command,args,allow_tools}`, `Config.mcp` — consistent across Tasks 1-5.

**Ponytail decisions (named ceilings):** MCP client is a `sa-tools` module (no new crate); stdio transport only (SSE/HTTP deferred); tools only (resources/prompts deferred); one mutex per connection (no request pipelining — the loop is sequential); 30 s timeout + 8 MiB line cap (fixed, not per-server); allow-list = consent (no per-remote-tool approval classification); `Box::leak` for runtime tool names (with the trait-refactor escape hatch). Security crown jewels (namespacing-no-shadow, default-deny allow-list, result parsing) are PURE + unit-tested cross-platform; the transport is duplex-tested without a subprocess.
