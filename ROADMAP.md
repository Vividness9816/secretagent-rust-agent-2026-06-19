# SecretAgent — Roadmap

Full functional parity with Nous Research's **Hermes Agent**, reimplemented as a single
self-contained Rust binary (clean-room, MIT), built in shippable phases. Each phase ends with a
concrete acceptance test.

- **Authoritative spec:** `~/Downloads/SecretAgent-Build-Plan.md` — §4 *Parity Inventory* is the
  acceptance contract, §10 the build order.
- **Founding decisions:** the ADRs in `~/.claude/second-brain/decisions/ADR-2026061*-secretagent-*`
  and `ADR-20260620-*` / `ADR-20260621-secretagent-phase4-daemon-messaging-cron` (the ADR wins on
  any conflict).
- **Per-slice detail:** `PROGRESS.md` (the slice ledger) + `docs/superpowers/plans/`.

Legend: ✅ complete + CI-green · 🟡 in progress · ⬜ not started

---

## ✅ Phase 0 — Foundation
4-crate just-in-time workspace, age-file vault behind a `Vault` trait, sole-writer blake3
hash-chained audit, `doctor`, CI cross-compile matrix + per-OS self-contained-binary assertions.
**Acceptance MET:** `doctor` runs headless from one self-contained binary on Linux/macOS/Windows;
the vault round-trips a secret with no plaintext on disk/in the DB/in the audit.

## ✅ Phase 1 — Talking agent with memory
`sa-memory` (SQLite + FTS5, every index rebuildable), `sa-providers` (one OpenAI-compatible
streaming adapter covering Ollama + OpenAI), `sa-core` per-turn loop, `secretagent chat`.
**Acceptance MET (live vs Ollama):** a fact stated in session 1 is recalled in session 2 after a
daemon restart.

## ✅ Phase 2 — Tools + safe execution
`sa-tools` (`fetch`/`read_file`/`write_file`/`execute_code` + registry + MCP client), `sa-exec`
(the `Sandbox` seam: `LandlockSandbox` Linux-only + `RefuseSandbox`), the `Tainted<T>` injection
guard + pure `Policy` approval/egress/path gates in `sa-core`, `secretagent run`.
**Acceptance MET:** a sandboxed multi-tool task; every call audited; a prompt-injection payload in
fetched content does not alter behavior; MCP tools load namespaced + allow-listed.

## ✅ Phase 3 — The learning loop
Versioned SQLite migrations, SQLite-canonical **skills** (born `Untrusted` + inert, approval-gated
activation, deterministic rubric scoring, drafted from assistant-reasoning only), the stated-
preference **user model** + SOUL.md/context files, memory **summarization**.
**Acceptance MET:** a novel task auto-creates a reusable skill that the same task reuses + scores
next session; a stated preference is reflected next session and cannot be derived from untrusted
content (a cross-session adversarial replay test gates the skill boundary).

## ✅ Phase 4 — Daemon + messaging + cron *(ADR-20260621)* — COMPLETE
The always-on `gateway` daemon, OS service install, a `Connector` trait + Telegram/Discord/Email,
and an NL→cron scheduler. The remote trust boundary is the security spine. **All 3 acceptances met**
(service install + reboot config, **live Telegram E2E proven 2026-06-23**, NL→cron scheduler fires
+ delivers).
- **✅ 4a — Trust spine:** the `Principal`/`RunContext` model (a connector message is a `Remote`,
  structurally unable to auto-approve / write memory), audit attribution by principal, the gateway
  loop skeleton.
