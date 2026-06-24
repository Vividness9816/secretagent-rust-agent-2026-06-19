# SecretAgent Phase 5c — Subagent Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:test-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Let the agent delegate a sub-task to a fresh **subagent** that runs with authority **≤ its own** — proving acceptance (b): "a subagent runs a parallel pipeline via execute_code."

**Architecture:** A 3rd `Principal::Subagent { parent: Box<RunContext> }`. Side-effect authority **delegates** to the boxed parent (≤ parent, capped — a subagent can never exceed it), but `may_persist` / `may_auto_activate_skill` / `is_operator` are hard-**false** and provenance is hard-**Untrusted** (or-worse) regardless of parent. A bounded `depth` counter on `RunContext` fails closed (no fork-bomb). Spawn is wired INTO `run_task` as a synthetic `subagent` tool-spec dispatched specially (it re-enters `run_task` with `ctx.subagent_of()` + depth−1); the sub-run's answer returns to the parent as `Tainted` tool data.

**Tech Stack:** Rust, the existing `RunContext`/`Principal`/`Tainted` trust spine (ADR-20260621 §M1–M4), `ScriptedProvider` for hermetic loop tests. **Zero new deps.** `sa-core` module, no crate.

## Global Constraints

- **No new crate, no new dep** — `sa-core` module + `sa-core-types` enum variant only (JIT-crate rule).
- **The 4 invariants hold:** single self-contained binary (no new dep); SQLite canonical; subagent answer is `Tainted`, never an instruction; no secret in DB/audit/logs.
- **Trust model:** a subagent NEVER `may_persist`, NEVER auto-activates a skill, is NEVER `is_operator`; its input is stamped `Untrusted`; its side-effect authority is `≤` its parent's, by delegation.
- **Depth/fan-out bound:** `MAX_SUBAGENT_DEPTH` (mirroring `MAX_TOOL_STEPS = 8`); fan-out per level already capped by `MAX_TOOL_STEPS`; fail-closed at depth 0.
- **TDD**; commit per task; conventional-commit subject; footer = blank line then `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>` then `Claude-Session: phase-5c`. Append ` # self-audit-ok` to every `git commit` (the PreToolUse hook).
- **Gates before push:** `cargo fmt --all --check` 0 / `cargo clippy --all-targets --all-features -D warnings` 0 / `cargo test --all` on **both** Windows + WSL. `cargo deny check` green. rustls-only unchanged (no dep added).

## File Structure

- `crates/sa-core-types/src/principal.rs` — the trust derivation: `Subagent` variant, `depth`, `MAX_SUBAGENT_DEPTH`, `subagent_of`, narrowed methods, `PartialEq/Eq` on `RunContext`. + unit tests (zero I/O).
- `crates/sa-core/src/lib.rs` — `run_task`: append the `subagent` spec (only when `depth > 0`), intercept the `subagent` tool-call, depth-bounded boxed recursion, audit, `Tainted` re-feed. + integration tests.
- No bin change (run/gateway already pass a `RunContext`; the subagent tool appears automatically). No config change.

---

### Task 1: `Principal::Subagent` + `RunContext` narrowing (sa-core-types)

**Files:**
- Modify: `crates/sa-core-types/src/principal.rs`

**Interfaces:**
- Consumes: `Provenance` (types.rs).
- Produces:
  - `pub const MAX_SUBAGENT_DEPTH: usize = 2;`
  - `Principal::Subagent { parent: Box<RunContext> }`
  - `RunContext { principal, allow_list, depth: usize }` (now `#[derive(Debug, Clone, PartialEq, Eq)]`)
  - `RunContext::subagent_of(&self) -> RunContext`
  - narrowed `is_operator` / `may_run_side_effect` / `may_auto_activate_skill` / `may_persist` / `provenance` / `audit_label`.

- [ ] **Step 1: Write the failing tests** (append to `mod tests` in principal.rs)

