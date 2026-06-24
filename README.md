# SecretAgent

A self-hosted, autonomous AI agent daemon — a single self-contained binary.

> **Status:** Phases 0–4 complete and CI-green. Phase 4 (daemon + messaging + cron) shipped all
> four slices — **4a** (the remote trust spine), **4b** (service install), **4c** (the connector
> boundary + Telegram/Discord/Email), **4d** (the NL→cron scheduler) — and **all three acceptances
> are met**: service install + reboot config, the **live Telegram end-to-end run** (proven against
> the owner's bot on 2026-06-23), and an NL scheduled job firing + delivering. **Phase 5 (backends +
> connectors + subagents + voice, ADR-20260623) is in progress** — **5a execution backends** shipped
> (a closed `enum Backend { Local, Docker, Ssh }`, shell-out, honest per-backend confinement,
> operator-frozen backend config, live-Docker-proven; multi-lens adversarial-reviewed) and **5b Slack
> connector** shipped (Socket Mode, `(team,user)` identity, vault-held xoxb-/xapp- tokens, envelope
> dedup; multi-lens adversarial-reviewed — live Slack E2E operator-gated to complete acceptance (a)).
> See `ROADMAP.md` for the
> phase map, `PROGRESS.md` for the slice ledger, **`docs/HANDOFF-phase5.md` to pick up the work**
> (5c subagent → 5d voice), `docs/superpowers/plans/` for the per-phase build plans, and
> `~/.claude/second-brain/decisions/ADR-2026062*-secretagent-*.md` for the architecture decisions.

## Heritage & differences

SecretAgent is an **independent Rust reimplementation**, not a fork of Hermes Agent.
It reimplements observable behavior; it copies no upstream source. It intentionally
diverges in three ways:

- **Security-first defaults** — vault-only credentials (never plaintext `.env`),
  sandboxed execution, strict-by-default, tool output treated as untrusted data.
- **Zero-friction install** — a single self-contained binary; no interpreter, no venv,
  no shell-rc mutation. (On Linux that binary is fully static musl; on macOS/Windows it
  is a native single executable linking only OS libraries.)
- **Honest provenance** — this section; an open `NOTICE`; issues are never silently
  edited or deleted.

See `NOTICE` for upstream credits.

## What works today (Phases 0–4)

- **`secretagent doctor`** — headless-safe health check (config, vault decrypt self-test,
  landlock capability, configured MCP servers). Exits 0 when healthy.
- **`secretagent vault init|set|get`** — age-encrypted **file** vault behind a `Vault` trait.
  Secrets are exposed only as a `SecretRef`; plaintext never reaches logs, the DB, or the
  audit log. (Keyring/TPM backends drop in later behind the same trait.)
- **`secretagent chat "<msg>" [--session S]`** — streams a reply from the configured
  OpenAI-compatible provider (local Ollama by default), and **remembers across runs**
  (SQLite + FTS5 recall).
