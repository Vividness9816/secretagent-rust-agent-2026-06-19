# SecretAgent — handoff to a fresh session (Phase 3: the learning loop)

Paste this whole file into a new session to continue the build.

---

You're continuing a multi-phase build of **SecretAgent**, a clean-room Rust agent daemon
(full-Hermes-Agent-parity destination, MIT, no Hermes source copied). **Phases 0–2 are
complete, CI-green, and pushed.** Your job is **Phase 3 — the learning loop.**

> **Progress (2026-06-20):** Architecture decided in
> `~/.claude/second-brain/decisions/ADR-20260620-secretagent-phase3-learning-loop.md`
> (council; **ADR wins on conflict**). Phase 3 is sliced **3a → 3b → 3c**.
> **Slice 3a is COMPLETE** (plan `docs/superpowers/plans/2026-06-20-secretagent-phase3a.md`):
> versioned migration runner + `user_model` (EAV-with-provenance) + stated-preference CLI
> (`pref set/list`, always `Provenance::Trusted`, the *only* writer) + SOUL.md/context +
> shared `compose_system`/`ContextBundle` seam. A security test locks "no preference from
> untrusted content."
> **Slice 3b is COMPLETE** (plan `docs/superpowers/plans/2026-06-20-secretagent-phase3b.md`):
> the skills learning loop + the skill-trust boundary. Migration 3 (`skills`/`skill_versions`/
> `skills_fts`, SQLite-canonical), `sa-core::eval` (Trajectory + deterministic rubric +
> **pure-Rust** drafter that reads assistant-reasoning only), `run_task` recall→approval-gated
> activation→inject (top-1 active only)→create/reuse+score, `activate_skill` on the existing
> approval gate, `skill list/activate` CLI. Skills born `Untrusted`+inert; the cross-session
> **adversarial replay test** (`poisoned_skill_is_born_untrusted_and_never_reinstructed_across_a_restart`)
> is the ship gate.
> **Slice 3c is COMPLETE** (plan `docs/superpowers/plans/2026-06-20-secretagent-phase3c.md`):
> memory summarization — Migration 4 `session_summaries`, `Provider::complete` (collect chat),
> `Agent::summarize_session` (rolling LLM summary of messages older than the recent window,
> behind the Provider seam, derived from user+assistant rows only), surfaced into
> `assemble_context` as context-not-instruction, `secretagent summarize` CLI. Self-audited PASS.
> **➡ PHASE 3 COMPLETE (3a + 3b + 3c).** The spec's Phase-3 acceptance is satisfied as of 3b;
> 3c added the optional summarization leg. **Next milestone = Phase 4** (daemon + messaging +
> cron) — out of this handoff's scope; start it with its own `/council` + plan.

## Where it lives
- Repo: `C:\Users\dnoye\ClaudeSecondBrain\SecretAgent` (nested git repo, branch `master`).
- Private remote: `Vividness9816/secretagent-rust-agent-2026-06-19`.
- **Read first:** the `project-secretagent` memory (auto-loads), `README.md`, the spec
  `~/Downloads/SecretAgent-Build-Plan.md` (Phase 3 = line 231; §4 Parity Inventory is the
  destination), and the ADRs in `~/.claude/second-brain/decisions/ADR-2026061*-secretagent-*.md`
  (founding + Phase-2 sandbox — **AUTHORITATIVE; on conflict, the ADR wins**).

## Done so far (do NOT rebuild)
- **Phase 0** — 4-crate workspace + age-file vault + sole-writer blake3 audit + `doctor`.
- **Phase 1** — SQLite/FTS5 memory + OpenAI-compatible streaming provider + `secretagent chat`
  (live-proven cross-session recall vs local Ollama hermes3).
- **Phase 2a** (floor + injection guard) — `Tainted<T>` (no-Deref, compile-fail boundary),
  pure `Policy` + egress/path/approval, `sa-tools` (fetch/read_file/write_file), provider
  tool-calling, `sa-core::run_task` agentic loop (gate → run → taint → audit → re-feed as
  DATA), `secretagent run`.
- **Phase 2b** (landlock) — `sa-exec` crate: `Sandbox` trait + `RefuseSandbox` + Linux-only
  `LandlockSandbox` (confined `/bin/sh` via `pre_exec`, env-cleared); `execute_code` fail-closed
  unless landlock runtime-enforced; per-invocation screaming `--allow-unsandboxed-exec` override;
  kernel deny-corpus with positive-control + no-op canary.
- **Phase 2c** (MCP) — `sa-tools/src/mcp.rs`: stdio JSON-RPC 2.0 client; tools loaded
  **namespaced** (`server::tool`, no shadowing) + **allow-listed** (default-deny); per-request
  timeout + message/line caps; namespace-aware approval gate; honest doc that an MCP tool's I/O
  is the server process's scope (not confined by our in-process Policy).

