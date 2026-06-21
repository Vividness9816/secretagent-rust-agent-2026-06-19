# SecretAgent Phase 4a — Trust spine + daemon loop — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `run_task`'s bare `auto_approve: bool` with a typed `RunContext`/`Principal` that makes a remote sender's dangerous capabilities *unrepresentable*, add principal attribution to the audit log (back-compat), and stand up the `gateway` daemon-loop skeleton — the trust foundation every later Phase-4 slice builds on.

**Architecture:** A 2-variant `Principal` (`Operator{auto_approve}` | `Remote{connector,sender}`) in `sa-core-types`, carried in a `RunContext{principal, allow_list}`. `Remote` has no path to operator consent (M1, compile-time). `run_task` consults the context for the side-effect gate, skill auto-activation, durable-memory writes (M2), and the input provenance stamp. The audit `AuditEvent` gains an optional principal label that records *who* drove each action without breaking the existing hash chain on historical lines. A thin `gateway` command runs the agent in a `tokio` loop until a shutdown signal.

**Tech Stack:** Rust 2021, tokio (add `signal` feature), rusqlite, serde/serde_json, blake3, anyhow. No new crate this slice (the `sa-connectors` crate lands in 4c).

## Global Constraints

- **License:** MIT. Clean-room — no Hermes source copied.
- **4 invariants:** single self-contained binary per OS; SQLite single canonical store (every index rebuildable); tool/connector output is `Tainted`, never an instruction; no secret in DB/audit/logs.
- **JIT-crate rule:** a new crate ONLY at a real compile boundary, never a stub. **4a adds NO new crate.**
- **rustls only** (musl-static); no `native-tls`/openssl ever enters the graph.
- **TDD**; one atomic commit per task; conventional-commit subject (≤72 chars).
- **Commit footer (every commit):** a blank line, then `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`, then `Claude-Session: <session>`.
- **The `self-audit` PreToolUse hook blocks `git commit`** — append ` # self-audit-ok` to the bash command (the council/plan already adversarially reviewed this slice; a per-task `self-audit` agent runs only where a task says so).
- **Before every commit:** `cargo fmt --all` (then `--check` = 0), `cargo clippy --all-targets --all-features -- -D warnings` (0), relevant `cargo test` (pass). rustfmt rewraps long chains — expect a format pass.
- **Both-venue gate before push:** run the full suite on Windows (`cargo`) AND WSL (`wsl.exe bash -c 'export PATH="$HOME/.cargo/bin:$PATH" CARGO_TARGET_DIR="$HOME/sa-target"; cd /mnt/c/Users/dnoye/ClaudeSecondBrain/SecretAgent; cargo test --all'`). Then push and watch CI to green on all 5 jobs.
- **Commit `Cargo.lock`** in the same change as any dep/feature edit.
- **Tests assert `SCHEMA_VERSION`, never a literal.** (No migration this slice, but the rule stands.)
- **ADR:** `ADR-20260621-secretagent-phase4-daemon-messaging-cron` is binding. M1/M2/M3 definitions and the principal taxonomy come from it; this slice implements M1 (structural), M2 (guards), and the audit attribution. M3 (sender allow-list) is 4c.

---

## File structure (this slice)

- **Create** `crates/sa-core-types/src/principal.rs` — `Principal` enum + `RunContext` + its decision methods (pure, fully unit-tested). The trust spine.
- **Modify** `crates/sa-core-types/src/lib.rs` — `pub mod principal;` + re-export.
- **Modify** `crates/sa-core/src/lib.rs` — `run_task` signature (`auto_approve: bool` → `ctx: &RunContext`); the side-effect gate, skill auto-activation, M2 durable-write guard, provenance-by-principal stamp, principal label on audit events; update ~10 in-file test call sites; add new behavior tests.
- **Modify** `crates/sa-audit/src/lib.rs` — add `principal: Option<String>` to `AuditEvent` (`#[serde(default, skip_serializing_if = "Option::is_none")]`); add a back-compat verify test.
- **Modify** `secretagent/src/run.rs` — pass `&RunContext::operator(auto_approve)`.
- **Create** `secretagent/src/gateway.rs` — `GatewayState` + `run_until(shutdown)` daemon loop.
- **Modify** `secretagent/src/main.rs` — `mod gateway;` + a `Gateway` subcommand.
- **Modify** `Cargo.toml` — add `signal` to the workspace `tokio` features; commit `Cargo.lock`.

---

### Task 1: `Principal` + `RunContext` in `sa-core-types`

**Files:**
- Create: `crates/sa-core-types/src/principal.rs`
- Modify: `crates/sa-core-types/src/lib.rs`
- Test: in-file `#[cfg(test)]` in `principal.rs`

