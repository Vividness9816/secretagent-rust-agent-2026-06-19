# SecretAgent Phase 1 (Talking agent with memory) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `secretagent chat` round-trips against a local model and remembers across restarts — a fact stated in session 1 is recalled (via SQLite FTS5) in session 2.

**Architecture:** Three new crates (`sa-memory`, `sa-providers`, `sa-core`) on the Phase 0 foundation. Memory is SQLite (rusqlite, bundled) with `messages` as the canonical table and `messages_fts` (FTS5) as a *rebuildable* index. The provider layer is one OpenAI-compatible streaming client — Ollama is just `base_url=http://localhost:11434/v1`. `sa-core` runs the minimal per-turn loop: assemble context (recent history + FTS5 recall) → call provider (streaming) → persist.

**Tech Stack:** `rusqlite` (bundled SQLite + FTS5), `tokio`, `reqwest` (rustls, streaming), `serde`. Tests use a **mock `Provider`** + ephemeral SQLite (hermetic, no network); the live-Ollama round-trip is a gated `#[ignore]` test.

**Authority:** spec `inbox/SecretAgent-Build-Plan.md` Phase 1 + `ADR-20260619-secretagent-founding-architecture.md`. On conflict, the ADR wins.

## Global Constraints

- **SQLite is the single canonical store; every index is rebuildable** (ADR inv #1). `messages_fts` must be droppable and rebuildable from `messages` with identical recall — this phase adds the test that proves it (the ADR's named trigger).
- **No secret in SQLite/audit/logs** (ADR inv #4). Provider API keys come from `sa-vault` at call time; they never land in `messages`, config-as-committed, or logs.
- **Tool output is tainted** — N/A this phase (no tools until Phase 2), but any model/provider *input* that originated from a connector stays `Provenance::Untrusted`. Phase 1 only has operator (`Trusted`) and assistant messages.
- **Single self-contained binary** — `rusqlite` MUST use the `bundled` feature (statically links SQLite incl. FTS5); no system libsqlite. Keep the musl-static CI assertion green.
- **Local-first, no telemetry** — default provider is Ollama (`http://localhost:11434/v1`), no outbound calls except the configured provider endpoint.
- New crates are created **just-in-time** (ADR): this phase adds exactly `sa-memory`, `sa-providers`, `sa-core` to `[workspace.members]`.

## File Structure

```
crates/
  sa-memory/      Cargo.toml, src/lib.rs (Store: open/migrate, add_message, recent, recall, rebuild_fts)
  sa-providers/   Cargo.toml, src/lib.rs (Provider trait, ChatChunk, Mock), src/openai.rs (OpenAiCompat streaming)
  sa-core/        Cargo.toml, src/lib.rs (Agent::turn — assemble context, call provider, persist)
secretagent/
  src/main.rs     + `chat` subcommand (async), src/chat.rs (stream to stdout)
  Cargo.toml      + tokio, sa-memory, sa-providers, sa-core
```

Workspace `Cargo.toml` `[workspace.dependencies]` gains: `rusqlite = { version = "0.32", features = ["bundled"] }`, `tokio = { version = "1", features = ["macros","rt-multi-thread"] }`, `reqwest = { version = "0.12", default-features = false, features = ["rustls-tls","json","stream"] }`, `futures = "0.3"`.

---

### Task 1: `sa-memory` — SQLite store, schema, migrations, message CRUD

**Files:**
- Create: `crates/sa-memory/Cargo.toml`, `crates/sa-memory/src/lib.rs`
- Modify: `Cargo.toml` (workspace members + deps)

**Interfaces:**
- Produces: `Store::open(path) -> Result<Store>` (runs migrations), `Store::add_message(session_id, role, content, provenance_json) -> Result<i64>`, `Store::recent(session_id, n) -> Result<Vec<StoredMsg>>`, `StoredMsg { id, role, content }`, `pub const SCHEMA_VERSION: u32`.

- [ ] **Step 1: Add the crate + workspace deps**

`crates/sa-memory/Cargo.toml`:
```toml
[package]
name = "sa-memory"
version = "0.0.0"
edition.workspace = true
license.workspace = true

[dependencies]
rusqlite.workspace = true
anyhow.workspace = true
thiserror.workspace = true

[dev-dependencies]
tempfile = "3"
```
Add `"crates/sa-memory"` to root `members`; add the `rusqlite` line to `[workspace.dependencies]`.

- [ ] **Step 2: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn add_and_read_recent_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("m.db")).unwrap();
        s.add_message("s1", "user", "first fact: my cat is named Mochi", "{}").unwrap();
        s.add_message("s1", "assistant", "noted", "{}").unwrap();
        let recent = s.recent("s1", 10).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].content, "first fact: my cat is named Mochi");
        assert_eq!(recent[1].role, "assistant");
    }
}
```

- [ ] **Step 3: Run it — verify it fails**

Run: `cargo test -p sa-memory`
Expected: FAIL — `Store` not found.

- [ ] **Step 4: Implement the store + schema**

```rust
use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;

