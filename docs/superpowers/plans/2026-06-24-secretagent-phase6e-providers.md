# SecretAgent Phase 6e — Providers (native Anthropic + operator model switch)

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:test-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** A native **Anthropic Messages API** provider (a 2nd `impl Provider`, not an OpenAI-compat shim — deletes the need for a standing proxy), provider **selection** centralized in the `build_provider` seam, and an **operator-only** `secretagent model <name>` switch (a config rewrite no Remote/cron principal can reach).

**Architecture (ADR-20260623-secretagent-phase6-milestone, slice 6e):** `sa-providers/src/anthropic.rs` implements `Provider::act`/`chat` by **translating** the agentic loop's OpenAI-format `Vec<Value>` messages to/from the Messages API (top-level `system`, `tool_use`/`tool_result` content blocks, `input_schema`, required `max_tokens`, `x-api-key`). `build_provider` becomes the single **selection seam** returning `Box<dyn Provider>` (chosen by `provider.kind`), which also collapses `summarize.rs`'s duplicated provider construction. The `model` switch is a clap subcommand (operator-only **by construction** — Remote/cron principals invoke only registry tools, never CLI subcommands) that rewrites `[provider] model` in `config.toml` format-preservingly via `toml_edit`.

**Tech Stack:** Rust, `reqwest` (rustls, operator-frozen client — OUTSIDE the egress seam, not model-reachable), `toml_edit` (NEW dep — pure-Rust, format-preserving config edit).

**Wire contract** (verified 2026-06-24 against platform.claude.com via the `anthropic-contract-verify` workflow): `POST {base}/v1/messages`; headers `x-api-key: <raw>` (no Bearer), `anthropic-version: 2023-06-01`, `content-type: application/json`; required body `model`+`messages`+`max_tokens` (default 4096); top-level `system` (first `{role:system}` only); tools `{name,description,input_schema}` (omit when empty); response `content[]` ordered blocks (text + tool_use may mix — iterate by order, first `tool_use` wins); `stop_reason` non-exhaustive; errors `{type:error,error:{type,message}}` incl. 529 `overloaded_error`.