- **`secretagent run "<task>" [--session S] [--yes] [--allow-unsandboxed-exec]`** — the
  agentic loop: the model may call **policy-gated, audited** tools to complete a task.
  - First-party tools: `fetch` (egress allow-list), `read_file` / `write_file` (path roots),
    `execute_code` (landlock-sandboxed, Linux).
  - **Injection guard:** tool output is structurally **tainted** (`Tainted<T>`) and re-fed to
    the model as `tool`-role DATA — never as an instruction. A prompt-injection payload in
    fetched content cannot change behavior. (Compile-fail test enforces the boundary.)
  - **Approval:** side-effectful tools (`write_file`, `execute_code`, and namespaced MCP
    equivalents) require `--yes`; otherwise denied (strict-by-default, headless).
  - **`execute_code` is fail-closed:** it runs only when **landlock is runtime-enforced**
    (Linux); on macOS/Windows it is refused, never run unconfined. The confined shell runs
    with a cleared environment (no `$SECRET` / `/proc/self/environ` exfiltration) and access
    limited to the policy's file roots. `--allow-unsandboxed-exec` is a per-invocation,
    never-persisted, **loudly-audited** override (the operator's own-box escape valve).
  - **MCP client:** connects to configured MCP servers over stdio (JSON-RPC 2.0) and loads
    their tools **namespaced** (`server::tool`, can't shadow a first-party tool) and
    **allow-listed** (default-deny). Remote output is untrusted/tainted like any tool; the
    namespace-aware approval gate still applies (`server::write_file` needs `--yes`).
- **Tamper-evident audit:** every credential + tool call is appended to a blake3
  hash-chained JSONL log (`sa-audit` is the sole writer, compiler-enforced); the record is
  `fsync`'d before an untrusted/irreversible dispatch and tolerates a torn final line on
  restart.
- **`secretagent pref set <dimension> <value>` / `pref list`** — a stated-preference user
  model (SQLite `user_model`, `Provenance::Trusted`, written **only** by the operator) surfaced
  into the model's system prompt. A global **SOUL.md** (+ optional `context.md`) in the config
  dir feeds personality/context. Preferences are **never** derived from tool/model output (a
  test locks this), so a prompt-injection payload cannot plant a preference. SQLite migrations
  are now version-gated (the store upgrades from a Phase-1/2 DB without data loss).
- **Learning loop (`secretagent skill list` / `skill activate <name>`)** — completing a novel
  agentic task auto-creates a reusable **skill** (SQLite-canonical; born `Untrusted` + inert).
  The same task next session recalls it (FTS5), and under `--yes` auto-activates (operator
  approval), reuses, and **scores** it (deterministic rubric — never an LLM). A skill body is
  drafted from the agent's OWN reasoning only — never from tool output — so an injected payload
  cannot launder across sessions into a trusted instruction (a cross-session adversarial replay
  test gates this). Activation is approval-gated like `write_file`; every lifecycle event is
  audited by name. A draft skill is **never** composed into the system prompt until activated.
- **`secretagent summarize [--session S]`** — compresses a long session's older context into a
  rolling **LLM summary** (behind the `Provider` seam), kept in SQLite and surfaced into
  assembled context so the agent retains the gist past the recent/recall window. Derived only
  from user+assistant messages (no tool output), framed as context-not-instruction.

### Phase 4 — daemon, messaging, cron (COMPLETE — 4a / 4b / 4c / 4d, all 3 acceptances met)

- **`secretagent gateway`** — the always-on daemon. It loads the configured messaging connectors,
  drives the agent from them, **ticks the NL→cron scheduler** (firing due jobs), and runs as a
  `tokio` loop until Ctrl-C / SIGTERM. With no connectors configured it is a do-nothing daemon that
  simply idles until shutdown (the scheduler runs only when ≥1 connector is configured — the only
  case a job has somewhere to deliver).
- **`secretagent schedule add | list | remove`** — natural-language scheduled jobs (4d).
  `schedule add "every morning at 7, summarize my starred issues" --connector telegram --chat <id>
  [--tool write_file ...]` asks the model for a 5-field cron expression, **gates it through a
  deterministic Rust validator** (rejects unparseable / 6-field / `@macro` / sub-5-minute-interval
  DoS), and persists the **frozen** job. The gateway fires each due job as a **`Remote` principal**
  carrying the job's frozen `allow_tools` (M4 — task / cron / grant are never re-derived at fire
  time; the run writes no durable memory) and delivers the result to the target connector. Cron is
  interpreted in **UTC**.
- **`secretagent service install | uninstall | status`** — installs the binary as an OS service
  that runs `gateway` on boot and survives reboot. **Linux** writes a `systemd` unit
  (`StateDirectory=secretagent` wires the data dir, `Restart=on-failure`, `WantedBy=multi-user.target`);
  **Windows** registers an auto-start service via the **SCM** (an in-binary dispatcher — *not* a
  `sc.exe` shell-out — so it runs *as* a real service). It writes only the unit/registration — no
  shell-rc mutation. (macOS / launchd is deferred behind the same seam.) `doctor` reports service health.
- **Messaging connectors — Telegram, Discord, Email, Slack** (`sa-connectors`): a `Connector` trait +
  four impls. An inbound message becomes an agent run and the reply is delivered back on the same
  transport. Telegram is hand-rolled `getUpdates` long-poll (raw `reqwest`); Discord is `twilight`'s
  poll-shard; Email is IMAP-poll + SMTP (`async-imap` + `lettre`); Slack is **Socket Mode** — an
  outbound WSS (`tokio-websockets`, no public inbound endpoint) opened via `apps.connections.open`,
  each envelope ACK'd and deduped by id, replies via `chat.postMessage`. **Every connector is
  rustls-only** so the single static binary holds. Connector secrets (bot/app tokens, IMAP/SMTP
  passwords) live in the **vault** (a `token_ref` / `app_token_ref` key-id), never in config or logs.
  Slack identity is the `(team_id, user_id)` tuple, so a same-username in another workspace can't
  impersonate the owner.
- **The remote trust boundary — the security spine** (ADR-20260621). A connector message is
  **untrusted input from the public internet**:
  - **Principal model:** a run is driven by an `Operator` (the local CLI — may `--yes`, may write
    durable memory) or a `Remote{connector,sender}` (untrusted). `Remote` is *structurally* unable
    to auto-approve a side-effect tool, auto-activate a skill, or write durable memory — a
    compile-time property, the same shape as `Tainted<T>`.
  - **M3 — default-deny sender allow-list:** a sender NOT on the binding's `allow_senders` is
    **rejected and audited before the agent ever runs**. Only operator-listed senders drive the bot.
  - **M1 / M2:** a `Remote` run reaches a side-effect tool ONLY via the binding's **frozen**
    `allow_tools` grant (never ad-hoc), and writes **no** skill / preference / user-model.
  - Connector input is stamped `Untrusted{source}` and flows through the existing injection guard as
    tool-role data; every inbound decision (accepted *or* rejected) is audited by principal.
  - The boundary was hardened by a **multi-lens adversarial review** before shipping (it caught and
    fixed a bot-token-in-logs leak; the M3 / Remote / parse structure held).

## Configuration

Config lives at the platform config dir's `config.toml` (override with
`SECRETAGENT_CONFIG_DIR`); data (vault, DB, audit log) at the data dir (override with
`SECRETAGENT_DATA_DIR`). Everything has a default, so an absent file is valid.