**Interfaces:**
- Produces: `enum Principal { Operator { auto_approve: bool }, Remote { connector: String, sender: String } }`; `struct RunContext { pub principal: Principal, pub allow_list: Vec<String> }`; constructors `RunContext::operator(bool)`, `RunContext::remote(into<String>, into<String>, Vec<String>)`; methods `is_operator() -> bool`, `may_run_side_effect(&str) -> bool`, `may_auto_activate_skill() -> bool`, `may_persist() -> bool`, `provenance() -> Provenance`, `audit_label() -> String`.
- Consumes: `crate::types::Provenance`.

- [ ] **Step 1: Write the failing tests**

Create `crates/sa-core-types/src/principal.rs`:

```rust
use crate::types::Provenance;

/// Who is driving a run. The dangerous capability — dispatching a side-effectful tool
/// with no per-tool grant, or auto-activating a draft skill — is reachable ONLY from
/// `Operator`. `Remote` carries no field or method that yields it, so "a remote message
/// auto-approved a side-effect" is *unrepresentable* (the `Tainted<T>` precedent).
/// ADR-20260621 M1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Principal {
    /// Local CLI/TTY operator. `auto_approve` is the `--yes` standing consent — valid only
    /// because a human is attending the run.
    Operator { auto_approve: bool },
    /// A connector-sourced sender (untrusted). Never auto-approves ad-hoc; reaches a
    /// side-effectful tool ONLY via the run's frozen, operator-armed `allow_list`.
    Remote { connector: String, sender: String },
}

/// One run's trust context: who is asking + the side-effect tools pre-authorized for this
/// binding/job. `allow_list` is empty for an `Operator` (which uses `auto_approve` instead);
/// for a `Remote` it is the operator-armed, frozen set (a connector binding or a cron job).
#[derive(Debug, Clone)]
pub struct RunContext {
    pub principal: Principal,
    pub allow_list: Vec<String>,
}

impl RunContext {
    /// Local CLI run. `auto_approve` = `--yes`.
    pub fn operator(auto_approve: bool) -> Self {
        Self {
            principal: Principal::Operator { auto_approve },
            allow_list: Vec::new(),
        }
    }

    /// Connector-/cron-driven run with a frozen, operator-armed side-effect allow-list.
    pub fn remote(
        connector: impl Into<String>,
        sender: impl Into<String>,
        allow_list: Vec<String>,
    ) -> Self {
        Self {
            principal: Principal::Remote {
                connector: connector.into(),
                sender: sender.into(),
            },
            allow_list,
        }
    }

    pub fn is_operator(&self) -> bool {
        matches!(self.principal, Principal::Operator { .. })
    }

    /// May this run dispatch a side-effectful tool (one `approval_required` flags)?
    /// `Operator`: iff `--yes`. `Remote`: iff the tool's BARE name (MCP `server::` stripped,
    /// matching `approval_required`) is in the frozen `allow_list`. NEVER via ad-hoc consent.
    pub fn may_run_side_effect(&self, tool: &str) -> bool {
        let bare = tool.rsplit("::").next().unwrap_or(tool);
        match &self.principal {
            Principal::Operator { auto_approve } => *auto_approve,
            Principal::Remote { .. } => self.allow_list.iter().any(|t| t == bare),
        }
    }

    /// May this run auto-activate the intent-bound draft skill it authored? `Operator` + `--yes`
    /// ONLY. `Remote` NEVER (M1/M2): a remote sender controls `slug(task)` and must not flip trust.
    pub fn may_auto_activate_skill(&self) -> bool {
        matches!(self.principal, Principal::Operator { auto_approve: true })
    }

    /// May this run WRITE durable memory (skills / prefs / user_model)? `Operator` ONLY (M2).
    /// Default-deny: a future 3rd principal is non-persisting until explicitly opted in here.
    pub fn may_persist(&self) -> bool {
        self.is_operator()
    }

    /// Provenance to stamp this run's user input. `Operator` → `Trusted`; `Remote` →
    /// `Untrusted { source }` (so connector input flows through the unchanged injection guard).
    pub fn provenance(&self) -> Provenance {
        match &self.principal {
            Principal::Operator { .. } => Provenance::Trusted,
            Principal::Remote { connector, sender } => Provenance::Untrusted {
                source: format!("{connector}:{sender}"),
            },
        }
    }

    /// Short, secret-free label for the audit log: `"operator"` | `"remote:telegram:<sender>"`.
    pub fn audit_label(&self) -> String {
        match &self.principal {
            Principal::Operator { .. } => "operator".to_string(),
            Principal::Remote { connector, sender } => format!("remote:{connector}:{sender}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operator_yes_may_run_side_effects_and_persist() {
        let ctx = RunContext::operator(true);
        assert!(ctx.is_operator());
        assert!(ctx.may_run_side_effect("write_file"));
        assert!(ctx.may_auto_activate_skill());
        assert!(ctx.may_persist());
        assert_eq!(ctx.provenance(), Provenance::Trusted);
    }

    #[test]
    fn operator_strict_denies_side_effects_but_may_persist() {
        let ctx = RunContext::operator(false);
        assert!(!ctx.may_run_side_effect("write_file"));
        assert!(!ctx.may_auto_activate_skill());
        assert!(ctx.may_persist()); // an attended operator still learns skills
        assert_eq!(ctx.provenance(), Provenance::Trusted);
    }

    #[test]
    fn remote_never_auto_approves_persists_or_activates() {
        let ctx = RunContext::remote("telegram", "12345", vec![]);
        assert!(!ctx.is_operator());
        assert!(!ctx.may_run_side_effect("write_file"));
        assert!(!ctx.may_auto_activate_skill());
        assert!(!ctx.may_persist()); // M2: a remote run writes no durable memory
        assert_eq!(
            ctx.provenance(),
            Provenance::Untrusted {
                source: "telegram:12345".into()
            }
        );
        assert_eq!(ctx.audit_label(), "remote:telegram:12345");
    }

    #[test]
    fn remote_reaches_a_side_effect_only_via_a_frozen_grant() {
        let ctx = RunContext::remote("cron", "job7", vec!["write_file".into()]);
        assert!(ctx.may_run_side_effect("write_file")); // operator-armed grant
        assert!(!ctx.may_run_side_effect("execute_code")); // not granted → denied
        // even with a grant, a remote run never writes durable memory or auto-activates
        assert!(!ctx.may_persist());
        assert!(!ctx.may_auto_activate_skill());
    }

    #[test]
    fn remote_grant_strips_mcp_namespace_like_approval_required() {
        // A frozen grant for "write_file" must also cover the namespaced "evil::write_file"
        // form, matching how approval_required strips `::` (no bypass via namespacing).
        let ctx = RunContext::remote("telegram", "1", vec!["write_file".into()]);
        assert!(ctx.may_run_side_effect("evil::write_file"));
        // and a remote without the grant is denied even on the namespaced name
        let ctx2 = RunContext::remote("telegram", "1", vec![]);
        assert!(!ctx2.may_run_side_effect("evil::write_file"));
    }
}
```

