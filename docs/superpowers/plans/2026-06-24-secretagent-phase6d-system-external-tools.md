# SecretAgent Phase 6d — System + External Tools

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:test-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** A `shell` tool that runs through the operator-frozen sandboxed `sa_exec::Backend` (never a raw `Command`), and a generic **`op_tool`** = an operator-FROZEN external-command adapter (the 5d-voice argv pattern generalized) where the model fills only one data arg.

**Architecture (ADR-20260623-secretagent-phase6-milestone, slice 6d):** `shell` is a thin tool delegating to the existing `ExecuteCode` path (same `sa_exec::Backend`, **strictly fail-closed — no unsandboxed override**; the approval gate already lists `"shell"`). `op_tool` spawns an operator-frozen argv (`Command::new(cmd[0]).args(cmd[1..] + [input])`, **NEVER `sh -c`**), the model supplies only the final `input` data arg (never the program/flags/URL/host — those are frozen in config), stdout is returned and re-tainted at the registry boundary, and errors carry **argv[0] only** (no secret leak — the 5d ruling). `op_tool` is a NARROW adapter, never a generic curl/bash escape hatch — that is the operator's responsibility, enforced by "model fills one arg only."

**Tech Stack:** Rust, the existing `sa_exec::Backend` + `ExecuteCode`, `std::process::Command` (argv, no shell). **Zero new crates.**

## Global Constraints
- **No raw `std::process::Command` for `shell`** — it routes through `sa_exec::Backend` (via `ExecuteCode`), preserving the landlock fail-closed contract + the `approval_required("shell")` gate. `shell` is fail-closed with **no** `allow_unsandboxed` override (stricter than `execute_code` by design).
- **`op_tool` is operator-frozen:** the program + all flags + any URL/host live in config `cmd`; the model fills ONLY the final `input` arg, appended as the last argv element. Spawn via argv, **never `sh -c`**. Errors/audit name **argv[0] only**.
- **Output is `Tainted`** — both tools return `String`; `sa-core` re-taints at the registry call site (no Tool-trait change).
- **No builtin shadowing:** an `op_tool` whose name collides with an already-registered tool is **skipped** (builtins win; register op_tools last).
- **TDD**; commit per task; footer `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>` + `Claude-Session: phase-6d`; append ` # self-audit-ok` to each `git commit`, push separately.
- **Gates before push:** fmt `--check` 0 / clippy all-features `-D warnings` 0 (test mod last) / `cargo test --all` BOTH venues (Win + WSL) / rustls-only clean / commit `Cargo.lock` if changed. Then watch CI green on all 5 jobs.

## File Structure
- `crates/sa-tools/src/tools/shell.rs` — **NEW.** `Shell` delegates to an inner `ExecuteCode` (fail-closed).
- `crates/sa-tools/src/tools/op_tool.rs` — **NEW.** `OpTool { name, cmd }` + argv spawn.
- `crates/sa-tools/src/tools/mod.rs` — `pub mod shell; pub mod op_tool;`
- `crates/sa-core-types/src/config.rs` — `OpToolConfig { name, cmd, description }` + `op_tools: Vec<OpToolConfig>` on `ToolsConfig` + parse test.
- `secretagent/src/setup.rs` — register `Shell` (with the operator backend) + one `OpTool` per `cfg.tools.op_tools` (skip name collisions).

---

### Task 1: the `shell` tool

**Files:** Create `crates/sa-tools/src/tools/shell.rs`; modify `tools/mod.rs`.

**Interfaces — Produces:** `pub struct Shell` + `pub fn Shell::with_backend(backend: sa_exec::Backend) -> Shell`.

- [ ] **Step 1: Failing tests** — schema is `{command}` (not `code`/`backend`); a `RefuseSandbox` backend makes `shell` fail-closed (err contains "refused"); the tool name is `"shell"` (so `approval_required` gates it).
- [ ] **Step 2: FAIL. Step 3: Implement** — `Shell { inner: ExecuteCode }`; `with_backend(b)` = `ExecuteCode::with_backend(b, false)` (no override); `run` maps `{command}` → `{code}` and delegates to `inner.run`. **Step 4: PASS. Step 5: Commit** `feat(6d): shell tool — sandboxed backend, fail-closed, no override (phase 6d)`.

### Task 2: `OpToolConfig` (sa-core-types)

**Files:** Modify `crates/sa-core-types/src/config.rs`.

**Interfaces — Produces:** `pub struct OpToolConfig { pub name: String, pub cmd: Vec<String>, pub description: Option<String> }` + `op_tools: Vec<OpToolConfig>` on `ToolsConfig`.

- [ ] **Step 1: Failing test** — absent `op_tools` → empty; `[[tools.op_tools]] name="imagegen" cmd=["gen","--out","/d/o.png","--prompt"]` parses.
- [ ] **Step 2: FAIL. Step 3: Implement** the struct + field (`#[serde(default)]`). **Step 4: PASS. Step 5: Commit** `feat(6d): OpToolConfig — operator-frozen external-command templates (phase 6d)`.

### Task 3: the `op_tool` adapter

**Files:** Create `crates/sa-tools/src/tools/op_tool.rs`; modify `tools/mod.rs`.

**Interfaces — Produces:** `pub struct OpTool` + `pub fn OpTool::new(name, cmd: Vec<String>, description: Option<String>) -> Result<OpTool>` (errors if `cmd` empty).

- [ ] **Step 1: Failing tests** — schema is `{input}` only (no program/flags exposed); `new("", vec![])` errors (empty cmd); a real spawn (`cmd=["printf","%s"]`, input `"hi"` → unix; `["cmd","/C","echo"]` → windows) returns the input in stdout; the model arg is appended LAST (a crafted input like `--evil` is data, not a flag — assert it appears in output, not interpreted... best-effort: assert stdout contains it).
- [ ] **Step 2: FAIL. Step 3: Implement** — `run` builds `argv = cmd[1..] ++ [input]`, `Command::new(cmd[0]).args(argv).output()`, returns stdout+stderr; errors say only `cmd[0]`. **Step 4: PASS. Step 5: Commit** `feat(6d): op_tool — operator-frozen external-command adapter (phase 6d)`.

### Task 4: wire shell + op_tools into the registry

**Files:** Modify `secretagent/src/setup.rs`.

- [ ] **Step 1: Failing test** — `build_registry(&Config::default(), false)` lists `shell`; with a config carrying an `op_tools` entry named `"vision"`, the registry lists `vision`; an `op_tool` named `"fetch"` is SKIPPED (builtin wins).
- [ ] **Step 2: FAIL. Step 3: Implement** — register `Shell::with_backend(backend_from_config(...))`; for each `cfg.tools.op_tools`, register `OpTool::new(...)` only if `registry.get(&name).is_none()` (else log-skip). **Step 4: PASS. Step 5: Commit** `feat(6d): register shell + op_tools (skip builtin name collisions) (phase 6d)`.

---

## Acceptance (ADR slice 6d)
- `shell` runs sandboxed (delegates to `execute_code`'s `sa_exec::Backend`; fail-closed on `RefuseSandbox`). ✓ Task 1.
- An `op_tool` round-trips with output that the registry taints; the model fills only the data arg. ✓ Task 3 + the registry taint invariant.
- `op_tool` cannot become a generic shell: argv-only (no `sh -c`), program/flags frozen, one data arg. ✓ Task 3.
