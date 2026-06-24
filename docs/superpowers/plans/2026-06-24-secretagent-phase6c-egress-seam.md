# SecretAgent Phase 6c — Egress-Guarded HTTP Seam + Network Tools

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:test-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** ONE shared `egress_get`/`egress_request` chokepoint that funnels every model-reachable HTTP call through a real SSRF guard — fixing the live `Fetch::run` SSRF — then `web_extract` / `http_request` / `web_search` built through that same seam.

**Architecture (ADR-20260623-secretagent-phase6-milestone, slice 6c):** A `sa-tools/src/egress.rs` module owns the only `reqwest` calls any model-reachable tool makes. It parses the URL with a real parser (`reqwest::Url`, already in-tree), rejects `@`-userinfo + non-http(s), resolves the host itself and denies loopback/link-local/RFC-1918/ULA/unspecified/multicast IPs unless the **IP literal** is explicitly in `egress_allow`, pins reqwest to the vetted IP (`.resolve`) to close the DNS-rebind TOCTOU, disables auto-redirects and re-vets host+IP on **every** hop, and caps body size + timeout. `Fetch` is re-pointed at it and `url_host` deleted. New network tools live in `sa-tools/src/tools/*.rs` (mirroring the per-file connectors); `web_search` gets an operator-frozen endpoint + a vault `*_ref` credential injected at construction (`WebSearch::with_key`, the `ExecuteCode::with_backend` precedent — no Tool-trait change, no "gateway" abstraction). Operator-frozen clients (`sa-providers`, `sa-connectors`) stay OUTSIDE the seam.

**Tech Stack:** Rust, `reqwest` (rustls, already a dep), `tokio::net` (DNS resolve), `std::net::IpAddr` range checks. **Zero new crates** — only a tokio `net` feature on `sa-tools`.

