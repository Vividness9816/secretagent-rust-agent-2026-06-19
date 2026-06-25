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

## ✅ Phase 5 — Backend & connector parity + subagents + voice *(ADR-20260623)* — BUILD COMPLETE
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

## ✅ Phase 6 — Full tool surface, polish, packaging *(/council **ADR-20260623-secretagent-phase6-milestone**)* — BUILD COMPLETE
A MILESTONE of ordered TDD slices — **refactor-first, packaging-early, self-update-last** — scoped
to a defensible **parity-by-mechanism** line (the agent reaches arbitrary tools via MCP + `op_tool`;
we ship the high-value bespoke set + defer the §4 long tail behind the established traits). **All 9
slices (6a–6i) shipped + CI-green.**
**Acceptance:** a clean install verifies on Linux/macOS/Windows; `secretagent doctor` passes on a
fresh box with zero fixups; the curated tool/provider/surface/ops/self-update set is green; the
deferred tail is documented honestly in **`docs/parity-tail.md`** (the Pillar-C §4 amendment). The
operator-gated live legs were **verified 2026-06-25**: Slack E2E ✅, SSH backend ✅, and voice ✅ proven;
native Anthropic ✅ plumbing-verified (reaches the API, fails closed without a key). The signed release
is the last gated step — the minisign keypair exists and the pubkey is pinned in the working tree
(uncommitted); commit the pin + add the GH secret `MINISIGN_SECRET_KEY` + cut a `v*` tag
(`docs/RELEASE.md`). See `PROGRESS.md` (Post-6) + `docs/HANDOFF-2026-06-25.md`.

- **✅ 6a — `assemble_agent` refactor** (`3937ef1`, CI-green): extracted the agent+registry assembly
  duplicated 4× into `setup::{build_provider,build_agent,build_registry}`, proven byte-identical
  (31-suite corpus + 2 seam tests, net −19 lines), divergences kept as explicit params.
- **✅ 6b — release packaging** (`7c32538`+`7282193`, CI-green; tagged-release operator-gated): `release.yml` (tag → matrix → sha256 checksums +
  minisign detached sig + optional Dylan-N Authenticode + distroless non-root multi-arch
  container + `compose.yaml`) + a fetch-verify-place installer (verify before place, prints PATH only);
  `doctor` binary-integrity line. macOS notarization DEFERRED (honest). *Acceptance: a signed tagged release installs + `doctor` passes on a fresh box.*
- **✅ 6c — egress-guarded HTTP seam + network tools** (`8f9281e..e3eb623`, CI `28075958143`): ONE `egress_get`/`egress_request -> Tainted` chokepoint
  (real URL parse, reject `@`-userinfo, deny IP-literal/loopback/link-local/RFC-1918/ULA unless the IP is allow-listed,
  reqwest pinned to the vetted IP, redirect re-check every hop, body/timeout caps) **FIXED the live `Fetch::run` SSRF** (`url_host` deleted); then
  `web_search`/`http_request`/`web_extract` through it. *Acceptance MET: SSRF corpus (metadata/loopback/userinfo/redirect/non-http) denied; allow-listed fetch+POST round-trip; output Tainted; self-audit PASS.*
- **✅ 6d — system + external tools** (`f15c583..91448a2`, CI `28076555268`): a `shell` tool (thin alias over the
  `execute_code` `sa_exec::Backend` path, fail-closed, name-gated by `approval_required`) + a generic **`op_tool`**
  (operator-frozen external command templates; argv-only never `sh -c`; model fills only a final data arg;
  errors name argv[0] only; registered last + skips builtin name collisions). *Acceptance MET: shell runs sandboxed (fail-closed on RefuseSandbox); op_tool round-trips, output tainted at the registry boundary.*
- **✅ 6e — providers** (`774e376..53eaf59`, CI `28094148385`): native Anthropic Messages API provider (a 2nd
  `impl Provider`, contract-verified — `input_schema`/`tool_use`/top-level `system`/`x-api-key`); `build_provider`
  = the single `Box<dyn Provider>` selection seam (openai|anthropic) + minimal per-role model map; operator-only
  `secretagent model <name>` switch (format-preserving `toml_edit` rewrite, structural — not a registry tool).
  5-lens adversarial review caught + fixed a scheduler bypassing the seam. *Acceptance MET: task runs against Anthropic; `model` switches; a Remote run can't repoint.*