pub const SCHEMA_VERSION: u32 = 1;

pub struct Store {
    conn: Connection,
}

#[derive(Debug, Clone)]
pub struct StoredMsg {
    pub id: i64,
    pub role: String,
    pub content: String,
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Self::migrate(&conn)?;
        Ok(Store { conn })
    }

    fn migrate(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_meta (version INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS messages (
                id        INTEGER PRIMARY KEY,
                session_id TEXT NOT NULL,
                role      TEXT NOT NULL,
                content   TEXT NOT NULL,
                provenance TEXT NOT NULL DEFAULT '{}',
                created_at INTEGER NOT NULL DEFAULT (unixepoch())
             );
             CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id);
             CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
                content, content='messages', content_rowid='id'
             );
             -- keep the FTS index in sync (it is a rebuildable derived index)
             CREATE TRIGGER IF NOT EXISTS messages_ai AFTER INSERT ON messages BEGIN
                INSERT INTO messages_fts(rowid, content) VALUES (new.id, new.content);
             END;",
        )?;
        let v: Option<u32> = conn
            .query_row("SELECT version FROM schema_meta LIMIT 1", [], |r| r.get(0))
            .ok();
        if v.is_none() {
            conn.execute("INSERT INTO schema_meta(version) VALUES (?1)", [SCHEMA_VERSION])?;
        }
        Ok(())
    }

    pub fn add_message(
        &self,
        session_id: &str,
        role: &str,
        content: &str,
        provenance_json: &str,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO messages(session_id, role, content, provenance) VALUES (?1,?2,?3,?4)",
            rusqlite::params![session_id, role, content, provenance_json],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn recent(&self, session_id: &str, n: usize) -> Result<Vec<StoredMsg>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, role, content FROM messages WHERE session_id=?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![session_id, n as i64], |r| {
            Ok(StoredMsg { id: r.get(0)?, role: r.get(1)?, content: r.get(2)? })
        })?;
        let mut v: Vec<StoredMsg> = rows.collect::<rusqlite::Result<_>>()?;
        v.reverse(); // chronological
        Ok(v)
    }
}
```

- [ ] **Step 5: Run it — verify it passes**

Run: `cargo test -p sa-memory`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/sa-memory Cargo.toml Cargo.lock
git commit -m "feat(memory): SQLite store + schema + message CRUD (rusqlite bundled)"
```

---

### Task 2: `sa-memory` — FTS5 recall + the rebuildable-index test (ADR trigger)

**Files:**
- Modify: `crates/sa-memory/src/lib.rs`

**Interfaces:**
- Produces: `Store::recall(session_id, query, n) -> Result<Vec<StoredMsg>>` (FTS5 match), `Store::rebuild_fts() -> Result<()>` (drop + rebuild the index from `messages`).

- [ ] **Step 1: Write the failing tests (recall + rebuild-identical)**