- [ ] **Step 2: Wire the module + run the tests to verify they fail to compile/exist**

Add to `crates/sa-core-types/src/lib.rs` (next to the other `pub mod` lines):

```rust
pub mod principal;
```

Run: `cargo test -p sa-core-types principal`
Expected: COMPILE ERROR or FAIL until the module is recognized — confirm the file is picked up, then PASS once Step 1's code compiles (the impl is written alongside the tests in this step, so this task's "fail" is the pre-module compile error; after adding `pub mod principal;` it should PASS).

- [ ] **Step 3: Confirm the whole crate still builds**

Run: `cargo test -p sa-core-types`
Expected: PASS (existing `types`/`policy`/`config`/`taint` tests + the 5 new principal tests).

- [ ] **Step 4: Format + clippy**

Run: `cargo fmt --all && cargo clippy -p sa-core-types --all-targets -- -D warnings`
Expected: no diff after fmt re-run; clippy clean.

- [ ] **Step 5: Commit**

```bash
git add crates/sa-core-types/src/principal.rs crates/sa-core-types/src/lib.rs
git commit -m "feat(core-types): Principal + RunContext trust context (phase 4a)" # self-audit-ok
```
(Append the footer lines in the commit body.)

---

### Task 2: Thread `RunContext` through `run_task` (M1 gate + M2 guard + provenance)

**Files:**
- Modify: `crates/sa-core/src/lib.rs` (`run_task` signature + 4 decision sites + ~10 in-file test call sites)
- Modify: `secretagent/src/run.rs` (the one production call site)
- Test: in-file `#[cfg(test)]` in `crates/sa-core/src/lib.rs`

**Interfaces:**
- Consumes: `sa_core_types::principal::RunContext` (Task 1).
- Produces: `Agent::run_task(&self, session_id: &str, user_input: &str, registry: &Registry, policy: &Policy, audit: &mut Audit, ctx: &RunContext) -> Result<String>` (replaces the trailing `auto_approve: bool`).

- [ ] **Step 1: Write the failing tests** (add to the `tests` module in `crates/sa-core/src/lib.rs`)

