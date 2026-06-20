# SecretAgent Phase 3c (memory summarization) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:executing-plans (inline TDD). Steps use checkbox (`- [ ]`) syntax.

**Goal:** A long session's older context is compressed into a rolling **LLM summary** (behind the `Provider` seam), stored canonically, and surfaced into assembled context — so the agent retains the gist of a long conversation past the recent/recall window without dumping every old message.

**Architecture:** A `session_summaries` table (one rolling summary per session, Migration 4). `Provider` gains a default `complete()` (collect the existing streaming `chat`) — no new provider impl. `Agent::summarize_session` loads the session's messages, asks the provider to summarize everything older than a small recent window (folding in the prior summary), and stores the result keyed to a watermark message id. `assemble_context` prepends the summary as a leading context message. A `secretagent summarize` CLI triggers it; auto-triggering in the hot loop is deferred (Phase-4 scheduler territory).

**Tech Stack:** existing crates only. No new deps.

**Authority:** ADR-20260620 Fork E (slice 3c = "memory summarization, behind Provider; weakest acceptance; slice last"). Spec §4.1 ("FTS5 cross-session recall **with LLM summarization of older context**"), §7.6. Founding ADR (JIT; SQLite-canonical, every index rebuildable — a summary is canonical primary content, not a derived index, so no rebuild concern). **NOT in scope (ADR/Pragmatist YAGNI):** `memory_episodic`/`memory_semantic` triple stores, embeddings, the auto-trigger.

## Global Constraints

- **The summary is derived ONLY from the `messages` table (user + assistant rows).** Tool output is never `add_message`'d (run_task/turn store only user+assistant; tool output is transient prompt-only), so the summary carries **no raw tool output** — consistent with the starvation principle. (Residual: an assistant message that model-echoed injected text could be compressed into the summary — the same ADR-accepted model-echo residual as 3b; no NEW trust escalation since those messages are already in recall/recent context.)
- **Behind the `Provider` seam:** the summary text comes from `provider.complete` (default = collect `chat`); the *logic* (when/what/where) is deterministic + unit-testable with `MockProvider`.
- **No auto-trigger in the hot loop** (ponytail) — `summarize_session` is explicit (CLI / future scheduler). Existing turn/run_task behavior is untouched, so all Phase 0–3b tests stay green.
- **SQLite-canonical:** the summary is a row; it is overwritten in place (rolling). No derived index, so no invariant-#1 rebuild obligation.
- **Tests assert `SCHEMA_VERSION`, not version literals** (prior migration tests pinned literals and broke each bump — fix them this slice).
- **TDD; commit per task** with the `Co-Authored-By` + `Claude-Session` footer. Before every commit: `cargo fmt --all -- --check` (0) / `cargo clippy --all-targets --all-features -- -D warnings` (0) / `cargo test` (pass). The self-audit hook blocks `git commit` — append ` # self-audit-ok`. Gate WSL + Windows; push; watch CI green.

## File Structure

```
crates/
  sa-memory/src/lib.rs    + MIGRATION 4 (session_summaries); SCHEMA_VERSION 3->4; Summary struct;
                            summary()/set_summary()/all_messages(); fix version-literal tests
  sa-providers/src/lib.rs + Provider::complete default (collect chat); test
  sa-core/src/lib.rs      + Agent::summarize_session; assemble_context prepends the summary; tests
secretagent/
  src/summarize.rs (NEW)  run(): open store + provider + Agent::summarize_session
  src/main.rs             + Summarize { session } subcommand
  tests/summarize.rs (NEW) CLI smoke (no provider reachable → clean message, exit handling)
```

---

### Task 1: Migration 4 — `session_summaries` + Store API (sa-memory)

**Files:** Modify `crates/sa-memory/src/lib.rs`; tests in same.