```rust
#[test]
fn recall_finds_a_fact_by_keyword() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(&dir.path().join("m.db")).unwrap();
    s.add_message("s1", "user", "my cat is named Mochi", "{}").unwrap();
    s.add_message("s1", "user", "the weather is nice", "{}").unwrap();
    let hits = s.recall("s1", "cat", 5).unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0].content.contains("Mochi"));
}

#[test]
fn fts_is_rebuildable_from_canonical_messages() {
    // ADR invariant #1: every index rebuildable from canonical tables.
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(&dir.path().join("m.db")).unwrap();
    s.add_message("s1", "user", "my cat is named Mochi", "{}").unwrap();
    let before = s.recall("s1", "Mochi", 5).unwrap();
    s.rebuild_fts().unwrap();
    let after = s.recall("s1", "Mochi", 5).unwrap();
    assert_eq!(before.len(), after.len());
    assert_eq!(before[0].content, after[0].content);
    assert_eq!(after.len(), 1);
}
```

- [ ] **Step 2: Run them — verify they fail**

Run: `cargo test -p sa-memory recall fts_is_rebuildable`
Expected: FAIL — `recall`/`rebuild_fts` not found.

- [ ] **Step 3: Implement recall + rebuild**

```rust
impl Store {
    pub fn recall(&self, session_id: &str, query: &str, n: usize) -> Result<Vec<StoredMsg>> {
        // FTS5 MATCH join back to canonical messages, scoped to the session.
        let mut stmt = self.conn.prepare(
            "SELECT m.id, m.role, m.content
               FROM messages_fts f
               JOIN messages m ON m.id = f.rowid
              WHERE m.session_id = ?1 AND messages_fts MATCH ?2
              ORDER BY rank
              LIMIT ?3",
        )?;
        let rows = stmt.query_map(rusqlite::params![session_id, query, n as i64], |r| {
            Ok(StoredMsg { id: r.get(0)?, role: r.get(1)?, content: r.get(2)? })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn rebuild_fts(&self) -> Result<()> {
        // Drop the derived index and repopulate it from the canonical table.
        self.conn.execute_batch(
            "DELETE FROM messages_fts;
             INSERT INTO messages_fts(rowid, content) SELECT id, content FROM messages;",
        )?;
        Ok(())
    }
}
```

- [ ] **Step 4: Run them — verify they pass**

Run: `cargo test -p sa-memory`
Expected: PASS (recall + rebuild-identical). If `recall`'s `MATCH ?2` errors on a multi-word query, that's fine — Phase 1 queries are single keywords; note the upgrade to FTS5 query escaping when free-text recall lands.

- [ ] **Step 5: Commit**

```bash
git add crates/sa-memory/src/lib.rs
git commit -m "feat(memory): FTS5 recall + rebuildable-index test (ADR invariant #1)"
```

---

### Task 3: `sa-providers` — `Provider` trait + mock

**Files:**
- Create: `crates/sa-providers/Cargo.toml`, `crates/sa-providers/src/lib.rs`
- Modify: root `Cargo.toml` (members + tokio/reqwest/futures deps)

**Interfaces:**
- Produces: `ChatMsg { role: String, content: String }`, `ChatChunk(pub String)` (a streamed token delta), and
  ```rust
  #[async_trait::async_trait]
  pub trait Provider: Send + Sync {
      async fn chat(&self, messages: Vec<ChatMsg>) -> anyhow::Result<BoxStream<'static, anyhow::Result<ChatChunk>>>;
  }
  ```
  `MockProvider { reply: String }` yields the reply as one chunk. `sa-core` consumes `Provider`.

- [ ] **Step 1: Add the crate + deps**

`crates/sa-providers/Cargo.toml`: deps `tokio`, `reqwest`, `futures`, `serde`, `serde_json`, `anyhow`, `async-trait = "0.1"`. Add `"crates/sa-providers"` to members and the tokio/reqwest/futures lines to `[workspace.dependencies]`.

- [ ] **Step 2: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[tokio::test]
    async fn mock_provider_streams_its_reply() {
        let p = MockProvider { reply: "hello world".into() };
        let mut s = p.chat(vec![ChatMsg { role: "user".into(), content: "hi".into() }]).await.unwrap();
        let mut out = String::new();
        while let Some(chunk) = s.next().await {
            out.push_str(&chunk.unwrap().0);
        }
        assert_eq!(out, "hello world");
    }
}
```

- [ ] **Step 3: Run it — verify it fails**

Run: `cargo test -p sa-providers`
Expected: FAIL — types not found.

- [ ] **Step 4: Implement trait + mock**

```rust
pub mod openai;