```rust
#[tokio::test]
async fn remote_run_stamps_untrusted_and_creates_no_skill() {
    use sa_audit::Audit;
    use sa_core_types::principal::RunContext;
    use sa_providers::ScriptedProvider;
    use sa_tools::Registry;

    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("m.db");
    let task = "summarize the changelog";
    // A successful remote-driven task that WOULD mint a skill if it were an operator run.
    {
        let store = Store::open(&db).unwrap();
        let provider = ScriptedProvider::new(vec![ProviderAction::Text("did it".into())]);
        let registry = Registry::new();
        let policy = Policy::default();
        let mut audit = Audit::open(&dir.path().join("a.jsonl")).unwrap();
        let agent = Agent::new(store, Box::new(provider), SystemContext::default());
        agent
            .run_task(
                "s1",
                task,
                &registry,
                &policy,
                &mut audit,
                &RunContext::remote("telegram", "999", vec![]),
            )
            .await
            .unwrap();
    }
    // M2: a remote run writes NO durable skill.
    let skills = Store::open(&db).unwrap().list_skills().unwrap();
    assert!(skills.is_empty(), "remote run must not create a skill: {skills:?}");
    // The remote user turn was stamped Untrusted (its provenance is the source label).
    let prov: Vec<String> = Store::open(&db)
        .unwrap()
        .message_provenances("s1")
        .unwrap();
    assert!(
        prov.iter().any(|p| p.contains("telegram:999")),
        "remote user turn must be stamped Untrusted{{source}}: {prov:?}"
    );
}

#[tokio::test]
async fn remote_side_effect_denied_without_a_grant_but_runs_with_one() {
    use sa_audit::Audit;
    use sa_core_types::principal::RunContext;
    use sa_providers::ScriptedProvider;
    use sa_tools::Registry;

    let dir = tempfile::tempdir().unwrap();
    let make = || {
        ScriptedProvider::new(vec![
            ProviderAction::ToolCall {
                id: "c0".into(),
                name: "write_file".into(),
                args: serde_json::json!({"path": "x", "content": "y"}),
            },
            ProviderAction::Text("done".into()),
        ])
    };
    let mut registry = Registry::new();
    registry.register(Box::new(MockTool { name: "write_file", output: "WROTE".into() }));
    let policy = Policy::default();

    // Remote, NO grant → denied (no ad-hoc consent path exists).
    {
        let store = Store::open(&dir.path().join("a.db")).unwrap();
        let mut audit = Audit::open(&dir.path().join("a.jsonl")).unwrap();
        let agent = Agent::new(store, Box::new(make()), SystemContext::default());
        agent
            .run_task("s", "go", &registry, &policy, &mut audit,
                &RunContext::remote("telegram", "1", vec![]))
            .await
            .unwrap();
        let log = std::fs::read_to_string(dir.path().join("a.jsonl")).unwrap();
        assert!(log.contains("tool.denied"), "ungranted remote side-effect must deny: {log}");
        assert!(!log.contains("tool.write_file"), "tool must not run: {log}");
        assert!(log.contains("remote:telegram:1"), "audit must record the principal: {log}");
    }
    // Remote WITH a frozen grant → runs (operator pre-armed it).
    {
        let store = Store::open(&dir.path().join("b.db")).unwrap();
        let mut audit = Audit::open(&dir.path().join("b.jsonl")).unwrap();
        let agent = Agent::new(store, Box::new(make()), SystemContext::default());
        agent
            .run_task("s", "go", &registry, &policy, &mut audit,
                &RunContext::remote("telegram", "1", vec!["write_file".into()]))
            .await
            .unwrap();
        let log = std::fs::read_to_string(dir.path().join("b.jsonl")).unwrap();
        assert!(log.contains("tool.write_file"), "granted remote side-effect must run: {log}");
    }
}
```

This requires a new read-only `Store` helper `message_provenances(session_id) -> Result<Vec<String>>`. Add it to `crates/sa-memory/src/lib.rs` (near `recent`/`all_messages`):

```rust
/// The serialized provenance strings of a session's messages, oldest-first.
/// Read-only; used to assert a remote turn was stamped Untrusted (test/forensic).
pub fn message_provenances(&self, session_id: &str) -> Result<Vec<String>> {
    let mut stmt = self
        .conn
        .prepare("SELECT provenance FROM messages WHERE session_id = ?1 ORDER BY id")?;
    let rows = stmt
        .query_map([session_id], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}
```

- [ ] **Step 2: Run the new tests — verify they fail**

Run: `cargo test -p sa-core remote_run_stamps -- --nocapture` (and `remote_side_effect`)
Expected: COMPILE FAIL (`run_task` still takes `bool`; `message_provenances` missing).

- [ ] **Step 3: Refactor `run_task` to take `&RunContext`**

In `crates/sa-core/src/lib.rs`:

3a. Add the import near the other `sa_core_types` uses:
```rust
use sa_core_types::principal::RunContext;
```