**Interfaces:**
- `pub const SCHEMA_VERSION: u32 = 4`; Migration 4 creates `session_summaries(session_id TEXT PRIMARY KEY, through_id INTEGER NOT NULL, summary TEXT NOT NULL, updated_at INTEGER)`.
- `pub struct Summary { pub through_id: i64, pub text: String }`
- `Store::summary(&self, session_id) -> Result<Option<Summary>>`
- `Store::set_summary(&self, session_id, through_id: i64, text: &str) -> Result<()>` (upsert)
- `Store::all_messages(&self, session_id) -> Result<Vec<StoredMsg>>` (oldest-first; ponytail: loads the whole session — fine for an occasional explicit summarize; windowed paging if a session ever gets pathological)

- [ ] **Step 1: Write failing tests** (append to `mod tests`):

```rust
    #[test]
    fn migration_4_creates_session_summaries() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("m.db")).unwrap();
        let v: u32 = s
            .conn
            .query_row("SELECT version FROM schema_meta LIMIT 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        assert_eq!(SCHEMA_VERSION, 4);
        let n: i64 = s
            .conn
            .query_row("SELECT count(*) FROM session_summaries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn summary_upserts_and_all_messages_is_oldest_first() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("m.db")).unwrap();
        assert!(s.summary("s1").unwrap().is_none());
        s.add_message("s1", "user", "first", "{}").unwrap();
        s.add_message("s1", "assistant", "second", "{}").unwrap();
        let all = s.all_messages("s1").unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].content, "first");
        s.set_summary("s1", all[1].id, "a summary").unwrap();
        s.set_summary("s1", all[1].id, "a better summary").unwrap(); // upsert
        let got = s.summary("s1").unwrap().unwrap();
        assert_eq!(got.text, "a better summary");
        assert_eq!(got.through_id, all[1].id);
    }
```

- [ ] **Step 2: Run → fail** (WSL): `cargo test -p sa-memory migration_4` / `summary_upserts` → FAIL (no table / no method).

- [ ] **Step 3: Implement.** Bump `pub const SCHEMA_VERSION: u32 = 4;`. Append the Migration 4 tuple after `(3, "...")`:

```rust
            (
                4,
                "CREATE TABLE session_summaries (
                    session_id TEXT PRIMARY KEY,
                    through_id INTEGER NOT NULL,
                    summary    TEXT NOT NULL,
                    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
                 );",
            ),
```

