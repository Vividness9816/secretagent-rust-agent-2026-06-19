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
}