3b. Change the signature (around line 263-271): replace the trailing parameter
```rust
        auto_approve: bool,
```
with
```rust
        ctx: &RunContext,
```

3c. Provenance stamp (around line 287): replace
```rust
            let trusted = serde_json::to_string(&Provenance::Trusted)?;
            store.add_message(session_id, "user", user_input, &trusted)?;
```
with
```rust
            let input_prov = serde_json::to_string(&ctx.provenance())?;
            store.add_message(session_id, "user", user_input, &input_prov)?;
```
Then in the skill-activation block just below, the activation still records the operator's
`Provenance::Trusted` when it flips a skill — keep a local `trusted` for that write:
```rust
            let trusted = serde_json::to_string(&Provenance::Trusted)?;
```
(Place it immediately before the `if let Some(skill) = store.get_skill_by_name(&own)?` block,
since `activate_skill(&own, &trusted)` below needs it.)

3d. Skill auto-activation gate (around line 301-303): replace
```rust
                    if auto_approve {
                        store.activate_skill(&own, &trusted)?;
```
with
```rust
                    if ctx.may_auto_activate_skill() {
                        store.activate_skill(&own, &trusted)?;
```

3e. Side-effect approval gate (around line 371): replace
```rust
                    if approval_required(&name) && !auto_approve {
```
with
```rust
                    if approval_required(&name) && !ctx.may_run_side_effect(&name) {
```

3f. Principal label on the audit events inside `run_task`. For each `audit.append_synced(AuditEvent { action: ..., key_id: ... })` call in `run_task` (the `skill.activate`, `skill.activate.denied`, `skill.reuse`, `tool.denied`, and `tool.{name}` events), add the principal field:
```rust
                        audit.append_synced(AuditEvent {
                            action: "tool.denied".into(),
                            key_id: name.clone(),
                            principal: Some(ctx.audit_label()),
                        })?;
```
(Apply the same `principal: Some(ctx.audit_label())` to every `AuditEvent` constructed inside `run_task`. The `AuditEvent` field is added in Task 3 — if doing Task 3 after this, temporarily use `..Default::default()`; recommended order is **Task 3 first**, then this. See note below.)

3g. M2 durable-write guard (around line 356): replace
```rust
                    self.learn_from_trajectory(session_id, &traj, audit)?;
```
with
```rust
                    if ctx.may_persist() {
                        self.learn_from_trajectory(session_id, &traj, audit, ctx)?;
                    }
```
and thread `ctx` into `learn_from_trajectory`'s signature so its internal `skill.create` audit
event can carry `principal: Some(ctx.audit_label())`:
```rust
    fn learn_from_trajectory(
        &self,
        session_id: &str,
        traj: &Trajectory,
        audit: &mut Audit,
        ctx: &RunContext,
    ) -> Result<()> {
```
and in its `skill.create` `AuditEvent`, add `principal: Some(ctx.audit_label())`.

> **Ordering note:** the `principal` field on `AuditEvent` is introduced in **Task 3**. Do Task 3 **before** finishing Task 2's audit edits, OR land Task 2's logic first with the existing 2-field `AuditEvent` and add `principal` to all sites in Task 3. Either is fine; the recommended order is **Task 3 → Task 2-audit-edits** so the field exists when you reference it. The TDD steps below assume Task 3's field is present.

- [ ] **Step 4: Update the production call site**

In `secretagent/src/run.rs`, replace the `run_task` call's trailing argument:
```rust
            &mut audit,
            auto_approve,
```
with
```rust
            &mut audit,
            &sa_core::RunContext::operator(auto_approve),
```
Add `pub use sa_core_types::principal::RunContext;` to `crates/sa-core/src/lib.rs` (re-export) so `run.rs` can name it as `sa_core::RunContext` without a new dep line, OR import `sa_core_types::principal::RunContext` directly in `run.rs` (it already depends on `sa_core_types`). Prefer the re-export.

- [ ] **Step 5: Update the ~10 in-file `run_task` test call sites**

In every existing `run_task(...)` call inside `crates/sa-core/src/lib.rs` tests, replace the trailing `false` / `true` with `&RunContext::operator(false)` / `&RunContext::operator(true)`. Add `use sa_core_types::principal::RunContext;` to the `tests` module's `use super::*;` block (or a local `use`). Sites: the injection test, both calls in the approval test, `run_task_system_message...`, `untrusted_tool_output...`, both calls in `novel_task_creates_a_skill...`, both in `poisoned_skill...`, both in `an_unrelated_keyword...`.

- [ ] **Step 6: Run the full `sa-core` + `sa-memory` suite**

Run: `cargo test -p sa-core -p sa-memory`
Expected: PASS — the new Task-2 tests plus all existing tests (now using `RunContext::operator`). The `poisoned_skill` and `approval` tests prove the operator path is byte-for-byte unchanged.

