# SecretAgent — handoff to a fresh session (Phase 5 continued: 5b Slack → 5c subagent → 5d voice)

Paste this whole file into a new session to continue the build. It is self-contained.

---

You're continuing a multi-phase build of **SecretAgent**, a clean-room Rust agent daemon
(full-Hermes-Agent-parity destination, **MIT**, **no Hermes source copied**). **Phases 0–4 are
complete and CI-green. Phase 5 slices 5a (execution backends), 5b (Slack connector), and 5c
(subagent) are complete and CI-green.** **One slice remains: ⬜ 5d voice** — architecture-decided in
**ADR-20260623** (no new `/council` needed; go `writing-plans` → inline TDD). The 5b/5c sections
below are kept as DONE reference; **start at 5d.**

> **5c DONE (commits `b6d8599`/`b452af2`/`4c3241d`)** — `Principal::Subagent { parent: Box<RunContext> }`
> + `RunContext::subagent_of`: side-effect authority **delegates** to the parent (≤ parent, capped),
> persistence/skill-activation/`is_operator` hard-false, input hard-`Untrusted`. Spawn wired into
> `run_task` as a synthetic depth-bounded `subagent` tool (`MAX_SUBAGENT_DEPTH = 2`); the sub-run
> carries the registry (calls `execute_code`) and returns `Tainted` data. **Remote/cron runs get
> depth 0** (no untrusted fan-out — adversarial-review fix). 3-lens review → SHIP. See
> `docs/superpowers/plans/2026-06-23-secretagent-phase5c-subagent.md` + `PROGRESS.md`.

## Where it lives
- Repo: `C:\Users\dnoye\ClaudeSecondBrain\SecretAgent` (nested git repo, branch `master`).
- Private remote: `Vividness9816/secretagent-rust-agent-2026-06-19`. `master` is at **`4c3241d`**
  (slices 5a/5b/5c complete); working tree clean, everything pushed.
- **Confirm state first:** `git log --oneline -8` and `git status`.

## Read first (authoritative; ADR wins on conflict)
- The `project-secretagent` memory (auto-loads) — the running ledger of every phase.
- **`~/.claude/second-brain/decisions/ADR-20260623-secretagent-phase5-backends-connectors-subagents-voice.md`**
  — the **binding Phase-5 architecture** for all four slices. Plus the four prior ADRs
  (`ADR-20260619-…-founding`, `ADR-20260620-…-phase2-sandbox`, `ADR-20260620-…-phase3-learning-loop`,
  `ADR-20260621-…-phase4-daemon-messaging-cron`). Standing rules: the **JIT-crate rule**; the
  **4 invariants** (single self-contained binary per OS — heavy/optional deps are runtime-OPTIONAL
  feature-gated, NEVER install-time-required; SQLite single canonical store + every index
  rebuildable; tool/connector/voice output is `Tainted`, never an instruction; no secret in
  DB/audit/logs); the **Principal/RunContext trust model** (`Operator` vs `Remote`; M1 structural /
  M2 no-durable-write / M3 default-deny `(connector,sender)` allow-list before `run_task` / M4
  freeze-at-arm-time); **rustls-only** (no openssl/native-tls/aws-lc-sys/zstd-sys — the musl-static
  invariant).
