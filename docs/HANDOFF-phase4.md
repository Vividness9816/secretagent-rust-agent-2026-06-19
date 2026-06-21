# SecretAgent — handoff to a fresh session (Phase 4: daemon + messaging + cron)

Paste this whole file into a new session to continue the build. It is self-contained.

---

You're continuing a multi-phase build of **SecretAgent**, a clean-room Rust agent daemon
(full-Hermes-Agent-parity destination, MIT, **no Hermes source copied**). **Phases 0–3 are
complete, CI-green, and pushed.** Your job is **Phase 4 — daemon + messaging + cron.**

## Where it lives
- Repo: `C:\Users\dnoye\ClaudeSecondBrain\SecretAgent` (nested git repo, branch `master`).
- Private remote: `Vividness9816/secretagent-rust-agent-2026-06-19`. `master` is at `d5e605e`
  (Phase 3 complete); working tree clean, everything pushed.
- **Confirm state first:** `git log --oneline -10` and `git status`.

## Read first (authoritative; ADR wins on conflict)
- The `project-secretagent` memory (auto-loads) — the running ledger of every phase.
- `~/.claude/second-brain/decisions/ADR-2026061*-secretagent-*.md` + `ADR-20260620-secretagent-phase3-learning-loop.md`
  — the founding architecture, the Phase-2 sandbox decision, and the Phase-3 learning-loop
  decision. **These bind you.** Key standing rules: the **JIT-crate rule** (a crate ONLY at a
  real compile boundary — never a stub) and the **4 invariants** (single self-contained binary
  per OS; SQLite single canonical store, every index rebuildable; tool output is tainted, never
  an instruction; no secret in DB/audit/logs).
- The spec `~/Downloads/SecretAgent-Build-Plan.md` — **Phase 4 = line 235**; §4 Parity Inventory
  is the v1 destination; §5 the target crate map; §6 the data model (`cron_jobs`, `connectors_state`,
  `subagents` are Phase-4/5 tables); §12 Open Decisions (#5 NL-schedule parsing, #6 connector
  priority) are yours to resolve + record.
- `README.md` (current capabilities) + `docs/superpowers/plans/2026-06-20-secretagent-phase{0,1,2a,2b,2c,3a,3b,3c}.md`
  (the executed per-slice plans — the house style for new plans) + `docs/HANDOFF-phase3.md`
  (the prior handoff; Phases 0–3 detail).

## Done so far (do NOT rebuild)
- **Phase 0** — 4-crate workspace + age-file vault + sole-writer blake3 audit + `doctor` + CI matrix.
- **Phase 1** — SQLite/FTS5 memory + OpenAI-compatible streaming provider + `secretagent chat`
  (live-proven cross-session recall vs local Ollama `hermes3`).
- **Phase 2** (2a floor+injection-guard / 2b landlock / 2c MCP client) — `Tainted<T>` boundary,
  pure `Policy` + approval/egress/path gates, `sa-tools` (fetch/read_file/write_file/execute_code),
  `sa-core::run_task` agentic loop, landlock-confined `execute_code` (Linux), namespaced+allow-listed
  MCP client.
- **Phase 3** (3a / 3b / 3c) — the **learning loop**:
  - **3a:** versioned SQLite migration runner (gated on a `schema_meta` version pointer),
    `user_model` stated-preference store (`pref set/list`, the ONLY writer, always `Trusted`),
    SOUL.md/context files, the `compose_system`/`ContextBundle`/`SystemContext` seam.
  - **3b:** SQLite-canonical **skills** (`skills`/`skill_versions`/`skills_fts`), `sa-core::eval`
    (a `Trajectory` + a **deterministic** rubric + a **pure-Rust** drafter that reads the agent's
    assistant-reasoning ONLY), `run_task` recall→**intent-bound** (`slug(task)`) approval-gated
    activation→inject→create/reuse+score, `skill list/activate` CLI. The skill-trust boundary is
    structural (born `Untrusted`/inert; the composed body is frozen post-create; a cross-session
    adversarial replay test is the ship gate). Hardened against a 12-finding adversarial review.
  - **3c:** memory summarization — `session_summaries`, `Provider::complete` (collect `chat`),
    `Agent::summarize_session` (rolling LLM summary of older context, behind the Provider seam,
    surfaced as context-not-instruction), `secretagent summarize` CLI.

