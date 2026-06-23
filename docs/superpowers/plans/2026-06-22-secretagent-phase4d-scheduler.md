# SecretAgent Phase 4d — NL→cron Scheduler Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist natural-language scheduled jobs (LLM proposes a cron expr, a deterministic Rust validator gates it), and have the gateway fire each due job as a frozen-allow-list `Remote` run and deliver the result to a connector — acceptance #3: an NL job ("every morning at 7, summarize X") fires and delivers.

**Architecture:** A pure `sa-core::schedule` module owns NL→cron parsing (`cron`+`chrono`, fully encapsulated behind an i64/String API) and the DoS-floor validator. `sa-memory` migration 5 adds `cron_jobs` (+ forward-schema `connectors_state`) with frozen per-job `allowed_tools`. The gateway's `run_until` gains a `tokio::select!` scheduler tick (only when ≥1 connector is configured — the only case delivery is possible) that fires due jobs via `RunContext::remote("cron", id, frozen_tools)` and delivers by constructing the target connector and calling its stateless `send`. `policy::path_allowed` gains symlink resolution on the write path.

**Tech Stack:** Rust 2021, `cron` 0.17, `chrono` 0.4 (already in tree), `rusqlite` 0.32, `tokio`, existing `sa-core`/`sa-memory`/`sa-connectors`/`sa-audit`.

## Global Constraints

- **TDD**; commit per task; conventional-commit subject; footer = blank line then `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>` then `Claude-Session: phase-4d`.
- The `self-audit` PreToolUse hook blocks `git commit` — append ` # self-audit-ok` to the bash command.
- Before every commit: `cargo fmt --all` then `--check`=0 / `cargo clippy --all-targets --all-features -- -D warnings`=0 / relevant `cargo test`.
- **rustls-only** — no openssl/native-tls/aws-lc-sys/zstd-sys. `cron`+`chrono` are pure-Rust (chrono already resolved at 0.4.45); verify the TLS-dep grep stays empty.
- **Commit `Cargo.lock`** with the dep change.
- **M4 (freeze at arm time):** a job's `action` text, `cron_expr`, and `allowed_tools` are persisted at creation and NEVER re-derived at fire time. A fired job runs as `Remote` carrying its frozen `allowed_tools`.
- **Validator gates, LLM never gates:** an unparseable, non-5-field, or sub-minimum-interval (`* * * * *`) expression is rejected by deterministic Rust.
- Cron expressions are interpreted in **UTC** (deterministic, testable; per-job timezone is the named upgrade path).
- SQLite migrations are plain `CREATE`; tests assert `SCHEMA_VERSION`, never a literal version number except the one migration-N test that pins the new value.

---

### Task 1: `sa-core::schedule` — NL→cron parse + deterministic validator

**Files:**
- Modify: `crates/sa-core/Cargo.toml` (add `cron`, `chrono` deps)
- Create: `crates/sa-core/src/schedule.rs`
- Modify: `crates/sa-core/src/lib.rs:13` (add `pub mod schedule;`)
- Test: in-file `#[cfg(test)]` in `schedule.rs`

**Interfaces:**
- Produces:
  - `pub fn validate_cron(expr: &str) -> anyhow::Result<String>` — trims, enforces exactly 5 whitespace fields, parses, rejects sub-minimum-interval; returns the normalized 5-field expr.
  - `pub fn next_fire_unix(expr: &str, after_unix: i64) -> anyhow::Result<i64>` — unix seconds of the next fire strictly after `after_unix` (UTC). chrono/cron stay private.
  - `pub fn propose_cron_prompt(nl: &str) -> String` — the LLM instruction (pure).
  - `pub async fn nl_to_cron(provider: &dyn sa_providers::Provider, nl: &str) -> anyhow::Result<String>` — propose via `provider.complete`, extract, then `validate_cron` (the gate).
  - `pub const MIN_INTERVAL_SECS: i64 = 300;`

- [ ] **Step 1: Add deps**

In `crates/sa-core/Cargo.toml` under `[dependencies]`:

```toml
chrono = { version = "0.4", default-features = false, features = ["clock", "std"] }
cron = "0.17"
```