- `README.md`, `ROADMAP.md`, `PROGRESS.md` (the slice ledger), and
  `docs/superpowers/plans/2026-06-23-secretagent-phase5a-execution-backends.md` (the executed 5a
  plan — the house style for the per-slice plans you'll write for 5b/5c/5d).
- The spec `~/Downloads/SecretAgent-Build-Plan.md` — §4 Parity Inventory, §5 backends/connectors,
  §10 Phase 5, §7 (execute_code = the "zero-context-cost pipeline").

## Done so far (do NOT rebuild)
- **Phases 0–4** complete + CI-green: foundation/vault/audit → memory+providers+agentic loop →
  tools + landlock sandbox + MCP → the learning loop → the daemon (service install, Telegram/
  Discord/Email connectors + the M3 boundary, NL→cron scheduler). Live Telegram E2E proven.
- **Phase 5 council + ADR-20260623** (9-agent, 2-round). Scoped to the acceptance, deferring the
  parity long-tail (≈19 connectors, Daytona/Singularity/Modal) behind the established traits.
- **Slice 5a — execution backends (commits `82d015e`/`dc41f99`/`cd76c24`/`6fb49ab`/`f625762`):**
  - `sa-exec`: a CLOSED operator-frozen **`enum Backend { Local, Docker, Ssh }`** + honest
    **`Confinement`** (`LocalKernel(LandlockStatus)` | `Container{image}` | `RemoteHost{host}`).
    `Backend::local()` delegates to the existing landlock `Sandbox` verbatim; `Docker`/`Ssh`
    **shell out** — `run_docker` (`docker run --rm -i --network=none -v <roots>[:ro] <image> /bin/sh
    -s`) / `run_ssh` (`ssh <host> /bin/sh -s`) via `pipe_code` (the model snippet on **STDIN**, never
    argv). **Zero new deps** → musl-static holds by construction; runtime-optional (fail-closed if
    the CLI is absent).
  - **BLOCKER #1 (honest status):** Docker/SSH NEVER report `LandlockStatus::Enforced` — tested.
  - **BLOCKER #2 (frozen-config backend):** `[exec]` config (`ExecConfig{backend,image,host}`,
    default `local`) → `secretagent/src/exec.rs::backend_from_config`, frozen into `execute_code`
    in `run`/`gateway`; `execute_code`'s schema stays `{code}`-only (no model-chosen backend, tested).
  - `ExecuteCode` (sa-tools) holds a `sa_exec::Backend`; `with_sandbox` wraps `Backend::Local` so the
    2b fail-closed/override tests are unchanged; the `--allow-unsandboxed-exec` override is
    **local-only**. `doctor` reports the backend's honest confinement + CLI availability;
    `exec::audit_backend_armed` writes an `exec.backend` audit event at arm time.
  - **6-lens adversarial-review Workflow** (9 agents) ran before push → 2 findings fixed
    (audit-records-backend; the docker/ssh env-hygiene invariant — do NOT `env_clear` the client, it
    needs HOME/SSH_AUTH_SOCK/DOCKER_HOST; the untrusted snippet is already protected since docker/ssh
    forward no env by default; NEVER add `-e`/`--env-file`/`SendEnv`).
  - **Live Docker proven** (`docker run --rm -i --network=none alpine /bin/sh -s` with the snippet on
    stdin ran "hello-from-container" + `--network=none` blocked egress). Both venues green; rustls/
    `cargo deny` clean. SSH live check needs a host (documented residual).

---

## TASK — slices 5b → 5c → 5d (build in this danger/dependency order)

For each slice: `writing-plans` → inline TDD → both-venue gate → push → watch CI. **5b gets the
multi-lens adversarial-review Workflow before push** (the new untrusted-input boundary); 5c/5d do
NOT (a `compile_fail`/unit test for 5c's downgrade + the CI C-lib gate for 5d suffice — the Skeptic's
concentrated-signal call).

