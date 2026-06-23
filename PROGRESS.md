# SecretAgent — Progress ledger

Slice-by-slice build status. `master` is the integration branch; every slice is committed
TDD-style and gated (fmt 0 / clippy -D warnings 0 / tests pass) on **both** Windows and WSL/Linux
before push, then CI is watched green on all 5 jobs (`check` + 4 cross-compile legs:
Linux x86_64-musl & aarch64-musl, Windows MSVC, macOS aarch64).

**Current HEAD:** `master @ 6fb49ab` — Phases 0–4 complete; **Phase 5 in progress** (5a execution backends done).

---

## Phase 5 — backends + connectors + subagents + voice *(ADR-20260623; plan: `docs/superpowers/plans/2026-06-23-secretagent-phase5a-execution-backends.md`)*

### ✅ 5a — Execution backends (Docker + SSH)
| Commit | What |
|---|---|
| `82d015e` | `sa-exec`: closed `enum Backend { Local, Docker, Ssh }` + honest `Confinement`. `Local` delegates to the existing landlock `Sandbox` verbatim; Docker/SSH **shell out** (snippet via stdin, never argv; `docker run --rm -i --network=none -v <roots>`; `ssh <host> /bin/sh -s`), **zero new deps** → musl-static holds by construction, runtime-optional (fail-closed if the CLI is absent). |
| `dc41f99` | `sa-tools`: `ExecuteCode` dispatches via `Backend` (`with_sandbox` wraps `Backend::Local` so the 2b fail-closed/override tests are unchanged); schema stays `{code}`-only (no model-chosen backend); override is local-only. |
| `cd76c24` | `[exec]` config (backend=local\|docker\|ssh, default local) → `exec::backend_from_config`, frozen into `execute_code` in run+gateway; `doctor` reports the backend's honest confinement + CLI availability. |
| `6fb49ab` | **adversarial-review fixes** — MEDIUM: `exec.backend` audit event at arm time (the gate "the audit records the backend"); LOW: documented the docker/ssh client env-hygiene invariant (no `env_clear` — the client needs HOME/SSH_AUTH_SOCK/DOCKER_HOST; never add `-e`/`SendEnv`). |