- [ ] **Step 2: Write the failing tests** (`crates/sa-core/src/schedule.rs`, test module)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use sa_providers::{ChatChunk, Provider};

    #[test]
    fn validate_accepts_a_daily_morning_cron() {
        assert_eq!(validate_cron(" 0 7 * * * ").unwrap(), "0 7 * * *");
    }

    #[test]
    fn validate_rejects_garbage_and_wrong_field_count() {
        assert!(validate_cron("not a cron").is_err());
        assert!(validate_cron("0 7 * *").is_err(), "4 fields");
        assert!(validate_cron("0 0 7 * * *").is_err(), "6 fields (seconds) not accepted");
        assert!(validate_cron("99 7 * * *").is_err(), "minute out of range");
    }

    #[test]
    fn validate_rejects_sub_minimum_interval_dos() {
        assert!(validate_cron("* * * * *").is_err(), "every-minute is the DoS floor");
        assert!(validate_cron("0,1 * * * *").is_err(), "bursty 60s gap rejected");
        assert!(validate_cron("*/5 * * * *").is_ok(), "every 5 min is allowed");
    }

    #[test]
    fn next_fire_is_deterministic_in_utc() {
        // 2026-01-01T00:00:00Z = 1767225600. Next "0 7 * * *" is 2026-01-01T07:00:00Z.
        let after = 1_767_225_600;
        let next = next_fire_unix("0 7 * * *", after).unwrap();
        assert_eq!(next, after + 7 * 3600);
        // strictly after: asking again from the fire time yields the next day
        assert_eq!(next_fire_unix("0 7 * * *", next).unwrap(), next + 24 * 3600);
    }

    struct One(String);
    #[async_trait::async_trait]
    impl Provider for One {
        async fn chat(
            &self,
            _m: Vec<sa_providers::ChatMsg>,
        ) -> anyhow::Result<futures::stream::BoxStream<'static, anyhow::Result<ChatChunk>>> {
            let r = self.0.clone();
            Ok(Box::pin(futures::stream::once(async move { Ok(ChatChunk(r)) })))
        }
    }

    #[tokio::test]
    async fn nl_to_cron_validates_a_well_formed_llm_reply() {
        let p = One("`0 7 * * *`".into()); // model wraps it in backticks
        assert_eq!(nl_to_cron(&p, "every morning at 7").await.unwrap(), "0 7 * * *");
    }

    #[tokio::test]
    async fn nl_to_cron_rejects_a_bad_llm_reply() {
        let p = One("every minute: * * * * *".into());
        assert!(nl_to_cron(&p, "spam me").await.is_err());
    }
}
```

- [ ] **Step 3: Run tests, verify they fail**

Run: `cargo test -p sa-core schedule`
Expected: FAIL (module/functions not defined).

- [ ] **Step 4: Implement `schedule.rs`**

```rust
//! NL→cron scheduling (ADR-20260621 slice 4d). The LLM PROPOSES a cron expression; this
//! module's deterministic validator GATES it — an unparseable, wrong-arity, or
//! sub-minimum-interval (`* * * * *` DoS) expression is rejected in pure Rust. cron/chrono are
//! implementation details: the public API is i64 unix-seconds + String, so callers (the bin,
//! sa-memory) never grow a chrono dependency. Cron is interpreted in UTC.
//! ponytail: UTC-only; a per-job timezone column is the upgrade if local-time intent matters.

use anyhow::{bail, Context, Result};
use chrono::{TimeZone, Utc};
use cron::Schedule;
use sa_providers::{ChatMsg, Provider};
use std::str::FromStr;

/// A scheduled job must not fire more often than this. `* * * * *` (60s) is rejected; the
/// smallest 5-field cron granularity is one minute, so this floor bounds unattended token spend.
/// ponytail: 5 min is a sane assistant-job floor; lower it (or make it per-job) only if a real
/// high-frequency job is needed.
pub const MIN_INTERVAL_SECS: i64 = 300;

/// Parse the model's 5-field cron string into the `cron` crate's 6-field (seconds-leading) form.
/// Enforces EXACTLY 5 standard fields (minute hour dom month dow) — rejecting 6-field/`@macro`
/// output keeps the validator strict and the seconds-field always 0.
fn to_schedule(expr: &str) -> Result<Schedule> {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() != 5 {
        bail!("expected a 5-field cron expression, got {} fields", fields.len());
    }
    // cron crate is sec-leading 6/7-field; prepend a literal 0-seconds.
    let sixed = format!("0 {}", fields.join(" "));
    Schedule::from_str(&sixed).with_context(|| format!("unparseable cron: {expr}"))
}

