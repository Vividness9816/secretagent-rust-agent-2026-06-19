//! Discord connector — twilight poll-based shard (`recv` = `next_event` loop) + twilight-http
//! `create_message` (`send`). The shard is a `Stream<Item = Result<Message, _>>`; `next_event`
//! polls it and yields the next gateway `Event`. We only drive on `MESSAGE_CREATE` text from
//! non-bot authors — every other event loops (never `Ok(None)`, which is reserved for a genuinely
//! closed shard stream). The bot token is injected at construction (from the vault) — never logged.
//!
//! TLS/musl: twilight is pinned to `rustls-webpki-roots` (no platform-verifier, no native-tls) and
//! the pure-Rust `zlib` transport compression (NOT the default `zstd`, which links a C lib). The
//! binary therefore stays musl-static with a rustls-only crypto surface.

use crate::{Connector, InboundMsg, OutboundMsg};
use anyhow::{Context, Result};
use async_trait::async_trait;
use twilight_gateway::{Event, EventTypeFlags, Intents, Shard, ShardId, StreamExt};
use twilight_http::Client as HttpClient;
use twilight_model::channel::Message as DiscordMessage;
use twilight_model::id::{marker::ChannelMarker, Id};

pub struct DiscordConnector {
    id: String,
    shard: Shard,
    http: HttpClient,
}

impl DiscordConnector {
    pub fn new(id: impl Into<String>, token: impl Into<String>) -> Self {
        let token = token.into();
        // GUILD_MESSAGES delivers MESSAGE_CREATE; MESSAGE_CONTENT (a privileged intent) is what
        // makes `content` non-empty for messages not addressed to the bot. Both are required to
        // receive readable text — without MESSAGE_CONTENT, content arrives blank and every message
        // is skipped (mapped to None).
        let intents = Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT;
        let shard = Shard::new(ShardId::ONE, token.clone(), intents);
        let http = HttpClient::new(token);
        Self {
            id: id.into(),
            shard,
            http,
        }
    }
}

/// PURE map of a Discord `MessageCreate` to an `InboundMsg`, or `None` if it should be skipped
/// (a bot author — including ourselves, which prevents a reply loop — or empty content). Content
/// is NEVER trusted: the gateway stamps it `Untrusted`. `sender` is the author id (the M3
/// identity); `chat` is the channel id to reply to. Kept pure so it is unit-testable with no
/// network (we cannot live-test a connector).
pub fn map_message(msg: &DiscordMessage, connector: &str) -> Option<InboundMsg> {
    if msg.author.bot {
        return None;
    }
    let text = msg.content.trim();
    if text.is_empty() {
        return None;
    }
    Some(InboundMsg {
        connector: connector.to_string(),
        sender: msg.author.id.get().to_string(),
        chat: msg.channel_id.get().to_string(),
        text: msg.content.clone(),
    })
}

#[async_trait]
impl Connector for DiscordConnector {
    fn id(&self) -> &str {
        &self.id
    }

    async fn recv(&mut self) -> Result<Option<InboundMsg>> {
        // Poll the shard until a real, mappable message arrives. A non-message event, a bot/empty
        // message (skipped by map_message), or a per-event receive error all LOOP — only a closed
        // shard stream (`next_event` → None) ends the connector. A receive error on a single event
        // is transient (reconnect/decode); it is surfaced to the caller's retry/backoff is overkill
        // here because the shard reconnects internally, so we just continue.
        loop {
            let item = match self.shard.next_event(EventTypeFlags::MESSAGE_CREATE).await {
                Some(item) => item,
                None => return Ok(None), // shard stream closed → clean shutdown
            };
            match item {
                Ok(Event::MessageCreate(m)) => {
                    if let Some(msg) = map_message(&m.0, &self.id) {
                        return Ok(Some(msg));
                    }
                    // bot/empty message — keep polling
                }
                Ok(_) => {} // some other event slipped through the filter — ignore, keep polling
                Err(e) => {
                    // A single receive/decode error is not fatal: the shard reconnects internally.
                    // Log without the token (twilight errors don't carry it) and keep polling.
                    tracing::warn!("discord: receive error: {e} — continuing");
                }
            }
        }
    }

    async fn send(&mut self, reply: OutboundMsg) -> Result<()> {
        let channel_id: Id<ChannelMarker> = Id::new(
            reply
                .chat
                .parse::<u64>()
                .with_context(|| format!("discord chat id is not a u64: {:?}", reply.chat))?,
        );
        // Discord rejects an empty message and caps at 2000 chars — clamp so a quirky model reply
        // (empty or overlong) still delivers instead of erroring.
        let content = crate::clamp_reply(&reply.text, 2000);
        self.http
            .create_message(channel_id)
            .content(&content)
            .await
            .context("discord create_message")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use twilight_model::id::Id;
    use twilight_model::user::User;
    use twilight_model::util::Timestamp;

    // Build a minimal Discord Message for the pure-mapping tests. We construct only the fields
    // map_message reads; the rest are defaulted to the type's zero/empty value.
    // `Message::interaction` is deprecated (use interaction_metadata) but is still a required field
    // of the struct literal, so the literal must set it — allow the deprecation just here.
    #[allow(deprecated)]
    fn msg(author_id: u64, bot: bool, channel_id: u64, content: &str) -> DiscordMessage {
        let user = User {
            accent_color: None,
            avatar: None,
            avatar_decoration: None,
            avatar_decoration_data: None,
            banner: None,
            bot,
            discriminator: 0,
            email: None,
            flags: None,
            global_name: None,
            id: Id::new(author_id),
            locale: None,
            mfa_enabled: None,
            name: "u".into(),
            premium_type: None,
            primary_guild: None,
            public_flags: None,
            system: None,
            verified: None,
        };
        DiscordMessage {
            activity: None,
            application: None,
            application_id: None,
            attachments: Vec::new(),
            author: user,
            call: None,
            channel_id: Id::new(channel_id),
            components: Vec::new(),
            content: content.into(),
            edited_timestamp: None,
            embeds: Vec::new(),
            flags: None,
            guild_id: None,
            id: Id::new(1),
            interaction: None,
            interaction_metadata: None,
            kind: twilight_model::channel::message::MessageType::Regular,
            member: None,
            mention_channels: Vec::new(),
            mention_everyone: false,
            mention_roles: Vec::new(),
            mentions: Vec::new(),
            message_snapshots: Vec::new(),
            pinned: false,
            poll: None,
            reactions: Vec::new(),
            reference: None,
            referenced_message: None,
            role_subscription_data: None,
            sticker_items: Vec::new(),
            thread: None,
            timestamp: Timestamp::from_secs(1).unwrap(),
            tts: false,
            webhook_id: None,
        }
    }

    #[test]
    fn map_message_extracts_human_text() {
        let m = msg(42, false, 99, "hello");
        let got = map_message(&m, "discord").expect("a human text message maps");
        assert_eq!(
            got,
            InboundMsg {
                connector: "discord".into(),
                sender: "42".into(),
                chat: "99".into(),
                text: "hello".into(),
            }
        );
    }

    #[test]
    fn map_message_skips_bot_authors() {
        // A bot author (including ourselves) is skipped — this is what prevents a reply loop.
        assert!(map_message(&msg(7, true, 8, "I am a bot"), "discord").is_none());
    }

    #[test]
    fn map_message_skips_empty_or_whitespace_content() {
        assert!(map_message(&msg(1, false, 2, ""), "discord").is_none());
        assert!(map_message(&msg(1, false, 2, "   \n  "), "discord").is_none());
    }
}
