# SecretAgent

A self-hosted, autonomous AI agent daemon — a single self-contained binary.

> **Status:** Phases 0–2 complete and CI-green (foundation → memory/providers/agentic loop →
> tools + landlock sandbox + MCP client). Phase 3 is next (unscoped). See
> `docs/superpowers/plans/` for the per-phase build plans, `docs/HANDOFF-phase3.md` to pick up
> the work, and `~/.claude/second-brain/decisions/ADR-2026061*-secretagent-*.md` for the
> architecture decisions.

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

## What works today (Phases 0–2)

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
```

## Architecture

A small **just-in-time** crate workspace (crates are added at the phase that needs them, never
pre-stubbed):

| Crate | Responsibility |
|-------|----------------|
| `secretagent` (bin) | clap CLI: `doctor` / `vault` / `chat` / `run` |
| `sa-core-types` | canonical `Message`/`ToolCall` + non-optional `Provenance`, `Tainted<T>` injection guard, pure `Policy` + decision fns, config |
| `sa-vault` | age-encrypted file vault behind a `Vault` trait; `SecretRef` |
| `sa-audit` | sole-writer blake3 hash-chained append-only JSONL |
| `sa-memory` | SQLite (bundled, static) + FTS5 recall; every index rebuildable |
| `sa-providers` | `Provider` trait + one OpenAI-compatible streaming + tool-calling adapter |
| `sa-tools` | `Tool` trait + registry + `fetch`/`read_file`/`write_file`/`execute_code` + the MCP client |
| `sa-exec` | the `Sandbox` seam: `LandlockSandbox` (Linux, `cfg`-gated) + `RefuseSandbox` |
| `sa-core` | the per-turn chat loop + the agentic `run_task` tool loop (gate → run → taint → audit → re-feed) |

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
