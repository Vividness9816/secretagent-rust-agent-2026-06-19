//! Slack connector — Socket Mode (an outbound WSS; no public inbound endpoint, fitting a NAT'd
//! daemon). Receive: `apps.connections.open` (xapp- token) -> wss URL -> envelopes; ACK every
//! `events_api` envelope; map `message` events to `InboundMsg`. Send: `chat.postMessage` (xoxb-
//! token). Identity = the (team_id, user_id) tuple encoded `"<team>:<user>"` so M3 keys on it
//! (a user in another workspace can never collide with the registered owner). Both tokens come
//! from the vault — never logged; the wss URL carries a one-time ticket secret and is never logged
//! either. Parse/map are pure (the Telegram `parse_updates` / Discord `map_message` precedent).

use crate::{Connector, InboundMsg, OutboundMsg};
use anyhow::{Context, Result};
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::time::Duration;
use tokio_websockets::{ClientBuilder, MaybeTlsStream, Message, WebSocketStream};

/// The concrete Socket Mode WS stream type returned by `ClientBuilder::connect()`.
type Ws = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// A Socket Mode envelope, classified for the recv loop. `events_api` carries the inner event
/// payload AND an `envelope_id` that MUST be ACK'd over the WS within ~3s or Slack redelivers.
#[derive(Debug, PartialEq, Eq)]
pub enum Envelope {
    Hello,
    Disconnect,
    EventsApi { envelope_id: String, payload: Value },
    Other,
}

/// PURE: classify a Socket Mode envelope. Unknown/irrelevant types (or an `events_api` with no
/// `envelope_id` to ACK) -> `Other` (ignored). No network, no secrets — unit-testable.
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
                Envelope::EventsApi {
                    envelope_id,
                    payload,
                }
            }
        }
        _ => Envelope::Other,
    }
}

/// PURE: map an `events_api` payload to an `InboundMsg`, or `None` to skip. Skips non-`message`
/// events, bot/own messages (`bot_id` present), edits/joins/etc (`subtype` present), empty text,
/// and any missing team/user/channel id. Content is NEVER trusted — the gateway stamps it
/// `Untrusted`. `sender` = "<team_id>:<user_id>" (the M3 identity).
pub fn map_message(payload: &Value, connector: &str) -> Option<InboundMsg> {
    let event = payload.get("event")?;
    if event.get("type").and_then(|v| v.as_str()) != Some("message") {
        return None;
    }
    // Skip bots (incl. ourselves -> prevents reply loops) and non-user subtypes (edits, joins, ...).
    if event.get("bot_id").is_some() || event.get("subtype").is_some() {
        return None;
    }
    let text = event
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if text.is_empty() {
        return None;
    }
    // Identity = (team_id, user_id) tuple. team_id is on the payload; user is on the event.
    let team = payload
        .get("team_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
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

pub struct SlackConnector {
    id: String,
    bot_token: String,
    app_token: String,
    base: String,
    client: reqwest::Client,
    ws: Option<Ws>,
    buf: VecDeque<InboundMsg>,
    /// Recently-seen envelope ids — Socket Mode is at-least-once, so a redelivered envelope must
    /// not be processed (and possibly side-effected) twice. Bounded ring (see `note_envelope`).
    seen: VecDeque<String>,
}

/// Cap on the recent-envelope dedup ring (`seen`). A redelivery arrives within seconds of the
/// original, so a small recent window suffices; older ids age out.
const SEEN_CAP: usize = 256;

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
            seen: VecDeque::new(),
        }
    }

    /// `apps.connections.open` -> the one-time wss URL. The URL carries a `ticket` secret and is
    /// NEVER logged; errors are stripped of any URL (without_url) before they can enter a log. The
    /// xapp- token rides in the bearer header (never the URL), which reqwest never includes in errors.
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
            let code = json
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            anyhow::bail!("slack apps.connections.open not ok: {code}");
        }
        json.get("url")
            .and_then(|v| v.as_str())
            .map(String::from)
            .context("slack apps.connections.open: no url")
    }

    /// Connect (or reconnect) the Socket Mode WS. The wss URL is obtained fresh each time and never
    /// logged; BOTH the URI-validation error and the connect error can carry the ticket-bearing URL,
    /// so each is replaced with a generic message rather than propagated verbatim.
    async fn connect_ws(&mut self) -> Result<()> {
        let url = self.open_socket_url().await?;
        let (ws, _resp) = build_socket_client(&url)?
            .connect()
            .await
            .map_err(|_| anyhow::anyhow!("slack socket connect failed"))?;
        self.ws = Some(ws);
        Ok(())
    }
}