```rust
    #[test]
    fn subagent_is_not_operator_and_never_persists_or_activates() {
        let sub = RunContext::operator(true).subagent_of();
        assert!(!sub.is_operator());
        assert!(!sub.may_persist()); // M2: no durable writes
        assert!(!sub.may_auto_activate_skill()); // never flips trust
        assert_eq!(sub.audit_label(), "subagent:operator");
        // or-worse: Untrusted even from a Trusted operator parent
        assert!(matches!(sub.provenance(), Provenance::Untrusted { .. }));
    }

    #[test]
    fn subagent_side_effect_authority_never_exceeds_parent() {
        // operator --yes parent → child may run side-effects (≤ parent, by delegation)
        let sub_yes = RunContext::operator(true).subagent_of();
        assert!(sub_yes.may_run_side_effect("execute_code"));
        assert!(sub_yes.may_run_side_effect("write_file"));
        // operator strict parent → child may NOT (parent couldn't either)
        let sub_strict = RunContext::operator(false).subagent_of();
        assert!(!sub_strict.may_run_side_effect("execute_code"));
        // remote parent with a narrow frozen grant → child inherits exactly that ⊆ set
        let sub_remote =
            RunContext::remote("telegram", "1", vec!["execute_code".into()]).subagent_of();
        assert!(sub_remote.may_run_side_effect("execute_code"));
        assert!(!sub_remote.may_run_side_effect("write_file"));
    }

    #[test]
    fn subagent_authority_is_a_subset_of_parent_for_every_tool() {
        for parent in [
            RunContext::operator(true),
            RunContext::operator(false),
            RunContext::remote("t", "1", vec!["write_file".into()]),
        ] {
            let child = parent.subagent_of();
            for tool in ["write_file", "shell", "execute_code", "read_file", "fetch"] {
                if child.may_run_side_effect(tool) {
                    assert!(
                        parent.may_run_side_effect(tool),
                        "subagent exceeded parent on {tool}"
                    );
                }
            }
        }
    }

    #[test]
    fn subagent_depth_is_bounded_and_fails_closed() {
        let p = RunContext::operator(true);
        assert_eq!(p.depth, MAX_SUBAGENT_DEPTH);
        let c = p.subagent_of();
        assert_eq!(c.depth, MAX_SUBAGENT_DEPTH - 1);
        // saturating: drilling past zero never underflows/panics
        let mut ctx = RunContext::operator(true);
        for _ in 0..(MAX_SUBAGENT_DEPTH + 5) {
            ctx = ctx.subagent_of();
        }
        assert_eq!(ctx.depth, 0);
    }

    #[test]
    fn nested_subagent_label_chains_and_stays_le_parent() {
        let sub2 = RunContext::operator(true).subagent_of().subagent_of();
        assert_eq!(sub2.audit_label(), "subagent:subagent:operator");
        assert!(sub2.may_run_side_effect("execute_code")); // still ≤ operator
        assert!(!sub2.may_persist());
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p sa-core-types principal`
Expected: FAIL (`subagent_of`/`depth`/`MAX_SUBAGENT_DEPTH` not found).

- [ ] **Step 3: Implement** — edit principal.rs:

1. Add the const above `Principal`:
```rust
/// Max subagent nesting depth (mirrors `MAX_TOOL_STEPS`). Fan-out per level is already
/// bounded by `MAX_TOOL_STEPS`; this bounds the chain so a subagent can't fork-bomb.
// ponytail: depth 2, fan-out/level ≤ MAX_TOOL_STEPS → worst case bounded; lower this or add
// a global spawn budget if a remote-driven run's token amplification ever bites.
pub const MAX_SUBAGENT_DEPTH: usize = 2;
```

2. Add the 3rd variant + doc to `Principal`:
```rust
    /// A delegated sub-run spawned by another run. Side-effect authority DELEGATES to
    /// `parent` (≤ parent, capped — never exceeds it), but it NEVER persists, NEVER
    /// auto-activates a skill, is NEVER `is_operator`, and its input is `Untrusted`.
    Subagent { parent: Box<RunContext> },
```

