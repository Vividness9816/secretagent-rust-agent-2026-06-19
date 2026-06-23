# SecretAgent — Progress ledger

Slice-by-slice build status. `master` is the integration branch; every slice is committed
TDD-style and gated (fmt 0 / clippy -D warnings 0 / tests pass) on **both** Windows and WSL/Linux
before push, then CI is watched green on all 5 jobs (`check` + 4 cross-compile legs:
Linux x86_64-musl & aarch64-musl, Windows MSVC, macOS aarch64).

**Current HEAD:** `master @ b382e10` — Phases 0–3 complete; Phase 4 slices 4a/4b/4c complete.

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

### ✅ 4c — Connectors + the M3 boundary — *acceptance #2 (live E2E pending)*
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

### ⬜ 4d — Scheduler (NL→cron) — *acceptance #3*
Not started. `sa-scheduler` NL→cron (LLM proposes + a deterministic validator gates; reject
unparseable / sub-minimum-interval), a `cron_jobs` migration (SCHEMA_VERSION 4→5), the gateway
scheduler tick, a **frozen** per-job allow-list (M4), **write-root symlink resolution**, and
delivery to a connector.

---

## Outstanding for Phase 4 completion
1. **Live Telegram E2E** (acceptance #2) — everything is built + CI-green; needs an operator bot
   token + numeric sender id to run end-to-end. Steps in `docs/HANDOFF-phase4-continued.md`.
2. **Slice 4d** (acceptance #3) — the scheduler.

## Accepted residuals (documented, not bugs)
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
