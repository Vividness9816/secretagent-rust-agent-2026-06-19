//! Phase 6c — the single egress chokepoint. EVERY model-reachable HTTP call funnels through here
//! so the SSRF guard (real URL parse, `@`-userinfo rejection, IP-range deny-list, per-redirect-hop
//! re-check, body/timeout caps) is enforced in exactly one place. Operator-frozen clients
//! (`sa-providers`, `sa-connectors`) are NOT model-reachable and deliberately stay outside.
//!
//! Returns `Tainted<String>` (`Provenance::Untrusted`) — the boundary is explicit at the seam; the
//! `Tool` trait stays `-> String` and `sa-core` re-taints at the registry call site (no trait change).

use anyhow::{anyhow, bail, Result};
use reqwest::{Method, Url};
use sa_core_types::policy::{egress_allowed, Policy};
use sa_core_types::taint::Tainted;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

const MAX_BODY: usize = 8 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(20);
const MAX_REDIRECTS: u32 = 5;

/// A request through the seam. `headers` is for OPERATOR-injected values only (e.g. a web_search
/// API key set at tool construction) — never model-chosen header maps.
pub struct EgressRequest<'a> {
    pub method: Method,
    pub url: &'a str,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

/// The common case: a guarded GET. Used by `fetch` / `web_extract` / `web_search`.
pub async fn egress_get(policy: &Policy, url: &str) -> Result<Tainted<String>> {
    egress_request(
        policy,
        EgressRequest {
            method: Method::GET,
            url,
            headers: Vec::new(),
            body: None,
        },
    )
    .await
}

/// The chokepoint. Re-vets scheme + userinfo + host allow-list + resolved-IP range on EVERY redirect
/// hop, pinning reqwest to the vetted IP to close the DNS-rebind TOCTOU.
pub async fn egress_request(policy: &Policy, req: EgressRequest<'_>) -> Result<Tainted<String>> {
    let mut url = Url::parse(req.url).map_err(|e| anyhow!("egress: bad url: {e}"))?;
    let mut hops = 0u32;
    loop {
        let scheme = url.scheme();
        if scheme != "http" && scheme != "https" {
            bail!("egress denied: non-http(s) scheme '{scheme}'");
        }
        // The @-userinfo trick (`http://allowed.com@169.254.169.254/`) is how the old `url_host`
        // string-splitter was bypassed — a real parser puts the host AFTER the `@`, so reject any
        // userinfo outright.
        if !url.username().is_empty() || url.password().is_some() {
            bail!("egress denied: URL userinfo is not allowed");
        }
        let host = url
            .host_str()
            .ok_or_else(|| anyhow!("egress denied: URL has no host"))?
            .to_string();
        if !egress_allowed(policy, &host) {
            bail!("egress denied: {host}");
        }
        let port = url
            .port_or_known_default()
            .ok_or_else(|| anyhow!("egress: no port for {host}"))?;
        let addr = pick_allowed_addr(policy, &host, port).await?;

        // One-shot client: redirects OFF (we re-vet each hop ourselves), pinned to the vetted IP.
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(TIMEOUT)
            .resolve(&host, addr)
            .build()?;
        let mut rb = client.request(req.method.clone(), url.clone());
        for (k, v) in &req.headers {
            rb = rb.header(k.as_str(), v.as_str());
        }
        if let Some(b) = &req.body {
            rb = rb.body(b.clone());
        }
        let resp = rb.send().await?;

        if resp.status().is_redirection() {
            let loc = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| anyhow!("egress: redirect without a Location header"))?;
            let next = url
                .join(loc)
                .map_err(|e| anyhow!("egress: bad redirect target: {e}"))?;
            hops += 1;
            if hops > MAX_REDIRECTS {
                bail!("egress denied: too many redirects (>{MAX_REDIRECTS})");
            }
            url = next; // re-vet host + IP on the next iteration
            continue;
        }

        let resp = resp.error_for_status()?;
        let body = read_capped(resp, MAX_BODY).await?;
        return Ok(Tainted::untrusted(body, format!("egress:{host}")));
    }
}

/// Resolve `host` and return the first socket addr the policy permits. An IP is blocked if it falls
/// in an SSRF-target range UNLESS its literal string is itself in `egress_allow` (the operator/test
/// explicitly typed e.g. `127.0.0.1` — that is the "unless explicitly allow-listed" clause).
async fn pick_allowed_addr(policy: &Policy, host: &str, port: u16) -> Result<SocketAddr> {
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| anyhow!("egress: DNS resolve failed for {host}: {e}"))?
        .collect();
    if addrs.is_empty() {
        bail!("egress denied: {host} did not resolve");
    }
    for a in &addrs {
        let ip = a.ip();
        if !is_blocked_ip(ip) || ip_explicitly_allowed(policy, ip) {
            return Ok(*a);
        }
    }
    bail!("egress denied: {host} resolves only to blocked (private/loopback/link-local) addresses");
}

/// The "unless explicitly allow-listed" clause: a blocked IP is permitted only if the operator put
/// that exact IP in `egress_allow`. Compare by PARSED `IpAddr`, not by string, so any canonical form
/// the operator wrote (`::1` or the expanded `0:0:0:0:0:0:0:1`) matches the resolved address.
/// Hostname entries simply don't parse as IPs, so they never grant this exception.
fn ip_explicitly_allowed(policy: &Policy, ip: IpAddr) -> bool {
    policy
        .egress_allow
        .iter()
        .filter_map(|h| h.parse::<IpAddr>().ok())
        .any(|allowed| allowed == ip)
}