3. `RunContext`: add `pub depth: usize` and derive `PartialEq, Eq`:
```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunContext {
    pub principal: Principal,
    pub allow_list: Vec<String>,
    pub depth: usize,
}
```

4. Set `depth: MAX_SUBAGENT_DEPTH` in both `operator()` and `remote()` constructors.

5. Add `subagent_of`:
```rust
    /// Derive a sub-run context whose authority is ≤ this one. Side-effects delegate to
    /// `self` (capped at parity); persistence/activation are hard-false; depth−1 (fail-closed).
    pub fn subagent_of(&self) -> RunContext {
        RunContext {
            principal: Principal::Subagent {
                parent: Box::new(self.clone()),
            },
            allow_list: Vec::new(), // unused for Subagent — authority delegates to `parent`
            depth: self.depth.saturating_sub(1),
        }
    }
```

6. Narrow the methods — add a `Subagent` arm to each:
```rust
    // is_operator: a subagent is NEVER the operator (this gates may_persist) — no arm change
    // needed; matches!(.., Operator) is already false for Subagent.

    // may_run_side_effect: delegate to the parent (≤ parent, capped). bare-name already stripped.
    Principal::Subagent { parent } => parent.may_run_side_effect(bare),

    // may_auto_activate_skill stays `matches!(.., Operator { auto_approve: true })` → false. OK.
    // may_persist stays `self.is_operator()` → false for Subagent. OK.

    // provenance: or-worse — a subagent's task is model-generated (possibly injection-influenced),
    // so ALWAYS Untrusted, regardless of a Trusted parent.
    Principal::Subagent { parent } => Provenance::Untrusted {
        source: format!("subagent:{}", parent.audit_label()),
    },

    // audit_label: chain the parent label.
    Principal::Subagent { parent } => format!("subagent:{}", parent.audit_label()),
```
(`may_run_side_effect` strips the `::` namespace BEFORE the match, so pass the already-stripped `bare` into the parent delegation — do `parent.may_run_side_effect(bare)`.)

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p sa-core-types principal`
Expected: PASS (new + all existing principal tests).

- [ ] **Step 5: Commit**

```bash
git add crates/sa-core-types/src/principal.rs && git commit -m "feat(subagent): Principal::Subagent + RunContext::subagent_of — authority ≤ parent (phase 5c)" # self-audit-ok
```

---

### Task 2: Wire `subagent` into `run_task` (sa-core)

**Files:**
- Modify: `crates/sa-core/src/lib.rs`

**Interfaces:**
- Consumes: `RunContext::subagent_of`, `MAX_SUBAGENT_DEPTH` (Task 1); `Tainted::untrusted` (existing); `ScriptedProvider` (test).
- Produces: a `subagent` tool the model can call inside `run_task`.

- [ ] **Step 1: Write the failing integration tests** (append to `mod tests` in lib.rs)

```rust
    // Acceptance (b): a subagent runs execute_code (a "pipeline" step) under an operator --yes
    // parent — authority delegates (≤ parent) — and its result returns to the parent as data.
    #[tokio::test]
    async fn subagent_runs_execute_code_under_an_operator_parent() {
        use sa_audit::Audit;
        use sa_providers::ScriptedProvider;
        use sa_tools::Registry;

        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("m.db")).unwrap();
        // Shared queue, consumed across BOTH the parent and the subagent run_task calls:
        // 1) parent → spawn subagent  2) subagent → execute_code  3) subagent → answer  4) parent → answer
        let provider = ScriptedProvider::new(vec![
            ProviderAction::ToolCall {
                id: "p0".into(),
                name: "subagent".into(),
                args: serde_json::json!({"task": "run the pipeline via execute_code"}),
            },
            ProviderAction::ToolCall {
                id: "s0".into(),
                name: "execute_code".into(),
                args: serde_json::json!({"code": "print(1)"}),
            },
            ProviderAction::Text("subresult=42".into()),
            ProviderAction::Text("parent done: subresult=42".into()),
        ]);
        let mut registry = Registry::new();
        registry.register(Box::new(MockTool {
            name: "execute_code",
            output: "RAN_PIPELINE".into(),
        }));
        let policy = Policy::default();
        let mut audit = Audit::open(&dir.path().join("audit.jsonl")).unwrap();
        let agent = Agent::new(store, Box::new(provider), SystemContext::default());

        let answer = agent
            .run_task(
                "s1",
                "decompose and delegate",
                &registry,
                &policy,
                &mut audit,
                &RunContext::operator(true),
            )
            .await
            .unwrap();
        assert_eq!(answer, "parent done: subresult=42");

        let log = std::fs::read_to_string(dir.path().join("audit.jsonl")).unwrap();
        assert!(log.contains("subagent.spawn"), "spawn must be audited: {log}");
        // the subagent's execute_code ran and is attributed to the subagent principal
        assert!(
            log.contains("tool.execute_code"),
            "subagent's execute_code must run: {log}"
        );
        assert!(
            log.contains("subagent:operator"),
            "the subagent's action must be attributed to subagent:operator: {log}"
        );
    }

    // ≤ parent, live: under a STRICT operator parent the subagent's execute_code is DENIED.
    #[tokio::test]
    async fn subagent_side_effect_is_denied_under_a_strict_parent() {
        use sa_audit::Audit;
        use sa_providers::ScriptedProvider;
        use sa_tools::Registry;

        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("m.db")).unwrap();
        let provider = ScriptedProvider::new(vec![
            ProviderAction::ToolCall {
                id: "p0".into(),
                name: "subagent".into(),
                args: serde_json::json!({"task": "try execute_code"}),
            },
            ProviderAction::ToolCall {
                id: "s0".into(),
                name: "execute_code".into(),
                args: serde_json::json!({"code": "print(1)"}),
            },
            ProviderAction::Text("subagent gave up".into()),
            ProviderAction::Text("parent done".into()),
        ]);
        let mut registry = Registry::new();
        registry.register(Box::new(MockTool {
            name: "execute_code",
            output: "SHOULD_NOT_RUN".into(),
        }));
        let policy = Policy::default();
        let mut audit = Audit::open(&dir.path().join("audit.jsonl")).unwrap();
        let agent = Agent::new(store, Box::new(provider), SystemContext::default());

        agent
            .run_task(
                "s1",
                "delegate",
                &registry,
                &policy,
                &mut audit,
                &RunContext::operator(false), // strict parent
            )
            .await
            .unwrap();

        let log = std::fs::read_to_string(dir.path().join("audit.jsonl")).unwrap();
        assert!(log.contains("subagent.spawn"), "spawn still happens: {log}");
        assert!(
            log.contains("tool.denied"),
            "the subagent's side-effect must be denied (≤ strict parent): {log}"
        );
        assert!(
            !log.contains("tool.execute_code"),
            "execute_code must NOT have run under a strict parent: {log}"
        );
    }

    // Fork-bomb bound: nesting past MAX_SUBAGENT_DEPTH fails closed (the deepest spawn is refused).
    #[tokio::test]
    async fn subagent_spawn_is_refused_past_the_depth_bound() {
        use sa_audit::Audit;
        use sa_providers::ScriptedProvider;
        use sa_tools::Registry;
        use sa_core_types::principal::MAX_SUBAGENT_DEPTH;

        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("m.db")).unwrap();
        // Each level tries to spawn one deeper, then answers. With MAX_SUBAGENT_DEPTH spawns
        // succeeding and the (MAX+1)-th refused, build a queue that drives exactly that.
        let mut actions = Vec::new();
        for i in 0..(MAX_SUBAGENT_DEPTH + 1) {
            actions.push(ProviderAction::ToolCall {
                id: format!("c{i}"),
                name: "subagent".into(),
                args: serde_json::json!({"task": format!("level {i}")}),
            });
        }
        // the deepest run (depth 0) saw a refusal; every run then answers, unwinding outward
        for i in 0..(MAX_SUBAGENT_DEPTH + 1) {
            actions.push(ProviderAction::Text(format!("done {i}")));
        }
        let provider = ScriptedProvider::new(actions);
        let registry = Registry::new();
        let policy = Policy::default();
        let mut audit = Audit::open(&dir.path().join("audit.jsonl")).unwrap();
        let agent = Agent::new(store, Box::new(provider), SystemContext::default());

        agent
            .run_task(
                "s1",
                "deep",
                &registry,
                &policy,
                &mut audit,
                &RunContext::operator(true),
            )
            .await
            .unwrap();

        let log = std::fs::read_to_string(dir.path().join("audit.jsonl")).unwrap();
        let spawns = log.matches("subagent.spawn").count();
        assert_eq!(
            spawns, MAX_SUBAGENT_DEPTH,
            "exactly MAX_SUBAGENT_DEPTH spawns succeed; the next is refused: {log}"
        );
        assert!(
            log.contains("subagent.denied"),
            "the over-depth spawn must be refused + audited: {log}"
        );
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p sa-core subagent`
Expected: FAIL (no `subagent` handling — the call routes to "[unknown tool: subagent]", no `subagent.spawn` audit).

- [ ] **Step 3: Implement** — in `run_task`:

(a) After building `specs`, offer the synthetic tool only when more depth remains:
```rust
        let mut specs = specs;
        if ctx.depth > 0 {
            specs.push(ToolSpec {
                name: "subagent".into(),
                description: "Delegate a self-contained sub-task to a fresh subagent that runs \
                    with authority no greater than yours (it cannot persist memory, cannot \
                    auto-approve, and inherits your tool permissions). Returns the subagent's \
                    final answer as data. Use it to break independent sub-tasks out of your own \
                    context."
                    .into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "task": {"type": "string", "description": "the sub-task to delegate"}
                    },
                    "required": ["task"]
                }),
            });
        }