### 5b — Slack connector (Socket Mode) — completes acceptance (a)
A 4th `Connector` impl in `sa-connectors`, **reusing the 4c trait + `InboundMsg`/`OutboundMsg` +
`MockConnector` + the M3 `dispatch_inbound` boundary + `RunContext::remote` VERBATIM** (the gateway
gets a `"slack"` arm in `construct_connector`). Feature-gated `slack` like `discord`/`email`.
- **Transport = Socket Mode** (ADR-decided; no public inbound endpoint, fits the NAT'd daemon).
  Receive: `apps.connections.open` (reqwest POST, the `xapp-` app-level token) → a WSS URL → connect
  the WebSocket → receive envelopes (`hello`, `events_api` carrying `message` events, `disconnect`)
  → **ACK each envelope** (send `{"envelope_id": …}` back over the WS) → map to `InboundMsg`. Send:
  `chat.postMessage` (reqwest POST, the `xoxb-` bot token).
  - **WS client:** a feature-gated **rustls** WS lib — prefer **`tokio-websockets`** (already in-tree
    via `twilight-gateway`, so no NEW transitive surface; pin rustls + `ring`, no `zstd`/`aws-lc-sys`)
    — mirroring the Discord/twilight decision. Hand-roll the reqwest calls (`apps.connections.open`,
    `chat.postMessage`) like Telegram.
  - **ALTERNATIVE the operator may prefer (ask if unsure):** stateless `conversations.history`
    polling (zero new deps, hand-rolled reqwest exactly like Telegram, no public endpoint, but less
    real-time + needs a configured channel id). Socket Mode is the ADR's literal choice; polling is
    the lazier zero-dep cut. Default to Socket Mode unless the operator says otherwise.
- **Identity = the `(team_id, user_id)` tuple, NOT a bare string** (Skeptic: a user in another
  workspace must not collide with the registered owner). Encode the `sender` as `"<team_id>:<user_id>"`
  (or extend `InboundMsg`); the M3 `allow_senders` keys on it. **Skip bot/own messages** (prevent
  reply loops, like Discord's `map_message`). Signing-secret HMAC is **N/A under Socket Mode**
  (revisit only if ever moved to the Events API / a public endpoint).
- **Secrets:** the `xapp-` + `xoxb-` tokens come from the **vault** (`token_ref` + a new
  `app_token_ref` on `ConnectorConfig`), NEVER logged — strip any token-bearing URL/error like the 4c
  Telegram `reqwest::Error::without_url` fix. Apply the existing `clamp_reply` to the send path.
- **Tests (no live workspace):** pure `parse_envelope`/`map_message` unit tests (the Telegram
  `parse_updates` precedent) + `MockConnector` dispatch tests. **Live Slack E2E is operator-gated**
  (needs a Slack app + `xapp-`/`xoxb-` tokens + the operator's Slack user/team id — like the Telegram
  E2E; build it testable-without and defer the live run, the 4c/Discord/Email precedent).
- **Adversarial-review Workflow before push** (the 4c ship-gate retargeted): a non-allow-listed
  `(team,user)` injection payload never reaches `run_task`, writes nothing durable, the tokens never
  hit the log, the envelope-ACK loop can't be DoS'd, and a subsequent Operator run never
  auto-activates anything the remote attempted to seed.

### 5c — Subagent — acceptance (b)
A `sa-core` **MODULE** (not a crate). The trust derivation reuses `RunContext`/`Principal`:
- **`Principal::Subagent { parent: Box<Principal> }`** (a 3rd variant — `principal.rs`'s doc-comment
  already anticipates "a future 3rd principal is non-persisting until explicitly opted in"). Audited
  as `subagent:<parent_label>` via the existing free-text `audit_label`.
- **`RunContext::subagent_of(&parent) -> RunContext`** — a TYPED ≤-parent narrowing (mirrors how
  `Remote` was made safe): `allow_list ⊆ parent.allow_list`, `may_persist()` forced false,
  `may_auto_activate_skill()` false, provenance inherited-or-worse. Narrow the 4 existing
  `RunContext` methods (one `Subagent` arm each). **A subagent can never exceed parent authority** —
  prove it with a unit test next to the existing `remote_*` tests (zero I/O), the `Tainted`/`Principal`
  precedent.
- **Depth/fan-out bound:** a `MAX_SUBAGENT_DEPTH` const (mirroring `MAX_TOOL_STEPS=8`) + a depth
  counter carried in `RunContext`, decremented on spawn, **fail-closed at zero** (no fork-bomb).
- **Spawn mechanism:** wire it INTO `run_task` (a `subagent` tool-spec the model can call, dispatched
  specially in the `run_task` loop — NOT a standalone `Registry` `Tool`, because a `Tool::run` lacks
  the `Agent`/registry/audit/depth handles needed to re-enter the loop). On a `subagent` call,
  re-enter `self.run_task(sub_session, sub_task, registry, policy, audit, &ctx.subagent_of())` with
  depth-1; the sub-run carries the tool registry (so it can call `execute_code` → the acceptance's
  "parallel pipeline via execute_code"); its answer returns to the parent as **`Tainted` tool data**
  (the existing `Tainted::untrusted` re-feed at the `run_task` tool-result site), never as an
  instruction.
- *Gate (acceptance b):* a subagent runs a parallel pipeline via execute_code; the ≤-parent / no-
  persist / no-auto-activate / depth-bound properties hold (unit + `compile_fail` tests).

### 5d — Voice — acceptance (c)
A feature-gated `voice` **MODULE** (in the bin), **shell-out** to a `whisper`/`piper` binary (or a
cloud STT/TTS endpoint over the existing `reqwest`) — **link NO audio C-lib** (`whisper-rs`/ONNX/
`cpal` FFI is the not-in-musl path, DEFERRED). Because it shells out, it ships in ALL builds
(musl included — it's just `Command::new`); its runtime dependency (the binary, or a cloud key) is
`doctor`-probed.
- CLI round-trip: `secretagent voice <input.wav>` → spawn the STT binary (file/stdin) → transcript →
  `run_task` → spawn the TTS binary → `output.wav` (or play via a system player). Operate on audio
  files / shell a recorder-player — no linked audio device lib.
- **STT output provenance:** the ADR says stamp it **`Untrusted`** (a voice command is untrusted
  input; the transcript is data, never an instruction). The minimal acceptance-passing reading is to
  run the transcript as an attended **Operator-strict** turn (no `--yes` → no side-effects) so a
  spoken "run execute_code" can't auto-approve — `self-audit` this provenance call (the ADR's
  Untrusted stamp vs Operator-strict; both deny unattended side-effects). Voice-over-a-connector
  (Discord VC) would be unambiguously `Remote` — out of scope here (CLI only).
- *Gate (acceptance c):* voice round-trips in the CLI; the STT-output-is-`Tainted`/no-auto-side-
  effect property holds (unit test); the CI C-lib gate stays green (no audio C-lib in the musl tree).

---

## Operator-gated live tests (build testable-WITHOUT; defer the live run, the 4c precedent)
- **5b Slack:** a Slack app with `xapp-` (Socket Mode app-level) + `xoxb-` (bot) tokens, the bot
  added to a channel, and the operator's Slack `(team_id, user_id)`. Store the tokens via the `!`-
  prefixed `vault set` (never paste a token into the chat). Like the Telegram E2E.
- **5a SSH live check (carried over):** an SSH host to target — `[exec] backend="ssh" host="user@h"`
  then a `run` that calls `execute_code`. (The Docker half is already proven.)
- **5d voice:** a `whisper`/`piper` binary on PATH (or a cloud STT/TTS key).

## Conventions / gates (non-negotiable — held through Phases 0–5a)
- **TDD**; commit per task; conventional-commit subject; footer = a blank line then
  `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>` then `Claude-Session: phase-5b` (bump per
  slice).
- The **`self-audit` PreToolUse hook blocks `git commit`** — append ` # self-audit-ok` to the bash
  command.
- Before EVERY commit: `cargo fmt --all` (then `--check` = 0) / `cargo clippy --all-targets
  --all-features -- -D warnings` (0) / relevant `cargo test`.
- **Both-venue gate before push** (Windows `cargo test --all` + WSL `wsl.exe bash -c 'export
  PATH="$HOME/.cargo/bin:$PATH" CARGO_TARGET_DIR="$HOME/sa-target"; cd
  /mnt/c/Users/dnoye/ClaudeSecondBrain/SecretAgent; cargo test --all'`). WSL is the landlock/Linux
  venue (the landlock code can't compile on Windows). Watch `cargo`'s exit code (the nested-`$()`
  grep counts mangle through `wsl.exe bash -c`; `CARGO_EXIT=0` is the definitive signal).
- **Keep rustls-only** — no openssl/native-tls/aws-lc-sys/zstd-sys (`wsl … cargo tree -e features -p
  secretagent | grep -iE "openssl|native-tls|aws-lc-sys|zstd-sys"` empty); `cargo deny check` green.
  **Commit `Cargo.lock`** with any dep/feature change. A new WS dep for 5b must be rustls+`ring`,
  no `zstd`.
- Then watch CI green on all 5 jobs: get the run whose `headSha` matches HEAD (`gh run list --branch
  master --limit 6 --json databaseId,headSha`), then `gh run watch "$RUN" --exit-status --interval
  25`. `gh` is at `/c/Program Files/GitHub CLI/gh.exe`.

## Build / CI gotchas (already solved — keep them)
- Local Ollama has `hermes3:latest` (tools-capable) for live tests; set `model` in `config.toml`.
- A bin crate runs its modules' `#[cfg(test)]` unit tests under `cargo test -p secretagent`; the
  connector/boundary tests run with no live network via `MockConnector`.
- The Docker backend's `/bin/sh` is passed as a **literal argv element** by `std::process::Command`
  (no MSYS mangling) — a manual Git-Bash `docker run … /bin/sh` test mangles it to `C:/…/sh`; use
  `MSYS_NO_PATHCONV=1` or test via the Rust binary.
- Feature-gate heavy/optional connector + voice deps; the bin enables them. The `LF will be replaced
  by CRLF` git warnings are benign.
- For dep bumps / a new WS crate: `cargo deny` may flag a license — add it to the allow-list
  deliberately (the CDLA/0BSD precedents).