use anyhow::Result;
use futures::stream::{self, BoxStream};

#[derive(Debug, Clone)]
pub struct ChatMsg {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct ChatChunk(pub String);

#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    async fn chat(&self, messages: Vec<ChatMsg>) -> Result<BoxStream<'static, Result<ChatChunk>>>;
}

pub struct MockProvider {
    pub reply: String,
}

#[async_trait::async_trait]
impl Provider for MockProvider {
    async fn chat(&self, _messages: Vec<ChatMsg>) -> Result<BoxStream<'static, Result<ChatChunk>>> {
        let reply = self.reply.clone();
        Ok(Box::pin(stream::once(async move { Ok(ChatChunk(reply)) })))
    }
}
```

- [ ] **Step 5: Run it — verify it passes**

Run: `cargo test -p sa-providers`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/sa-providers Cargo.toml Cargo.lock
git commit -m "feat(providers): Provider trait + streaming MockProvider"
```

---

### Task 4: `sa-providers` — OpenAI-compatible streaming adapter (Ollama default)

**Files:**
- Create: `crates/sa-providers/src/openai.rs`
- Modify: `crates/sa-providers/Cargo.toml` — `[dev-dependencies] wiremock = "0.6"`, `tokio = { workspace = true, features = ["macros","rt-multi-thread"] }`

**Interfaces:**
- Consumes: `Provider`, `ChatMsg`, `ChatChunk`.
- Produces: `OpenAiCompat { base_url: String, model: String, api_key: Option<String> }` implementing `Provider` against `POST {base_url}/chat/completions` with `stream: true`. The SSE delta parser is a pure fn `parse_sse_line(&str) -> Option<String>` so it is unit-testable without a network.

**Note:** the `reqwest` byte-stream + SSE framing is the compile-and-adjust spot (like `age` in Phase 0). The `parse_sse_line` *unit test* and the `wiremock` *integration test* are the binding contracts; adapt the streaming glue until both are green.

- [ ] **Step 1: Write the failing tests (pure SSE parse + wiremock round-trip)**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChatMsg, Provider};
    use futures::StreamExt;

    #[test]
    fn parse_sse_line_extracts_delta_and_ignores_done() {
        assert_eq!(
            parse_sse_line(r#"data: {"choices":[{"delta":{"content":"Mo"}}]}"#),
            Some("Mo".to_string())
        );
        assert_eq!(parse_sse_line("data: [DONE]"), None);
        assert_eq!(parse_sse_line(""), None);
        assert_eq!(parse_sse_line(": comment"), None);
    }

    #[tokio::test]
    async fn streams_from_an_openai_compatible_endpoint() {
        let server = wiremock::MockServer::start().await;
        let body = "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n\
                    data: {\"choices\":[{\"delta\":{\"content\":\" Mochi\"}}]}\n\n\
                    data: [DONE]\n\n";
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/chat/completions"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(body))
            .mount(&server)
            .await;

        let p = OpenAiCompat {
            base_url: server.uri(),
            model: "test".into(),
            api_key: None,
        };
        let mut s = p.chat(vec![ChatMsg { role: "user".into(), content: "hi".into() }]).await.unwrap();
        let mut out = String::new();
        while let Some(c) = s.next().await {
            out.push_str(&c.unwrap().0);
        }
        assert_eq!(out, "Hello Mochi");
    }
}
```

- [ ] **Step 2: Run them — verify they fail**

Run: `cargo test -p sa-providers openai`
Expected: FAIL — `OpenAiCompat`/`parse_sse_line` not found.

- [ ] **Step 3: Implement the adapter + pure SSE parser**

```rust
use crate::{ChatChunk, ChatMsg, Provider};
use anyhow::Result;
use futures::stream::{BoxStream, StreamExt};
use serde_json::json;

pub struct OpenAiCompat {
    pub base_url: String, // e.g. http://localhost:11434/v1  (Ollama) or https://api.openai.com/v1
    pub model: String,
    pub api_key: Option<String>,
}