- [ ] **Step 7: Format + clippy + commit**

Run: `cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings`
```bash
git add crates/sa-core/src/lib.rs crates/sa-memory/src/lib.rs secretagent/src/run.rs
git commit -m "feat(core): run_task takes RunContext; remote denies side-effects + writes (phase 4a)" # self-audit-ok
```

---

### Task 3: Audit principal attribution (back-compat)

**Files:**
- Modify: `crates/sa-audit/src/lib.rs` (add the field + a back-compat test)
- Test: in-file `#[cfg(test)]` in `crates/sa-audit/src/lib.rs`

**Interfaces:**
- Produces: `AuditEvent { action: String, key_id: String, principal: Option<String> }` where `principal` defaults to `None` and is omitted from the serialized form when `None` (so historical lines round-trip byte-identically and `verify_chain` still passes).

> **Do this task BEFORE Task 2's audit edits** (see Task 2 ordering note).

- [ ] **Step 1: Write the failing back-compat test**

Add to the `tests` module in `crates/sa-audit/src/lib.rs`:

```rust
#[test]
fn old_lines_without_principal_still_verify_and_new_lines_carry_it() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("audit.jsonl");

    // Simulate a pre-Phase-4 log line written WITHOUT a `principal` key, plus its valid hash.
    // (Hand-build it the way `append` would have: seq 0, prev "", event has only action+key_id.)
    let legacy_event = AuditEvent { action: "vault.set".into(), key_id: "API_KEY".into(), principal: None };
    let legacy_hash = entry_hash(0, "", &legacy_event);
    let legacy_line = format!(
        "{{\"seq\":0,\"prev\":\"\",\"event\":{{\"action\":\"vault.set\",\"key_id\":\"API_KEY\"}},\"hash\":\"{legacy_hash}\"}}"
    );
    std::fs::write(&p, format!("{legacy_line}\n")).unwrap();

    // The legacy line (no `principal` key) must still verify — proving Option+skip is byte-stable.
    assert!(
        Audit::verify_chain(&p).unwrap(),
        "a pre-Phase-4 audit line without `principal` must still verify"
    );

    // Append a NEW line that DOES carry a principal; the chain must continue + verify.
    {
        let mut a = Audit::open(&p).unwrap();
        a.append(AuditEvent {
            action: "tool.write_file".into(),
            key_id: "write_file".into(),
            principal: Some("remote:telegram:123".into()),
        })
        .unwrap();
    }
    assert!(Audit::verify_chain(&p).unwrap(), "chain must continue after the schema add");
    let body = std::fs::read_to_string(&p).unwrap();
    assert!(body.contains("remote:telegram:123"), "new line records the principal");
    // The legacy line on disk still has NO `principal` key (we never rewrote it).
    let first = body.lines().next().unwrap();
    assert!(!first.contains("principal"), "legacy line must remain principal-free on disk");
}
```

- [ ] **Step 2: Run it — verify it fails**

Run: `cargo test -p sa-audit old_lines_without_principal -- --nocapture`
Expected: COMPILE FAIL (`AuditEvent` has no `principal` field).

- [ ] **Step 3: Add the field**

