# SecretAgent Phase 5b — Slack Connector (Socket Mode) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (inline, per the operator) to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
> Executed inline (autonomous) per ADR-20260623. TDD, commit-per-task, both-venue gate, multi-lens adversarial-review Workflow before push.

**Goal:** Add a 4th `Connector` — Slack over **Socket Mode** — reusing the 4c trait + `InboundMsg`/`OutboundMsg` + `MockConnector` + the M3 `dispatch_inbound` boundary + `RunContext::remote` **verbatim**. Completes acceptance (a): a task runs in a Docker backend on a remote host **driven from Slack**.

**Architecture:** Receive = `apps.connections.open` (reqwest POST, `xapp-` app-level token) → a `wss://` URL → a feature-gated rustls WebSocket (`tokio-websockets`, already in-tree via twilight) → receive envelopes (`hello` / `events_api` / `disconnect`) → **ACK each `events_api` envelope** over the WS → map to `InboundMsg`. Send = `chat.postMessage` (reqwest POST, `xoxb-` bot token). Identity = the **`(team_id, user_id)` tuple** encoded as `sender = "<team_id>:<user_id>"` so the M3 `allow_senders` keys on it verbatim (a user in another workspace can never collide with the registered owner). Both tokens come from the vault, never logged. The parse + map logic is **pure functions** (`parse_envelope`/`map_message`) so the whole boundary is unit-testable with zero network (the Telegram `parse_updates` / Discord `map_message` precedent).

**Tech Stack:** Rust, `reqwest` (already a sa-connectors dep, rustls), `tokio-websockets` 0.13 (rustls/webpki, already in `Cargo.lock` via `twilight-gateway`), `futures-util` (already an optional sa-connectors dep), `serde_json`.

## Global Constraints

- **TDD**; commit per task; conventional-commit subject; footer = a blank line then `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>` then `Claude-Session: phase-5b`.
- The **`self-audit` PreToolUse hook blocks `git commit`** — append ` # self-audit-ok` to the bash command.
- Before EVERY commit: `cargo fmt --all` (then `--check` = 0) / `cargo clippy --all-targets --all-features -- -D warnings` (0) / relevant `cargo test`.
- **Both-venue gate before push:** Windows `cargo test --all` + WSL `CARGO_TARGET_DIR=$HOME/sa-target cargo test --all`. Watch `CARGO_EXIT=0`.
- **rustls-only** — no openssl/native-tls/aws-lc-sys/zstd-sys. The new WS dep MUST be rustls + `ring`/webpki, no `zstd`/`aws-lc`. Verify `wsl … cargo tree -e features -p secretagent | grep -iE "openssl|native-tls|aws-lc-sys|zstd-sys"` stays empty; `cargo deny check` green. **Commit `Cargo.lock`** (a new direct dep changes it).
- **Secrets:** the `xapp-` + `xoxb-` tokens come from the vault (`token_ref` = bot token, new `app_token_ref` = app token), NEVER logged. The `apps.connections.open` response URL carries a one-time **`ticket` secret** — strip any token/URL-bearing error before it can be logged (the 4c `reqwest::Error::without_url` precedent), and never `tracing` the wss URL.
- **Identity** = `sender = "<team_id>:<user_id>"` (NOT a bare user id). **Skip bot/own/subtype messages** (no reply loops, the Discord `map_message` precedent).
- Signing-secret HMAC is **N/A under Socket Mode** (no public inbound endpoint) — revisit only if moved to the Events API.
- Feature-gated `slack` like `discord`/`email`; the bin enables it.

---

### Task 1: config — `app_token_ref` on `ConnectorConfig`

Slack needs **two** tokens (the `xapp-` app-level token for Socket Mode + the `xoxb-` bot token for `chat.postMessage`). `token_ref` already carries one vault key-id (reused as the bot token); add `app_token_ref` for the app token.

**Files:**
- Modify: `crates/sa-core-types/src/config.rs` (add field to `ConnectorConfig`; update the doc comment + the `kind` enum comment to include `"slack"`).
- Test: same file (`#[cfg(test)] mod tests`).

**Interfaces:**
- Produces: `ConnectorConfig.app_token_ref: Option<String>` (a vault key-id for the `xapp-` token; `None` for non-Slack bindings).

- [ ] **Step 1: Write the failing test** — append to `crates/sa-core-types/src/config.rs` tests:

```rust
#[test]
fn config_parses_slack_with_app_token_ref() {
    let toml = r#"
[[connectors]]
name = "slack-main"
kind = "slack"
token_ref = "SLACK_BOT_TOKEN"
app_token_ref = "SLACK_APP_TOKEN"
allow_senders = ["T01ABCD:U05WXYZ"]
allow_tools = ["execute_code"]
"#;
    let c: Config = toml::from_str(toml).unwrap();
    assert_eq!(c.connectors[0].kind, "slack");
    assert_eq!(c.connectors[0].token_ref.as_deref(), Some("SLACK_BOT_TOKEN"));
    assert_eq!(c.connectors[0].app_token_ref.as_deref(), Some("SLACK_APP_TOKEN"));
    // The M3 identity is the (team, user) tuple, never a bare user id.
    assert_eq!(c.connectors[0].allow_senders, vec!["T01ABCD:U05WXYZ".to_string()]);
    // A binding without app_token_ref still parses (telegram/discord/email ignore it).
    let c2: Config = toml::from_str(
        "[[connectors]]\nname=\"t\"\nkind=\"telegram\"\ntoken_ref=\"X\"\n",
    ).unwrap();
    assert!(c2.connectors[0].app_token_ref.is_none());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p sa-core-types config_parses_slack_with_app_token_ref`
Expected: FAIL — `no field 'app_token_ref' on ConnectorConfig`.

- [ ] **Step 3: Write minimal implementation** — in `ConnectorConfig`, after `token_ref`, add:

```rust
    /// Slack Socket Mode only: the vault key-id for the `xapp-` app-level token (Socket Mode).
    /// `token_ref` stays the `xoxb-` bot token (chat.postMessage). `None` for other kinds.
    pub app_token_ref: Option<String>,
```

Update the `kind` doc line to `/// "telegram" | "discord" | "email" | "slack"`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p sa-core-types` → PASS (all config tests).

- [ ] **Step 5: Commit**

```bash
git add crates/sa-core-types/src/config.rs && git commit -m "feat(config): add app_token_ref for Slack Socket Mode (phase 5b)

Slack needs two vault-held tokens: xoxb- (bot, token_ref) + xapp- (app-level,
app_token_ref) for Socket Mode. Other connector kinds ignore it.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
Claude-Session: phase-5b" # self-audit-ok
```

---

### Task 2: `sa-connectors` — `slack` feature + pure `parse_envelope` / `map_message`

The whole Socket Mode parse/map logic is **pure functions** — unit-testable with no network. The transport (`recv`/`send`) is Task 3.

**Files:**
- Modify: `Cargo.toml` (workspace) — add `tokio-websockets` to `[workspace.dependencies]`.
- Modify: `crates/sa-connectors/Cargo.toml` — `tokio-websockets` optional dep + `slack` feature.
- Modify: `crates/sa-connectors/src/lib.rs` — `#[cfg(feature = "slack")] pub mod slack;`.
- Create: `crates/sa-connectors/src/slack.rs` (the pure fns + tests this task; the connector impl Task 3).

**Interfaces:**
- Produces:
  - `pub enum Envelope { Hello, Disconnect, EventsApi { envelope_id: String, payload: serde_json::Value }, Other }`
  - `pub fn parse_envelope(json: &serde_json::Value) -> Envelope`
  - `pub fn map_message(payload: &serde_json::Value, connector: &str) -> Option<InboundMsg>` — from an `events_api` payload, extract a human text `message` into `InboundMsg { sender: "<team>:<user>", chat: channel, text }`, or `None` to skip (bot/own/subtype/empty/missing ids).

- [ ] **Step 1: Write the failing tests** — create `crates/sa-connectors/src/slack.rs` with ONLY the pure fns' signatures stubbed (`unimplemented!()`) + the tests:

```rust
//! Slack connector — Socket Mode (an outbound WSS; no public inbound endpoint, fitting a NAT'd
//! daemon). Receive: `apps.connections.open` (xapp- token) -> wss URL -> envelopes; ACK every
//! `events_api` envelope; map `message` events to `InboundMsg`. Send: `chat.postMessage` (xoxb-
//! token). Identity = the (team_id, user_id) tuple encoded `"<team>:<user>"` so M3 keys on it.
//! Both tokens come from the vault — never logged; the wss URL carries a one-time ticket secret
//! and is never logged either. Parse/map are pure (the Telegram/Discord unit-test precedent).

use crate::{Connector, InboundMsg, OutboundMsg};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::time::Duration;

/// A Socket Mode envelope, classified for the recv loop. `events_api` carries the inner event
/// payload AND an `envelope_id` that MUST be ACK'd over the WS within ~3s.
#[derive(Debug, PartialEq, Eq)]
pub enum Envelope {
    Hello,
    Disconnect,
    EventsApi { envelope_id: String, payload: Value },
    Other,
}

/// PURE: classify a Socket Mode envelope. Unknown/irrelevant types -> `Other` (ignored).
pub fn parse_envelope(json: &Value) -> Envelope {
    unimplemented!()
}

/// PURE: map an `events_api` payload to an `InboundMsg`, or `None` to skip. Skips bot/own messages
/// (`bot_id` present), edits/joins/etc (`subtype` present), empty text, and any missing id. Content
/// is NEVER trusted — the gateway stamps it `Untrusted`. `sender` = "<team_id>:<user_id>".
pub fn map_message(payload: &Value, connector: &str) -> Option<InboundMsg> {
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_envelope_classifies_each_type() {
        assert_eq!(parse_envelope(&json!({"type": "hello"})), Envelope::Hello);
        assert_eq!(
            parse_envelope(&json!({"type": "disconnect", "reason": "warning"})),
            Envelope::Disconnect
        );
        assert_eq!(parse_envelope(&json!({"type": "slash_commands"})), Envelope::Other);
        let env = json!({
            "type": "events_api",
            "envelope_id": "env-123",
            "payload": {"team_id": "T1", "event": {"type": "message"}}
        });
        match parse_envelope(&env) {
            Envelope::EventsApi { envelope_id, payload } => {
                assert_eq!(envelope_id, "env-123");
                assert_eq!(payload["team_id"], "T1");
            }
            other => panic!("expected EventsApi, got {other:?}"),
        }
    }

    fn payload(team: &str, event: Value) -> Value {
        json!({"team_id": team, "event": event})
    }

    #[test]
    fn map_message_extracts_human_text_with_tuple_identity() {
        let p = payload(
            "T01ABCD",
            json!({"type": "message", "user": "U05WXYZ", "channel": "C09", "text": "hi there"}),
        );
        let got = map_message(&p, "slack").expect("a human text message maps");
        assert_eq!(
            got,
            InboundMsg {
                connector: "slack".into(),
                sender: "T01ABCD:U05WXYZ".into(), // the (team, user) tuple
                chat: "C09".into(),
                text: "hi there".into(),
            }
        );
    }

    #[test]
    fn map_message_skips_bots_subtypes_and_empties() {
        // A bot author (incl. ourselves) -> bot_id present -> skipped (prevents reply loops).
        assert!(map_message(
            &payload("T1", json!({"type": "message", "bot_id": "B1", "user": "U1", "channel": "C1", "text": "x"})),
            "slack"
        ).is_none());
        // A non-user event subtype (message_changed, channel_join, ...) -> skipped.
        assert!(map_message(
            &payload("T1", json!({"type": "message", "subtype": "message_changed", "user": "U1", "channel": "C1", "text": "x"})),
            "slack"
        ).is_none());
        // Empty/whitespace text -> skipped.
        assert!(map_message(
            &payload("T1", json!({"type": "message", "user": "U1", "channel": "C1", "text": "   "})),
            "slack"
        ).is_none());
        // A non-message event (reaction_added, etc) -> skipped.
        assert!(map_message(
            &payload("T1", json!({"type": "reaction_added", "user": "U1", "channel": "C1"})),
            "slack"
        ).is_none());
        // Missing team_id or user -> skipped (can't form a safe identity).
        assert!(map_message(
            &payload("", json!({"type": "message", "user": "U1", "channel": "C1", "text": "x"})),
            "slack"
        ).is_none());
        assert!(map_message(
            &payload("T1", json!({"type": "message", "channel": "C1", "text": "x"})),
            "slack"
        ).is_none());
    }
}
```

- [ ] **Step 2: Add the dep + feature so the test crate compiles.**

In `Cargo.toml` (workspace `[workspace.dependencies]`), after the twilight block, add:

```toml
# Slack (feature `slack`) — Socket Mode WSS. Pinned to the version already in-tree via
# twilight-gateway (rustls + webpki-roots, NO native-tls/aws-lc/zstd → musl-static holds).
tokio-websockets = { version = "0.13", default-features = false, features = ["client", "rustls-webpki-roots", "fastrand", "sha1_smol"] }
```

In `crates/sa-connectors/Cargo.toml` deps, add:

```toml
tokio-websockets = { workspace = true, optional = true }
```

and in `[features]` add (note: `slack` reuses `futures-util`, already declared optional for email):

```toml
slack = ["dep:tokio-websockets", "dep:futures-util"]
```

In `crates/sa-connectors/src/lib.rs`, beside the discord/email mod gates, add:

```rust
#[cfg(feature = "slack")]
pub mod slack;
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p sa-connectors --features slack slack::`
Expected: FAIL — the tests panic on `unimplemented!()` (the crate compiles; the pure fns are stubs).

> If the build instead errors on the `tokio-websockets` feature names, run `cargo doc -p tokio-websockets --no-deps --open` (or read `~/.cargo/registry/src/*/tokio-websockets-0.13*/Cargo.toml`) to confirm the exact feature flags for a rustls-webpki client, and adjust. The lockfile already resolves it with `tokio-rustls` + `webpki-roots`, so a rustls client feature set exists.

- [ ] **Step 4: Write minimal implementation** — replace the two `unimplemented!()` bodies:

```rust
pub fn parse_envelope(json: &Value) -> Envelope {
    match json.get("type").and_then(|t| t.as_str()) {
        Some("hello") => Envelope::Hello,
        Some("disconnect") => Envelope::Disconnect,
        Some("events_api") => {
            let envelope_id = json
                .get("envelope_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let payload = json.get("payload").cloned().unwrap_or(Value::Null);
            if envelope_id.is_empty() {
                Envelope::Other
            } else {
                Envelope::EventsApi { envelope_id, payload }
            }
        }
        _ => Envelope::Other,
    }
}

pub fn map_message(payload: &Value, connector: &str) -> Option<InboundMsg> {
    let event = payload.get("event")?;
    if event.get("type").and_then(|v| v.as_str()) != Some("message") {
        return None;
    }
    // Skip bots (incl. ourselves -> reply loops) and non-user subtypes (edits, joins, ...).
    if event.get("bot_id").is_some() || event.get("subtype").is_some() {
        return None;
    }
    let text = event.get("text").and_then(|v| v.as_str()).unwrap_or("").trim();
    if text.is_empty() {
        return None;
    }
    // Identity = (team_id, user_id) tuple. team_id is on the payload; user is on the event.
    let team = payload.get("team_id").and_then(|v| v.as_str()).unwrap_or("");
    let user = event.get("user").and_then(|v| v.as_str()).unwrap_or("");
    let channel = event.get("channel").and_then(|v| v.as_str()).unwrap_or("");
    if team.is_empty() || user.is_empty() || channel.is_empty() {
        return None;
    }
    Some(InboundMsg {
        connector: connector.to_string(),
        sender: format!("{team}:{user}"),
        chat: channel.to_string(),
        text: text.to_string(),
    })
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p sa-connectors --features slack slack::` → PASS.

- [ ] **Step 6: fmt + clippy + commit**

```bash
cargo fmt --all && cargo clippy -p sa-connectors --features slack --all-targets -- -D warnings
git add Cargo.toml Cargo.lock crates/sa-connectors/Cargo.toml crates/sa-connectors/src/lib.rs crates/sa-connectors/src/slack.rs
git commit -m "feat(slack): pure parse_envelope + map_message with (team,user) identity (phase 5b)

Socket Mode envelope classification + message mapping as pure, network-free
functions (the Telegram/Discord unit-test precedent). Identity is the
(team_id,user_id) tuple so M3 allow_senders can't be spoofed cross-workspace.
Adds the tokio-websockets dep (rustls, already in-tree via twilight) + slack feature.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
Claude-Session: phase-5b" # self-audit-ok
```

---

### Task 3: `sa-connectors` — `SlackConnector` (Socket Mode connect + ACK loop + send)

**Files:**
- Modify: `crates/sa-connectors/src/slack.rs` (add `SlackConnector` + `impl Connector` + the token-no-leak test).

**Interfaces:**
- Consumes: `parse_envelope`, `map_message` (Task 2); `Connector`/`InboundMsg`/`OutboundMsg`/`clamp_reply` (lib.rs).
- Produces: `pub struct SlackConnector`; `SlackConnector::new(id, bot_token, app_token) -> Self`; `SlackConnector::with_base(id, bot_token, app_token, base) -> Self` (test seam, mirrors Telegram).

- [ ] **Step 1: Write the failing test** — append to `slack.rs` tests:

```rust
#[tokio::test]
async fn tokens_never_leak_in_a_connect_error() {
    // apps.connections.open points at a closed port -> the request fails. Neither token nor the
    // base must appear in the formatted error chain (without_url strips the URL; the Bearer token
    // is a header reqwest never includes in errors). Regression for the 4c secret-leak findings.
    let mut c = SlackConnector::with_base(
        "slack",
        "xoxb-SECRET-BOT",
        "xapp-SECRET-APP",
        "http://127.0.0.1:1",
    );
    let err = c.recv().await.expect_err("a request to a closed port must error");
    let shown = format!("{err:#}");
    assert!(!shown.contains("xoxb-SECRET-BOT"), "bot token leaked: {shown}");
    assert!(!shown.contains("xapp-SECRET-APP"), "app token leaked: {shown}");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p sa-connectors --features slack slack::tokens_never_leak`
Expected: FAIL — `SlackConnector` / `with_base` undefined.

- [ ] **Step 3: Write the implementation** — add to `slack.rs` (above the tests). Verify the `tokio_websockets::ClientBuilder` API against the in-tree 0.13.2 before finalizing (see Task 2 Step 3 note); the shape below matches 0.13's `ClientBuilder::new().uri(&str)?.connect()` returning `(WebSocketStream, _)` that is `Stream + Sink<Message>`:

```rust
use futures_util::{SinkExt, StreamExt};
use tokio_websockets::{ClientBuilder, Message};

type Ws = tokio_websockets::WebSocketStream<tokio_websockets::MaybeTlsStream<tokio::net::TcpStream>>;

pub struct SlackConnector {
    id: String,
    bot_token: String,
    app_token: String,
    base: String,
    client: reqwest::Client,
    ws: Option<Ws>,
    buf: VecDeque<InboundMsg>,
}

impl SlackConnector {
    pub fn new(
        id: impl Into<String>,
        bot_token: impl Into<String>,
        app_token: impl Into<String>,
    ) -> Self {
        Self::with_base(id, bot_token, app_token, "https://slack.com/api")
    }

    /// Construct against a custom API base (for tests against a closed port / mock server).
    pub fn with_base(
        id: impl Into<String>,
        bot_token: impl Into<String>,
        app_token: impl Into<String>,
        base: impl Into<String>,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .expect("reqwest client");
        Self {
            id: id.into(),
            bot_token: bot_token.into(),
            app_token: app_token.into(),
            base: base.into(),
            client,
            ws: None,
            buf: VecDeque::new(),
        }
    }

    /// `apps.connections.open` -> the one-time wss URL. The URL carries a `ticket` secret and is
    /// NEVER logged; errors are stripped of any URL (without_url) before they can enter a log.
    async fn open_socket_url(&self) -> Result<String> {
        let resp = self
            .client
            .post(format!("{}/apps.connections.open", self.base))
            .bearer_auth(&self.app_token)
            .send()
            .await
            .map_err(reqwest::Error::without_url)
            .context("slack apps.connections.open")?;
        let json: Value = resp
            .json()
            .await
            .map_err(reqwest::Error::without_url)
            .context("slack apps.connections.open body")?;
        if json.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            // Surface the Slack error CODE (e.g. "invalid_auth") but never the token/url.
            let code = json.get("error").and_then(|v| v.as_str()).unwrap_or("unknown");
            anyhow::bail!("slack apps.connections.open not ok: {code}");
        }
        json.get("url")
            .and_then(|v| v.as_str())
            .map(String::from)
            .context("slack apps.connections.open: no url")
    }

    /// Connect (or reconnect) the Socket Mode WS. The wss URL is obtained fresh each time and never
    /// logged.
    async fn connect_ws(&mut self) -> Result<()> {
        let url = self.open_socket_url().await?;
        let (ws, _resp) = ClientBuilder::new()
            .uri(&url)
            .context("slack socket uri")?
            .connect()
            .await
            // A connect error can carry the ticket-bearing wss URL — do NOT propagate it verbatim.
            .map_err(|_| anyhow::anyhow!("slack socket connect failed"))?;
        self.ws = Some(ws);
        Ok(())
    }
}

#[async_trait]
impl Connector for SlackConnector {
    fn id(&self) -> &str {
        &self.id
    }

    async fn recv(&mut self) -> Result<Option<InboundMsg>> {
        loop {
            if let Some(m) = self.buf.pop_front() {
                return Ok(Some(m));
            }
            if self.ws.is_none() {
                // Connect; a failure (auth/network) propagates so drive_connector backs off 5s and
                // retries — same contract as Telegram's recv error path. The first connect's error
                // is what the no-leak test asserts on.
                self.connect_ws().await?;
            }
            let ws = self.ws.as_mut().expect("ws connected above");
            match ws.next().await {
                Some(Ok(msg)) => {
                    let Some(text) = msg.as_text() else { continue }; // ping/pong/binary -> ignore
                    let Ok(json) = serde_json::from_str::<Value>(text) else { continue };
                    match parse_envelope(&json) {
                        Envelope::EventsApi { envelope_id, payload } => {
                            // ACK within ~3s or Slack redelivers. If the ACK send fails, drop the
                            // socket and reconnect (the envelope will be redelivered).
                            if ws.send(Message::text(json!({"envelope_id": envelope_id}).to_string())).await.is_err() {
                                self.ws = None;
                                continue;
                            }
                            if let Some(m) = map_message(&payload, &self.id) {
                                self.buf.push_back(m);
                            }
                        }
                        Envelope::Disconnect => {
                            // Slack asks us to reconnect (refresh/warning) — drop + reopen.
                            self.ws = None;
                        }
                        Envelope::Hello | Envelope::Other => {} // ignore, keep reading
                    }
                }
                Some(Err(_)) | None => {
                    // WS error or clean close -> reconnect on the next loop. A persistent failure
                    // is bounded by reconnecting through open_socket_url (which can return Err ->
                    // drive_connector's 5s backoff).
                    self.ws = None;
                }
            }
        }
    }

    async fn send(&mut self, reply: OutboundMsg) -> Result<()> {
        // Slack rejects an empty message; practical text cap ~4000 chars. Clamp so a quirky reply
        // (empty/overlong) still delivers (the Telegram/Discord clamp_reply precedent).
        let text = crate::clamp_reply(&reply.text, 4000);
        self.client
            .post(format!("{}/chat.postMessage", self.base))
            .bearer_auth(&self.bot_token)
            .json(&json!({"channel": reply.chat, "text": text}))
            .send()
            .await
            .map_err(reqwest::Error::without_url)
            .context("slack chat.postMessage")?
            .error_for_status()
            .map_err(reqwest::Error::without_url)
            .context("slack chat.postMessage status")?;
        Ok(())
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p sa-connectors --features slack slack::` → PASS (pure tests + no-leak test).

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt --all && cargo clippy -p sa-connectors --features slack --all-targets -- -D warnings
git add crates/sa-connectors/src/slack.rs
git commit -m "feat(slack): SlackConnector Socket Mode recv/send (phase 5b)