Add the struct near `Skill`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub through_id: i64,
    pub text: String,
}
```

Add to `impl Store`:

```rust
    /// The rolling summary for a session, if any.
    pub fn summary(&self, session_id: &str) -> Result<Option<Summary>> {
        use rusqlite::OptionalExtension;
        Ok(self
            .conn
            .query_row(
                "SELECT through_id, summary FROM session_summaries WHERE session_id=?1",
                [session_id],
                |r| {
                    Ok(Summary {
                        through_id: r.get(0)?,
                        text: r.get(1)?,
                    })
                },
            )
            .optional()?)
    }

    /// Upsert the rolling summary (covers messages up to `through_id`).
    pub fn set_summary(&self, session_id: &str, through_id: i64, text: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO session_summaries(session_id, through_id, summary) VALUES (?1, ?2, ?3)
             ON CONFLICT(session_id) DO UPDATE SET
                through_id=excluded.through_id, summary=excluded.summary, updated_at=unixepoch()",
            rusqlite::params![session_id, through_id, text],
        )?;
        Ok(())
    }

    /// All messages for a session, oldest-first. ponytail: whole-session load for an
    /// occasional explicit summarize; window/paginate if a session ever gets pathological.
    pub fn all_messages(&self, session_id: &str) -> Result<Vec<StoredMsg>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, role, content FROM messages WHERE session_id=?1 ORDER BY id ASC",
        )?;
        let rows = stmt.query_map([session_id], |r| {
            Ok(StoredMsg {
                id: r.get(0)?,
                role: r.get(1)?,
                content: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
```

- [ ] **Step 4: Fix the version-literal tests** so they survive future bumps: in `migration_3_creates_skills_tables_and_bumps_version`, change `assert_eq!(v, 3); assert_eq!(SCHEMA_VERSION, 3);` → `assert_eq!(v, SCHEMA_VERSION);` (keep the skills/skill_versions existence checks). In `a_v2_db_upgrades_to_v3_without_losing_user_model`, change `assert_eq!(v, 3)` → `assert_eq!(v, SCHEMA_VERSION)`.

- [ ] **Step 5: Run → pass** (WSL): `cargo test -p sa-memory`. **Step 6: Gate + commit** (`feat(memory): migration 4 — session_summaries + summary API`).

---

### Task 2: `Provider::complete` default (sa-providers)

**Files:** Modify `crates/sa-providers/src/lib.rs`; test in same.

**Interfaces:** `Provider::complete(&self, messages: Vec<ChatMsg>) -> Result<String>` — default collects `chat` into a String. (Overridable; the OpenAI-compat adapter inherits the default — correct, it just consumes its own stream.)

- [ ] **Step 1: Failing test** (append to `mod tests`):

```rust
    #[tokio::test]
    async fn complete_collects_the_chat_stream() {
        let p = MockProvider { reply: "the summary".into() };
        let out = p.complete(vec![ChatMsg { role: "user".into(), content: "summarize".into() }]).await.unwrap();
        assert_eq!(out, "the summary");
    }
```

- [ ] **Step 2: Run → fail** (`no method complete`). **Step 3: Implement** — add to the `Provider` trait (after `act`):

```rust
    /// One-shot completion: collect the streaming `chat` into a single String. Default works
    /// for any provider that implements `chat` (used by memory summarization, Phase 3c).
    async fn complete(&self, messages: Vec<ChatMsg>) -> Result<String> {
        let mut stream = self.chat(messages).await?;
        let mut out = String::new();
        while let Some(chunk) = stream.next().await {
            out.push_str(&chunk?.0);
        }
        Ok(out)
    }
```

(`StreamExt` is already imported in this file via `futures::stream::{self, BoxStream}` + usage; if `next` isn't in scope add `use futures::StreamExt;` at the top.)

- [ ] **Step 4: Run → pass** (`cargo test -p sa-providers`). **Step 5: Gate + commit** (`feat(providers): Provider::complete default (collect chat) for summarization`).

---

### Task 3: `Agent::summarize_session` + surface in `assemble_context` (sa-core)

**Files:** Modify `crates/sa-core/src/lib.rs`; tests in same.

**Interfaces:**
- `Agent::summarize_session(&self, session_id) -> Result<bool>` — summarizes messages older than `SUMMARY_KEEP_RECENT` that are newer than the current watermark; folds in the prior summary; stores via `set_summary`; returns `true` if it summarized, `false` if nothing to do. Uses `self.provider.complete`.
- `assemble_context` prepends the session summary (if any) as a leading `ChatMsg`.
- `const SUMMARY_KEEP_RECENT: usize = 6;`

- [ ] **Step 1: Failing tests** (append to `mod tests`):

```rust
    #[tokio::test]
    async fn summarize_session_compresses_older_messages_and_surfaces_them() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("m.db")).unwrap();
        // 12 messages → older-than-keep window exists.
        for i in 0..12 {
            store.add_message("s1", "user", &format!("fact number {i}"), "{}").unwrap();
        }
        let agent = Agent::new(
            store,
            Box::new(MockProvider { reply: "SUMMARY: facts 0..5".into() }),
            SystemContext::default(),
        );
        assert!(agent.summarize_session("s1").await.unwrap(), "should summarize");

        // The summary is now surfaced as the leading context message.
        let store2 = Store::open(&dir.path().join("m.db")).unwrap();
        let ctx = assemble_context(&store2, "s1", "what were the facts").unwrap();
        assert_eq!(ctx[0].role, "system");
        assert!(ctx[0].content.contains("SUMMARY: facts 0..5"));

        // Idempotent: nothing new to summarize → false.
        let agent2 = Agent::new(
            Store::open(&dir.path().join("m.db")).unwrap(),
            Box::new(MockProvider { reply: "x".into() }),
            SystemContext::default(),
        );
        assert!(!agent2.summarize_session("s1").await.unwrap(), "no new older messages");
    }

    #[tokio::test]
    async fn summarize_session_noop_on_short_session() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("m.db")).unwrap();
        store.add_message("s1", "user", "only one", "{}").unwrap();
        let agent = Agent::new(store, Box::new(MockProvider { reply: "x".into() }), SystemContext::default());
        assert!(!agent.summarize_session("s1").await.unwrap());
    }