```toml
[provider]
base_url = "http://localhost:11434/v1"   # OpenAI-compatible; default = local Ollama
model = "hermes3:latest"
# api_key_ref = "OPENAI_KEY"             # vault key-id; omit for keyless backends (Ollama)

[policy]
egress_allow = ["api.github.com"]        # exact-host allow-list for `fetch` (default-deny)
read_roots = ["/work"]                   # `read_file` roots
write_roots = ["/work/out"]              # `write_file` roots + landlock-writable for execute_code

[[mcp]]                                  # zero or more MCP servers (default: none)
name = "rose"
command = "rose-glass-mcp"
args = ["--db", "/path/index.db"]
allow_tools = ["search"]                 # default-deny: only these load, namespaced as rose::search

[[connectors]]                           # zero or more messaging connectors (default: none)
name = "telegram"
kind = "telegram"                        # telegram | discord | email | slack
token_ref = "TELEGRAM_BOT_TOKEN"         # vault key-id for the bot token (never plaintext)
allow_senders = ["123456789"]            # M3 default-deny: only these sender ids may drive the bot
allow_tools = []                         # frozen side-effect grant for this binding (empty = read-only)
# Email bindings also set: imap_host/imap_port/smtp_host/smtp_port/username/from (token_ref = password)

# [[connectors]]                         # Slack (Socket Mode) — needs TWO vault tokens:
# name = "slack"
# kind = "slack"
# token_ref = "SLACK_BOT_TOKEN"          # xoxb- bot token (chat.postMessage)
# app_token_ref = "SLACK_APP_TOKEN"      # xapp- app-level token (Socket Mode connection)
# allow_senders = ["T01ABCD:U05WXYZ"]    # M3 identity is the "<team_id>:<user_id>" tuple
# allow_tools = ["execute_code"]
```

## Architecture

A small **just-in-time** crate workspace (crates are added at the phase that needs them, never
pre-stubbed):

| Crate | Responsibility |
|-------|----------------|
| `secretagent` (bin) | clap CLI (`doctor`/`vault`/`chat`/`run`/`pref`/`skill`/`summarize`/`schedule`/`gateway`/`service`) + the gateway run loop + the **M3 dispatch boundary** (`dispatch_inbound`) + the scheduler tick (`fire_job`) |
| `sa-core-types` | canonical `Message`/`ToolCall` + non-optional `Provenance`, `Tainted<T>` injection guard, pure `Policy` + decision fns, the `Principal`/`RunContext` trust types, config |
| `sa-vault` | age-encrypted file vault behind a `Vault` trait; `SecretRef` |
| `sa-audit` | sole-writer blake3 hash-chained append-only JSONL |
| `sa-memory` | SQLite (bundled, static) + FTS5 recall; every index rebuildable |
| `sa-providers` | `Provider` trait + one OpenAI-compatible streaming + tool-calling adapter |
| `sa-tools` | `Tool` trait + registry + `fetch`/`read_file`/`write_file`/`execute_code` + the MCP client |
| `sa-exec` | the `Sandbox` seam: `LandlockSandbox` (Linux, `cfg`-gated) + `RefuseSandbox` |
| `sa-core` | the per-turn chat loop + the agentic `run_task` tool loop (gate → run → taint → audit → re-feed), principal-gated for the remote boundary; `schedule` (NL→cron LLM-propose + deterministic UTC validator) |
| `sa-connectors` | the `Connector` trait + Telegram/Discord/Email impls (feature-gated, **rustls-only**); `InboundMsg`/`OutboundMsg` + a `MockConnector` test seam |

**Invariants** (CI-enforced): one self-contained binary per OS; SQLite is the single
canonical store (derived indexes rebuildable); tool output is tainted, never an instruction;
no secret in the DB/audit/logs.

## Building & testing

```bash
cargo build --release            # native single binary
cargo test --all                 # unit + integration; landlock corpus runs on Linux
cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings
```

CI (`.github/workflows/ci.yml`) runs fmt/clippy/test/cargo-deny/cargo-audit on `ubuntu-latest`
plus a cross-compile matrix (Linux x86_64 & aarch64 musl, macOS, Windows) with per-OS
self-contained-binary assertions and a headless `doctor` check.

> **Landlock note:** `execute_code` confinement and its kernel deny-corpus only run on Linux
> (the `landlock` dependency is target-gated out of the macOS/Windows build graph). They are
> verified on a real landlock kernel via CI's `ubuntu-latest` leg.

## License

MIT.