apps.connections.open (xapp-) -> wss -> envelope loop with per-envelope ACK ->
map_message; chat.postMessage (xoxb-) with clamp_reply. Reconnect on disconnect/
close. Both tokens vault-held + never logged; the ticket-bearing wss URL and any
URL-bearing error are stripped. Tokens-never-leak regression test.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
Claude-Session: phase-5b" # self-audit-ok
```

---

### Task 4: gateway `"slack"` arm + bin enables the feature + M3 tuple-dispatch test

**Files:**
- Modify: `secretagent/src/gateway.rs` (`construct_connector` — add a `"slack"` arm; add a mock-dispatch test proving M3 keys on the tuple).
- Modify: `secretagent/Cargo.toml` (enable the `slack` feature on the sa-connectors dep).

**Interfaces:**
- Consumes: `SlackConnector::new` (Task 3); `load_token` (gateway.rs); `dispatch_inbound` + `MockConnector` (existing).

- [ ] **Step 1: Write the failing test** — append to `gateway.rs` `#[cfg(test)] mod tests`. This proves the M3 boundary keys on the full `"<team>:<user>"` tuple (the security property that motivated the tuple identity), reusing `MockConnector` (no network):

```rust
#[tokio::test]
async fn slack_tuple_identity_is_what_m3_allow_lists() {
    // A binding allow-listing the OWNER's (team,user) tuple accepts the owner but rejects a
    // same-user-id sender from a DIFFERENT workspace (the cross-workspace collision the tuple
    // identity prevents). Pure boundary test via dispatch_inbound — no Slack, no network.
    let (agent, registry, policy, mut audit) = test_agent().await;
    let bind = ConnectorConfig {
        name: "slack".into(),
        kind: "slack".into(),
        allow_senders: vec!["T_OWNER:U_ME".into()],
        ..Default::default()
    };
    let owner = InboundMsg { connector: "slack".into(), sender: "T_OWNER:U_ME".into(), chat: "C1".into(), text: "hi".into() };
    let imposter = InboundMsg { connector: "slack".into(), sender: "T_EVIL:U_ME".into(), chat: "C1".into(), text: "hi".into() };
    let ok = dispatch_inbound(&agent, &bind, &owner, &registry, &policy, &mut audit).await.unwrap();
    assert!(ok.is_some(), "the allow-listed owner tuple is accepted");
    let rejected = dispatch_inbound(&agent, &bind, &imposter, &registry, &policy, &mut audit).await.unwrap();
    assert!(rejected.is_none(), "a same-user-id sender from another workspace is rejected by M3");
}
```

