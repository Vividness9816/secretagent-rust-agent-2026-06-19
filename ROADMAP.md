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
- **⬜ 5b — Slack connector** (Socket Mode) — completes acceptance (a).
- **⬜ 5c — Subagent** (`Principal::Subagent` + `subagent_of` ≤-parent narrowing) — acceptance (b).
- **⬜ 5d — Voice** (feature-gated shell-out module) — acceptance (c).
- **Pick up 5b/5c/5d in a fresh session via `docs/HANDOFF-phase5.md`** (self-contained).

## ⬜ Phase 6 — Full tool surface, polish, packaging
60+ tools (browser automation, vision, image gen, TTS), `sa-tui` polish, Skills Hub sync,
`backup`/restore, self-`update`, trajectory export, signed multi-arch binaries + container +
installers.
**Acceptance:** the entire §4 Parity Inventory is green; a clean install verifies on
Linux/macOS/Windows with zero manual fixups.