- **✅ 6f — TUI** (`a28f4f7`, CI `28131407225`): a bin module `secretagent/src/tui.rs` (NOT a crate) + reedline
  (multiline via backslash validator / history / slash-autocomplete), reusing 6a + `Agent::run_task` as the
  interactive operator; feature-gated `tui` (default-on; headless build drops reedline). Pure helpers unit-tested.
  *Acceptance MET: drives a task end-to-end; token-streaming deferred → 6i (run_task is non-streaming, like the Anthropic chat single-chunk).*
- **✅ 6g — ops** (`39570e0..f2050f1`): `backup`/`restore` (memory.db via the SQLite Online Backup
  API — never cp a live WAL DB; the encrypted vault travels as ciphertext; restore removes stale WAL
  sidecars + chmod 600s the artifacts + verifies the audit chain) + a secret-free `export`
  (messages+audit → JSONL, recognizable secrets redacted, fail-closed re-scan) + a `doctor`
  audit-chain integrity line. A 4-lens adversarial-review workflow verified 14 findings; fixed a HIGH
  WAL-replay data-loss bug + the AKIA decoy detector gap + perms/coherence/self-target hardening.
  *Acceptance MET: backup→restore round-trips a live DB (vault encrypted, identity 0600); export is secret-free.*
- **✅ 6h — self-update** (`0be9730..22203d1`; operator chose BUILD over the ADR's DEFER default):
  `secretagent self-update [--check]` — temp-download → verify a detached **minisign** signature vs a
  pubkey **PINNED in the binary** (`const`, never fetched) → **no-downgrade** (version from the SIGNED
  manifest) → the downloaded binary's **sha256 must match** → **atomic rename** → audit. Crypto =
  `minisign-verify` (zero-dep, musl-clean; same scheme as 6b); test-signing via the `minisign` DEV-dep
  so the **negative controls** (tampered manifest / wrong key / downgrade / sha256-mismatch) are
  self-contained. Ships **inert/fail-closed** until the operator pins their key (+ `[update] base_url`).
  `release.yml` emits + signs `latest.json`. A 5-lens RCE adversarial review (18 findings; the integrity
  chain HELD) hardened it: bounded/timed download, Windows atomic-swap rollback, O_EXCL unpredictable
  temp, audit-before-swap. *Acceptance MET: tampered/downgrade refused (unit-proven); a genuine update
  verifies + swaps atomically + audits; the live network swap is operator-gated (pin key + cut a release).*
- **✅ 6i — parity-tail doc + acceptance:** `docs/parity-tail.md` — the honest map of what shipped vs
  what's deferred-behind-which-trait (+ the trigger to build each) + the §4 acceptance amendment
  (Pillar C): parity-**by-mechanism** (MCP + `op_tool` reach arbitrary tools) + a curated bespoke set,
  not a padded "60+ all green". **Closes the Phase-6 milestone.**

## ✅ Post-6 (2026-06-25) — execute-mode REPL + live operator verification
- **`secretagent tui --yes`** adds an **execute mode** to the interactive REPL (`b4c307e`, master): the
  default REPL denies side-effects; `--yes` auto-approves them for the session (a bare `secretagent`
  launcher uses it). `47c6c51` gitignores the minisign signing key out of an accidental `git add -A`.
- **Operator-gated legs verified:** Slack E2E ✅, SSH backend ✅, voice ✅ proven; native Anthropic ✅
  plumbing-verified (needs a real `ANTHROPIC_API_KEY`). The **signed release** is the last step — the
  keypair exists and the pubkey is pinned in the working tree (uncommitted): commit the pin + add the GH
  secret + bump the version + cut a `v*` tag. Full detail in `PROGRESS.md` (Post-6) + `docs/HANDOFF-2026-06-25.md`.

**Deferred-with-triggers (ADR §Revisit):** browser-automation via chromiumoxide (musl/exfil); in-process
vision/image/audio C-libs (use op_tool shell-out); daytona/singularity/modal backends (behind `Backend`);
Skills Hub sync; the 16 remaining connectors (behind `Connector`); macOS notarization; per-tool rate
limits / egress DSL.