```
(`specs` is currently `let specs: Vec<ToolSpec> = ...;` — change to `let mut specs` or shadow as above.)

(b) In the `ProviderAction::ToolCall { id, name, args }` arm, BEFORE the `approval_required` check, intercept the synthetic tool:
```rust
                    if name == "subagent" {
                        traj.tool_names.push(name.clone());
                        let sub_task = args
                            .get("task")
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        // Fail-closed: refuse at the depth floor or on an empty task.
                        let result = if ctx.depth == 0 || sub_task.is_empty() {
                            audit.append_synced(AuditEvent {
                                action: "subagent.denied".into(),
                                key_id: "subagent".into(),
                                principal: Some(ctx.audit_label()),
                            })?;
                            "[subagent denied: depth limit reached or empty task]".to_string()
                        } else {
                            let sub_ctx = ctx.subagent_of();
                            audit.append_synced(AuditEvent {
                                action: "subagent.spawn".into(),
                                key_id: sub_ctx.audit_label(),
                                principal: Some(ctx.audit_label()),
                            })?;
                            let sub_session = format!("{session_id}::sub.{}", traj.steps);
                            // async recursion → box the future.
                            match Box::pin(self.run_task(
                                &sub_session,
                                &sub_task,
                                registry,
                                policy,
                                audit,
                                &sub_ctx,
                            ))
                            .await
                            {
                                Ok(a) => a,
                                Err(e) => format!("[subagent error: {e}]"),
                            }
                        };
                        // The subagent's answer is untrusted data, never an instruction.
                        let tainted = Tainted::untrusted(result, "subagent".to_string());
                        messages.push(call_echo);
                        messages.push(json!({"role": "tool", "tool_call_id": id,
                            "content": tainted.as_data()}));
                        continue;
                    }
