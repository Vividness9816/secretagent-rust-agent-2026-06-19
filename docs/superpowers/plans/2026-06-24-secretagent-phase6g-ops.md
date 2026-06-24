# SecretAgent Phase 6g — Ops (backup/restore + trajectory export)

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:test-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Operational `backup` / `restore` of the live data dir and a **secret-free** `export` of a session's trajectory, so an operator can move/recover an agent and share/inspect what it did without leaking secrets.

**Architecture (ADR-20260623-secretagent-phase6-milestone, slice 6g):**
- **backup** snapshots the four data-dir artifacts into a destination directory: `memory.db` via the **SQLite Online Backup API** (never `cp` a live WAL DB), and `store.age` / `identity.age` / `audit.jsonl` copied byte-for-byte. The age vault **stays encrypted** (we copy the ciphertext `store.age`, never decrypt). The copied identity is `chmod 600` (it is the private key wherever it lands).
- **restore** copies the four artifacts from the backup dir back into `data_dir()`, `chmod 600`s the identity, and **verifies the restored audit hash-chain** (reports verified / UNVERIFIED, never silently). Overwrites the live data dir — the operator stops the daemon first.
- **export** reads a session's `messages` + the audit log → JSONL. Message `content` is **redacted** when it matches the canonical recognizable-secret detector (`sa_memory::looks_like_secret`); audit events are secret-free by construction (key NAMES + principal only). The artifact is then **re-scanned line-by-line** with the same detector and the export **fails closed** (deletes the file, errors) if anything still trips — making "secret-free" an *enforced postcondition w.r.t. the detector*, not a hope.
- `doctor` gains an **audit-chain integrity** line (verify the live `audit.jsonl`), the ops-relevant health check.

**Tech Stack:** Rust, `rusqlite` `backup` feature (Online Backup API — part of the already-bundled SQLite, **no new crate**, musl-clean), stdlib `std::fs` for the flat-file copies, `serde_json` for the JSONL.

## Global Constraints
- **Never `cp` a live WAL DB:** `memory.db` is snapshotted through `Connection::backup`. The flat files (`store.age`/`identity.age`/`audit.jsonl`) are append-or-replace-whole, so a byte copy is consistent for them.
- **Vault stays encrypted in the archive:** copy `store.age` ciphertext; never decrypt. The identity travels with it (or the backup is undecryptable) — so the backup dir is as sensitive as the data dir; `chmod 600` the identity and warn the operator.
- **Secret-free export is enforced, not assumed:** redact-then-rescan; fail closed on any residual hit. Honest scope: the detector catches *recognizable* secret material (the same one guarding skill writes); an arbitrary high-entropy pasted secret with no recognizable shape is the documented limit (the safe-by-construction alternative is the audit-only trajectory). Redaction is whole-field (`[redacted]`), never partial (no leak via a substring boundary).
- **Backup scope = data-dir operational state** (`identity.age`/`store.age`/`memory.db`/`audit.jsonl`). `config.toml`/`SOUL.md`/`context.md` are operator-authored config in `config_dir()` (often a different dir) — out of scope, documented.
- **rustls-only / musl-static unchanged;** the `backup` feature pulls **no new crate** (commit `Cargo.lock` if it moves at all).
- **TDD**; commit per task; footer `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>` + `Claude-Session: phase-6g`; append ` # self-audit-ok`; push separately.
- **Gates:** fmt/clippy(all-features) 0; `cargo test --all` BOTH venues (Windows + WSL musl); rustls-only clean; CI green on all 5 jobs. **Focused adversarial review before push** (6g touches the encrypted vault + the audit chain + a secret-redaction boundary).

## File Structure
- `Cargo.toml` (workspace) — `rusqlite` features `["bundled"]` → `["bundled","backup"]`.
- `crates/sa-memory/src/lib.rs` — `pub fn looks_like_secret` (expose the existing detector) + `Store::backup_to(&self, dst: &Path)` (Online Backup API) + tests.
- `crates/sa-audit/src/lib.rs` — `Audit::read_events(path) -> Result<Vec<AuditEvent>>` (tolerant reader; empty if absent) + test.
- `secretagent/src/ops.rs` — **NEW.** `backup(dest)` / `restore(src)` / `export(session, out)` + pure `redact`/`scan_secret_free` helpers + unit tests.
- `secretagent/src/doctor.rs` — audit-chain verify line.
- `secretagent/src/main.rs` — `Cmd::{Backup,Restore,Export}` + dispatch + `mod ops;`.
- `secretagent/Cargo.toml` — `[dev-dependencies]` add `sa-memory` + `sa-core-types` (seed/verify the DB in the round-trip integration test).

---

### Task 1: sa-memory — `backup_to` + public `looks_like_secret`

**Files:** `Cargo.toml` (workspace rusqlite feature), `crates/sa-memory/src/lib.rs`.

**Interfaces — Produces:** `pub fn sa_memory::looks_like_secret(&str) -> bool`; `Store::backup_to(&self, dst: &Path) -> Result<()>`.