**Current crates** (`secretagent` bin + 8 libs): `sa-core-types`, `sa-vault`, `sa-audit`,
`sa-memory`, `sa-providers`, `sa-tools`, `sa-exec`, `sa-core`. **CLI surface today:** `doctor`,
`vault init|set|get`, `chat`, `run`, `pref set|list`, `skill list|activate`, `summarize`.
**SQLite schema is at `SCHEMA_VERSION = 4`** (messages+fts / user_model / skills+skill_versions+fts /
session_summaries). Test counts: sa-memory 18, sa-core 19, sa-providers 6, secretagent integration ×N.

## YOUR TASK: Phase 4 — daemon + messaging + cron
Spec (line 235-237): **`sa-gateway`** daemon, **`Connector`** trait + first connectors
(**Telegram, Discord, Email**), **`sa-scheduler`** (NL → cron, unattended runs, delivery).
*Acceptance:* (1) the daemon **installs as a service and survives reboot**; (2) the agent is driven
**end-to-end from Telegram**; (3) a **natural-language scheduled job** ("every morning at 7,
summarize X") fires and **delivers to a connector**.

**Start with `/council`** (not straight to planning) — Phase 4 has real architecture forks worth
a recorded ADR before building:
- **Daemon shape & service install:** a long-running `tokio` gateway vs. a thin dispatcher; how
  `secretagent service install` writes a systemd unit / launchd plist / Windows Service (spec §9)
  and *only* that (no shell-rc mutation). Reboot-survival is acceptance #1 — design the
  install + run loop for it. Honor the single-binary + headless invariants.
- **`Connector` trait + the first three:** the inbound (poll vs webhook) + outbound (delivery)
  shape that Telegram/Discord/Email share; how connector secrets live in the **vault** (never
  config/plaintext); how an inbound message becomes a `run_task`/`turn` and the reply is delivered.
  Pick which connector proves the E2E acceptance (Telegram). `teloxide`/`serenity`/SMTP are the
  spec's suggestions — confirm or revise with the JIT-crate + cargo-deny lens.
- **`sa-scheduler` NL→cron (spec §12.5):** pure-LLM parse vs **LLM + a deterministic validator**
  (the spec prefers the latter for unattended reliability). How jobs persist (`cron_jobs` table,
  §6), how unattended runs are gated (no human at the approval prompt — what is auto-approved vs
  refused for a scheduled run?), and how output is delivered to a connector.
- **The untrusted-input boundary widens here:** connector messages are **untrusted input** — they
  must enter as `run_task` input under the *same* injection guard + approval gating, and a remote
  sender must never reach a side-effectful tool or the `--allow-unsandboxed-exec` switch without
  the operator's standing policy. Treat this as the slice's security spine (adversarially verify it).
- **Decompose into slices** (mirror 2a/2b/2c, 3a/3b/3c) — e.g. 4a gateway+service-install,
  4b Connector trait + Telegram E2E, 4c sa-scheduler — each its own plan + acceptance gate.

After the ADR: **writing-plans** → inline **TDD**, gating + committing each task, push, watch CI
green, **stop at each slice's acceptance gate** for review.

## Conventions / gates (non-negotiable — held through Phases 0–3)
- **TDD**; commit per task with conventional-commit messages ending with the footer:
  `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>` then a `Claude-Session:` line.
- Before EVERY commit: `cargo fmt --all -- --check` (0) / `cargo clippy --all-targets
  --all-features -- -D warnings` (0) / relevant `cargo test` (pass).
- **Run `cargo fmt --all` (not just `--check`) after hand-writing code** — rustfmt rewraps long
  call chains, closures, and `matches!`; expect a format pass before the gate passes.
- The **`self-audit` PreToolUse hook blocks `git commit`** — append ` # self-audit-ok` to the
  bash command.
- **Adversarially verify any new trust boundary BEFORE push** — connectors + the scheduler add the
  biggest new untrusted-input surface in the project. Use a multi-lens **adversarial-review
  Workflow** (the Phase-2c MCP review and the Phase-3b skills review each caught real,
  ship-blocking findings — incl. an approval-bypass and a cross-session launder); a single
  `self-audit` agent for lower-risk surfaces.
- After push, **watch CI to green**: `RUN_ID=$("/c/Program Files/GitHub CLI/gh.exe" run list
  --branch master --limit 1 --json databaseId --jq '.[0].databaseId'); "/c/Program Files/GitHub
  CLI/gh.exe" run watch "$RUN_ID" --exit-status --interval 25`. Confirm `conclusion=success` on
  all 5 jobs. Fix red before moving on.

## Build / CI gotchas (already solved — keep them)
- **WSL is the Linux build/test venue** (landlock can't compile on Windows). Build there:
  `wsl.exe bash -c 'export PATH="$HOME/.cargo/bin:$PATH" CARGO_TARGET_DIR="$HOME/sa-target"; cd
  /mnt/c/Users/dnoye/ClaudeSecondBrain/SecretAgent; cargo …'`. **Gate cross-platform code on BOTH
  WSL and Windows** (`cargo` is on PATH on the Windows box too). CI mirrors WSL (ubuntu-latest,
  landlock ABI 3) + a cross matrix (Linux x86_64&aarch64-musl, macOS, Windows MSVC).
- `cargo-deny` allow-list already covers the closure (`CDLA-Permissive-2.0`, landlock's
  `MIT OR Apache-2.0`); keep `RUSTSEC-2024-0370` ignored. A new connector dep (teloxide/serenity/
  SMTP) will likely add licenses/advisories — extend `deny.toml` deliberately, don't broaden blindly.
- `rust-cache` is gated `if: !matrix.cross` (warm cache breaks the cross-musl build). Static check
  uses `readelf -d | grep NEEDED` (NOT `ldd` — musl is static-PIE). Keep `rusqlite` `bundled` +
  `reqwest` `rustls-tls` so the musl-static binary holds — a connector that pulls `native-tls`/
  openssl would break it; prefer rustls-based clients.
- **Commit `Cargo.lock` in the same change whenever you add a dep/feature** (a `tokio` feature
  once pulled in `signal-hook-registry` and the lock drift was easy to miss).
- **SQLite migrations:** add a new `(version, SQL)` tuple to the `MIGRATIONS` const in
  `crates/sa-memory/src/lib.rs` + bump `SCHEMA_VERSION` (now 4 → 5). Use **plain `CREATE`** (the
  runner version-gates it). **Tests must assert `SCHEMA_VERSION`, never a version literal** (literal
  pins broke on every bump). A test that simulates an OLD DB must **drop ALL newer tables** before
  reopening, or a plain-CREATE migration hits "table already exists".
- Local Ollama has `hermes3:latest` (tools-capable) for live tests — set `model` in `config.toml`.
- Tooling notes: `gh` is at `/c/Program Files/GitHub CLI/gh.exe`; **no `python3` on the Windows
  Git Bash** (use `wsl.exe bash -c 'python3 …'` or `jq` to parse JSON); the `LF will be replaced by
  CRLF` git warnings are benign (autocrlf).

## Carried-forward deferrals & accepted residuals (don't re-litigate; build on them)
- **Vault:** age-file backend only; OS keyring is feature-gated off the default + acceptance path
  (drops in behind the existing `Vault` trait). **Connector secrets go in the vault.**
- **Sandbox/exec:** landlock-only; `execute_code` is fail-closed off Linux; seccomp/namespaces/
  `ExecutionBackend` trait deferred. `--allow-unsandboxed-exec` is a per-invocation, loudly-audited
  override — **a scheduled/connector-driven run must never silently get it.**
- **Skills (3b):** composed body is **frozen post-create** (reuse appends lineage only; body
  re-improvement needs a future re-approval flow). Activation is intent-bound to `slug(task)`.
  Accepted residual: under `--yes`, a model that echoes injected text into its own create-run
  answer can seed that one task's skill body — reviewable via `skill list`. Export-only SKILL.md
  serializer is deferred (not on any load path).
- **Summarization (3c):** explicit (`summarize` CLI) — **no auto-trigger in the hot loop**
  (a Phase-4 scheduler is the natural place to invoke it periodically). Episodic/semantic triple
  stores + embeddings are deferred (ADR/YAGNI).
- **Provenance:** the `Provenance` enum is `Trusted | Untrusted{source}` — no third variant; persist
  it as a serialized-string column where SQLite stores trust (skills + user_model do).

## Workflow patterns that worked this project (reuse them)
- `/council` for each architecture fork → a recorded **ADR** in `~/.claude/second-brain/decisions/`
  (ADR wins on conflict). Then **writing-plans** → **inline TDD** with per-task gate+commit.
- For a slice with real design detail, a **design fan-out Workflow** (parallel facets → synthesize
  the plan) de-risks before writing the plan.
- For any new trust boundary, an **adversarial-review Workflow** (find-lenses → independent verify)
  **before push**; fix confirmed findings, re-gate, then push.
- Full-suite + **both-venue** gate before every push; then watch CI to green.

Start by reading the spec's Phase 4 section + the ADRs + the `project-secretagent` memory, confirm
state with `git log --oneline -10`, then run `/council` on the Phase 4 architecture.