/// Normalize + fully validate a 5-field cron expression. Returns the canonical single-spaced
/// form. Rejects bad arity, unparseable fields, and any pattern whose MINIMUM gap between
/// consecutive fires is below `MIN_INTERVAL_SECS` (the DoS floor — catches `* * * * *` and
/// bursty patterns like `0,1 * * * *`).
pub fn validate_cron(expr: &str) -> Result<String> {
    let schedule = to_schedule(expr)?;
    // Min gap over the next several fires from a fixed reference (catches bursty minima, not just
    // the first interval). ponytail: 10 samples bounds the check; widen if a pathological pattern
    // slips a sub-floor gap past the 10th fire.
    let reference = Utc.timestamp_opt(0, 0).single().context("epoch")?;
    let fires: Vec<_> = schedule.after(&reference).take(11).collect();
    if fires.len() < 2 {
        bail!("cron expression never fires");
    }
    let min_gap = fires
        .windows(2)
        .map(|w| (w[1] - w[0]).num_seconds())
        .min()
        .unwrap_or(0);
    if min_gap < MIN_INTERVAL_SECS {
        bail!("schedule fires every {min_gap}s — below the {MIN_INTERVAL_SECS}s minimum");
    }
    Ok(expr.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// Unix seconds of the next fire strictly after `after_unix` (UTC).
pub fn next_fire_unix(expr: &str, after_unix: i64) -> Result<i64> {
    let schedule = to_schedule(expr)?;
    let after = Utc
        .timestamp_opt(after_unix, 0)
        .single()
        .context("invalid after_unix")?;
    schedule
        .after(&after)
        .next()
        .map(|dt| dt.timestamp())
        .context("cron expression has no next fire")
}

/// The instruction handed to the model to turn an NL request into a cron expression.
pub fn propose_cron_prompt(nl: &str) -> String {
    format!(
        "Convert this scheduling request into a SINGLE standard 5-field cron expression \
         (minute hour day-of-month month day-of-week), interpreted in UTC. Output ONLY the cron \
         expression on one line — no prose, no @macros, no seconds field.\n\nRequest: {nl}"
    )
}

/// Ask the model for a cron expression, then GATE it through `validate_cron`. The model proposes;
/// the validator decides. Strips surrounding backticks/quotes and takes the last whitespace-run of
/// exactly-5 tokens the reply contains (models often add a stray word).
pub async fn nl_to_cron(provider: &dyn Provider, nl: &str) -> Result<String> {
    let reply = provider
        .complete(vec![ChatMsg {
            role: "user".into(),
            content: propose_cron_prompt(nl),
        }])
        .await?;
    // Extract the first line that, once stripped of backticks/quotes, is exactly 5 cron-ish tokens.
    let candidate = reply
        .lines()
        .map(|l| l.trim().trim_matches(|c| c == '`' || c == '"' || c == '\'').trim())
        .find(|l| l.split_whitespace().count() == 5)
        .unwrap_or_else(|| reply.trim());
    validate_cron(candidate)
}
```

Add `pub mod schedule;` after the existing `pub mod eval;` at `crates/sa-core/src/lib.rs:13`.

- [ ] **Step 5: Run tests, verify pass**

Run: `cargo test -p sa-core schedule` → all pass. If `to_schedule`/dow numbering surprises a test, pin the observed behavior (the acceptance case `0 7 * * *` has no dow field).

- [ ] **Step 6: fmt/clippy + commit**

```bash
cargo fmt --all && cargo clippy -p sa-core --all-targets -- -D warnings
git add crates/sa-core/Cargo.toml crates/sa-core/src/schedule.rs crates/sa-core/src/lib.rs Cargo.lock
git commit -m "feat(schedule): NL->cron parse + deterministic UTC validator (phase 4d) # self-audit-ok"
```

---

### Task 2: `cron_jobs` migration 5 + sa-memory CRUD

**Files:**
- Modify: `crates/sa-memory/src/lib.rs` (SCHEMA_VERSION 4→5; migration tuple; `CronJob` struct; CRUD methods; tests)

**Interfaces:**
- Produces:
  - `pub struct CronJob { pub id: i64, pub nl_spec: String, pub cron_expr: String, pub action: String, pub target_connector: String, pub target_chat: String, pub allowed_tools: String /* JSON */, pub last_run: Option<i64>, pub next_run: i64, pub enabled: bool }`
  - `pub fn add_cron_job(&self, nl_spec, cron_expr, action, target_connector, target_chat, allowed_tools_json: &str, next_run: i64) -> Result<i64>`
  - `pub fn due_jobs(&self, now_unix: i64) -> Result<Vec<CronJob>>` (enabled && next_run <= now)
  - `pub fn mark_fired(&self, id: i64, last_run: i64, next_run: i64) -> Result<()>`
  - `pub fn list_cron_jobs(&self) -> Result<Vec<CronJob>>`
  - `pub fn remove_cron_job(&self, id: i64) -> Result<usize>`

- [ ] **Step 1: Write failing tests** (append to `crates/sa-memory/src/lib.rs` test module)

```rust
#[test]
fn migration_5_creates_cron_jobs_and_bumps_version() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(&dir.path().join("m.db")).unwrap();
    let v: u32 = s.conn.query_row("SELECT version FROM schema_meta LIMIT 1", [], |r| r.get(0)).unwrap();
    assert_eq!(v, SCHEMA_VERSION);
    assert_eq!(SCHEMA_VERSION, 5);
    for t in ["cron_jobs", "connectors_state"] {
        let n: i64 = s.conn.query_row(&format!("SELECT count(*) FROM {t}"), [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0, "{t} should exist and be empty");
    }
}

#[test]
fn a_v4_db_upgrades_to_v5_without_losing_summaries() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("v4.db");
    {
        let s = Store::open(&db).unwrap();
        s.set_summary("s1", 1, "a summary").unwrap();
        s.conn.execute("UPDATE schema_meta SET version = 4", []).unwrap();
        // Drop post-v4 tables so the runner re-applies migration 5.
        s.conn.execute("DROP TABLE IF EXISTS connectors_state", []).ok();
        s.conn.execute("DROP TABLE IF EXISTS cron_jobs", []).ok();
    }
    let s = Store::open(&db).unwrap();
    let v: u32 = s.conn.query_row("SELECT version FROM schema_meta LIMIT 1", [], |r| r.get(0)).unwrap();
    assert_eq!(v, SCHEMA_VERSION);
    assert!(s.summary("s1").unwrap().is_some(), "summaries must survive");
    let n: i64 = s.conn.query_row("SELECT count(*) FROM cron_jobs", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0);
}

#[test]
fn cron_job_crud_and_due_filtering() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(&dir.path().join("m.db")).unwrap();
    let id = s.add_cron_job("every morning at 7", "0 7 * * *", "summarize the news",
        "telegram", "12345", r#"["write_file"]"#, 1000).unwrap();
    // Not due at t=999, due at t=1000.
    assert!(s.due_jobs(999).unwrap().is_empty());
    let due = s.due_jobs(1000).unwrap();
    assert_eq!(due.len(), 1);
    let j = &due[0];
    assert_eq!(j.id, id);
    assert_eq!(j.action, "summarize the news");
    assert_eq!(j.target_connector, "telegram");
    assert_eq!(j.target_chat, "12345");
    assert_eq!(j.allowed_tools, r#"["write_file"]"#);
    assert_eq!(j.last_run, None);
    assert!(j.enabled);
    // Fire it: advance next_run; no longer due at 1000.
    s.mark_fired(id, 1000, 90_000).unwrap();
    assert!(s.due_jobs(1000).unwrap().is_empty());
    assert_eq!(s.list_cron_jobs().unwrap()[0].last_run, Some(1000));
    // Remove.
    assert_eq!(s.remove_cron_job(id).unwrap(), 1);
    assert!(s.list_cron_jobs().unwrap().is_empty());
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test -p sa-memory cron` → FAIL.

- [ ] **Step 3: Implement.** Bump `pub const SCHEMA_VERSION: u32 = 5;` and append migration tuple `(5, ...)` to `MIGRATIONS`:

```rust
(
    5,
    "CREATE TABLE cron_jobs (
        id               INTEGER PRIMARY KEY,
        nl_spec          TEXT NOT NULL,
        cron_expr        TEXT NOT NULL,
        action           TEXT NOT NULL,
        target_connector TEXT NOT NULL,
        target_chat      TEXT NOT NULL,
        allowed_tools    TEXT NOT NULL DEFAULT '[]',
        last_run         INTEGER,
        next_run         INTEGER NOT NULL,
        enabled          INTEGER NOT NULL DEFAULT 1,
        created_at       INTEGER NOT NULL DEFAULT (unixepoch())
     );
     CREATE INDEX idx_cron_due ON cron_jobs(enabled, next_run);
     -- Forward schema per ADR-20260621 §8 (connector cursor). No 4d consumer yet — the
     -- Telegram connector keeps its getUpdates offset in memory; persistence lands when
     -- connector restart-resilience is built.
     CREATE TABLE connectors_state (
        connector  TEXT PRIMARY KEY,
        cursor     TEXT,
        enabled    INTEGER NOT NULL DEFAULT 1
     );",
),
```

Add the `CronJob` struct (near `Skill`) and the methods (near the skill methods):

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronJob {
    pub id: i64,
    pub nl_spec: String,
    pub cron_expr: String,
    pub action: String,
    pub target_connector: String,
    pub target_chat: String,
    /// Frozen per-job side-effect grant, serialized JSON array (M4). Opaque here — the caller
    /// (which has serde_json) parses it at fire time. NEVER re-derived.
    pub allowed_tools: String,
    pub last_run: Option<i64>,
    pub next_run: i64,
    pub enabled: bool,
}

fn map_cron_job(r: &rusqlite::Row) -> rusqlite::Result<CronJob> {
    Ok(CronJob {
        id: r.get(0)?,
        nl_spec: r.get(1)?,
        cron_expr: r.get(2)?,
        action: r.get(3)?,
        target_connector: r.get(4)?,
        target_chat: r.get(5)?,
        allowed_tools: r.get(6)?,
        last_run: r.get(7)?,
        next_run: r.get(8)?,
        enabled: r.get::<_, i64>(9)? != 0,
    })
}
```

```rust
/// Persist a scheduled job. `allowed_tools_json` is the FROZEN per-job grant (M4) — stored
/// verbatim, never re-derived. `cron_expr` must already be validated by sa-core::schedule.
#[allow(clippy::too_many_arguments)]
pub fn add_cron_job(
    &self,
    nl_spec: &str,
    cron_expr: &str,
    action: &str,
    target_connector: &str,
    target_chat: &str,
    allowed_tools_json: &str,
    next_run: i64,
) -> Result<i64> {
    self.conn.execute(
        "INSERT INTO cron_jobs(nl_spec, cron_expr, action, target_connector, target_chat,
            allowed_tools, next_run) VALUES (?1,?2,?3,?4,?5,?6,?7)",
        rusqlite::params![nl_spec, cron_expr, action, target_connector, target_chat,
            allowed_tools_json, next_run],
    )?;
    Ok(self.conn.last_insert_rowid())
}

const CRON_COLS: &str = "id, nl_spec, cron_expr, action, target_connector, target_chat,
    allowed_tools, last_run, next_run, enabled";

/// Enabled jobs whose next_run is at or before `now_unix`, soonest first.
pub fn due_jobs(&self, now_unix: i64) -> Result<Vec<CronJob>> {
    let sql = format!(
        "SELECT {CRON_COLS} FROM cron_jobs WHERE enabled = 1 AND next_run <= ?1 ORDER BY next_run"
    );
    let mut stmt = self.conn.prepare(&sql)?;
    let rows = stmt.query_map([now_unix], map_cron_job)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Record a fire: set last_run + the recomputed next_run.
pub fn mark_fired(&self, id: i64, last_run: i64, next_run: i64) -> Result<()> {
    self.conn.execute(
        "UPDATE cron_jobs SET last_run = ?2, next_run = ?3 WHERE id = ?1",
        rusqlite::params![id, last_run, next_run],
    )?;
    Ok(())
}

pub fn list_cron_jobs(&self) -> Result<Vec<CronJob>> {
    let sql = format!("SELECT {CRON_COLS} FROM cron_jobs ORDER BY id");
    let mut stmt = self.conn.prepare(&sql)?;
    let rows = stmt.query_map([], map_cron_job)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn remove_cron_job(&self, id: i64) -> Result<usize> {
    Ok(self.conn.execute("DELETE FROM cron_jobs WHERE id = ?1", [id])?)
}
```

- [ ] **Step 4: Run, verify pass** — `cargo test -p sa-memory` → all pass.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy -p sa-memory --all-targets -- -D warnings
git add crates/sa-memory/src/lib.rs
git commit -m "feat(memory): cron_jobs migration 5 + frozen-allowlist CRUD (phase 4d) # self-audit-ok"
```

---

### Task 3: write-root symlink resolution in `policy::path_allowed`

**Files:**
- Modify: `crates/sa-core-types/src/policy.rs` (`path_allowed` write branch; helper; tests)

**Interfaces:**
- Consumes: nothing new.
- Produces: `path_allowed` unchanged signature; write paths now additionally rejected if the longest existing ancestor resolves (via `canonicalize`) outside the canonicalized write root.

- [ ] **Step 1: Write the failing test** (append to policy.rs tests; Unix-gated — Windows symlinks need privilege)

```rust
#[cfg(unix)]
#[test]
fn a_symlinked_write_root_cannot_escape_via_canonicalize() {
    use std::os::unix::fs::symlink;
    let tmp = tempfile::tempdir().unwrap();
    let safe = tmp.path().join("safe");
    let secret = tmp.path().join("secret");
    std::fs::create_dir_all(&safe).unwrap();
    std::fs::create_dir_all(&secret).unwrap();
    // A symlink INSIDE the write root pointing out of it.
    let escape = safe.join("escape");
    symlink(&secret, &escape).unwrap();
    let p = Policy { write_roots: vec![safe.clone()], ..Default::default() };
    // Lexically `safe/escape/x` starts_with `safe`, but it resolves into `secret` → must deny.
    assert!(!path_allowed(&p, &escape.join("x"), true), "symlinked escape must be denied");
    // A genuine path inside the real root is still allowed.
    assert!(path_allowed(&p, &safe.join("ok.txt"), true));
}
```

Add `use tempfile;` is not needed (tempfile is a dev-dep of sa-core-types? verify — if absent, add `tempfile = "3"` under `[dev-dependencies]` in `crates/sa-core-types/Cargo.toml`).

- [ ] **Step 2: Run, verify fail** — `cargo test -p sa-core-types symlink` → FAIL (escape currently allowed).

- [ ] **Step 3: Implement.** Update `path_allowed`'s write branch + add the resolver. Replace the body:

```rust
pub fn path_allowed(p: &Policy, path: &Path, write: bool) -> bool {
    let norm = match normalize(path) {
        Some(n) => n,
        None => return false,
    };
    let roots = if write { &p.write_roots } else { &p.read_roots };
    let lexically_ok = roots.iter().any(|r| {
        normalize(r).map(|rn| norm.starts_with(&rn)).unwrap_or(false)
    });
    if !lexically_ok {
        return false;
    }
    // Defense-in-depth for WRITES: if the filesystem entries exist, a symlinked ancestor must not
    // resolve outside the (canonicalized) write root. When nothing exists yet there is no symlink
    // to exploit, so the lexical result stands (keeps the pure cross-platform deny-corpus valid).
    // ponytail: write path only — reads can be extended with the same helper if a read-symlink
    // exfil threat becomes real.
    if write {
        for r in roots {
            if resolves_within(r, path) {
                return true;
            }
        }
        // No root could be resolved (e.g. roots don't exist on disk) → trust the lexical pass.
        if roots.iter().all(|r| std::fs::canonicalize(r).is_err()) {
            return true;
        }
        return false;
    }
    true
}

/// True if `target`'s longest existing ancestor, canonicalized (resolving symlinks) and rejoined
/// with the non-existing remainder, stays within the canonicalized `root`. Returns false if the
/// root cannot be canonicalized (caller decides the fallback).
fn resolves_within(root: &Path, target: &Path) -> bool {
    let croot = match std::fs::canonicalize(root) {
        Ok(c) => c,
        Err(_) => return false,
    };
    // Walk up until we find an existing ancestor we can canonicalize.
    let mut existing = target.to_path_buf();
    let mut remainder: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if let Ok(c) = std::fs::canonicalize(&existing) {
            let mut resolved = c;
            for part in remainder.iter().rev() {
                resolved.push(part);
            }
            return resolved.starts_with(&croot);
        }
        match existing.file_name() {
            Some(name) => {
                remainder.push(name.to_os_string());
                if !existing.pop() {
                    return false;
                }
            }
            None => return false,
        }
    }
}
```

- [ ] **Step 4: Run, verify pass** — `cargo test -p sa-core-types` → all pass (existing pure tests with non-existent `/work` paths still pass: their roots can't canonicalize → lexical result stands).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy -p sa-core-types --all-targets -- -D warnings
git add crates/sa-core-types/src/policy.rs crates/sa-core-types/Cargo.toml Cargo.lock
git commit -m "harden(policy): resolve write-root symlinks before allow (phase 4d) # self-audit-ok"
```

---

### Task 4: gateway `fire_job` + scheduler tick

**Files:**
- Modify: `secretagent/src/gateway.rs` (add `fire_job`; restructure `run_until` has-connectors branch into a `select!` tick loop; tests)

**Interfaces:**
- Consumes: `sa_core::schedule::next_fire_unix`, `sa_memory::{Store, CronJob}`, `RunContext::remote`, `construct_connector`, `Connector::send`.
- Produces:
  - `async fn fire_job(agent: &Agent, job: &CronJob, registry: &Registry, policy: &Policy, audit: &mut Audit, connector: &mut dyn Connector) -> Result<()>` — runs the job as `Remote`, delivers the answer.

- [ ] **Step 1: Write failing test** (gateway.rs test module)

```rust
use sa_connectors::MockConnector;
use sa_memory::CronJob;

fn cron_job(allowed_tools: &str) -> CronJob {
    CronJob {
        id: 7, nl_spec: "every morning".into(), cron_expr: "0 7 * * *".into(),
        action: "summarize the news".into(), target_connector: "telegram".into(),
        target_chat: "c1".into(), allowed_tools: allowed_tools.into(),
        last_run: None, next_run: 0, enabled: true,
    }
}

#[tokio::test]
async fn due_job_fires_as_remote_and_delivers_writing_no_skill() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("m.db");
    let audit_path = dir.path().join("a.jsonl");
    let agent = Agent::new(
        Store::open(&db).unwrap(),
        Box::new(ScriptedProvider::new(vec![ProviderAction::Text("here is the news".into())])),
        SystemContext::default(),
    );
    let mut audit = Audit::open(&audit_path).unwrap();
    let registry = Registry::new();
    let policy = Policy::default();
    let mut conn = MockConnector::new("telegram", vec![]);
    let sent = conn.sent.clone();

    fire_job(&agent, &cron_job("[]"), &registry, &policy, &mut audit, &mut conn).await.unwrap();

    let delivered = sent.lock().unwrap();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].chat, "c1");
    assert_eq!(delivered[0].text, "here is the news");
    assert!(Store::open(&db).unwrap().list_skills().unwrap().is_empty(), "M2: cron run writes no skill");
    let log = std::fs::read_to_string(&audit_path).unwrap();
    assert!(log.contains("remote:cron:7"), "fire is attributed to the cron Remote principal: {log}");
}
```

- [ ] **Step 2: Run, verify fail** — `cargo test -p secretagent due_job_fires` → FAIL.

- [ ] **Step 3: Implement `fire_job`** (add to gateway.rs):

```rust
/// Fire one due scheduled job (ADR-20260621 slice 4d / M4). Runs `job.action` as a `Remote`
/// principal carrying the job's FROZEN `allowed_tools` (never re-derived) and delivers the answer
/// to `connector`. Writes no durable memory (M2) and audits the fire by principal.
pub async fn fire_job(
    agent: &Agent,
    job: &CronJob,
    registry: &Registry,
    policy: &Policy,
    audit: &mut Audit,
    connector: &mut dyn Connector,
) -> Result<()> {
    // Frozen grant — parsed from the stored JSON, never recomputed from the task.
    let allow_tools: Vec<String> = serde_json::from_str(&job.allowed_tools).unwrap_or_default();
    let ctx = RunContext::remote("cron", job.id.to_string(), allow_tools);
    audit.append_synced(AuditEvent {
        action: "cron.fire".into(),
        key_id: job.id.to_string(),
        principal: Some(ctx.audit_label()),
    })?;
    let session = format!("cron:{}", job.id);
    let answer = agent
        .run_task(&session, &job.action, registry, policy, audit, &ctx)
        .await?;
    connector
        .send(OutboundMsg { chat: job.target_chat.clone(), text: answer })
        .await
}
```

Add imports to gateway.rs: `use sa_core::schedule::next_fire_unix;` and `use sa_memory::CronJob;` (Store already imported).

- [ ] **Step 4: Wire the scheduler tick into `run_until`.** Replace the has-connectors tail (the `shutdown.await` + abort block, lines ~124-161) so that after spawning connectors it ticks the scheduler. Keep the `cfg.connectors.is_empty()` early return unchanged (no connector ⇒ no delivery target ⇒ idle, preserves the no-FS shell test). Insert before the spawn loop:

```rust
let sched_store = Store::open(&config::db_path())?;
let connector_bindings = cfg.connectors.clone(); // for delivery lookup after the spawn loop moves cfg.connectors
```

Replace the final `shutdown.await; ... for h in handles { h.abort(); } Ok(())` with a select loop:

```rust
tracing::info!("gateway: {} connector(s) running", state.connectors.len());

let mut ticker = tokio::time::interval(std::time::Duration::from_secs(30));
ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
tokio::pin!(shutdown);
loop {
    tokio::select! {
        _ = &mut shutdown => break,
        _ = ticker.tick() => {
            if let Err(e) = tick_scheduler(
                &sched_store, &agent, &registry, &policy, &audit, &connector_bindings, &vault,
            ).await {
                tracing::warn!("gateway: scheduler tick failed: {e:#}");
            }
        }
    }
}
tracing::info!("gateway: shutdown — aborting {} connector task(s)", handles.len());
for h in handles {
    h.abort();
}
Ok(())
```

Add `tick_scheduler` (constructs the target connector per fire — Telegram `send` is stateless):

```rust
/// One scheduler pass: fire every due job and persist its new next_run. ponytail: builds a fresh
/// connector per fire (send is stateless) — no cross-task channel to the polling connector tasks.
async fn tick_scheduler(
    store: &Store,
    agent: &Arc<Agent>,
    registry: &Arc<Registry>,
    policy: &Arc<Policy>,
    audit: &Arc<Mutex<Audit>>,
    connectors: &[ConnectorConfig],
    vault: &AgeFileVault,
) -> Result<()> {
    let now = now_unix();
    for job in store.due_jobs(now)? {
        let binding = match connectors.iter().find(|c| c.name == job.target_connector) {
            Some(b) => b,
            None => {
                tracing::warn!("cron job {} targets unknown connector '{}' — skipped", job.id, job.target_connector);
                // Advance next_run so an undeliverable job doesn't spin every tick.
                let next = next_fire_unix(&job.cron_expr, now).unwrap_or(now + 3600);
                store.mark_fired(job.id, now, next)?;
                continue;
            }
        };
        match construct_connector(binding, vault)? {
            Some(mut conn) => {
                let mut a = audit.lock().await;
                if let Err(e) = fire_job(agent, &job, registry, policy, &mut a, conn.as_mut()).await {
                    tracing::warn!("cron job {} failed: {e:#}", job.id);
                }
            }
            None => tracing::warn!("cron job {} connector kind unsupported — skipped", job.id),
        }
        let next = next_fire_unix(&job.cron_expr, now)?;
        store.mark_fired(job.id, now, next)?;
    }
    Ok(())
}

/// Unix seconds now (no chrono in the bin — schedule math is encapsulated in sa-core).
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
```

Note: `run_until`'s `shutdown: impl Future` is already owned; `tokio::pin!(shutdown)` makes it pollable in the loop. The connector spawn loop must iterate `cfg.connectors` by value as today (it already does); `connector_bindings` is the clone taken before it.

- [ ] **Step 5: Run, verify pass** — `cargo test -p secretagent` → all pass (the existing `gateway_runs_and_shuts_down_cleanly` and the two 4c dispatch tests stay green).

- [ ] **Step 6: Commit**

```bash
cargo fmt --all && cargo clippy -p secretagent --all-targets -- -D warnings
git add secretagent/src/gateway.rs
git commit -m "feat(gateway): scheduler tick fires due cron jobs as Remote + delivers (phase 4d) # self-audit-ok"
```

---

### Task 5: `secretagent schedule` CLI (add / list / remove)

**Files:**
- Create: `secretagent/src/schedule.rs`
- Modify: `secretagent/src/main.rs` (declare `mod schedule;`, add `Cmd::Schedule` + `ScheduleOp`, dispatch)

**Interfaces:**
- Consumes: `sa_core::schedule::{nl_to_cron, next_fire_unix}`, `Store::{add_cron_job, list_cron_jobs, remove_cron_job}`, the provider-build pattern from `gateway.rs`/`run.rs`.
- Produces: `pub async fn add(...)`, `pub fn list()`, `pub fn remove(id: i64)`.

- [ ] **Step 1: Implement `secretagent/src/schedule.rs`** (CLI glue — covered by the tested lib fns; verified by the live runbook, not a unit test that needs Ollama/FS):

```rust
//! `secretagent schedule` — arm/list/remove NL-scheduled jobs (Phase 4d). `add` asks the model
//! for a cron expression and GATES it through sa-core's deterministic validator before persisting
//! the FROZEN job (action + cron + allow-list — M4). The gateway fires due jobs.

use anyhow::{Context, Result};
use sa_core::schedule::{next_fire_unix, nl_to_cron};
use sa_core_types::config;
use sa_memory::Store;
use sa_providers::openai::OpenAiCompat;
use sa_vault::{age_file::AgeFileVault, Vault};
use secrecy::ExposeSecret;

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub async fn add(
    request: &str,
    connector: &str,
    chat: &str,
    tools: &[String],
) -> Result<()> {
    let cfg = config::Config::load()?;
    let vault = AgeFileVault::open_or_init(&config::identity_path(), &config::store_path())?;
    let api_key = match &cfg.provider.api_key_ref {
        Some(k) => vault.get(k)?.map(|s| s.expose_secret().to_string()),
        None => None,
    };
    let provider = OpenAiCompat {
        base_url: cfg.provider.base_url.clone(),
        model: cfg.provider.model.clone(),
        api_key,
    };
    let cron_expr = nl_to_cron(&provider, request)
        .await
        .context("the model did not propose a valid cron expression")?;
    let next = next_fire_unix(&cron_expr, now_unix())?;
    let allow_json = serde_json::to_string(tools)?;
    let store = Store::open(&config::db_path())?;
    let id = store.add_cron_job(request, &cron_expr, request, connector, chat, &allow_json, next)?;
    println!("scheduled job {id}: `{cron_expr}` (UTC) → {connector}; next fire at unix {next}");
    if !tools.is_empty() {
        println!("  frozen tool grant: {}", tools.join(", "));
    }
    Ok(())
}

pub fn list() -> Result<()> {
    let store = Store::open(&config::db_path())?;
    let jobs = store.list_cron_jobs()?;
    if jobs.is_empty() {
        println!("no scheduled jobs");
        return Ok(());
    }
    for j in jobs {
        let last = j.last_run.map(|t| t.to_string()).unwrap_or_else(|| "never".into());
        println!(
            "[{}] {} `{}` → {} (next {}, last {}, {})",
            j.id, j.nl_spec, j.cron_expr, j.target_connector, j.next_run, last,
            if j.enabled { "enabled" } else { "disabled" }
        );
    }
    Ok(())
}

pub fn remove(id: i64) -> Result<()> {
    let store = Store::open(&config::db_path())?;
    if store.remove_cron_job(id)? == 0 {
        eprintln!("no such job: {id}");
        std::process::exit(2);
    }
    println!("removed job {id}");
    Ok(())
}
```

- [ ] **Step 2: Wire into main.rs.** Add `mod schedule;` to the module list. Add to `enum Cmd`:

```rust
/// Schedule NL jobs the gateway fires (cron, delivered to a connector).
Schedule {
    #[command(subcommand)]
    op: ScheduleOp,
},
```

Add the subcommand enum:

```rust
#[derive(Subcommand)]
enum ScheduleOp {
    /// Arm a job: `schedule add "<request>" --connector <name> --chat <id> [--tool write_file ...]`.
    Add {
        request: String,
        #[arg(long)]
        connector: String,
        #[arg(long)]
        chat: String,
        /// FROZEN per-job side-effect grant (repeatable). Default: none (read-only run).
        #[arg(long = "tool")]
        tools: Vec<String>,
    },
    /// List scheduled jobs.
    List,
    /// Remove a job by id.
    Remove { id: i64 },
}
```

Add to the `match cli.cmd` arm list:

```rust
Cmd::Schedule { op } => match op {
    ScheduleOp::Add { request, connector, chat, tools } =>
        schedule::add(&request, &connector, &chat, &tools).await,
    ScheduleOp::List => schedule::list(),
    ScheduleOp::Remove { id } => schedule::remove(id),
},
```

- [ ] **Step 3: Build + clippy** — `cargo build -p secretagent && cargo clippy -p secretagent --all-targets -- -D warnings` → clean.

- [ ] **Step 4: Commit**

```bash
cargo fmt --all
git add secretagent/src/schedule.rs secretagent/src/main.rs
git commit -m "feat(cli): secretagent schedule add/list/remove (phase 4d) # self-audit-ok"
```

---

### Task 6: adversarial self-audit + both-venue gate + push

- [ ] **Step 1:** Dispatch ONE `self-audit` agent on the scheduler trust boundary (M4 freeze, the symlink resolver, the validator gate, the Remote-principal fire path). Fix any real finding (commit `fix(...) # self-audit-ok`).
- [ ] **Step 2:** Both-venue gate: Windows `cargo test --all`; WSL `wsl.exe bash -c 'export PATH="$HOME/.cargo/bin:$PATH" CARGO_TARGET_DIR="$HOME/sa-target"; cd /mnt/c/Users/dnoye/ClaudeSecondBrain/SecretAgent; cargo test --all'`.
- [ ] **Step 3:** Confirm rustls-only: `wsl … cargo tree -e features -p secretagent | grep -iE "openssl|native-tls|aws-lc-sys|zstd-sys"` is empty; `cargo deny check` green.
- [ ] **Step 4:** Push `master`; watch CI green on all 5 jobs (verify `headSha` matches HEAD before `gh run watch`).
- [ ] **Step 5:** Update `README.md` (schedule command + acceptance #3), `PROGRESS.md`, `ROADMAP.md`, and the `project-secretagent` memory.

---

## Self-Review

**Spec coverage (ADR-20260621 §8 + handoff TASK 2):**
- NL→cron LLM-propose + deterministic validator → Task 1 ✅
- `cron_jobs` migration SCHEMA_VERSION 4→5, frozen `allowed_tools` → Task 2 ✅
- `connectors_state` (ADR §8) → Task 2, forward-schema/no-consumer-yet ✅ (deviation documented)
- M4 freeze at arm time → Task 2 (stored verbatim) + Task 4 (`fire_job` parses, never re-derives) ✅
- Write-root symlink resolution → Task 3 ✅
- Gateway tokio loop ticks the scheduler, reuses `RunContext::remote` → Task 4 ✅
- Delivery to a connector → Task 4 (`fire_job` → `connector.send`) + Task 5 (target binding) ✅
- Acceptance #3 (NL job fires + delivers) → Task 5 arms it, Task 4 fires it; proven by the live runbook + the `due_job_fires_...` unit test ✅

**Placeholder scan:** none — every code step is complete.

**Type consistency:** `CronJob` fields identical across Task 2 (def), Task 4 (`fire_job`/`tick_scheduler`), Task 5 (CLI). `next_fire_unix`/`nl_to_cron`/`validate_cron` signatures consistent Task 1 ↔ Tasks 4/5. `allowed_tools` is a JSON `String` end-to-end (sa-memory opaque → parsed at fire site).
