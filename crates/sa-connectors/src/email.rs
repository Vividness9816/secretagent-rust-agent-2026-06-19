//! Email connector — IMAP poll (receive) + SMTP send. recv() polls the INBOX for UNSEEN mail and
//! maps each to an `InboundMsg` (sender = From address, chat = From address = the reply target,
//! text = plain body); send() delivers an SMTP message to `chat`. The IMAP/SMTP password is
//! injected at construction (from the vault) — never logged.
//!
//! TLS/musl: IMAP runs over a caller-supplied `tokio-rustls` TLS stream (async-imap is given the
//! stream via `Client::new`; NO async-native-tls). SMTP uses lettre's `tokio1-rustls-tls`. Trust
//! anchors come from `webpki-roots`. The binary therefore stays musl-static with a rustls-only
//! crypto surface — no openssl, no native-tls.

use crate::{Connector, InboundMsg, OutboundMsg};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use futures_util::TryStreamExt;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message as SmtpMessage, Tokio1Executor};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use tokio_rustls::TlsConnector;

/// Static config for the email connector. `password` is the resolved secret (from the vault); the
/// caller never passes a vault key-id here. All other fields are operator-set transport addresses.
#[derive(Clone)]
pub struct EmailConfig {
    pub id: String,
    pub imap_host: String,
    pub imap_port: u16,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub username: String,
    /// The envelope/header From for outgoing replies (defaults to `username` if empty).
    pub from: String,
    pub password: String,
}

pub struct EmailConnector {
    cfg: EmailConfig,
    /// FIFO of messages fetched in the last successful poll but not yet returned to the caller.
    buf: std::collections::VecDeque<InboundMsg>,
    /// Poll interval when the INBOX has no unseen mail (so recv loops without busy-spinning).
    poll_interval: Duration,
}

impl EmailConnector {
    pub fn new(cfg: EmailConfig) -> Self {
        Self {
            cfg,
            buf: std::collections::VecDeque::new(),
            poll_interval: Duration::from_secs(30),
        }
    }

    /// The address to put in outgoing mail's From (falls back to the login username).
    fn reply_from(&self) -> &str {
        if self.cfg.from.is_empty() {
            &self.cfg.username
        } else {
            &self.cfg.from
        }
    }

    /// Open a TLS stream to the IMAP host using rustls + webpki-roots (no native-tls). Returned as
    /// a `tokio-rustls` stream that async-imap drives directly (it implements tokio AsyncRead/Write
    /// under the `runtime-tokio` feature).
    async fn imap_tls_stream(&self) -> Result<tokio_rustls::client::TlsStream<TcpStream>> {
        let root_store = RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        let config = ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();
        let connector = TlsConnector::from(Arc::new(config));
        let dnsname = ServerName::try_from(self.cfg.imap_host.clone())
            .with_context(|| format!("invalid IMAP host name: {}", self.cfg.imap_host))?;
        let tcp = TcpStream::connect((self.cfg.imap_host.as_str(), self.cfg.imap_port))
            .await
            .context("connect IMAP TCP")?;
        let tls = connector
            .connect(dnsname, tcp)
            .await
            .context("IMAP TLS handshake")?;
        Ok(tls)
    }

    /// Connect + log in + SELECT INBOX + fetch every UNSEEN message into `self.buf`, marking each
    /// \Seen so it is not re-delivered. Returns the number of messages fetched. A failure here is
    /// surfaced to the caller (which logs + backs off) — never silently dropped, and the password
    /// never enters an error (async-imap errors don't carry it).
    async fn poll_inbox(&mut self) -> Result<usize> {
        let tls = self.imap_tls_stream().await?;
        let client = async_imap::Client::new(tls);
        let mut session = client
            .login(&self.cfg.username, &self.cfg.password)
            .await
            .map_err(|(e, _client)| anyhow!("IMAP login failed: {e}"))?;

        session.select("INBOX").await.context("IMAP SELECT INBOX")?;
        let unseen = session
            .uid_search("UNSEEN")
            .await
            .context("IMAP UID SEARCH UNSEEN")?;

        let mut fetched = 0usize;
        for uid in unseen {
            // RFC822 = the full raw message; we parse From + plain body from it ourselves. UID
            // FETCH keeps us aligned with the UID SEARCH above even if the mailbox changes.
            let set = uid.to_string();
            let mut stream = session
                .uid_fetch(&set, "RFC822")
                .await
                .context("IMAP UID FETCH")?;
            while let Some(item) = stream.try_next().await.context("IMAP fetch stream")? {
                if let Some(body) = item.body() {
                    if let Some(msg) = parse_inbound_email(body, &self.cfg.id) {
                        self.buf.push_back(msg);
                        fetched += 1;
                    }
                }
            }
            drop(stream);
            // Mark \Seen so the message is not re-delivered on the next poll.
            let mut store = session
                .uid_store(&set, "+FLAGS (\\Seen)")
                .await
                .context("IMAP UID STORE \\Seen")?;
            while store
                .try_next()
                .await
                .context("IMAP store stream")?
                .is_some()
            {}
        }

        let _ = session.logout().await; // best-effort; a logout failure must not lose fetched mail
        Ok(fetched)
    }