> Check the existing gateway tests for the real helper name that builds `(agent, registry, policy, audit)` — the plan calls it `test_agent()`; if the file names it differently (e.g. an inline construction), mirror that exact setup. Reuse the existing `binding(...)` helper if it already takes `allow_senders`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p secretagent slack_tuple_identity` (Windows is fine — no Slack network).
Expected: FAIL — until the test helper compiles / the assertion is wired. (Conceptually the M3 code already keys on the exact string, so the test should pass once it compiles — if so, it documents + locks the property; keep it.)

- [ ] **Step 3: Wire the gateway arm + bin feature.**

In `secretagent/src/gateway.rs` `construct_connector`, add after the `"email"` arm:

```rust
        "slack" => {
            // Bot token (xoxb-, chat.postMessage) from token_ref; app token (xapp-, Socket Mode)
            // from app_token_ref. Both vault-held, never logged.
            let bot = load_token(binding, &vault_for(binding, vault))?; // see note: reuse load_token
            let app_key = binding.app_token_ref.as_deref().ok_or_else(|| {
                anyhow::anyhow!("slack connector '{}' needs an app_token_ref (xapp- vault key-id)", binding.name)
            })?;
            let app = vault
                .get(app_key)?
                .ok_or_else(|| anyhow::anyhow!("vault has no token under '{app_key}'"))?;
            Ok(Some(Box::new(sa_connectors::slack::SlackConnector::new(
                binding.name.clone(),
                bot.expose_secret().to_string(),
                app.expose_secret().to_string(),
            ))))
        }
```

> Simplify to match the existing `load_token(binding, vault)` signature exactly (it takes `&ConnectorConfig, &AgeFileVault`): use `let bot = load_token(binding, vault)?;` for the `token_ref` bot token, then load `app_token_ref` inline as shown (the `vault_for` pseudo-call above is a placeholder — delete it, call `load_token(binding, vault)?`).

In `secretagent/Cargo.toml`, change the sa-connectors line to enable `slack`:

```toml
sa-connectors = { path = "../crates/sa-connectors", features = ["discord", "email", "slack"] }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p secretagent slack_tuple_identity` → PASS. Then `cargo build -p secretagent` (the `slack` arm compiles with the feature on).

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings
git add secretagent/src/gateway.rs secretagent/Cargo.toml Cargo.lock
git commit -m "feat(gateway): wire slack connector arm + enable slack feature (phase 5b)

construct_connector builds SlackConnector from the vault-held xoxb-/xapp- tokens
(token_ref + app_token_ref). M3 dispatch test proves allow_senders keys on the
full (team,user) tuple, rejecting a cross-workspace same-user-id imposter.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
Claude-Session: phase-5b" # self-audit-ok
```

---

### Task 5: adversarial-review Workflow + both-venue gate + docs + push + CI

- [ ] **Step 1: Multi-lens adversarial-review Workflow** on the 5b boundary (the new untrusted-input surface). Lenses:
  - **M3 bypass:** can a non-allow-listed `(team,user)` ever reach `run_task`? (envelope spoofing `team_id`/`user`, missing-id fallthrough, `subtype`/`bot_id` skip-evasion).
  - **Secret-leak:** do the `xoxb-`/`xapp-` tokens or the ticket-bearing wss URL ever hit a log/error/audit? (connect error, `apps.connections.open` error body, `chat.postMessage` 4xx, the `tracing` calls).
  - **Reply-loop / DoS:** can the bot's own messages re-enter? can the envelope-ACK loop be wedged or spun (a malformed envelope, an un-ACK'able flood, a reconnect storm)?
  - **Provenance:** is the Slack input stamped `Untrusted{source}` and re-fed as data, never an instruction? does a Slack-seeded draft skill stay inert for a later Operator run (no auto-activate)?
  - **Injection-via-content:** a message whose text says "ignore your rules / run execute_code on host X" — does it stay tool-role data, and can it influence backend selection? (it must not — backend is frozen config, 5a).

  Run it as a `Workflow` (the 5a precedent: dimensions → adversarial verify). Fix each confirmed finding as its own commit.