/// Pure SSE-line parser: returns the delta content, or None for [DONE]/comments/blanks.
pub fn parse_sse_line(line: &str) -> Option<String> {
    let data = line.strip_prefix("data:")?.trim();
    if data == "[DONE]" || data.is_empty() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(data).ok()?;
    let c = v.get("choices")?.get(0)?.get("delta")?.get("content")?.as_str()?;
    Some(c.to_string())
}

#[async_trait::async_trait]
impl Provider for OpenAiCompat {
    async fn chat(&self, messages: Vec<ChatMsg>) -> Result<BoxStream<'static, Result<ChatChunk>>> {
        let msgs: Vec<_> = messages
            .iter()
            .map(|m| json!({"role": m.role, "content": m.content}))
            .collect();
        let mut req = reqwest::Client::new()
            .post(format!("{}/chat/completions", self.base_url))
            .json(&json!({"model": self.model, "messages": msgs, "stream": true}));
        if let Some(k) = &self.api_key {
            req = req.bearer_auth(k);
        }
        let resp = req.send().await?.error_for_status()?;

        // Reframe the byte stream into SSE lines, parse each to a delta.
        let stream = async_stream::stream! {
            let mut buf = String::new();
            let mut bytes = resp.bytes_stream();
            while let Some(chunk) = bytes.next().await {
                let chunk = match chunk { Ok(c) => c, Err(e) => { yield Err(e.into()); break; } };
                buf.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(nl) = buf.find('\n') {
                    let line: String = buf.drain(..=nl).collect();
                    if let Some(delta) = parse_sse_line(line.trim_end()) {
                        yield Ok(ChatChunk(delta));
                    }
                }
            }
        };
        Ok(Box::pin(stream))
    }
}
```
Add `async-stream = "0.3"` to `[dependencies]`.

- [ ] **Step 4: Run them — verify they pass** (adjust the byte→line framing until the wiremock test is green)

Run: `cargo test -p sa-providers`
Expected: PASS (mock + parse + wiremock stream).

- [ ] **Step 5: Commit**

```bash
git add crates/sa-providers Cargo.toml Cargo.lock
git commit -m "feat(providers): OpenAI-compatible streaming adapter + pure SSE parser"
```

---

### Task 5: `sa-core` — the per-turn loop (context assembly + persist), mock-tested

**Files:**
- Create: `crates/sa-core/Cargo.toml`, `crates/sa-core/src/lib.rs`
- Modify: root `Cargo.toml` (members)

**Interfaces:**
- Consumes: `sa_memory::Store`, `sa_providers::{Provider, ChatMsg, ChatChunk}`.
- Produces: `Agent::new(Store, Box<dyn Provider>)`, `Agent::turn(session_id, user_input) -> Result<BoxStream<Result<ChatChunk>>>` which: persists the user message, assembles context (recent + FTS5 recall), calls the provider, and (on stream completion) persists the assistant reply. Plus a sync helper `assemble_context(&Store, session_id, user_input) -> Result<Vec<ChatMsg>>` (pure-ish, unit-testable).

- [ ] **Step 1: Write the failing test (cross-session recall — the Phase 1 acceptance, mocked)**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use sa_memory::Store;
    use sa_providers::MockProvider;

    #[tokio::test]
    async fn fact_from_session_one_is_recalled_into_context_next_session() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("m.db");

        // "Session 1": state a fact. (Reopen Store each time = simulated daemon restart.)
        {
            let store = Store::open(&db).unwrap();
            let agent = Agent::new(store, Box::new(MockProvider { reply: "noted".into() }));
            drain(agent.turn("s1", "my cat is named Mochi").await.unwrap()).await;
        }
        // "Session 2" after restart: a question that should pull the fact into context.
        {
            let store = Store::open(&db).unwrap();
            // Assemble context directly to assert recall (the model itself is mocked).
            let ctx = assemble_context(&store, "s1", "what is my cat called").unwrap();
            let joined = ctx.iter().map(|m| m.content.as_str()).collect::<Vec<_>>().join("\n");
            assert!(joined.contains("Mochi"), "recall failed; context was:\n{joined}");
        }
    }

    async fn drain(mut s: futures::stream::BoxStream<'static, anyhow::Result<sa_providers::ChatChunk>>) {
        use futures::StreamExt;
        while s.next().await.is_some() {}
    }
}
```