/// Build the Socket Mode client for `url`, stripping the ticket-bearing URL from any URI-validation
/// error — `ClientBuilder::uri`'s `InvalidUri` embeds the offending URL, which would leak the
/// one-time ticket if it reached a log via `{:#}` (the gateway logs recv errors). Pure → testable.
fn build_socket_client(url: &str) -> Result<ClientBuilder<'static>> {
    ClientBuilder::new()
        .uri(url)
        .map_err(|_| anyhow::anyhow!("slack socket uri invalid (url withheld)"))
}

/// Record `id` in the bounded recent-envelope ring; return true if it is NEW (process it), false if
/// already seen (a Socket Mode redelivery — skip, so a side-effect-armed message never runs twice).
/// ponytail: linear scan over a <=SEEN_CAP ring; swap to a HashSet+ring if envelope volume ever
/// makes this hot.
fn note_envelope(seen: &mut VecDeque<String>, id: &str, cap: usize) -> bool {
    if seen.iter().any(|s| s == id) {
        return false;
    }
    seen.push_back(id.to_string());
    if seen.len() > cap {
        seen.pop_front();
    }
    true
}

#[async_trait]
impl Connector for SlackConnector {
    fn id(&self) -> &str {
        &self.id
    }

    async fn recv(&mut self) -> Result<Option<InboundMsg>> {
        loop {
            // `buf` holds <=1: we read ONE ws frame per loop and return as soon as it yields a
            // message, so a flood is bounded by TCP backpressure, not unbounded local memory.
            if let Some(m) = self.buf.pop_front() {
                return Ok(Some(m));
            }
            if self.ws.is_none() {
                // Connect; a failure (auth/network) propagates so drive_connector backs off 5s and
                // retries — the same contract as Telegram's recv error path. The first connect's
                // error is what the no-leak test asserts on.
                self.connect_ws().await?;
            }
            let ws = self.ws.as_mut().expect("ws connected above");
            match ws.next().await {
                Some(Ok(msg)) => {
                    // ping/pong/binary -> ignore (the lib auto-replies to pings as we poll).
                    let Some(text) = msg.as_text() else { continue };
                    let Ok(value) = serde_json::from_str::<Value>(text) else {
                        continue;
                    };
                    match parse_envelope(&value) {
                        Envelope::EventsApi {
                            envelope_id,
                            payload,
                        } => {
                            // ACK within ~3s or Slack redelivers. If the ACK send fails, drop the
                            // socket and reconnect (the envelope will be redelivered, NOT yet
                            // recorded as seen, so it processes exactly once on redelivery).
                            let ack = json!({ "envelope_id": &envelope_id }).to_string();
                            if ws.send(Message::text(ack)).await.is_err() {
                                self.ws = None;
                                continue;
                            }
                            // At-least-once: a post-ACK socket drop (ACK flushed locally but not
                            // received by Slack) triggers a redelivery. Dedup so a side-effect-armed
                            // message never runs twice; an already-seen id is skipped (already ACK'd).
                            if !note_envelope(&mut self.seen, &envelope_id, SEEN_CAP) {
                                continue;
                            }
                            if let Some(m) = map_message(&payload, &self.id) {
                                self.buf.push_back(m);
                            }
                        }
                        // Slack asks us to reconnect (refresh/warning) — drop + reopen next loop.
                        Envelope::Disconnect => self.ws = None,
                        // hello / anything else — ignore, keep reading.
                        Envelope::Hello | Envelope::Other => {}
                    }
                }
                // WS error or clean close -> reconnect on the next loop. A persistent failure is
                // bounded by reconnecting through open_socket_url (which can return Err -> the
                // gateway's 5s backoff).
                Some(Err(_)) | None => self.ws = None,
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
            .json(&json!({ "channel": reply.chat, "text": text }))
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
        assert_eq!(
            parse_envelope(&json!({"type": "slash_commands"})),
            Envelope::Other
        );
        // An events_api with no envelope_id can't be ACK'd -> Other (don't process a non-ackable).
        assert_eq!(
            parse_envelope(&json!({"type": "events_api", "payload": {}})),
            Envelope::Other
        );
        let env = json!({
            "type": "events_api",
            "envelope_id": "env-123",
            "payload": {"team_id": "T1", "event": {"type": "message"}}
        });
        match parse_envelope(&env) {
            Envelope::EventsApi {
                envelope_id,
                payload,
            } => {
                assert_eq!(envelope_id, "env-123");
                assert_eq!(payload["team_id"], "T1");
            }
            other => panic!("expected EventsApi, got {other:?}"),
        }
    }

    fn payload(team: &str, event: Value) -> Value {
        json!({"team_id": team, "event": event})
    }

    #[tokio::test]
    async fn tokens_never_leak_in_a_connect_error() {
        // apps.connections.open points at a closed port -> the request fails. Neither token nor the
        // base must appear in the formatted error chain (without_url strips the URL; the bearer
        // tokens are headers reqwest never includes in errors). Regression for the 4c secret-leak.
        let mut c = SlackConnector::with_base(
            "slack",
            "xoxb-SECRET-BOT",
            "xapp-SECRET-APP",
            "http://127.0.0.1:1",
        );
        let err = c
            .recv()
            .await
            .expect_err("a request to a closed port must error");
        let shown = format!("{err:#}");
        assert!(
            !shown.contains("xoxb-SECRET-BOT"),
            "bot token leaked: {shown}"
        );
        assert!(
            !shown.contains("xapp-SECRET-APP"),
            "app token leaked: {shown}"
        );
    }

    #[test]
    fn malformed_socket_uri_error_hides_the_ticket() {
        // adversarial-review HIGH: a malformed wss URL (a compromised/MITM'd apps.connections.open
        // response) must NOT leak its one-time `ticket` secret through the URI-validation error —
        // ClientBuilder::uri's InvalidUri embeds the URL. build_socket_client strips it.
        let err = match build_socket_client("wss://bad host/link?ticket=SECRET-TICKET-9f3") {
            Err(e) => e,
            Ok(_) => panic!("a malformed URI must error"),
        };
        let shown = format!("{err:#}");
        assert!(
            !shown.contains("SECRET-TICKET-9f3"),
            "the ticket secret must never appear in the uri error: {shown}"
        );
    }

    #[test]
    fn note_envelope_dedups_redeliveries_and_bounds_the_ring() {
        // adversarial-review HIGH: a redelivered envelope (same id) must be skipped so a
        // side-effect-armed message never runs twice; the ring is bounded.
        let mut seen = VecDeque::new();
        assert!(
            note_envelope(&mut seen, "e1", 3),
            "first sighting -> process"
        );
        assert!(
            !note_envelope(&mut seen, "e1", 3),
            "redelivery of e1 -> skip"
        );
        assert!(note_envelope(&mut seen, "e2", 3));
        assert!(note_envelope(&mut seen, "e3", 3));
        assert!(note_envelope(&mut seen, "e4", 3), "fourth id evicts e1");
        assert_eq!(seen.len(), 3, "ring stays bounded at cap");
        // e1 has aged out of the bounded window — treated as new again (the ring only guards the
        // recent redelivery window, which is all Socket Mode redelivery needs).
        assert!(note_envelope(&mut seen, "e1", 3));
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
            &payload(
                "T1",
                json!({"type": "message", "bot_id": "B1", "user": "U1", "channel": "C1", "text": "x"})
            ),
            "slack"
        )
        .is_none());
        // A non-user event subtype (message_changed, channel_join, ...) -> skipped.
        assert!(map_message(
            &payload(
                "T1",
                json!({"type": "message", "subtype": "message_changed", "user": "U1", "channel": "C1", "text": "x"})
            ),
            "slack"
        )
        .is_none());
        // Empty/whitespace text -> skipped.
        assert!(map_message(
            &payload(
                "T1",
                json!({"type": "message", "user": "U1", "channel": "C1", "text": "   "})
            ),
            "slack"
        )
        .is_none());
        // A non-message event (reaction_added, etc) -> skipped.
        assert!(map_message(
            &payload(
                "T1",
                json!({"type": "reaction_added", "user": "U1", "channel": "C1"})
            ),
            "slack"
        )
        .is_none());
        // Missing team_id or user -> skipped (can't form a safe identity).
        assert!(map_message(
            &payload(
                "",
                json!({"type": "message", "user": "U1", "channel": "C1", "text": "x"})
            ),
            "slack"
        )
        .is_none());
        assert!(map_message(
            &payload(
                "T1",
                json!({"type": "message", "channel": "C1", "text": "x"})
            ),
            "slack"
        )
        .is_none());
    }
}