/// True if `ip` is in a range a model-reachable egress must never reach (the SSRF target set).
/// IPv4-mapped IPv6 (`::ffff:169.254.169.254`) is unwrapped first so the mapping can't smuggle a
/// blocked v4 address past the v6 arm.
fn is_blocked_ip(ip: IpAddr) -> bool {
    let ip = match ip {
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(v6)),
        v4 => v4,
    };
    match ip {
        IpAddr::V4(a) => {
            let o = a.octets();
            a.is_loopback()
                || a.is_private()
                || a.is_link_local()
                || a.is_unspecified()
                || a.is_broadcast()
                || a.is_multicast()
                || a.is_documentation()
                || (o[0] == 100 && (o[1] & 0xc0) == 0x40) // 100.64.0.0/10 CGNAT
        }
        IpAddr::V6(a) => {
            let s = a.segments();
            a.is_loopback()
                || a.is_unspecified()
                || a.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00 // fc00::/7 unique-local
                || (s[0] & 0xffc0) == 0xfe80 // fe80::/10 link-local
        }
    }
}

/// Read a response body, refusing once it exceeds `cap` (a malicious allow-listed host can't OOM us).
async fn read_capped(mut resp: reqwest::Response, cap: usize) -> Result<String> {
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        if buf.len() + chunk.len() > cap {
            bail!("egress denied: response body exceeds {cap} bytes");
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sa_core_types::types::Provenance;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn allow(hosts: &[&str]) -> Policy {
        Policy {
            egress_allow: hosts.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    /// A one-shot HTTP/1.1 server on 127.0.0.1:0 — accepts a single connection, discards the
    /// request, writes the canned response. Dep-free (raw tokio TCP).
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

    #[tokio::test]
    async fn ssrf_corpus_is_denied_before_any_body_returns() {
        let p = allow(&["allowed.com", "localhost"]);
        // cloud-metadata IP literal (link-local) — host not allow-listed AND blocked IP
        assert_deny(&p, "http://169.254.169.254/latest/meta-data/").await;
        // @-userinfo trick: real parser puts host after @, userinfo rejected
        assert_deny(&p, "http://allowed.com@169.254.169.254/").await;
        // loopback IP literal, not allow-listed
        assert_deny(&p, "http://127.0.0.1/admin").await;
        // non-http(s) scheme
        assert_deny(&p, "file:///etc/passwd").await;
        assert_deny(&p, "ftp://allowed.com/x").await;
        // `localhost` IS allow-listed by NAME but resolves to 127.0.0.1, which is NOT IP-allow-listed
        assert_deny(&p, "http://localhost:9/x").await;
    }

    async fn assert_deny(p: &Policy, url: &str) {
        let err = egress_get(p, url).await.unwrap_err().to_string();
        assert!(
            err.contains("denied") || err.contains("userinfo") || err.contains("scheme"),
            "expected denial for {url}, got: {err}"
        );
    }

    #[tokio::test]
    async fn allow_listed_ip_round_trips_and_is_tainted() {
        let addr =
            serve_once("HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nHELLO")
                .await;
        let p = allow(&["127.0.0.1"]); // explicit IP literal = the allow-listed exception
        let out = egress_get(&p, &format!("http://127.0.0.1:{}/x", addr.port()))
            .await
            .unwrap();
        assert_eq!(out.as_data(), "HELLO");
        assert!(matches!(out.provenance(), Provenance::Untrusted { .. }));
    }

    #[tokio::test]
    async fn redirect_to_internal_is_denied_on_the_next_hop() {
        let addr = serve_once(
            "HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )
        .await;
        let p = allow(&["127.0.0.1"]); // first hop allowed; the redirect target is not
        let err = egress_get(&p, &format!("http://127.0.0.1:{}/x", addr.port()))
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("denied"),
            "redirect to internal must deny: {err}"
        );
    }

    #[test]
    fn blocked_ip_ranges_cover_the_ssrf_targets() {
        for ip in [
            "127.0.0.1",
            "169.254.169.254",
            "10.0.0.5",
            "192.168.1.1",
            "172.16.0.1",
            "0.0.0.0",
            "100.64.0.1",
            "::1",
            "fc00::1",
            "fe80::1",
            "::ffff:169.254.169.254", // v4-mapped must be unwrapped + caught
        ] {
            assert!(is_blocked_ip(ip.parse().unwrap()), "{ip} should be blocked");
        }
        for ip in ["93.184.216.34", "1.1.1.1", "2606:4700:4700::1111"] {
            assert!(
                !is_blocked_ip(ip.parse().unwrap()),
                "{ip} should be allowed"
            );
        }
    }

    #[test]
    fn ip_allow_exception_matches_canonical_and_expanded_forms() {
        let loop4: IpAddr = "127.0.0.1".parse().unwrap();
        let loop6: IpAddr = "::1".parse().unwrap();
        assert!(ip_explicitly_allowed(&allow(&["127.0.0.1"]), loop4));
        assert!(ip_explicitly_allowed(&allow(&["::1"]), loop6));
        // The fix: a non-canonical (expanded) IPv6 entry still matches the resolved ::1.
        assert!(ip_explicitly_allowed(&allow(&["0:0:0:0:0:0:0:1"]), loop6));
        // A hostname entry never grants the IP-literal exception; a different IP never matches.
        assert!(!ip_explicitly_allowed(&allow(&["example.com"]), loop4));
        assert!(!ip_explicitly_allowed(&allow(&["10.0.0.1"]), loop4));
    }
}