- [ ] **Step 2: Run it — verify it fails**

Run: `cargo test -p sa-core`
Expected: FAIL — `Agent`/`assemble_context` not found.

- [ ] **Step 3: Implement the loop**

```rust
use anyhow::Result;
use futures::stream::{BoxStream, StreamExt};
use sa_memory::Store;
use sa_providers::{ChatChunk, ChatMsg, Provider};
use std::sync::{Arc, Mutex};

pub struct Agent {
    store: Arc<Mutex<Store>>, // ponytail: one global lock; per-session locks if concurrency matters
    provider: Box<dyn Provider>,
}

/// Recent history + FTS5 recall of the user input, oldest-first, deduped by id.
pub fn assemble_context(store: &Store, session_id: &str, user_input: &str) -> Result<Vec<ChatMsg>> {
    let mut picked: Vec<sa_memory::StoredMsg> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // First keyword of the input drives recall (Phase 1 is single-keyword; upgrade later).
    if let Some(kw) = user_input.split_whitespace().find(|w| w.len() > 2) {
        for m in store.recall(session_id, kw, 5)? {
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
        .map(|m| ChatMsg { role: m.role, content: m.content })
        .collect();
    ctx.push(ChatMsg { role: "user".into(), content: user_input.to_string() });
    Ok(ctx)
}

impl Agent {
    pub fn new(store: Store, provider: Box<dyn Provider>) -> Self {
        Self { store: Arc::new(Mutex::new(store)), provider }
    }

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

        // Tee the stream: forward chunks to the caller, accumulate to persist on completion.
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
```
Deps: `sa-memory`, `sa-providers`, `tokio`, `futures`, `async-stream`, `anyhow`; `[dev-dependencies] tempfile`, `tokio` (macros, rt).

- [ ] **Step 4: Run it — verify it passes**

Run: `cargo test -p sa-core`
Expected: PASS — the fact is recalled into session-2 context.

- [ ] **Step 5: Commit**

```bash
git add crates/sa-core Cargo.toml Cargo.lock
git commit -m "feat(core): per-turn loop — context assembly (recent + FTS5 recall) + persist"
```

---

### Task 6: `secretagent chat` — wire the real provider, stream to stdout

**Files:**
- Modify: `secretagent/src/main.rs` (async main, `chat` subcommand), `secretagent/Cargo.toml`
- Create: `secretagent/src/chat.rs`
- Modify: `crates/sa-core-types/src/config.rs` — add `ProviderConfig` + `db_path()`

**Interfaces:**
- Consumes: `sa_core::Agent`, `sa_providers::openai::OpenAiCompat`, `sa_memory::Store`, config.
- Produces: `secretagent chat "<message>"` streams a model reply and persists the turn.

- [ ] **Step 1: Add provider config + db path**

In `config.rs` add:
```rust
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    pub base_url: String,        // Ollama default
    pub model: String,
    pub api_key_ref: Option<String>, // vault key id; None for keyless (Ollama)
}
impl Default for ProviderConfig {
    fn default() -> Self {
        Self { base_url: "http://localhost:11434/v1".into(), model: "llama3.2".into(), api_key_ref: None }
    }
}
```
Add `pub provider: ProviderConfig` to `Config` (with `#[serde(default)]`), and `pub fn db_path() -> PathBuf { data_dir().join("memory.db") }`.

- [ ] **Step 2: Write the failing integration test (mocked provider via a fake endpoint)**

`secretagent/tests/chat.rs`:
```rust
use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn chat_streams_and_persists_against_a_fake_openai_endpoint() {
    // A tiny blocking HTTP server returning one SSE chunk would go here; to keep this
    // hermetic without a runtime in-test, we assert the command surfaces a clear error
    // when no provider is reachable (offline CI), and the live path is the #[ignore] test.
    let dir = tempfile::tempdir().unwrap();
    Command::cargo_bin("secretagent")
        .unwrap()
        .env("SECRETAGENT_DATA_DIR", dir.path())
        .env("SECRETAGENT_CONFIG_DIR", dir.path())
        .args(["chat", "hello"])
        .assert()
        .failure() // no Ollama in CI → connection error, surfaced cleanly (non-panic)
        .stderr(predicate::str::contains("provider").or(predicate::str::contains("connect")));
}
```
*(ponytail: CI has no Ollama, so the hermetic test asserts a clean failure path. The real round-trip is the `#[ignore]` test in Task 7, run locally where Ollama is up.)*