- [ ] **Step 1: Failing tests** — `backup_to_produces_a_readable_consistent_copy` (open a DB, add messages, `backup_to(tmp2)`, open tmp2 as a fresh `Store`, recent() matches — proves the Online Backup API snapshots a live WAL DB into a self-contained file); `looks_like_secret_is_public_and_flags_recognizable_secrets` (a direct call now the fn is `pub`).
- [ ] **Step 2: FAIL. Step 3: Implement** — add `"backup"` to the rusqlite features; `pub fn looks_like_secret`; `backup_to` via `self.conn.backup(rusqlite::DatabaseName::Main, dst, None)`.
- [ ] **Step 4: PASS. Step 5: Commit** `feat(6g): Store::backup_to via SQLite Online Backup API + pub looks_like_secret (phase 6g)`.

### Task 2: sa-audit — `read_events`

**Files:** `crates/sa-audit/src/lib.rs`.

**Interfaces — Produces:** `Audit::read_events(path: &Path) -> anyhow::Result<Vec<AuditEvent>>` (empty if the file is absent; tolerates a torn final line like `open`).

- [ ] **Step 1: Failing test** — `read_events_returns_appended_events_and_tolerates_a_torn_tail` (append 2, read → 2 with the right actions; append a partial non-JSON line, read → still 2).
- [ ] **Step 2: FAIL. Step 3: Implement** — read lines, parse each `Entry`, collect `entry.event`; on a parse error of the last line break; non-existent path → `Ok(vec![])`.
- [ ] **Step 4: PASS. Step 5: Commit** `feat(6g): Audit::read_events — tolerant secret-free event reader (phase 6g)`.

### Task 3: bin `ops.rs` — backup / restore / export + doctor line

**Files:** Create `secretagent/src/ops.rs`; modify `secretagent/src/main.rs`, `secretagent/src/doctor.rs`, `secretagent/Cargo.toml` (dev-deps).

**Interfaces — Produces:** `ops::backup(&Path)` / `ops::restore(&Path)` / `ops::export(&str, Option<PathBuf>)`; pure `redact(&str) -> String`, `scan_secret_free(&str) -> bool`.

- [ ] **Step 1: Failing tests** —
  - unit (`ops.rs`): `redact` returns `[redacted]` for a secret-bearing string and the content verbatim otherwise; `scan_secret_free` true on clean text, false on a line carrying a secret.
  - integration (`tests/cli.rs`): `backup_then_restore_round_trips_db_and_vault` (DATA_DIR A: `vault init` + `vault set`; seed a message via `Store`; `backup <dir>`; into a fresh DATA_DIR B: `restore <dir>`; assert `vault get` returns the secret AND `Store` at B shows the message); `export_is_secret_free` (seed a secret-bearing message + a clean one; `export --out <file>`; the file omits the secret, contains `[redacted]`, and contains the clean content).
- [ ] **Step 2: FAIL. Step 3: Implement** — `backup`: `create_dir_all(dest)`; if `db_path` exists, `Store::open(db).backup_to(dest/memory.db)`; copy each existing flat file; `chmod 600` the copied identity; print the manifest + the "contains your private identity key" warning. `restore`: copy each present artifact from src → `data_dir()`; `chmod 600` the identity; `Audit::verify_chain` the restored log → print verified/UNVERIFIED; print the "overwrites the live data dir; stop the daemon first" warning. `export`: zip `all_messages` + `message_provenances` (both `ORDER BY id`, aligned) → `{type:"message",role,provenance,content:redact(content)}` lines + `read_events(audit_path)` → `{type:"audit",seq,action,key_id,principal}` lines; write to `out` (or `data_dir()/trajectory-<session>.jsonl`); **re-scan** every written line with `scan_secret_free` → on any failure `remove_file` + bail. `doctor`: verify `audit_path()` chain (ok/warn/info), never gating exit. `main.rs`: three flat subcommands.
- [ ] **Step 4: PASS. Step 5: Commit** `feat(6g): backup/restore + secret-free trajectory export + doctor audit-chain line (phase 6g)`.

### Task 4: review + gate + ship
- [ ] Focused **adversarial review** (vault/identity handling, audit-chain verify, secret-free export) before push; fix findings.
- [ ] Both-venue gate (Windows `cargo test --all` + WSL musl); rustls-only clean; `cargo fmt --check` / `clippy -D warnings`.
- [ ] Push; watch CI green on all 5 jobs (re-run the flaky aarch64-musl `ring` leg if it fails); update PROGRESS.md/ROADMAP.md + the project memory.

---

## Acceptance (ADR slice 6g)
- **backup → restore round-trips a live DB** (Online Backup API; the vault + audit travel encrypted; identity restored 0600). ✓ Task 1 + Task 3 integration test.
- **export is secret-free** (redact-then-rescan, fail-closed). ✓ Task 3 unit + integration tests.
- `doctor` verifies the audit hash-chain. ✓ Task 3.
- **Honest scope / residuals (→ 6i):** export redaction catches *recognizable* secrets (detector-bounded); config-dir files are out of backup scope; restore overwrites (stop the daemon first); single-file tarball archive is the named upgrade over the backup *directory* (zero-dep now).
