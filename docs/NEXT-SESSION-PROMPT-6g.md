# Fresh-session prompt — SecretAgent Phase 6, continue at 6g

Paste everything below the line into a clean session.

---

Continue building **SecretAgent** (clean-room Rust agent daemon, full-Hermes parity v1, MIT) at
`C:\Users\dnoye\ClaudeSecondBrain\SecretAgent` (branch **master**).

**FIRST:** run `git log --oneline -8` and `git status` — master should be at **`4b7205f`**, working
tree clean, everything pushed. Then **READ `docs/HANDOFF-phase6.md` IN FULL** — it is the
self-contained, authoritative handoff (current state, the binding per-slice ADR architecture, the
scope line, the conventions/gates, the build gotchas). Also load the **`project-secretagent` memory**
and `~/.claude/second-brain/decisions/ADR-20260623-secretagent-phase6-milestone.md`.

**STATE:** Phases 0–5 complete + CI-green. **Phase 6 (parity v1) is /council-decided**
(ADR-20260623-phase6-milestone: 9 slices, refactor-first / packaging-early / self-update-last, an
honest "parity-by-mechanism" scope). **Slices 6a–6f are shipped + CI-green:**
- **6a** assemble_agent refactor; **6b** release packaging (signed/minisign/distroless/installer).
- **6c** egress-guarded HTTP seam (`sa-tools/src/egress.rs`) — fixed a live SSRF; `web_extract`/`http_request`/`web_search` through it.
- **6d** system+external tools — `shell` (fail-closed `sa_exec::Backend` alias) + `op_tool` (operator-frozen external-command adapter).
- **6e** providers — **native Anthropic** Messages API provider (`sa-providers/src/anthropic.rs`), `build_provider` = the single `Box<dyn Provider>` selection seam (openai|anthropic) + per-role model map, operator-only `secretagent model <name>` switch.
- **6f** TUI — `secretagent/src/tui.rs` reedline REPL (feature `tui`, default-on).

**YOUR TASK — build slice 6g next (ops), then 6h, then 6i, in order.** Each = a TDD slice ending in a
concrete acceptance test. Full per-slice architecture is in `docs/HANDOFF-phase6.md` §TASK; the short
form:

### 6g — ops (backup/restore + trajectory export)
- **`backup` / `restore`:** use the **SQLite Online Backup API** (rusqlite `Backup`) — **NEVER `cp` a
  live WAL DB**. The age vault (`store.age` / `identity.age`) stays **encrypted** in the archive;
  **`chmod 600` the identity on restore**; **verify the audit hash-chain** after restore. Each a clap
  subcommand + a `doctor` line, consistent with `service`/`schedule`.
- **`trajectory export`:** read `messages`/audit → JSON/JSONL, **secret-free** (exclude/redact
  `messages.content` or grep the artifact to prove no secret leaks — §11).
- *Acceptance:* backup→restore round-trips a live DB (data intact, identity `0600`, audit chain
  verifies); export is provably secret-free.

### 6h — self-update (LAST — or DEFER)
An RCE-as-a-service-with-vault-access primitive. **Ship ONLY with the full contract:** download-to-temp
→ **verify a detached signature against a public key PINNED in the binary** (`include_bytes!`, never
fetched) → **no-downgrade** (version from the *signed* payload) → **atomic rename** → audit event; with
**negative-control tests** that a tampered binary AND a downgrade are both rejected. **If that can't be
proven cleanly this milestone, DEFER it** (manual re-install is a safe v1) — it is the first thing to
cut. Decide explicitly and tell the user which (build-vs-defer is a real fork worth surfacing). The 6b
release pipeline already produces minisign signatures, so the pinned-verify contract is reachable —
but only build it if you can prove the negative controls.

### 6i — parity-tail doc + acceptance
`docs/parity-tail.md` (what shipped vs deferred-behind-which-trait + why) + the honest §4 acceptance
amendment (Pillar C). **Roll up the accumulated deferrals** already recorded in PROGRESS.md per slice:
Anthropic SSE streaming + full per-role routing + error-envelope parsing (6e); TUI token-streaming +
in-TUI approval prompt + persistent history (6f); `op_tool` not `approval_required`-gated (6d);
`web_extract` naive HTML strip (6c). *Acceptance:* the doc is accurate; `doctor` passes clean.