```

- [ ] **Step 2: Run → fail.** **Step 3: Implement.** Add the const near `MAX_TOOL_STEPS`:

```rust
const SUMMARY_KEEP_RECENT: usize = 6;
```

Add the method to `impl Agent`:

```rust
    /// Compress older session context into a rolling LLM summary (Phase 3c). Summarizes
    /// messages that are older than the recent window AND newer than the current watermark,
    /// folding in the prior summary. Returns false if there is nothing new to summarize.
    /// Derives ONLY from the `messages` table (user+assistant rows; no raw tool output).
    pub async fn summarize_session(&self, session_id: &str) -> Result<bool> {
        let (older, watermark, prior): (Vec<ChatMsg>, i64, Option<String>) = {
            let store = self.store.lock().unwrap();
            let all = store.all_messages(session_id)?;
            if all.len() <= SUMMARY_KEEP_RECENT {
                return Ok(false);
            }
            let prior = store.summary(session_id)?;
            let through = prior.as_ref().map(|s| s.through_id).unwrap_or(0);
            let cutoff = all.len() - SUMMARY_KEEP_RECENT; // keep the most recent verbatim
            let older: Vec<ChatMsg> = all[..cutoff]
                .iter()
                .filter(|m| m.id > through)
                .map(|m| ChatMsg { role: m.role.clone(), content: m.content.clone() })
                .collect();
            let watermark = all[..cutoff].last().map(|m| m.id).unwrap_or(through);
            if older.is_empty() {
                return Ok(false);
            }
            (older, watermark, prior.map(|s| s.text))
        };

        // Build the summarization prompt (operator/agent content only — no tool output).
        let mut prompt = String::from(
            "Summarize the earlier conversation below concisely, preserving key facts, names, and decisions. Output only the summary.\n",
        );
        if let Some(p) = &prior {
            prompt.push_str("\nPrior summary:\n");
            prompt.push_str(p);
        }
        prompt.push_str("\n\nEarlier messages:\n");
        for m in &older {
            prompt.push_str(&format!("{}: {}\n", m.role, m.content));
        }
        let summary = self
            .provider
            .complete(vec![ChatMsg { role: "user".into(), content: prompt }])
            .await?;

        let store = self.store.lock().unwrap();
        store.set_summary(session_id, watermark, summary.trim())?;
        Ok(true)
    }
```

Surface it in `assemble_context` — at the very end, before returning, prepend the summary if present. Change the tail of `assemble_context`:

```rust
    let mut ctx: Vec<ChatMsg> = picked
        .into_iter()
        .map(|m| ChatMsg { role: m.role, content: m.content })
        .collect();
    ctx.push(ChatMsg { role: "user".into(), content: user_input.to_string() });
    // Phase 3c: a rolling summary of older context leads the history (bounded recall of a
    // long session). Derived from user+assistant messages only.
    if let Some(s) = store.summary(session_id)? {
        ctx.insert(0, ChatMsg {
            role: "system".into(),
            content: format!("Summary of earlier conversation in this session:\n{}", s.text),
        });
    }
    Ok(ctx)
```

- [ ] **Step 4: Run → pass** (`cargo test -p sa-core`). The injection/recall/skill tests stay green (their sessions are short → no summary; `assemble_context` summary branch is inert). **Step 5: Gate + commit** (`feat(core): summarize_session + surface rolling summary in context`).

---

### Task 4: `secretagent summarize` CLI (secretagent)

**Files:** Create `secretagent/src/summarize.rs`; modify `secretagent/src/main.rs`; test `secretagent/tests/summarize.rs`.

**Interfaces:** `summarize::run(session) -> Result<()>` — opens store + the configured provider, calls `Agent::summarize_session`, prints whether it summarized.

- [ ] **Step 1: CLI test** `secretagent/tests/summarize.rs`:

```rust
use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn summarize_short_session_is_a_clean_noop() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.toml"),
        "[provider]\nbase_url = \"http://127.0.0.1:1/v1\"\nmodel = \"none\"\n",
    )
    .unwrap();
    // Empty/short session: summarize_session returns false BEFORE any provider call, so this
    // succeeds even with an unreachable provider (proves the no-op short-circuit).
    Command::cargo_bin("secretagent")
        .unwrap()
        .env("SECRETAGENT_DATA_DIR", dir.path())
        .env("SECRETAGENT_CONFIG_DIR", dir.path())
        .args(["summarize", "--session", "empty"])
        .assert()
        .success()
        .stdout(predicate::str::contains("nothing to summarize"));
}
```

- [ ] **Step 2: Run → fail** (`unrecognized subcommand`). **Step 3: Create `secretagent/src/summarize.rs`:**

```rust
use anyhow::Context;
use sa_core::{Agent, SystemContext};
use sa_core_types::config;
use sa_memory::Store;
use sa_providers::openai::OpenAiCompat;