- **✅ 4b — Service install** *(acceptance #1):* `service install` registers a systemd unit (Linux)
  / SCM auto-start service (Windows) that runs `gateway` on boot. *Reboot-survival proven by
  install-config assertions + a manual reboot check; CI can't install a privileged service.*
- **✅ 4c — Connectors + the M3 boundary** *(acceptance #2 buildable):* the `Connector` trait + the
  M3 default-deny sender allow-list (`dispatch_inbound`) + Telegram/Discord/Email; hardened by a
  multi-lens adversarial review. **Live Telegram E2E still pending** (needs an operator bot token —
  see `docs/HANDOFF-phase4-continued.md`).
- **✅ 4d — Scheduler** *(acceptance #3):* `sa-core::schedule` NL→cron (LLM proposes a 5-field cron
  expr + a deterministic UTC validator gates — rejects unparseable / sub-5-min DoS), a `cron_jobs`
  migration (SCHEMA_VERSION 4→5), the gateway scheduler tick firing each due job as a `Remote`
  principal with its frozen per-job allow-list (M4), write-root symlink resolution, and delivery to
  a connector. Self-audited; `secretagent schedule add|list|remove` arms jobs.
- **✅ Live Telegram E2E (acceptance #2) proven 2026-06-23** against the owner's real bot
  (audit: `connector.accepted remote:telegram:<owner>` → run → reply delivered; the M1/M2 boundary
  fired live). Two connector-robustness fixes landed during the run: clamp empty/oversized replies
  (`fd9887d`) + raise the Telegram timeout for cold-start headroom (`d96fc8a`).
- **Phase 4 COMPLETE** — all four slices shipped + CI-green, all three acceptances met. **Next = Phase 5.**

## 🟡 Phase 5 — Backend & connector parity + subagents + voice *(ADR-20260623)*
Execution backends (Docker, SSH), Slack, `sa-subagent`, `sa-voice` — scoped to the acceptance,
deferring the parity long-tail (≈19 connectors, Daytona/Singularity/Modal) behind the established
traits. **Acceptance:** a task runs in a Docker backend on a remote host driven from Slack; a
subagent runs a parallel pipeline via execute_code; voice round-trips in the CLI.
- **✅ 5a — Execution backends:** a closed `enum Backend { Local, Docker, Ssh }` — `Local` delegates
  to the existing landlock path verbatim; Docker/SSH shell out (snippet via stdin, `--network=none`,
  zero deps → musl holds by construction). **Honest per-backend `status()`** (Docker/SSH never borrow
  the local landlock verdict); **backend = operator-frozen `[exec]` config, never a model arg**;
  `doctor` reports it; the audit records the armed backend. Multi-lens adversarial review (6 lenses)
  ran before push (2 findings fixed: audit-records-backend + env-hygiene note). Live Docker proven
  (`--network=none` blocks egress). SSH live check needs a host (documented).
- **✅ 5b — Slack connector** (Socket Mode): a 4th `Connector` reusing the 4c trait + M3 boundary +
  `RunContext::remote` verbatim. `apps.connections.open` (xapp-) → wss → per-envelope-ACK loop →
  `map_message`; `chat.postMessage` (xoxb-). **Identity = the `(team_id,user_id)` tuple** (no
  cross-workspace collision); both tokens vault-held + never logged (ticket-bearing wss URL stripped
  too); `envelope_id` dedup so an at-least-once redelivery never double-runs. `tokio-websockets`
  (rustls/`ring`, already in-tree via twilight → no new license surface). 5-lens adversarial review
  (10 agents) ran before push (2 HIGH fixed, 1 false-positive dismissed). Live Slack E2E is
  operator-gated (needs a Slack app + tokens) — **completes acceptance (a)** once run.
- **✅ 5c — Subagent** *(acceptance b):* a 3rd `Principal::Subagent { parent: Box<RunContext> }` +
  `RunContext::subagent_of`. Side-effect authority **delegates** to the parent (≤ parent, capped),
  but persistence / skill-activation / `is_operator` are hard-false and the subagent's input is
  hard-`Untrusted`. Spawn is wired into `run_task` as a synthetic depth-bounded `subagent` tool
  (`MAX_SUBAGENT_DEPTH = 2`, fail-closed at 0); the sub-run carries the registry (so it can call
  `execute_code`) and returns its answer as `Tainted` data. **Remote/cron runs get depth 0** (no
  untrusted-triggered fan-out). 3-lens adversarial review → SHIP, no CRITICAL/HIGH (2 MEDIUM+LOW
  amplification findings fixed). Acceptance (b) proven hermetically.
- **✅ 5d — Voice** *(acceptance c; /council ADR-20260623-secretagent-phase5d-voice):* a feature-gated
  `secretagent voice <input.wav>` bin module that **shells out** (never `sh -c`) to operator-configured
  `[voice] stt_cmd`/`tts_cmd` argv templates. The transcript runs as **`RunContext::remote("voice", …)`**
  — Untrusted, **no-persist**, no-auto-activate, default-deny side-effects (frozen `allow_tools`),
  depth-0, **no `--yes`** (the council's decisive call: `may_persist()` keys off the Principal, so an
  operator-strict run would mint operator-attributed skills from untrusted audio). Answer → TTS via
  stdin; output wav at a fixed path; transcript capped; audit/doctor report argv[0] only; zero new
  deps. `self-audit` → SHIP. Live whisper/piper round-trip is operator-gated.

**✅ Phase 5 BUILD COMPLETE** — 5a/5b/5c/5d all shipped + CI-green. Operator-gated live tests remain
(Slack E2E, SSH backend, whisper/piper voice). Next = **Phase 6** (full §4 parity inventory, polish, packaging).

## 🟡 Phase 6 — Full tool surface, polish, packaging *(/council **ADR-20260623-secretagent-phase6-milestone**)*
A MILESTONE of ordered TDD slices — **refactor-first, packaging-early, self-update-last** — scoped
to a defensible **parity-by-mechanism** line (the agent reaches arbitrary tools via MCP + `op_tool`;
we ship the high-value bespoke set + defer the §4 long tail behind the established traits).
**Acceptance:** a clean install verifies on Linux/macOS/Windows; `secretagent doctor` passes on a
fresh box with zero fixups; the curated tool/provider/surface/ops set is green (the deferred tail is
documented honestly in `docs/parity-tail.md`).

- **✅ 6a — `assemble_agent` refactor** (`3937ef1`, CI-green): extracted the agent+registry assembly
  duplicated 4× into `setup::{build_provider,build_agent,build_registry}`, proven byte-identical
  (31-suite corpus + 2 seam tests, net −19 lines), divergences kept as explicit params.
- **✅ 6b — release packaging** (`7c32538`+`7282193`, CI-green; tagged-release operator-gated): `release.yml` (tag → matrix → sha256 checksums +
  minisign detached sig + optional Dylan-N Authenticode + distroless non-root multi-arch
  container + `compose.yaml`) + a fetch-verify-place installer (verify before place, prints PATH only);
  `doctor` binary-integrity line. macOS notarization DEFERRED (honest). *Acceptance: a signed tagged release installs + `doctor` passes on a fresh box.*
- **⬜ 6c — egress-guarded HTTP seam + network tools:** ONE `egress_get(policy,url)->Tainted` chokepoint
  (real URL parse, reject `@`-userinfo, deny IP-literal/loopback/link-local/RFC-1918 unless allow-listed,
  redirect re-check every hop, body/timeout caps) that **FIXES the live `Fetch::run` SSRF**; then
  `web_search`/`http_request`/`web_extract` through it. *Acceptance: SSRF corpus (metadata/loopback/userinfo/redirect) denied; an allow-listed search round-trips.*
- **⬜ 6d — system + external tools:** a `shell` tool via `sa_exec` (or = `execute_code`; never raw
  `Command`) + a generic **`op_tool`** (operator-frozen external command templates for vision/image-gen/
  TTS/browser-CLI — Tainted stdout, allow-listed host, model fills only a data arg). *Acceptance: shell runs sandboxed; an op_tool round-trips with Tainted output.*
- **⬜ 6e — providers:** Anthropic native 2nd `impl Provider`; OpenAI/OpenRouter via `base_url`+key;
  operator-only `secretagent model` switch; minimal multi-model per-role map (plan/execute/summarize).
  *Acceptance: a task runs against Anthropic; `model <name>` switches with no restart; a Remote run can't repoint the endpoint.*
- **⬜ 6f — TUI:** a bin module `secretagent/src/tui/` (NOT a crate) + reedline (multiline/history/slash-
  autocomplete/streaming), reusing 6a + `Agent::run_task`/`turn`. *Acceptance: the TUI drives a task end-to-end with streaming output.*
- **⬜ 6g — ops:** `backup`/`restore` (SQLite Online Backup API; vault stays encrypted; identity chmod
  600 on restore; audit chain verified) + `trajectory export` (JSON/JSONL, secret-free). *Acceptance: backup→restore round-trips a live DB; export is secret-free.*
- **⬜ 6h — self-update (LAST, or DEFERRED):** temp-download → verify detached sig vs a binary-PINNED
  pubkey → no-downgrade (version from signed payload) → atomic rename → audit; negative-control tests
  (tampered + downgrade both rejected). *If the full contract can't be proven this milestone, DEFER (manual re-install is safe).*
- **⬜ 6i — parity-tail doc + acceptance:** `docs/parity-tail.md` (shipped vs deferred-behind-which-trait
  + why) + the honest §4 acceptance amendment (Pillar C).

**Deferred-with-triggers (ADR §Revisit):** browser-automation via chromiumoxide (musl/exfil); in-process
vision/image/audio C-libs (use op_tool shell-out); daytona/singularity/modal backends (behind `Backend`);
Skills Hub sync; the 16 remaining connectors (behind `Connector`); macOS notarization; per-tool rate
limits / egress DSL.