**The two non-negotiable ADR blockers, both done + tested:** (#1) honest per-backend `status()` — Docker/SSH NEVER report landlock-`Enforced`; (#2) backend = operator-frozen config, NEVER a model tool arg (schema-has-no-backend-arg test). A **6-lens adversarial-review Workflow** (9 agents) ran before push (3 candidates → 2 verified real → both fixed; 1 refuted). **Live Docker proven** (snippet ran in an alpine container; `--network=none` blocked egress). Both venues green; rustls/C-lib purity unchanged; `cargo deny` clean. SSH live check needs a host (documented residual, like reboot/Discord/Email).

### ⬜ 5b Slack (Socket Mode) · ⬜ 5c subagent (`Principal::Subagent` + `subagent_of`) · ⬜ 5d voice (feature-gated shell-out)
*Pick these up in a fresh session via **`docs/HANDOFF-phase5.md`** (self-contained: state, the ADR architecture per slice, the operator-gated live tests, the conventions/gates).*

---

## Phases 0–3 — complete + CI-green
See `ROADMAP.md` for the per-phase acceptance. Foundation → memory/providers/agentic loop →
tools + landlock sandbox + MCP client → the learning loop (skills + user model + summarization).
Per-slice plans: `docs/superpowers/plans/2026-06-{19,20}-secretagent-phase{0..3c}.md`.
Prior handoff: `docs/HANDOFF-phase3.md`.

## Phase 4 — daemon + messaging + cron *(ADR-20260621; plans: `docs/superpowers/plans/2026-06-21-secretagent-phase4{a,b,c}-*.md`)*

### ✅ 4a — Trust spine + daemon loop
| Commit | What |
|---|---|
| `aab77b0` | `Principal` (`Operator{auto_approve}` \| `Remote{connector,sender}`) + `RunContext` in `sa-core-types` |
| `22703ed` | `run_task`'s `auto_approve: bool` → `&RunContext`; remote denies side-effects + writes (M1/M2); input stamped by principal |
| `a333994` | `AuditEvent.principal: Option<String>` (back-compat hash chain) + per-action attribution |
| `515193e` | `gateway` daemon-loop skeleton + `GatewayState` + clean Ctrl-C/SIGTERM shutdown |

Self-audit PASS (no path for a `Remote` to reach operator consent / persist / activate). CI green.

### ✅ 4b — Service install (Linux systemd + Windows SCM) — *acceptance #1*
| Commit | What |
|---|---|
| `5ed97ed` | `service install\|uninstall\|status`: pure systemd unit-text generator + Windows `windows-service` SCM dispatcher (in-binary, target-gated off the musl graph) + a `doctor` service line |
| `2d42809` | self-audit fixes: handle SCM `SHUTDOWN` on reboot (not just `Stop`); quote the systemd `ExecStart` path |

Reboot-survival is proven by `AutoStart`/`enable` config assertions + a **manual** reboot check
(CI cannot create a privileged service). CI green.

### ✅ 4c — Connectors + the M3 boundary — *acceptance #2 (live E2E PROVEN — see below)*
| Commit | What |
|---|---|
| `012429f` | new `sa-connectors` crate — `Connector` trait + `InboundMsg`/`OutboundMsg` + `MockConnector` + `ConnectorConfig` |
| `18c8b00` | `dispatch_inbound` — the M3 sender allow-list + Remote-run boundary + the cross-principal gate tests |
| `8372e25` | Telegram connector (hand-rolled `getUpdates`) + the gateway run loop driving connectors |
| `7548a29` | **harden:** strip the token-bearing URL from Telegram request errors (adversarial-review HIGH+MED) |
| `404335a` | Discord connector via `twilight` (rustls-webpki-roots + pure-Rust zlib) |
| `e2c7488` | Email connector — IMAP poll + SMTP send (`async-imap` + `lettre`, rustls/`ring`, musl-clean) |
| `b382e10` | docs: unstale `construct_connector`; flag the email From-spoof residual |

A **multi-lens adversarial-review Workflow** (16 agents, 6 lenses) ran before push and caught a
real bot-token-in-logs leak (fixed in `7548a29`); the M3 / Remote-escalation / injection /
parse / DoS lenses found nothing real — the structural boundary held. All connectors are
**rustls-only** (a subagent caught + fixed an `aws-lc-sys` C-lib threat by pinning `ring`, and a
`zstd-sys` threat via `zlib`); the musl-static single-binary invariant holds. CI green.

### ✅ 4d — Scheduler (NL→cron) — *acceptance #3* *(plan: `docs/superpowers/plans/2026-06-22-secretagent-phase4d-scheduler.md`)*
| Commit | What |
|---|---|
| `a8e84ac` | `sa-core::schedule` — NL→cron LLM-propose (`nl_to_cron`) + **deterministic UTC validator** (`validate_cron` rejects bad arity / unparseable / sub-`MIN_INTERVAL_SECS` DoS via a 10-sample min-gap scan); `cron`+`chrono` encapsulated behind an i64/String API |
| `e79558a` | `cron_jobs` migration 5 (SCHEMA_VERSION 4→5) with the **frozen** `action`/`cron_expr`/`allowed_tools` (M4) + forward-schema `connectors_state`; `CronJob` + add/due/mark_fired/list/remove CRUD; v4→v5 back-compat test |
| `39f79d2` | **harden:** `policy::path_allowed` resolves write-root **symlinks** (canonicalize the longest existing ancestor) before allowing an unattended write; lexical floor + pure deny-corpus preserved |
| `761c319` | gateway `fire_job` + `tick_scheduler`: `run_until`'s `select!` loop fires each due job as a `Remote` principal (M1/M2/M4), delivers via a freshly-constructed connector's stateless `send` |
| `6b47472` | `secretagent schedule add\|list\|remove` CLI (propose → validate → persist the frozen job) |
| `283ccba` | **self-audit fixes:** HIGH — a construct-error job no longer spins every tick (Err falls through to `mark_fired`); MEDIUM — `path_allowed` multi-root fallback decided per-root (no over-deny under an absent sibling root) |

A single **self-audit** agent reviewed the trust boundary before push (verdict REVISE → the HIGH +
MEDIUM above fixed, each with a regression test). M4 (freeze-at-arm-time), M1/M2 (a cron fire runs
as `Remote` — no durable write, no skill activation), the DoS floor (~30 adversarial patterns,
10-sample window never disagreed with a 5000-sample window), and the symlink resolver all held.
CI green; both venues green; rustls-only + `cargo deny` clean.

### ✅ Live Telegram E2E (acceptance #2) — PROVEN 2026-06-23
Driven end-to-end against the owner's real bot (**@Secret_Age_nt_Bot**) from an isolated env
(`C:\Users\dnoye\sa-e2e`). The audit shows `connector.accepted` / `remote:telegram:<owner-id>` →
the run → the reply delivered over Telegram; the M1/M2 boundary fired live (a `skill.activate.denied`
when the Remote run hit a leftover draft skill). Two connector-robustness fixes surfaced + landed
during the live run:
| Commit | What |
|---|---|
| `fd9887d` | **fix:** clamp empty/oversized model replies before delivery (`clamp_reply` — Telegram/Discord reject an empty body with a 400; an empty final model message silently dropped the reply). Applied in the connector `send` (covers inbound AND cron). |
| `d96fc8a` | **fix:** raise the Telegram client timeout 35s→60s — a 25s long-poll plus a cold ~11s TLS handshake exceeded 35s, timing out the FIRST `getUpdates` and delaying the first reply. |

---

## ✅ Phase 4 COMPLETE — all acceptances met
1. **#1 Service install + reboot survival** — install-config assertions + a documented manual reboot check.
2. **#2 Live Telegram E2E** — proven against the owner's real bot (above).
3. **#3 NL→cron scheduler** — `secretagent schedule add` arms a job; the gateway fires it as a frozen-allow-list `Remote` run and delivers (slice 4d).

## Accepted residuals (documented, not bugs)
- **Cron is interpreted in UTC** (deterministic + testable); a per-job timezone column is the named
  upgrade if local-time intent ("7am MY time") matters.
- **`MIN_INTERVAL_SECS` = 300** (5 min) is the unattended-job frequency floor (bounds token spend);
  tune it (or make it per-job) if a real high-frequency job is needed.
- **`connectors_state`** is created in migration 5 as forward-schema per ADR §8 but has no 4d
  consumer (the Telegram connector keeps its `getUpdates` offset in memory); cursor persistence
  lands when connector restart-resilience is built.
- **The scheduler runs only when ≥1 connector is configured** (the only case a job has a delivery
  target); a cron job targeting an unconfigured/unknown connector is logged + skipped (its
  `next_run` still advances, so it never spins).
- **Reboot-survival** is proven by install-config + a manual reboot check (CI can't install a
  privileged service); **macOS/launchd** service backend is deferred behind the same seam.
- **Discord/Email are live-deferred** (no creds this session) — compile-verified + unit-tested
  (pure mapping) on both venues, but not driven against a live server.
- **Email M3 is best-effort:** SMTP `From` is unauthenticated/spoofable, so email's sender
  allow-list is weaker than Telegram/Discord's platform-authenticated ids — harden with DKIM/SPF
  before trusting an email sender.
- **Gateway observability is minimal:** a panicking connector task can't take down the gateway and
  a transport blip retries with a short backoff, but a live `status` surface + per-connector
  down-marking are deferred (the `GatewayState` seam exists).
- Connector dispatch is serialized through one shared sole-writer audit lock (fine for a
  single-operator daemon; shard per-connector only if throughput ever matters).