**DO NOT** rebuild anything under 6a–6f, and **DO NOT** build the deferred tail (browser-automation /
the 16 remaining connectors / daytona/singularity/modal — they're behind the established traits with
triggers, per the ADR scope line).

## HOW TO WORK (standing conventions — all detailed in the handoff)
- **TDD per task; commit per task.** Conventional-commit subject; footer = blank line then
  `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>` then `Claude-Session: phase-6g` (bump per slice).
- The **`self-audit` PreToolUse hook blocks `git commit`** → append ` # self-audit-ok` to the bash
  command. **It is a shell COMMENT: anything chained after it (`&& git push`) is swallowed — run
  `git push` as a SEPARATE command.**
- Before every commit: `cargo fmt --all --check` (0) / `cargo clippy --all-targets --all-features -- -D
  warnings` (0; test module LAST in each file — `items-after-test-module`) / relevant `cargo test`.
- **Both-venue gate before push:** Windows `cargo test --all` + WSL
  `wsl.exe bash -c 'export PATH="$HOME/.cargo/bin:$PATH" CARGO_TARGET_DIR="$HOME/sa-target"; cd
  /mnt/c/Users/dnoye/ClaudeSecondBrain/SecretAgent; cargo test --all; echo CARGO_EXIT=$?'`
  (WSL is the landlock/musl venue; `CARGO_EXIT=0` is the definitive signal).
- **Keep rustls-only** — a NEW dep must be pure-Rust + musl-clean (`wsl … cargo tree -e features -p
  secretagent | grep -iE "openssl|native-tls|aws-lc-sys|zstd-sys"` empty). **Commit `Cargo.lock`** with any dep change.
- Then watch CI green on all 5 jobs: `gh` at `/c/Program Files/GitHub CLI/gh.exe`;
  `gh run list --branch master --limit 5 --json databaseId,headSha` then `gh run watch "$RUN"
  --exit-status --interval 25`. **CI flake:** the `aarch64-unknown-linux-musl` leg occasionally fails
  on `ring`'s C build (`aarch64-linux-musl-gcc` not found) — it's environmental; `gh run rerun <id> --failed`.
- **Use `/council`** only for a NEW architecture fork the milestone ADR didn't settle. **6h's
  build-vs-defer is a decision to surface to the user** (not necessarily a council).
- **After each slice:** update `PROGRESS.md` + `ROADMAP.md` + `docs/HANDOFF-phase6.md` + the
  `project-secretagent` memory (commit hash + CI run id + acceptance + deferrals), exactly as 6a–6f did.

## Ultracode working style (if ultracode is on)
The pattern that worked this session: **(1)** for a wire protocol or any external contract, run a
**contract-verify Workflow BEFORE coding** (fan-out agents verify the exact spec against authoritative
docs → a grounded build-spec) — this caught the Anthropic `input_schema`/`max_tokens`/`x-api-key`
specifics; **(2)** for a security- or correctness-critical slice, run a **multi-lens adversarial-review
Workflow BEFORE pushing** (protocol / edge-cases / security / regression / completeness-critic), then
apply the surviving findings — this caught a real bug in 6e (the scheduler bypassing the new provider
seam) and a secret-leak test gap. 6g's `backup`/`restore` touches the encrypted vault + the audit
chain, so it warrants a focused self-audit or review before push; 6h (if built) is RCE-grade and MUST
get an adversarial review of the verify-before-apply path.

## Operator-gated finishes (build testable-WITHOUT; defer the live step — the established precedent)
- **6b release:** `minisign -G -W`, pin the pubkey in `install.sh` + GH `MINISIGN_SECRET_KEY`, cut a `v*` tag.
- **Carried:** live Slack E2E, the SSH exec backend check, the whisper/piper voice round-trip, and now
  a **live Anthropic task with a real `x-api-key`** (the provider is wiremock-proven; a real-key run is operator-gated).