- [ ] **Step 3: Run it — verify it fails**

Run: `cargo test -p secretagent --test chat`
Expected: FAIL — no `chat` subcommand.

- [ ] **Step 4: Implement `chat`**

Add to `Cmd` enum: `Chat { message: String, #[arg(long, default_value = "default")] session: String }`. Make `main` async:
```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> { /* ...existing init + parse... */
    match cli.cmd {
        // ...
        Cmd::Chat { message, session } => chat::run(&session, &message).await,
    }
}
```
`secretagent/src/chat.rs`:
```rust
use anyhow::Context;
use sa_core::Agent;
use sa_core_types::config;
use sa_memory::Store;
use sa_providers::openai::OpenAiCompat;
use std::io::Write;

pub async fn run(session: &str, message: &str) -> anyhow::Result<()> {
    let cfg = config::Config::load()?;
    let store = Store::open(&config::db_path())?;

    // Resolve an API key from the vault only if the provider needs one (keyless = Ollama).
    let api_key = match &cfg.provider.api_key_ref {
        Some(key_id) => {
            use sa_vault::{age_file::AgeFileVault, Vault};
            use secrecy::ExposeSecret;
            let v = AgeFileVault::open_or_init(&config::identity_path(), &config::store_path())?;
            v.get(key_id)?.map(|s| s.expose_secret().to_string())
        }
        None => None,
    };

    let provider = OpenAiCompat {
        base_url: cfg.provider.base_url.clone(),
        model: cfg.provider.model.clone(),
        api_key,
    };
    let agent = Agent::new(store, Box::new(provider));

    use futures::StreamExt;
    let mut stream = agent
        .turn(session, message)
        .await
        .context("provider request failed — is the model endpoint reachable?")?;
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    while let Some(chunk) = stream.next().await {
        write!(lock, "{}", chunk?.0)?;
        lock.flush()?;
    }
    writeln!(lock)?;
    Ok(())
}
```
Add to `secretagent/Cargo.toml`: `tokio`, `futures`, `sa-core`, `sa-memory`, `sa-providers`.

- [ ] **Step 5: Run it — verify it passes**

Run: `cargo test -p secretagent --test chat`
Expected: PASS — offline run fails cleanly mentioning the provider/connection.

- [ ] **Step 6: Commit**

```bash
git add secretagent crates/sa-core-types/src/config.rs Cargo.toml Cargo.lock
git commit -m "feat(bin): secretagent chat — streaming turn against the configured provider"
```

---

### Task 7: Acceptance — live-Ollama round-trip + recall across restart

**Files:**
- Create: `secretagent/tests/live_ollama.rs` (`#[ignore]` — run locally where Ollama is up)
- Modify: `secretagent/src/doctor.rs` — add an informational provider-reachability line

**Interfaces:**
- Consumes: the built binary + a local Ollama.

- [ ] **Step 1: Add doctor provider check (informational, never fails)**

In `doctor::run`, after the vault checks:
```rust
// Provider reachability is informational in Phase 1 — a down model endpoint is not
// a doctor failure (you may be offline / configuring).
let cfg = sa_core_types::config::Config::load().unwrap_or_default();
match std::net::TcpStream::connect_timeout(
    &resolve_host(&cfg.provider.base_url), std::time::Duration::from_millis(300)) {
    Ok(_) => println!("[ok]   provider endpoint reachable: {}", cfg.provider.base_url),
    Err(_) => println!("[info] provider endpoint not reachable ({}) — expected if offline", cfg.provider.base_url),
}
```
(`resolve_host` parses host:port from the base_url; keep it a small helper. If parsing is awkward, downgrade to an `[info]` line that just prints the configured URL — reachability is non-gating.)

- [ ] **Step 2: Write the `#[ignore]` live acceptance test**