```
(`call_echo` is already built above this point in the existing arm — keep its construction before this block, or move the `subagent` interception to just after `call_echo` is built. Place this block immediately after the `let call_echo = json!({...});` line.)

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p sa-core subagent`
Expected: PASS (all 3 new tests).

- [ ] **Step 5: Full sa-core + clippy + fmt**

Run: `cargo test -p sa-core` then `cargo clippy --all-targets --all-features -- -D warnings` then `cargo fmt --all --check`
Expected: all green (no regression in the existing run_task tests).

- [ ] **Step 6: Commit**

```bash
git add crates/sa-core/src/lib.rs && git commit -m "feat(subagent): wire depth-bounded subagent spawn into run_task — returns Tainted data (phase 5c)" # self-audit-ok
```

---

### Task 3: Docs + both-venue gate + adversarial review + push

**Files:**
- Modify: `PROGRESS.md` (5c row), `ROADMAP.md` (5c ✅), `docs/HANDOFF-phase5.md` (mark 5c done, point to 5d).

- [ ] **Step 1:** Update the three docs: PROGRESS.md 5c slice ledger (commits + properties proven), ROADMAP.md `⬜ 5c` → `✅ 5c`, HANDOFF 5c → done.

- [ ] **Step 2: Both-venue gate.** Windows `cargo test --all`; WSL `wsl.exe bash -c 'export PATH="$HOME/.cargo/bin:$PATH" CARGO_TARGET_DIR="$HOME/sa-target"; cd /mnt/c/Users/dnoye/ClaudeSecondBrain/SecretAgent; cargo test --all; echo CARGO_EXIT=$?'`. Both must be 0. (WSL is the landlock venue; it must compile + pass.)

