//! `http_request` — a generic method+url+body HTTP call through the egress seam. The model fills
//! only `{method, url, body}`; it CANNOT set arbitrary headers (that surface stays operator-only via
//! tools like `web_search`). The seam re-vets host+IP on every hop regardless of method.

use crate::egress::{self, EgressRequest};
use crate::Tool;
use anyhow::{bail, Result};
use async_trait::async_trait;
use reqwest::Method;
use sa_core_types::policy::Policy;
use serde_json::{json, Value};

pub struct HttpRequest;

#[async_trait]
impl Tool for HttpRequest {
    fn name(&self) -> &str {
        "http_request"
    }
    fn description(&self) -> &str {
        "HTTP request (GET/POST/PUT/PATCH/DELETE/HEAD) to an allow-listed URL; returns the body \
         (untrusted). The host must be in the egress allow-list."
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "method":{"type":"string","enum":["GET","POST","PUT","PATCH","DELETE","HEAD"]},
                "url":{"type":"string"},
                "body":{"type":"string"}
            },
            "required":["url"]
        })
    }
    async fn run(&self, args: Value, policy: &Policy) -> Result<String> {
        let url = args
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("http_request: missing 'url'"))?;
        let method = parse_method(args.get("method").and_then(|v| v.as_str()).unwrap_or("GET"))?;
        let body = args
            .get("body")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        Ok(egress::egress_request(
            policy,
            EgressRequest {
                method,
                url,
                headers: Vec::new(),
                body,
            },
        )
        .await?
        .detaint("core re-taints tool output at the registry boundary"))
    }
}

fn parse_method(m: &str) -> Result<Method> {
    Ok(match m.to_ascii_uppercase().as_str() {
        "GET" => Method::GET,
        "POST" => Method::POST,
        "PUT" => Method::PUT,
        "PATCH" => Method::PATCH,
        "DELETE" => Method::DELETE,
        "HEAD" => Method::HEAD,
        other => bail!("http_request: unsupported method '{other}'"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn serve_once(response: &'static str) -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let _ = sock.write_all(response.as_bytes()).await;
                let _ = sock.flush().await;
            }
        });
        addr
    }

    #[test]
    fn schema_exposes_method_url_body_but_not_headers() {
        let s = HttpRequest.parameters().to_string();
        assert!(s.contains("method") && s.contains("url") && s.contains("body"));
        assert!(!s.contains("header"), "model must not set headers: {s}");
    }

    #[tokio::test]
    async fn denies_unlisted_host() {
        let p = Policy {
            egress_allow: vec!["example.com".into()],
            ..Default::default()
        };
        let err = HttpRequest
            .run(json!({"method":"POST","url":"http://evil.test/x"}), &p)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("denied"), "got {err}");
    }

    #[tokio::test]
    async fn post_round_trips_against_an_allow_listed_server() {
        let addr =
            serve_once("HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nPONG")
                .await;
        let p = Policy {
            egress_allow: vec!["127.0.0.1".into()],
            ..Default::default()
        };
        let out = HttpRequest
            .run(
                json!({"method":"POST","url":format!("http://127.0.0.1:{}/", addr.port()),"body":"ping"}),
                &p,
            )
            .await
            .unwrap();
        assert_eq!(out, "PONG");
    }
}
