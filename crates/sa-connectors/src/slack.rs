//! Slack connector — Socket Mode (an outbound WSS; no public inbound endpoint, fitting a NAT'd
//! daemon). Receive: `apps.connections.open` (xapp- token) -> wss URL -> envelopes; ACK every
//! `events_api` envelope; map `message` events to `InboundMsg`. Send: `chat.postMessage` (xoxb-
//! token). Identity = the (team_id, user_id) tuple encoded `"<team>:<user>"` so M3 keys on it
//! (a user in another workspace can never collide with the registered owner). Both tokens come
//! from the vault — never logged; the wss URL carries a one-time ticket secret and is never logged
//! either. Parse/map are pure (the Telegram `parse_updates` / Discord `map_message` precedent).

use crate::InboundMsg;
use serde_json::Value;

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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
