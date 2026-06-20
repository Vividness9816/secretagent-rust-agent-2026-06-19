use anyhow::Result;
use futures::stream::{BoxStream, StreamExt};
use sa_memory::{Store, StoredMsg};
use sa_providers::{ChatChunk, ChatMsg, Provider};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

pub struct Agent {
    // ponytail: one global lock around the store; per-session locks only if
    // concurrent sessions ever contend.
    store: Arc<Mutex<Store>>,
    provider: Box<dyn Provider>,
}

/// Build the model context: FTS5 recall on the input's keywords + recent history,
/// deduped by id, oldest-first, with the new user turn appended last.
pub fn assemble_context(store: &Store, session_id: &str, user_input: &str) -> Result<Vec<ChatMsg>> {
    let mut picked: Vec<StoredMsg> = Vec::new();
    let mut seen = HashSet::new();

    // Recall on each significant, alphanumeric-sanitized keyword. ponytail: simple
    // per-word terms; FTS5 phrase/operator escaping when free-text recall is needed.
    for raw in user_input.split_whitespace() {
        let kw: String = raw.chars().filter(|c| c.is_alphanumeric()).collect();
        if kw.len() < 3 {
            continue;
        }
        for m in store.recall(session_id, &kw, 3)? {
            if seen.insert(m.id) {
                picked.push(m);
            }
        }
    }
    for m in store.recent(session_id, 10)? {
        if seen.insert(m.id) {
            picked.push(m);
        }
    }
    picked.sort_by_key(|m| m.id);

    let mut ctx: Vec<ChatMsg> = picked
        .into_iter()
        .map(|m| ChatMsg {
            role: m.role,
            content: m.content,
        })
        .collect();
    ctx.push(ChatMsg {
        role: "user".into(),
        content: user_input.to_string(),
    });
    Ok(ctx)
}

impl Agent {
    pub fn new(store: Store, provider: Box<dyn Provider>) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
            provider,
        }
    }

    /// One turn: persist the user message, assemble context, call the provider, and
    /// (on stream completion) persist the accumulated assistant reply.
    pub async fn turn(
        &self,
        session_id: &str,
        user_input: &str,
    ) -> Result<BoxStream<'static, Result<ChatChunk>>> {
        let ctx = {
            let store = self.store.lock().unwrap();
            store.add_message(session_id, "user", user_input, "{}")?;
            assemble_context(&store, session_id, user_input)?
        };
        let upstream = self.provider.chat(ctx).await?;

        let store = self.store.clone();
        let session = session_id.to_string();
        let stream = async_stream::stream! {
            let mut acc = String::new();
            let mut upstream = upstream;
            while let Some(item) = upstream.next().await {
                match item {
                    Ok(c) => { acc.push_str(&c.0); yield Ok(c); }
                    Err(e) => { yield Err(e); }
                }
            }
            if let Ok(store) = store.lock() {
                let _ = store.add_message(&session, "assistant", &acc, "{}");
            }
        };
        Ok(Box::pin(stream))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sa_memory::Store;
    use sa_providers::{ChatChunk, MockProvider};

    async fn drain(mut s: BoxStream<'static, Result<ChatChunk>>) {
        while s.next().await.is_some() {}
    }

    #[tokio::test]
    async fn fact_from_session_one_is_recalled_into_context_next_session() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("m.db");

        // "Session 1": state a fact. Reopen Store each block = simulated daemon restart.
        {
            let store = Store::open(&db).unwrap();
            let agent = Agent::new(
                store,
                Box::new(MockProvider {
                    reply: "noted".into(),
                }),
            );
            drain(agent.turn("s1", "my cat is named Mochi").await.unwrap()).await;
        }
        // "Session 2" after restart: the fact must be pulled back into context.
        {
            let store = Store::open(&db).unwrap();
            let ctx = assemble_context(&store, "s1", "what is my cat called").unwrap();
            let joined = ctx
                .iter()
                .map(|m| m.content.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                joined.contains("Mochi"),
                "recall failed; context was:\n{joined}"
            );
        }
    }
}