In `crates/sa-audit/src/lib.rs`, change `AuditEvent`:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub action: String,
    pub key_id: String,
    /// Who drove this action: `"operator"` | `"remote:<connector>:<sender>"`. The forensic
    /// complement to the M1/M2/M3 prevention controls (ADR-20260621). `#[serde(default,
    /// skip_serializing_if)]` keeps pre-Phase-4 lines (no `principal` key) byte-identical on
    /// re-serialization, so `entry_hash`/`verify_chain` still pass them. NEVER a secret value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<String>,
}
```

- [ ] **Step 4: Fix the existing `sa-audit` tests + helpers**

Every existing `AuditEvent { action, key_id }` literal in `crates/sa-audit/src/lib.rs` tests (there are ~7) now needs `principal: None`. Add `principal: None,` to each. (Compiler-walked — `cargo test -p sa-audit` lists each site.)

- [ ] **Step 5: Run the suite — verify PASS**

Run: `cargo test -p sa-audit`
Expected: PASS — the new back-compat test + all existing chain/torn-line/leak tests. Critically, `mutating_an_entry_breaks_the_chain` and `append_reads_back_and_chain_verifies` still pass (the field is invisible when `None`).

- [ ] **Step 6: Format + clippy + commit**

Run: `cargo fmt --all && cargo clippy -p sa-audit --all-targets -- -D warnings`
```bash
git add crates/sa-audit/src/lib.rs
git commit -m "feat(audit): optional principal attribution, back-compat hash chain (phase 4a)" # self-audit-ok
```

> After Task 3, return to **Task 2 Step 3f/3g** and ensure every `AuditEvent` built inside `run_task`/`learn_from_trajectory` sets `principal: Some(ctx.audit_label())`, and every OTHER `AuditEvent { action, key_id }` literal in `sa-core`, `secretagent/src/run.rs`, and `secretagent/src/skill.rs` adds `principal: None` (or a label where one is known, e.g. `run.rs`'s override event → `Some("operator".into())`). Re-run `cargo test --all` to confirm the workspace compiles.

---

### Task 4: `gateway` daemon-loop skeleton

**Files:**
- Create: `secretagent/src/gateway.rs`
- Modify: `secretagent/src/main.rs` (`mod gateway;` + `Gateway` subcommand)
- Modify: `Cargo.toml` (add `signal` to `tokio` features) + commit `Cargo.lock`
- Test: `secretagent/tests/gateway.rs`

**Interfaces:**
- Produces: `pub struct GatewayState { ... }` (a connector-status map, empty this slice) with `pub fn new() -> Self`; `pub async fn run_until(shutdown: impl std::future::Future<Output = ()>) -> anyhow::Result<()>` that builds the shared agent/store/audit, logs start, and returns cleanly when `shutdown` resolves.

- [ ] **Step 1: Add the `signal` tokio feature**

In `Cargo.toml` `[workspace.dependencies]`, change:
```toml
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```
to:
```toml
tokio = { version = "1", features = ["macros", "rt-multi-thread", "signal"] }
```
Run: `cargo build -p secretagent` (pulls the feature into `Cargo.lock`).

- [ ] **Step 2: Write the failing test**

Create `secretagent/tests/gateway.rs`:
```rust
// The gateway loop must build its shared state and return cleanly when the shutdown
// future resolves — proving the daemon shell starts and stops without hanging.
#[tokio::test]
async fn gateway_runs_and_shuts_down_cleanly() {
    // An already-ready shutdown future → run_until returns immediately, no hang.
    let res = secretagent_gateway_run_until_ready().await;
    assert!(res.is_ok(), "gateway must shut down cleanly: {res:?}");
}

// Thin wrapper so the integration test can drive the lib-style entry without a real signal.
async fn secretagent_gateway_run_until_ready() -> anyhow::Result<()> {
    // SAFETY/ISOLATION: point the daemon at a temp data dir so it doesn't touch real state.
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("SECRETAGENT_DATA_DIR", dir.path());
    std::env::set_var("SECRETAGENT_CONFIG_DIR", dir.path());
    let r = secretagent::gateway::run_until(async {}).await;
    std::env::remove_var("SECRETAGENT_DATA_DIR");
    std::env::remove_var("SECRETAGENT_CONFIG_DIR");
    r
}
```

> This requires `secretagent` to expose a tiny lib surface. If `secretagent` is bin-only today,
> add `src/lib.rs` exposing `pub mod gateway;` (+ whatever `gateway` needs) and have `main.rs`
> `use secretagent::gateway;`. Keep the lib surface minimal — only `gateway` (and its deps) need
> to be `pub`. If a lib target already exists, just add `pub mod gateway;` there.

- [ ] **Step 3: Run the test — verify it fails**

Run: `cargo test -p secretagent --test gateway`
Expected: COMPILE FAIL (`secretagent::gateway` does not exist).

- [ ] **Step 4: Implement the skeleton**

Create `secretagent/src/gateway.rs`:
```rust
//! The always-on gateway daemon (Phase 4). This slice (4a) is the SHELL: it builds the
//! shared agent/store/audit, holds a `GatewayState`, and idles until a shutdown signal.
//! Connectors (4c) and the scheduler tick (4d) plug into the loop later.

use anyhow::Result;
use std::collections::HashMap;
use std::future::Future;

/// Runtime status of the daemon's connectors. Empty in 4a (no connectors yet); the seam
/// that 4c's connectors and `doctor`/`status` read. Liveness is recorded here, not in a
/// second representation.
#[derive(Debug, Default)]
pub struct GatewayState {
    /// connector id -> last-known status line (e.g. "polling", "down: <reason>").
    pub connectors: HashMap<String, String>,
}