- [ ] **Step 3: Adversarial review** (user-requested). A focused `self-audit` agent on the trust boundary: can a subagent (i) persist, (ii) auto-activate a skill, (iii) exceed the parent's side-effect set, (iv) fork-bomb past the depth bound, (v) leak its task/result as an instruction or into the audit payload, (vi) flip its own provenance to Trusted? Fix any real finding with a regression test.

- [ ] **Step 4:** `cargo deny check` (green; no dep change). Confirm rustls-only unchanged.

- [ ] **Step 5: Commit docs + push.**
```bash
git add PROGRESS.md ROADMAP.md docs/HANDOFF-phase5.md && git commit -m "docs(5c): subagent shipped — PROGRESS/ROADMAP/HANDOFF (phase 5c)" # self-audit-ok
git push origin master
```

- [ ] **Step 6: Watch CI green on all 5 jobs.**
```bash
gh run list --branch master --limit 6 --json databaseId,headSha
gh run watch "$RUN" --exit-status --interval 25
```

---

## Self-Review

- **Spec coverage:** acceptance (b) "subagent runs a parallel pipeline via execute_code" → Task 2 integration test (`subagent_runs_execute_code_under_an_operator_parent`). ≤-parent → Task 1 subset test + Task 2 strict-parent deny. No-persist/no-activate/Untrusted → Task 1. Depth bound → Task 1 (math) + Task 2 (live refusal). ✓
- **Placeholder scan:** none — every step has real code/commands. ✓
- **Type consistency:** `subagent_of(&self) -> RunContext`, `Principal::Subagent { parent: Box<RunContext> }`, `MAX_SUBAGENT_DEPTH`, audit actions `subagent.spawn`/`subagent.denied` — used identically across Tasks 1–2. ✓
- **Deferred (ponytail, noted):** true concurrent fan-out (`tokio::join` of subagents) is deferred — the `&mut Audit` lock serializes them anyway; "parallel" here = logical decomposition, sequential spawn. Per-tool narrowing below parent (the `allow_list` field is vestigial for Subagent — authority delegates) is deferred until a real need. No `compile_fail` test — ≤-parent is a runtime property, proven by the subset unit test (the honest tool).