- [ ] **Step 2: Both-venue gate.**

```bash
# Windows
cargo test --all
# WSL (Linux/landlock venue)
wsl.exe bash -c 'export PATH="$HOME/.cargo/bin:$PATH" CARGO_TARGET_DIR="$HOME/sa-target"; cd /mnt/c/Users/dnoye/ClaudeSecondBrain/SecretAgent; cargo test --all; echo CARGO_EXIT=$?'
```

Both green. Then rustls/C-lib purity unchanged:

```bash
wsl.exe bash -c 'export PATH="$HOME/.cargo/bin:$PATH"; cd /mnt/c/Users/dnoye/ClaudeSecondBrain/SecretAgent; cargo tree -e features -p secretagent | grep -iE "openssl|native-tls|aws-lc-sys|zstd-sys"' # expect EMPTY
wsl.exe bash -c 'export PATH="$HOME/.cargo/bin:$PATH"; cd /mnt/c/Users/dnoye/ClaudeSecondBrain/SecretAgent; cargo deny check' # green (add a license to the allow-list deliberately if tokio-websockets flags one)
```

- [ ] **Step 3: Update docs + memory.** `PROGRESS.md` (5b row: Slack Socket Mode connector shipped, acceptance (a) complete pending the operator-gated live E2E), `ROADMAP.md` (Phase 5 slice 5b done), `README.md` (Slack listed among connectors + the `[[connectors]] kind="slack"` config snippet with `token_ref`/`app_token_ref`/the `"<team>:<user>"` `allow_senders` note). Commit.

- [ ] **Step 4: Push + watch CI.**

```bash
git push origin master
RUN=$(/c/Program\ Files/GitHub\ CLI/gh.exe run list --branch master --limit 6 --json databaseId,headSha | ...)  # the run whose headSha == HEAD
/c/Program\ Files/GitHub\ CLI/gh.exe run watch "$RUN" --exit-status --interval 25
```

Green on all 5 jobs.

- [ ] **Step 5: Operator-gated live E2E (defer, document).** A Slack app with `xapp-` (Socket Mode app-level, `connections:write`) + `xoxb-` (bot, `chat:write` + the `message.im`/`message.channels` event subscriptions) tokens, the bot added to a channel/DM, and the operator's `(team_id, user_id)`. Store both via the `!`-prefixed `vault set` (never paste a token into the chat). Then `[[connectors]] kind="slack" token_ref=… app_token_ref=… allow_senders=["T…:U…"] allow_tools=["execute_code"]` + `[exec] backend="docker" image="alpine"` and a Slack message → a Docker-backed `execute_code` runs → reply (acceptance (a) end-to-end). Record the result in memory like the Telegram E2E.

---

## Self-Review

- ADR §5 (4th Connector, Socket Mode, `(team,user)` identity, tokens from vault, defer the long-tail) → Tasks 1–4 ✅
- Reuse the 4c trait + M3 + `RunContext::remote` verbatim → no change to `dispatch_inbound`/`Connector`; only a new impl + a `"slack"` arm ✅ (Task 4 test proves M3 keys on the tuple)
- Skip bot/own/subtype (no reply loops) → `map_message` (Task 2) ✅
- Tokens never logged + ticket-bearing wss URL never logged → `without_url` + the generic connect-error + the no-leak test (Task 3) ✅
- Signing-secret HMAC N/A under Socket Mode → not implemented; noted as a revisit trigger ✅
- WS dep is rustls/in-tree, no new audit surface → `tokio-websockets` pinned to the twilight-resolved 0.13, purity re-verified (Task 2 + Task 5) ✅
- Adversarial-review Workflow before push → Task 5 Step 1 ✅
- Live E2E operator-gated, built testable-without → Task 5 Step 5 (the Telegram precedent) ✅
- **Known-unknown flagged honestly:** the exact `tokio-websockets` 0.13 `ClientBuilder`/`MaybeTlsStream` API + feature names are verified at execution (Task 2 Step 3 note, Task 3 Step 3 note) rather than asserted from memory — the TDD compile cycle surfaces any mismatch.