Current crates: `secretagent` (bin), `sa-core-types`, `sa-vault`, `sa-audit`, `sa-memory`,
`sa-providers`, `sa-tools`, `sa-exec`, `sa-core`. Plans live in `docs/superpowers/plans/`.

## YOUR TASK: Phase 3 — the learning loop
Spec (line 231): **`sa-skills`** (post-exec evaluation → skill create/refine, **agentskills.io**
format), **memory summarization**, **dialectic user model**, **SOUL.md + context files**.
*Acceptance:* completing a novel task auto-creates a reusable skill; the **same task next
session reuses + scores** the skill; the **user model reflects a stated preference.**

**Start with `/council`** (not straight to planning). Phase 3 has real open design forks that
warrant a recorded ADR before building — at least:
- **Skill representation & storage** — agentskills.io format specifics; where skills live
  (a new `sa-skills` crate vs. a `sa-core` module — apply the JIT-crate rule from the founding
  ADR: a crate only when there's a real compile boundary); how a skill is keyed/retrieved
  (FTS5 over `sa-memory`? embeddings? — note SecretAgent has no embedding stack yet).
- **The evaluation/refine trigger** — what "post-exec evaluation" runs (the model judging its
  own trajectory? a rubric?), how a skill is *scored* on reuse, and how scores drive refine.
  Tie it to the existing `run_task` loop + the audit log (the trajectory is already recorded).
- **User model + SOUL.md** — representation of a "dialectic user model"; how a *stated*
  preference is captured and surfaced; how SOUL.md / context files feed the system prompt.
  Keep secrets out (the `SecretRef`/audit invariants still hold).
- **Decompose into slices** if it's large (mirror Phase 2's 2a/2b/2c split), each its own plan
  + acceptance gate.

After the ADR: **writing-plans** → build inline **TDD**, gating + committing each task, then
push and watch CI to green. Stop at each slice's acceptance gate for review.

## Conventions / gates (non-negotiable — these held through Phases 0–2)
- **TDD**; commit per task; conventional commits ending with the footer:
  `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>` then a `Claude-Session:` line.
- Before EVERY commit: `cargo fmt --all -- --check` (0) / `cargo clippy --all-targets
  --all-features -- -D warnings` (0) / relevant `cargo test` (pass). **Run fmt, not just
  clippy** — hand-written Rust isn't rustfmt-canonical and CI fmt-checks.
- The `self-audit` PreToolUse hook blocks `git commit` — append ` # self-audit-ok` to the
  bash command.
- Don't pipe a gating command into `| tail` and trust the exit code — `${PIPESTATUS[0]}` or
  check separately; `&&` chains continue over a masked failure.
- After pushing, watch CI to green (`gh run watch <id> --exit-status --interval 25`, full path
  `"/c/Program Files/GitHub CLI/gh.exe"`; it sometimes returns early — re-attach). Fix red
  before moving on.
- **Adversarially verify** security-relevant work before push (a multi-lens review workflow
  caught 8 real findings in the 2c MCP client — including an approval-gate bypass — before it
  shipped). Run a self-audit / review on any new trust boundary.

## Build + CI gotchas (already solved — keep them)
- **WSL is the Linux build/test venue.** Anything Linux-only (landlock) can't compile on the
  Windows dev box. WSL Ubuntu has rust installed + **landlock ABI 3 live**; build there with
  `wsl.exe bash -c 'export PATH="$HOME/.cargo/bin:$PATH" CARGO_TARGET_DIR="$HOME/sa-target";
  cd /mnt/c/Users/dnoye/ClaudeSecondBrain/SecretAgent; cargo ...'` (separate target dir keeps
  artifacts off the slow 9p mount and away from the Windows `target/`). Cross-platform code:
  gate on **both** WSL and Windows. CI mirrors WSL (ubuntu-latest, same landlock ABI).
- `cargo-deny` allow-list already covers the closure (incl. `CDLA-Permissive-2.0`, landlock's
  `MIT OR Apache-2.0`). Keep `RUSTSEC-2024-0370` ignored (age i18n proc-macro-error, build-only).
- `rust-cache` is gated `if: !matrix.cross` (warm cache breaks the cross-musl build). Keep it.
- Static-binary check uses `readelf -d | grep NEEDED` (NOT `ldd` — musl is static-PIE).
- rusqlite `bundled` + reqwest `rustls-tls` keep the musl-static binary. `age 0.10`, `secrecy 0.8`.
- Local Ollama has `hermes3:latest` (tools-capable) — set `model` in `config.toml` for live tests.
- When you add a `tokio` feature, **commit `Cargo.lock`** in the same change (the `process`
  feature pulled in `signal-hook-registry` and the lock drift was easy to miss).

Start by reading the spec's Phase 3 section + the founding ADR, confirm the state above with
`git log --oneline -15`, then run `/council` on the Phase 3 architecture.
