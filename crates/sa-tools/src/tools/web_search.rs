//! `web_search` — query an OPERATOR-FROZEN search endpoint through the egress seam. The model fills
//! only the `query`; the endpoint URL and the API key are operator config, injected at construction
//! (the `ExecuteCode::with_backend` precedent). The key rides an `Authorization: Bearer` header that
//! only the seam (never the model) sets.

use crate::egress::{self, EgressRequest};
use crate::Tool;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use reqwest::{Method, Url};
use sa_core_types::policy::Policy;
use serde_json::{json, Value};

pub struct WebSearch {
    endpoint: String,
    api_key: Option<String>,
}

impl WebSearch {
    /// Construct with the operator-frozen endpoint and an optional API key (already resolved from
    /// the vault by the registry builder — a key-id never reaches here, only the secret or `None`).
    pub fn with_key(endpoint: impl Into<String>, api_key: Option<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            api_key,
        }
    }

    /// Append the model's query as a `q=` param using the URL parser's encoder — so a query like
    /// `a&b` can't inject extra params or break out of the operator's frozen endpoint.
    fn build_url(&self, query: &str) -> Result<String> {
        let mut u =
            Url::parse(&self.endpoint).map_err(|e| anyhow!("web_search: bad search_url: {e}"))?;
        u.query_pairs_mut().append_pair("q", query);
        Ok(u.to_string())
    }
}

#[async_trait]
impl Tool for WebSearch {
    fn name(&self) -> &str {
        "web_search"
    }
    fn description(&self) -> &str {
        "Search the web via the operator-configured search endpoint; returns the raw results body \
         (untrusted). You provide only the query."
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"query":{"type":"string"}},"required":["query"]})
    }
    async fn run(&self, args: Value, policy: &Policy) -> Result<String> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("web_search: missing 'query'"))?;
        let url = self.build_url(query)?;
        let headers = match &self.api_key {
            Some(k) => vec![("Authorization".to_string(), format!("Bearer {k}"))],
            None => Vec::new(),
        };
        Ok(egress::egress_request(
            policy,
            EgressRequest {
                method: Method::GET,
                url: &url,
                headers,
                body: None,
            },
        )
        .await?
        .detaint("core re-taints tool output at the registry boundary"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_is_query_only() {
        let s = WebSearch::with_key("https://search.example/api", None)
            .parameters()
            .to_string();
        assert!(s.contains("query"));
        assert!(!s.contains("url") && !s.contains("header"));
    }

    #[test]
    fn build_url_encodes_the_query_and_cannot_inject_params() {
        let ws = WebSearch::with_key("https://search.example/api", None);
        let u = ws.build_url("a b&c=d").unwrap();
        assert!(u.starts_with("https://search.example/api?q="));
        assert!(!u.contains("a b"), "space must be encoded: {u}");
        // the `&` and `=` in the query are encoded, so no extra params are injected
        assert!(u.contains("%26") && u.contains("%3D"), "got {u}");
    }

    #[tokio::test]
    async fn denies_when_the_endpoint_host_is_unlisted() {
        let ws = WebSearch::with_key("https://search.example/api", Some("k".into()));
        let p = Policy::default(); // empty allow-list
        let err = ws.run(json!({"query":"hello"}), &p).await.unwrap_err();
        assert!(err.to_string().contains("denied"), "got {err}");
    }
}