    async fn send_smtp(&self, reply: &OutboundMsg) -> Result<()> {
        let email = SmtpMessage::builder()
            .from(
                self.reply_from()
                    .parse()
                    .with_context(|| format!("invalid From address: {}", self.reply_from()))?,
            )
            .to(reply
                .chat
                .parse()
                .with_context(|| format!("invalid recipient address: {}", reply.chat))?)
            .subject("Re: your message")
            .body(reply.text.clone())
            .context("build SMTP message")?;

        let creds = Credentials::new(self.cfg.username.clone(), self.cfg.password.clone());
        let mailer: AsyncSmtpTransport<Tokio1Executor> =
            AsyncSmtpTransport::<Tokio1Executor>::relay(&self.cfg.smtp_host)
                .context("SMTP relay config")?
                .port(self.cfg.smtp_port)
                .credentials(creds)
                .build();
        mailer.send(email).await.context("SMTP send")?;
        Ok(())
    }
}

/// PURE parse of a raw RFC822 message into an `InboundMsg`, or `None` if it has no usable From
/// address (we need a reply target). `sender` and `chat` are both the From address (the M3
/// identity + the reply target); `text` is the plain-text body. Content is NEVER trusted — the
/// gateway stamps it `Untrusted`. Kept pure so it is unit-testable with no IMAP server.
pub fn parse_inbound_email(raw: &[u8], connector: &str) -> Option<InboundMsg> {
    use mailparse::{addrparse, parse_mail, MailAddr, MailHeaderMap};

    let parsed = parse_mail(raw).ok()?;
    let from_header = parsed.get_headers().get_first_value("From")?;
    // Take the first single address from the (possibly group/multi) From header. Iterate by
    // reference (MailAddrList derefs to Vec but does not own-iterate cheaply) and clone the addr.
    let parsed_addrs = addrparse(&from_header).ok()?;
    let addr = parsed_addrs.iter().find_map(|a| match a {
        MailAddr::Single(s) => Some(s.addr.clone()),
        MailAddr::Group(_) => None,
    })?;
    if addr.is_empty() {
        return None;
    }
    // Plain-text body: get_body() decodes the top-level part's transfer encoding. For a multipart
    // message this is the first part; good enough for the agent's text-driven loop.
    let text = parsed.get_body().unwrap_or_default();
    Some(InboundMsg {
        connector: connector.to_string(),
        sender: addr.clone(),
        chat: addr,
        text,
    })
}

#[async_trait]
impl Connector for EmailConnector {
    fn id(&self) -> &str {
        &self.cfg.id
    }

    async fn recv(&mut self) -> Result<Option<InboundMsg>> {
        // Return buffered mail first; otherwise poll the INBOX until at least one unseen message
        // arrives. An empty poll sleeps then re-polls — recv only returns once there IS a message,
        // so it never spuriously signals shutdown (Ok(None) is reserved for a closed transport,
        // which an indefinitely-polling mailbox never is). A poll error propagates so the gateway
        // can log + back off + retry.
        loop {
            if let Some(m) = self.buf.pop_front() {
                return Ok(Some(m));
            }
            let n = self.poll_inbox().await?;
            if n == 0 {
                tokio::time::sleep(self.poll_interval).await;
            }
        }
    }

    async fn send(&mut self, reply: OutboundMsg) -> Result<()> {
        self.send_smtp(&reply).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_inbound_email_extracts_from_and_plain_body() {
        let raw = b"From: Alice <alice@example.org>\r\n\
To: bot@example.org\r\n\
Subject: hi\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
\r\n\
hello there\r\n";
        let got = parse_inbound_email(raw, "email").expect("a message with a From maps");
        assert_eq!(got.connector, "email");
        assert_eq!(got.sender, "alice@example.org");
        assert_eq!(got.chat, "alice@example.org", "reply target = From address");
        assert_eq!(got.text.trim(), "hello there");
    }

    #[test]
    fn parse_inbound_email_handles_bare_address_from() {
        let raw = b"From: bob@example.com\r\nSubject: x\r\n\r\nbody\r\n";
        let got = parse_inbound_email(raw, "email").expect("bare address From maps");
        assert_eq!(got.sender, "bob@example.com");
    }

    #[test]
    fn parse_inbound_email_none_without_from() {
        let raw = b"To: bot@example.org\r\nSubject: x\r\n\r\nbody\r\n";
        assert!(
            parse_inbound_email(raw, "email").is_none(),
            "no From → no reply target → skipped"
        );
    }
}