## Global Constraints
- **Native, not a shim:** translate messages; never route Anthropic through an OpenAI-compat path. The Anthropic client is operator-frozen and **NOT model-reachable** (it stays outside the 6c egress seam, like the connectors).
- **Field-name exactness** (the OpenAI-divergence bugs): tools key is **`input_schema`** not `parameters`; tool result id is **`tool_use_id`** not `tool_call_id`/`id`; `system` is **top-level** not a message; **omit `tools`** when empty (never `[]`).
- **Translation invariants:** consume only the **first** `{role:system}` (later ones skipped — injection guard); merge consecutive same-role messages so no two adjacent same-role messages and `tool_result` sits in a `user` message immediately after its `assistant` `tool_use`; arguments JSON-**string** → object with a `{}` fallback on malformed JSON; first response `tool_use` block → `ToolCall`, else concat `text` blocks → `Text`.
- **Secret policy:** `x-api-key` is a header only — **never** log request headers/body/`json()`/full error context (invariant #4). Module comment states it.
- **Constants:** `ANTHROPIC_API_VERSION="2023-06-01"`, `MAX_TOKENS_PER_CALL=4096` (one named place each — tunable).
- **Operator-only `model` switch:** structural — it's a CLI subcommand, never a registry tool; a Remote/cron run cannot invoke it. No model-switch tool is ever registered.
- **rustls-only;** `toml_edit` must be pure-Rust + musl-clean; **commit `Cargo.lock`**.
- **TDD**; commit per task; footer `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>` + `Claude-Session: phase-6e`; append ` # self-audit-ok`; push separately.
- **Gates:** fmt/clippy(all-features) 0; `cargo test --all` BOTH venues; rustls-only clean; CI green on all 5 jobs. **Focused adversarial review of the translation seam before push** (it's the correctness-critical part).

## File Structure
- `crates/sa-providers/src/anthropic.rs` — **NEW.** `Anthropic` struct + `translate`/`push_block`/`parse_action` (pure) + `Provider` impl (`act` agentic; `chat` non-streaming single chunk) + tests.
- `crates/sa-providers/src/lib.rs` — `pub mod anthropic;`
- `crates/sa-core-types/src/config.rs` — `ProviderConfig.kind` (default "openai") + `RoleModels {plan,execute,summarize}` (`models`) + `model_for(role)` + tests.
- `secretagent/src/setup.rs` — `build_provider` → `Result<Box<dyn Provider>>` selecting by `kind`, using `model_for("execute")`; `build_agent` + `summarize.rs` route through it.
- `secretagent/src/summarize.rs` — use `crate::setup::build_provider` (drops its duplicate vault read).
- `secretagent/src/model.rs` — **NEW.** `run(name)` rewrites `[provider] model` via `toml_edit`.
- `secretagent/src/main.rs` — `Cmd::Model { name }` + dispatch + `mod model;`.
- `secretagent/Cargo.toml` — `toml_edit` dep.

---

### Task 1: the native Anthropic provider (CORRECTNESS-CRITICAL)

**Files:** Create `crates/sa-providers/src/anthropic.rs`; modify `lib.rs` (`pub mod anthropic;`).

**Interfaces — Produces:** `pub struct Anthropic { base_url, model, api_key: Option<String> }` + `pub fn Anthropic::new(model, api_key) -> Anthropic` (base_url defaults to `https://api.anthropic.com`) + `impl Provider`.

- [ ] **Step 1: Failing tests** — `translate` (first-system-only; user/assistant-tool_use/tool→tool_result; alternation; merge keeps tool_result first); tools serialize under `input_schema`; empty tools omitted; malformed `arguments` → `{}`; `parse_action` (tool_use-first on mixed content, text concat, empty→`""`); wiremock `act` round-trips a `tool_use` response → `ToolCall` and a text response → `Text`, asserting the `x-api-key` + `anthropic-version` headers are sent.
- [ ] **Step 2: FAIL** (`cargo test -p sa-providers anthropic`).
- [ ] **Step 3: Implement** per the wire contract + invariants above; `chat` posts non-streaming and yields one `ChatChunk` (real SSE deferred — documented).
- [ ] **Step 4: PASS. Step 5: Commit** `feat(6e): native Anthropic Messages API provider (phase 6e)`.

### Task 2: provider selection + per-role model config

**Files:** Modify `crates/sa-core-types/src/config.rs`, `secretagent/src/setup.rs`, `secretagent/src/summarize.rs`.

**Interfaces — Produces:** `ProviderConfig.kind: String` (default `"openai"`); `ProviderConfig.models: RoleModels { plan, execute, summarize: Option<String> }`; `ProviderConfig::model_for(&self, role: &str) -> String`; `build_provider(&Config) -> Result<Box<dyn Provider>>`.

- [ ] **Step 1: Failing tests** — config parses `kind="anthropic"` + `[provider.models] execute="x"`; `model_for("execute")` returns the override and falls back to `provider.model`; `build_provider(default)` is Ok keyless (openai); `build_provider(anthropic-without-key)` is Ok (key None, no vault opened).
- [ ] **Step 2: FAIL. Step 3: Implement** — add fields + `model_for`; `build_provider` matches `kind` → `OpenAiCompat`/`Anthropic` (both keyless-constructible), model = `model_for("execute")`; `build_agent` = `Agent::new(store, build_provider(cfg)?, ...)`; `summarize.rs` uses `setup::build_provider`. Update the existing `build_provider` test (now returns `Box<dyn Provider>`). **Step 4: PASS. Step 5: Commit** `feat(6e): build_provider selection seam (openai|anthropic) + per-role model map (phase 6e)`.

### Task 3: the operator-only `model` switch

**Files:** Create `secretagent/src/model.rs`; modify `secretagent/src/main.rs`, `secretagent/Cargo.toml`.

- [ ] **Step 1: Failing test** — `model::set_model_in(doc_str, "claude-opus-4-8")` (a pure `&str -> String` helper over `toml_edit`) sets `[provider] model` (creating the table if absent) and **preserves** an existing comment/other keys.
- [ ] **Step 2: FAIL. Step 3: Implement** — `set_model_in` via `toml_edit::DocumentMut`; `run(name)` reads `config_dir()/config.toml` (or empty), applies it, writes back, prints the new model + the path. `Cmd::Model { name }` dispatches to it. **Step 4: PASS. Step 5: Commit** `feat(6e): operator-only `model` switch — format-preserving config rewrite (phase 6e)`.

---

## Acceptance (ADR slice 6e)
- A task runs against **Anthropic** (wiremock `act` round-trip proves the translation + wire format). ✓ Task 1.
- `model <name>` **switches with no restart** (rewrites config; next run/load uses it). ✓ Task 3.
- A **Remote run can't repoint the endpoint** — `model` is a CLI subcommand, never a registry tool. ✓ structural (Task 3 + no tool registered).
- **Deferred (honest, → 6i):** real SSE streaming for Anthropic `chat` (single-chunk for v1; the agentic `act` path is non-streaming); per-role map currently drives the single agent's `execute` model (full per-role provider routing is the rejected "routing engine"); error-envelope message parsing (HTTP status is the v1 signal).
