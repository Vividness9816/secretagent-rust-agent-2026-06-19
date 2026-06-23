# SecretAgent — handoff to a fresh session (Phase 4 continued: live Telegram E2E + 4d scheduler)

Paste this whole file into a new session to continue the build. It is self-contained.

---

You're continuing a multi-phase build of **SecretAgent**, a clean-room Rust agent daemon
(full-Hermes-Agent-parity destination, MIT, **no Hermes source copied**). **Phases 0–3 are
complete and CI-green. Phase 4 (daemon + messaging + cron) slices 4a, 4b, and 4c are complete and
CI-green.** Two things remain to finish Phase 4: **(1) the live Telegram end-to-end check
(acceptance #2)** and **(2) slice 4d — the NL→cron scheduler (acceptance #3).**

## Where it lives
- Repo: `C:\Users\dnoye\ClaudeSecondBrain\SecretAgent` (nested git repo, branch `master`).
- Private remote: `Vividness9816/secretagent-rust-agent-2026-06-19`. `master` is at **`b382e10`**
  (Phase 4c complete); working tree clean, everything pushed.
- **Confirm state first:** `git log --oneline -12` and `git status`.

## Read first (authoritative; ADR wins on conflict)
- The `project-secretagent` memory (auto-loads) — the running ledger of every phase.
- `~/.claude/second-brain/decisions/ADR-20260621-secretagent-phase4-daemon-messaging-cron.md`
  (**the binding Phase-4 architecture**) + the three prior ADRs (`ADR-20260619-…-founding`,
  `ADR-20260620-…-phase2-sandbox`, `ADR-20260620-…-phase3-learning-loop`). Standing rules: the
  **JIT-crate rule**, the **4 invariants** (single self-contained binary per OS; SQLite single
  canonical store + every index rebuildable; tool/connector output is `Tainted`, never an
  instruction; no secret in DB/audit/logs), and the **Principal trust model** (a connector message
  is a `Remote` principal — structurally unable to auto-approve a side-effect, auto-activate a
  skill, or write durable memory; **M1** structural, **M2** no-durable-write, **M3** default-deny
  `(connector,sender)` allow-list before `run_task`).
- `README.md` (capabilities through Phase 4c), `ROADMAP.md` (phase map), `PROGRESS.md` (slice
  ledger with commit hashes), `docs/superpowers/plans/2026-06-21-secretagent-phase4{a,b,c}-*.md`
  (the executed per-slice plans — the house style), and `docs/HANDOFF-phase4.md` (the original
  Phase-4 handoff).
- The spec `~/Downloads/SecretAgent-Build-Plan.md` — **4d uses §12.5** (NL-schedule: LLM + a
  deterministic validator) and **§6** (`cron_jobs` table).

## Done so far (do NOT rebuild)
- **4a (trust spine):** `Principal`/`RunContext` in `sa-core-types/src/principal.rs`; `run_task`
  takes `&RunContext` (not a bare `auto_approve: bool`); `AuditEvent.principal: Option<String>`;
  the `gateway` daemon-loop skeleton + `GatewayState` + clean shutdown.
- **4b (service install, acceptance #1):** `secretagent service install|uninstall|status` —
  `secretagent/src/service/{mod,linux,windows}.rs` (systemd unit writer + `windows-service` SCM
  dispatcher, target-gated) + a `doctor` service line.
- **4c (connectors + the M3 boundary, acceptance #2 buildable):** the `sa-connectors` crate
  (`Connector` trait + Telegram/Discord/Email + `MockConnector`); the M3 boundary is
  `secretagent/src/gateway.rs::dispatch_inbound` (default-deny `allow_senders` BEFORE `run_task` →
  `Remote` run with the binding's frozen `allow_tools` → reply; every inbound decision audited by
  principal); the gateway run loop drives connectors. **A multi-lens adversarial review ran before
  push and caught + fixed a bot-token-in-logs leak; the structural boundary held.** All connectors
  are **rustls-only** (musl-static holds — no openssl/native-tls/aws-lc-sys/zstd-sys).

---

## TASK 1 — the live Telegram E2E (acceptance #2)

Everything is built + CI-green; this is a manual run that needs the operator's bot token + numeric
Telegram id. **A staged, isolated E2E env already exists at `C:\Users\dnoye\sa-e2e`** (its own
vault with `TELEGRAM_BOT_TOKEN` set + a `config.toml`) — but the bot token used there was pasted
into a prior chat, so **regenerate it** (see "how to get / set your token" below) before relying on
it, or just redo the env fresh.

**Resume steps** (run with the E2E env vars so it never touches real config):
1. **Set the token** (so it never enters the chat): the operator runs, via the session's `!`
   prefix (PowerShell), with their real BotFather token in place of `PASTE_TOKEN`:
   ```
   ! $env:SECRETAGENT_DATA_DIR="C:/Users/dnoye/sa-e2e"; & "C:\Users\dnoye\ClaudeSecondBrain\SecretAgent\target\debug\secretagent.exe" vault init; $env:SECRETAGENT_DATA_DIR="C:/Users/dnoye/sa-e2e"; & "C:\Users\dnoye\ClaudeSecondBrain\SecretAgent\target\debug\secretagent.exe" vault set TELEGRAM_BOT_TOKEN "PASTE_TOKEN"
   ```
   (It prints `set TELEGRAM_BOT_TOKEN` — the value is never echoed.)
2. Write `C:\Users\dnoye\sa-e2e\config.toml` with the provider (`base_url =
   "http://localhost:11434/v1"`, `model = "hermes3:latest"` — confirm Ollama is up:
   `curl -s http://localhost:11434/api/tags`) and a `[[connectors]]` binding: `kind = "telegram"`,
   `token_ref = "TELEGRAM_BOT_TOKEN"`, `allow_senders = ["<operator-numeric-id>"]`, `allow_tools = []`.
3. Run the gateway (background): `SECRETAGENT_DATA_DIR='C:/Users/dnoye/sa-e2e'
   SECRETAGENT_CONFIG_DIR='C:/Users/dnoye/sa-e2e' RUST_LOG=info ./target/debug/secretagent.exe gateway`.
4. The operator opens the bot's chat in Telegram, taps **Start**, sends a message.
5. Confirm: the operator gets a reply; `C:\Users\dnoye\sa-e2e\audit.jsonl` shows
   `connector.accepted` with `remote:telegram:<id>` then the run; a message from a NON-allow-listed
   id shows `connector.rejected` and gets no reply (M3 proven live).
6. Stop the gateway (`taskkill //F //IM secretagent.exe`).

**Diagnostics if no reply:** `curl -s https://api.telegram.org/bot<token>/getMe` (token valid?) and
`/getWebhookInfo` (a set webhook makes `getUpdates` return 409 — delete it with `/deleteWebhook`).

**How to get / set your token (E2E setup step):**
- **Get a bot token:** in Telegram message **@BotFather** → `/newbot` → follow prompts → it gives a
  token like `12345:ABC-…`. To rotate a leaked token: `/revoke` then re-issue.
- **Get your numeric id:** message **@userinfobot** (or **@RawDataBot**) — it replies with your id.
- **Store the token without putting it in chat:** use the `!`-prefixed `vault set` command in step 1
  above (only `set TELEGRAM_BOT_TOKEN` prints back; the token stays out of the transcript).

---

## TASK 2 — slice 4d: the NL→cron scheduler (acceptance #3)

This is the last Phase-4 slice. **Acceptance #3:** an NL scheduled job ("every morning at 7,
summarize X") fires and delivers to a connector. The architecture is already decided in
**ADR-20260621** (no new `/council` needed — go straight to `writing-plans` → inline TDD):

- **`sa-core::schedule` module** (NOT a new crate): the NL→cron parse is **LLM proposes a 5-field
  cron expression + a deterministic Rust validator gates it** — parse it with a cron crate, bounds-
  check, and **reject** an unparseable or sub-minimum-interval (`* * * * *` DoS) expression. The
  validator gates; the LLM never gates. Pure function, unit-tested, CI-reproducible.
- **`cron_jobs` migration** (SCHEMA_VERSION **4→5**, in `crates/sa-memory/src/lib.rs` — add a
  `(5, "CREATE TABLE …")` tuple; plain `CREATE`; tests assert `SCHEMA_VERSION`, never a literal):
  columns per spec §6 — `nl_spec`, `cron_expr`, `action`/task text, `target_connector`,
  `allowed_tools` (the **frozen** per-job grant, serialized JSON), `last_run`, `next_run`, `enabled`.
- **M4 — freeze at arm time:** a job's task text, cron expr, and `allowed_tools` are persisted at
  creation and **never re-derived at fire time**. A fired job runs as a `Remote`-style principal
  carrying the job's frozen `allowed_tools`, output delivered to `target_connector`.
- **Pull write-root symlink resolution forward** into the unattended write path
  (`crates/sa-core-types/src/policy.rs::path_allowed` does NO symlink resolution today — canonicalize
  the write root + target before the `starts_with` check, so an unattended write can't escape via a
  symlink).
- **The gateway tokio loop ticks the scheduler** (it already has `tokio` time): on each due job, run
  it and deliver via the connector. Reuse the `dispatch_inbound`/`RunContext::remote` machinery.
- **Adversarially verify** the scheduler trust boundary before push (a single `self-audit` agent is
  fine here; the connector boundary already had the heavier multi-lens Workflow). Then both-venue
  gate + push + watch CI.

---

## Conventions / gates (non-negotiable — held through Phases 0–4c)
- **TDD**; commit per task; conventional-commit subject; footer = a blank line then
  `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>` then `Claude-Session: phase-4d`.
- Before EVERY commit: `cargo fmt --all` (then `--check` = 0) / `cargo clippy --all-targets
  --all-features -- -D warnings` (0) / relevant `cargo test`. **Run `cargo fmt --all` after writing
  code** (rustfmt rewraps).
- The **`self-audit` PreToolUse hook blocks `git commit`** — append ` # self-audit-ok` to the bash
  command.
- **Both-venue gate before push** (Windows `cargo test --all` + WSL `wsl.exe bash -c 'export
  PATH="$HOME/.cargo/bin:$PATH" CARGO_TARGET_DIR="$HOME/sa-target"; cd
  /mnt/c/Users/dnoye/ClaudeSecondBrain/SecretAgent; cargo test --all'`). **WSL is the landlock/Linux
  venue.** Then watch CI green on all 5 jobs: `RUN_ID=$("/c/Program Files/GitHub CLI/gh.exe" run
  list --branch master --limit 1 --json databaseId --jq '.[0].databaseId')` then `gh run watch
  "$RUN_ID" --exit-status --interval 25` — **but `--limit 1` can race a just-pushed run**, so verify
  the run's `headSha` matches HEAD (`gh run list --branch master --limit 4 --json
  databaseId,headSha,conclusion`).
- **Commit `Cargo.lock`** with any dep/feature change. Keep **rustls-only** — no openssl/native-tls/
  aws-lc-sys/zstd-sys (verify `wsl … cargo tree -e features -p secretagent | grep -iE
  "openssl|native-tls|aws-lc-sys|zstd-sys"` is empty); `cargo deny check` must stay green.
- `gh` is at `/c/Program Files/GitHub CLI/gh.exe`; no `python3` on Windows Git Bash; the
  `LF will be replaced by CRLF` git warnings are benign.

## Build / CI gotchas (already solved — keep them)
- Local Ollama has `hermes3:latest` (tools-capable) for live tests — set `model` in `config.toml`.
- A bin crate runs its modules' `#[cfg(test)]` unit tests under `cargo test -p secretagent`; the
  gateway boundary is tested that way (no live network) via `MockConnector`.
- SQLite migration tests must **drop all newer tables** before reopening an old-DB fixture, or a
  plain-`CREATE` migration hits "table already exists".
- Heavy/optional connector deps are **feature-gated** in `sa-connectors` (`discord`/`email`) and
  enabled by the bin; pin every TLS dep to rustls + `ring` (NOT the default `aws_lc_rs`) and avoid
  `zstd` (use `zlib`) so the musl-static binary holds.