impl GatewayState {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Run the gateway until `shutdown` resolves, then return cleanly. The CLI passes a real
/// signal future (Ctrl-C / SIGTERM); tests pass `async {}` for an immediate clean exit.
pub async fn run_until(shutdown: impl Future<Output = ()>) -> Result<()> {
    // Build the shared state the daemon will own. In 4a there are no connectors driving it;
    // we prove the shell stands up against the configured (here: temp) data dir and exits.
    let _state = GatewayState::new();
    tracing::info!("gateway: started (no connectors configured)");

    shutdown.await;
    tracing::info!("gateway: shutdown requested, stopping");
    Ok(())
}

/// The signal future the CLI uses: resolve on Ctrl-C, or (Unix) SIGTERM from systemd `stop`.
pub async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
```

- [ ] **Step 5: Wire the subcommand**

In `secretagent/src/main.rs`: if you added `src/lib.rs`, replace the `mod gateway;` approach with `use secretagent::gateway;`. Add to the `Cmd` enum:
```rust
    /// Run the always-on gateway daemon (messaging connectors + scheduler). Installed as a
    /// service by `service install`. Stops cleanly on Ctrl-C / SIGTERM.
    Gateway,
```
And in the `match cli.cmd` block:
```rust
        Cmd::Gateway => gateway::run_until(gateway::shutdown_signal()).await,
```

- [ ] **Step 6: Run the test — verify PASS**

Run: `cargo test -p secretagent --test gateway`
Expected: PASS (the loop builds state, logs, and returns on the ready shutdown).

- [ ] **Step 7: Whole-workspace gate**

Run: `cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all`
Expected: all green on Windows.

- [ ] **Step 8: Commit**

```bash
git add secretagent/src/gateway.rs secretagent/src/main.rs secretagent/src/lib.rs Cargo.toml Cargo.lock
git commit -m "feat(gateway): daemon-loop skeleton + GatewayState + clean shutdown (phase 4a)" # self-audit-ok
```

---

### Task 5: Slice gate — both-venue + adversarial check + push

**Files:** none (verification only)

- [ ] **Step 1: Self-audit the trust spine**

Dispatch the `self-audit` agent (Task tool, `subagent_type: "self-audit"`) on the slice diff (`git diff master...HEAD`), focused on: does `Remote` have ANY path to `auto_approve`/`may_persist`/`may_auto_activate`? Is the provenance stamp correct for both principals? Did any existing operator-path test silently change behavior? Fix anything it flags; re-gate.

- [ ] **Step 2: Both-venue full suite**

Windows: `cargo test --all`
WSL: `wsl.exe bash -c 'export PATH="$HOME/.cargo/bin:$PATH" CARGO_TARGET_DIR="$HOME/sa-target"; cd /mnt/c/Users/dnoye/ClaudeSecondBrain/SecretAgent; cargo test --all'`
Expected: both green.

- [ ] **Step 3: fmt --check + clippy (the CI gates)**

Run: `cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings`
Expected: 0 / 0.

- [ ] **Step 4: Push + watch CI**

```bash
git push origin master
RUN_ID=$("/c/Program Files/GitHub CLI/gh.exe" run list --branch master --limit 1 --json databaseId --jq '.[0].databaseId')
"/c/Program Files/GitHub CLI/gh.exe" run watch "$RUN_ID" --exit-status --interval 25
```
Expected: `conclusion=success` on all 5 jobs. Fix red before declaring 4a done.

- [ ] **Step 5: Slice acceptance gate (STOP for review)**

4a is done when: `run_task` takes `RunContext` (the bare `auto_approve: bool` is gone); a Remote run is denied side-effects without a frozen grant, allowed with one, writes no durable skill, and stamps its input `Untrusted{source}`; the audit log attributes each action to a principal and still verifies historical lines; the `gateway` command starts + shuts down cleanly; all prior tests green on both venues + CI. **Report status and STOP for user review before 4b.**

---

## Self-Review

**Spec coverage (ADR §4 + §9 slices → tasks):**
- M1 (Remote structurally cannot auto-approve) → Task 1 (`may_run_side_effect`/`may_auto_activate_skill` have no Remote→consent path) + Task 2 (gate consults `ctx`). ✓
- M2 (Remote writes no durable memory) → Task 1 (`may_persist`) + Task 2 (Step 3g guard). ✓
- Provenance by principal (Untrusted for connectors) → Task 1 (`provenance()`) + Task 2 (Step 3c). ✓
- Audit attribution + back-compat → Task 3. ✓
- Daemon loop + GatewayState seam → Task 4. ✓
- M3 (sender allow-list), connectors, service install, scheduler → **out of scope (4b/4c/4d), correctly deferred.** ✓
- No migration this slice (no new table) → `SCHEMA_VERSION` unchanged. ✓

**Placeholder scan:** none — every step shows full code/commands.

**Type consistency:** `RunContext`/`Principal` names + method signatures match between Task 1 (definition) and Task 2 (use); `message_provenances` defined in Task 2 Step 1 and used in the same test; `AuditEvent.principal: Option<String>` defined in Task 3 and referenced in Task 2's audit edits (ordering note resolves the dependency: Task 3 before Task 2's audit edits).