## Global Constraints
- **Zero new crate** (musl-static + rustls-only invariant): URL parse = `reqwest::Url`; DNS = `tokio::net::lookup_host` (add the `net` feature to `sa-tools`'s tokio); IP ranges = `std::net`. No `url`/`idna`/HTML-parser dep.
- **No tool calls `reqwest` directly** — every model-reachable network tool goes through `egress_get`/`egress_request`. Operator-frozen clients (provider/connectors) stay outside (not model-reachable).
- **Seam output is `Tainted<String>`** (`Provenance::Untrusted`); the `Tool` trait stays `-> Result<String>` (core re-taints at the registry call site — no trait change).
- **Credential model:** per-tool vault `key_ref` + a shared `default_key_ref` (the existing `*_ref` convention); secret read from the vault and injected at tool **construction** in `setup.rs::build_registry`, never a Tool-trait change, never logged.
- **TDD**; commit per task; conventional-commit subject; footer = blank line then `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>` then `Claude-Session: phase-6c`.
- **`self-audit` PreToolUse hook blocks `git commit`** → append ` # self-audit-ok` (a shell comment — run `git push` SEPARATELY).
- **Gates before push:** `cargo fmt --all --check` 0 / `cargo clippy --all-targets --all-features -D warnings` 0 (test module LAST in each file) / `cargo test --all` on **both** Windows + WSL; rustls-only unchanged; commit `Cargo.lock`. **Adversarial self-audit before pushing 6c** (the egress boundary). Then watch CI green on all 5 jobs.

## File Structure
- `crates/sa-tools/Cargo.toml` — add `net` to the tokio features.
- `crates/sa-tools/src/egress.rs` — **NEW.** `EgressRequest`, `egress_get`, `egress_request`, `is_blocked_ip`, `pick_allowed_addr`, `read_capped`. The chokepoint + the SSRF corpus tests.
- `crates/sa-tools/src/lib.rs` — `pub mod egress;` `pub mod tools;` ; re-point `Fetch::run` at `egress::egress_get`; **delete `url_host`**.
- `crates/sa-tools/src/tools/mod.rs` — **NEW.** `pub mod web_extract; pub mod http_request; pub mod web_search;`
- `crates/sa-tools/src/tools/web_extract.rs` — **NEW.** `WebExtract` tool + `strip_html` (naive, dep-free).
- `crates/sa-tools/src/tools/http_request.rs` — **NEW.** `HttpRequest` tool ({method,url,body} — no model-chosen headers).
- `crates/sa-tools/src/tools/web_search.rs` — **NEW.** `WebSearch { endpoint, api_key }` + `with_key`; query-templated guarded GET.
- `crates/sa-core-types/src/config.rs` — `ToolsConfig { search_url, search_key_ref, default_key_ref }` + `pub tools` on `Config` + parse test.
- `secretagent/src/setup.rs` — register `web_extract`+`http_request` (always) + `web_search` (if `search_url` set; key = `search_key_ref.or(default_key_ref)` read from the vault, injected at construction).

---

### Task 1: the egress seam (SECURITY CORE)

**Files:** Create `crates/sa-tools/src/egress.rs`; modify `crates/sa-tools/Cargo.toml` (tokio `net`), `crates/sa-tools/src/lib.rs` (`pub mod egress;`).

**Interfaces — Produces:**
- `pub struct EgressRequest<'a> { pub method: reqwest::Method, pub url: &'a str, pub headers: Vec<(String,String)>, pub body: Option<String> }`
- `pub async fn egress_get(policy: &Policy, url: &str) -> Result<Tainted<String>>`
- `pub async fn egress_request(policy: &Policy, req: EgressRequest<'_>) -> Result<Tainted<String>>`

- [ ] **Step 1: Failing tests** (`mod tests` in egress.rs): SSRF corpus all DENIED (metadata IP, `@`-userinfo, loopback host, non-http scheme, `localhost` allow-listed-by-name still denied because `127.0.0.1` is not IP-allow-listed); an allow-listed-IP round-trip against a one-shot local server returns the body with `Untrusted` provenance; a 302→link-local redirect is DENIED on the second hop. `serve_once(resp: &'static str) -> SocketAddr` spawns a tokio `TcpListener` on `127.0.0.1:0`, reads+discards the request, writes the canned response.
- [ ] **Step 2: Run → FAIL** (`cargo test -p sa-tools egress`).
- [ ] **Step 3: Implement** the seam:
  - `is_blocked_ip(IpAddr)` — unwrap v4-mapped v6 first; v4: loopback/private/link-local/unspecified/broadcast/multicast/documentation + 100.64/10; v6: loopback/unspecified/multicast + fc00::/7 + fe80::/10 (manual masks — no unstable `is_unique_local`).
  - `pick_allowed_addr(policy, host, port)` — `lookup_host`, return the first addr whose IP is unblocked OR whose `ip.to_string()` is in `egress_allow`; else `bail!`.
  - `egress_request` loop: parse (`reqwest::Url`), reject non-http(s) + userinfo, `egress_allowed` host gate, `pick_allowed_addr`, build a one-shot client (`redirect::Policy::none()`, `timeout`, `.resolve(host, addr)`), send; on 3xx re-`join` Location and loop (cap `MAX_REDIRECTS=5`); else `read_capped` (`MAX_BODY=8 MiB`) → `Tainted::untrusted(body, "egress:{host}")`.
  - `egress_get` = `egress_request` with `Method::GET`, no headers/body.
- [ ] **Step 4: Run → PASS.** `cargo test -p sa-tools egress`.
- [ ] **Step 5: Commit** `feat(6c): egress-guarded HTTP seam — SSRF-safe chokepoint (phase 6c)`.

### Task 2: re-point Fetch, delete url_host

**Files:** Modify `crates/sa-tools/src/lib.rs`.

- [ ] **Step 1:** Existing `fetch_denies_unlisted_host_without_making_a_request` stays (still must pass).
- [ ] **Step 2:** `Fetch::run` → `Ok(egress::egress_get(policy, url).await?.detaint("core re-taints tool output at the registry boundary"))`; **delete `url_host`**.
- [ ] **Step 3: Run → PASS** (`cargo test -p sa-tools`). **Step 4: Commit** `refactor(6c): re-point Fetch at the egress seam, delete url_host SSRF (phase 6c)`.

### Task 3: web_extract

**Files:** Create `crates/sa-tools/src/tools/{mod.rs,web_extract.rs}`; modify `lib.rs` (`pub mod tools;`).

- [ ] **Step 1: Failing test** — `strip_html("<style>x</style><p>Hi <b>there</b></p>")` == `"Hi there"`; `WebExtract.run` denies an unlisted host.
- [ ] **Step 2: FAIL. Step 3: Implement** — `strip_html` drops `<script>`/`<style>` blocks, strips tags, decodes `&amp;/&lt;/&gt;/&quot;/&#39;/&nbsp;`, collapses whitespace; `WebExtract.run` = `egress_get` then `strip_html`. **Step 4: PASS. Step 5: Commit** `feat(6c): web_extract tool — seam GET + naive HTML→text (phase 6c)`.

### Task 4: http_request

**Files:** Create `crates/sa-tools/src/tools/http_request.rs`; modify `tools/mod.rs`.

- [ ] **Step 1: Failing test** — schema exposes `{method,url,body}` and NOT headers; denies an unlisted host; a POST round-trips against `serve_once`.
- [ ] **Step 2: FAIL. Step 3: Implement** — parse `method` (GET/POST/PUT/PATCH/DELETE/HEAD, default GET), pass `body`, `headers: vec![]` (no model headers), through `egress_request`. **Step 4: PASS. Step 5: Commit** `feat(6c): http_request tool through the egress seam (phase 6c)`.

### Task 5: ToolsConfig + web_search

**Files:** Modify `crates/sa-core-types/src/config.rs`; create `crates/sa-tools/src/tools/web_search.rs`; modify `tools/mod.rs`.

- [ ] **Step 1: Failing tests** — `Config` parses `[tools] search_url/search_key_ref/default_key_ref` (absent → all `None`); `WebSearch::with_key(endpoint, Some(key))` schema is `{query}`; `run` denies when the endpoint host is unlisted; URL-encodes the query into `?q=`.
- [ ] **Step 2: FAIL. Step 3: Implement** — `ToolsConfig { search_url: Option<String>, search_key_ref: Option<String>, default_key_ref: Option<String> }` (`#[serde(default)]`) + `Config.tools`; `WebSearch { endpoint, api_key }`, builds `{endpoint}?q={encoded}`, injects `Authorization: Bearer {key}` header iff `api_key` set, via `egress_request`. **Step 4: PASS. Step 5: Commit** `feat(6c): web_search tool + ToolsConfig credential refs (phase 6c)`.

### Task 6: wire the tools into the registry

**Files:** Modify `secretagent/src/setup.rs`.

- [ ] **Step 1: Failing test** — `build_registry(&Config::default(), false)` lists `web_extract` + `http_request` (always) and NOT `web_search` (no `search_url`).
- [ ] **Step 2: FAIL. Step 3: Implement** — register `WebExtract` + `HttpRequest` unconditionally; if `cfg.tools.search_url` is set, read the key (`search_key_ref.or(default_key_ref)` → vault, same pattern as `build_provider`) and register `WebSearch::with_key`. **Step 4: PASS. Step 5: Commit** `feat(6c): register web_extract/http_request/web_search in the registry (phase 6c)`.

---

## Acceptance (ADR slice 6c)
- An SSRF corpus (cloud-metadata IP, loopback, `@`-userinfo, redirect-to-internal, non-http scheme) is **DENIED** before any body returns. ✓ Task 1 tests.
- An allow-listed search/fetch **round-trips**. ✓ Task 1 round-trip + Task 4 POST round-trip.
- The new tools' output is **`Tainted`**. ✓ seam returns `Tainted::untrusted`; core re-taints at the registry boundary.
- `url_host` (the SSRF string-splitter) is **deleted**; no model-reachable tool calls `reqwest` directly. ✓ Task 2.

## Self-Review
- **Spec coverage:** egress seam (T1), Fetch re-point + url_host delete (T2), web_extract (T3), http_request (T4), web_search + credential refs (T5), registry wiring (T6) — every ADR-6c clause mapped.
- **Type consistency:** `egress_get`/`egress_request`/`EgressRequest` names match across T1/T3/T4/T5; `WebSearch::with_key` mirrors `ExecuteCode::with_backend`; `ToolsConfig.{search_url,search_key_ref,default_key_ref}` match T5↔T6.
- **Deferred (honest):** `strip_html` is a naive tag-stripper, not a readability extractor (ponytail ceiling — swap a crate in if extraction quality matters); `http_request` redirects re-issue the same method (security re-vet holds regardless); DNS-rebind closed by `.resolve`-pinning, residual sub-millisecond TOCTOU accepted for v1.