pub async fn run(session: &str) -> anyhow::Result<()> {
    let cfg = config::Config::load()?;
    let store = Store::open(&config::db_path())?;
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
    let agent = Agent::new(store, Box::new(provider), SystemContext::default());
    if agent
        .summarize_session(session)
        .await
        .context("summarization failed — is the model endpoint reachable?")?
    {
        println!("summarized older context for session {session}");
    } else {
        println!("nothing to summarize for session {session}");
    }
    Ok(())
}
```

- [ ] **Step 4: Wire `main.rs`** — add `mod summarize;`; add to `enum Cmd`:

```rust
    /// Compress a session's older context into a rolling LLM summary.
    Summarize {
        #[arg(long, default_value = "default")]
        session: String,
    },
```

and the dispatch arm:

```rust
        Cmd::Summarize { session } => summarize::run(&session).await,
```

- [ ] **Step 5: Run → pass** (`cargo test -p secretagent --test summarize` + `cargo build --all`). **Step 6: Gate + commit** (`feat(cli): summarize command (rolling session summary)`).

---

### Task 5: Adversarial check + full gate (both venues) + docs + push + CI

- [ ] **Step 1: Adversarial verify** the (minor) new trust surface: dispatch one self-audit/review agent over the 3c diff — focus: can the summary launder raw tool output (it must not — `messages` holds no tool rows) or escalate trust by entering role:"system"? Confirm the summary derives only from `messages` (user+assistant) and that the model-echo residual is no worse than already-present context. Fix any real finding.
- [ ] **Step 2: Full gate BOTH venues** — WSL (`cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all`) AND Windows (same). Both green.
- [ ] **Step 3: Docs** — README "What works today": add a `summarize` bullet (rolling LLM summary of older context, behind the Provider seam, derived from user+assistant messages only). `docs/HANDOFF-phase3.md`: mark 3c complete → **Phase 3 COMPLETE** (3a+3b+3c).
- [ ] **Step 4: Commit docs + push + watch CI green** (`gh run watch <id> --exit-status --interval 25`).
- [ ] **Step 5: STOP** — report Phase 3 complete (all three slices) + the Phase-3 acceptance recap.

---

## Self-Review

- **Spec/ADR coverage:** §4.1 "LLM summarization of older context" → Tasks 1-4 (rolling summary behind Provider, surfaced in recall). ADR Fork-E 3c scope met. **Deferred (ADR/YAGNI, stated):** auto-trigger in the hot loop, `memory_episodic`/`memory_semantic`, embeddings.
- **Placeholder scan:** none — exact Rust + run commands throughout.
- **Type consistency:** `Summary{through_id:i64, text:String}`; `summary()`/`set_summary(session,through_id,text)`/`all_messages(session)`; `Provider::complete(messages)->Result<String>`; `Agent::summarize_session(session)->Result<bool>`; `SUMMARY_KEEP_RECENT`; `assemble_context` prepends role:"system" summary. `SCHEMA_VERSION=4`.
- **Regression guard:** no hot-loop change; short sessions short-circuit before any provider call; existing tests' sessions are short → summary branch inert. Version-literal tests fixed to assert `SCHEMA_VERSION`. Full `cargo test --all` on both venues (Task 5).