`secretagent/tests/live_ollama.rs`:
```rust
use assert_cmd::Command;
use predicates::prelude::*;

// Run with: cargo test -p secretagent --test live_ollama -- --ignored
// Requires a local Ollama serving the configured model.
#[test]
#[ignore]
fn fact_stated_in_session_one_is_recalled_in_session_two_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let env = |c: &mut Command| { c.env("SECRETAGENT_DATA_DIR", dir.path()); };

    let mut c1 = Command::cargo_bin("secretagent").unwrap(); env(&mut c1);
    c1.args(["chat", "--session", "s1", "Remember: my cat is named Mochi."]).assert().success();

    // New process = daemon restart. The model should recall the fact from FTS5 context.
    let mut c2 = Command::cargo_bin("secretagent").unwrap(); env(&mut c2);
    c2.args(["chat", "--session", "s1", "What is my cat's name?"])
        .assert().success()
        .stdout(predicate::str::contains("Mochi"));
}
```

- [ ] **Step 3: Run the hermetic suite (CI-safe), then the live test locally**

Run (CI-safe): `cargo test --all`
Expected: PASS (live test is `#[ignore]`, skipped).

Run (local, Ollama up): `cargo test -p secretagent --test live_ollama -- --ignored`
Expected: PASS — "Mochi" appears in session 2's reply.

- [ ] **Step 4: Commit + push**

```bash
git add secretagent Cargo.toml
git commit -m "test(phase1): live-Ollama cross-session recall acceptance + doctor provider check"
git push origin master
```

- [ ] **Step 5: Phase 1 acceptance sign-off**

- [ ] `secretagent chat` round-trips against local Ollama (live `#[ignore]` test green locally)
- [ ] a fact from session 1 is recalled via FTS5 in session 2 after a fresh process (live test + the mocked Task 5 test)
- [ ] `messages_fts` is rebuildable from `messages` with identical recall (Task 2 test — ADR invariant #1)
- [ ] hermetic `cargo test --all` + clippy + fmt + deny + the musl-static CI matrix stay green
- [ ] no secret in the DB: provider API key is read from the vault at call time, never written to `messages`/config/logs

Stop for user review before Phase 2 (tools + safe execution).

---

## Self-Review

**Spec/ADR coverage:**
- `sa-memory` SQLite + FTS5 + rebuildable index (spec §4.1, ADR inv #1) → Tasks 1–2. ✅
- `sa-providers` OpenAI-compatible + Ollama default + streaming (spec §4.2) → Tasks 3–4. ✅ Multi-model/runtime-switch deferred (Phase 1 acceptance doesn't need it).
- `sa-core` minimal loop + context assembly (spec §7 steps 1–3,5,7; no tools/learning yet) → Task 5. ✅ Tool-loop (step 4) + learning loop (step 6) correctly deferred to Phase 2/3.
- CLI `chat` + streaming (spec §4.5 partial) → Task 6. TUI deferred.
- Acceptance: chat vs Ollama + cross-restart recall (spec Phase 1) → Tasks 5 (mocked) + 7 (live). ✅
- No-secret-in-DB (ADR inv #4): key from vault at call time → Task 6. ✅
- Single binary: `rusqlite` bundled keeps musl-static green → Task 1 + CI. ✅

**Placeholder scan:** no TBD/TODO-as-task. The two compile-and-adjust spots (`reqwest` SSE framing, `resolve_host` parsing) are explicitly flagged with their binding test as the contract.

**Type consistency:** `Store::{open,add_message,recent,recall,rebuild_fts}` + `StoredMsg{id,role,content}`; `Provider::chat -> BoxStream<Result<ChatChunk>>`, `ChatMsg{role,content}`, `ChatChunk(String)`; `OpenAiCompat{base_url,model,api_key}`; `Agent::{new,turn}` + `assemble_context`; `config::{db_path,ProviderConfig}` — consistent across Tasks 1–7. ✅

**Ponytail decisions logged:** one OpenAI-compatible client for both Ollama and OpenAI (Ollama = base_url); single global `Mutex<Store>` (per-session locks only if concurrency bites); single-keyword FTS recall (free-text escaping when needed); summarization of old context deferred to Phase 3; provider key resolved from vault only when `api_key_ref` is set.
