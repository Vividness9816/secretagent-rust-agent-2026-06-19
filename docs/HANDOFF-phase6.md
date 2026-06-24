# SecretAgent — handoff to a fresh session (Phase 6: full tool surface, polish, packaging)

Paste this whole file (or the prompt that points at it) into a new session to continue the build.
It is self-contained.

---

You're continuing a multi-phase build of **SecretAgent**, a clean-room Rust agent daemon
(full-Hermes-Agent parity v1, **MIT**, **no Hermes source copied**). **Phases 0–5 are complete and
CI-green.** **Phase 6 (parity v1: tools + providers + TUI + packaging + ops) is IN PROGRESS** — the
milestone architecture is decided by **/council → ADR-20260623-secretagent-phase6-milestone**, and
**slices 6a (the `assemble_agent` refactor), 6b (release packaging), and 6c (the egress-guarded HTTP
seam + network tools — the live `Fetch::run` SSRF is FIXED) are shipped + CI-green.**
**Next = 6d.**

## Where it lives
- Repo: `C:\Users\dnoye\ClaudeSecondBrain\SecretAgent` (nested git repo, branch `master`).
- Private remote: `Vividness9816/secretagent-rust-agent-2026-06-19`. `master` is at **`7282193`**
  (6b complete; 6c then shipped through **`e3eb623`**); working tree clean, everything pushed.
- **Confirm state first:** `git log --oneline -8` and `git status`.

## Read first (authoritative; the ADR wins on conflict)
- The `project-secretagent` memory (auto-loads) — the running ledger of every phase/slice.
- **`~/.claude/second-brain/decisions/ADR-20260623-secretagent-phase6-milestone.md`** — the binding
  Phase-6 milestone decision (the 9-slice order, the scope line, the cross-cutting architecture).
  Plus the prior phase ADRs (founding / phase2-sandbox / phase3-learning / phase4-daemon /
  phase5 / phase5d-voice) for the standing invariants.
- `ROADMAP.md` (the 9-slice list + the deferred tail) + `PROGRESS.md` (the slice ledger) +
  `docs/RELEASE.md` (the §6b release/signing story).
- The spec `~/Downloads/SecretAgent-Build-Plan.md` — §4 Parity Inventory (the acceptance contract),
  §9 Install & Distribution, §10 Phase 6.
- The house slice style: any `docs/superpowers/plans/2026-06-23-secretagent-phase5*.md`.

## STANDING INVARIANTS (FIXED — do NOT relitigate)
Single self-contained binary per OS; heavy/optional deps are runtime-OPTIONAL + feature-gated, NEVER
install-time-required; **rustls-only** (no openssl/native-tls/aws-lc-sys/zstd-sys — the musl-static
invariant); SQLite single canonical store, every index rebuildable; tool/connector/voice output is
`Tainted`, never an instruction; the **Principal/RunContext trust spine** (Operator vs Remote vs
Subagent; M1 structural / M2 no-durable-write / M3 default-deny `(connector,sender)` allow-list /
M4 freeze-at-arm-time); the **JIT-crate rule** (a new crate only when it owns a compile-enforced
invariant); no secret in DB/audit/logs; zero-egress-by-default (egress is default-deny via
`policy.egress_allow`). The personal "Dylan N" Authenticode cert is available for Windows signing.

## Done so far (do NOT rebuild)
- **Phases 0–5** complete + CI-green: foundation/vault/audit → memory+FTS5+providers+agentic loop →
  tools + landlock `execute_code` + MCP + the `Tainted`/`Policy` injection guard → the learning loop
  → the daemon (service install, Telegram/Discord/Email/Slack connectors + the M3 boundary, NL→cron
  scheduler) → exec backends (local/docker/ssh) → subagents → CLI voice.
- **Phase 6 council + ADR-20260623-phase6-milestone** (5-seat, 2-round, genuine cross-over). The
  decisive calls: **refactor-first → packaging-early → self-update-last**; an honest
  **parity-by-mechanism** scope (the agent reaches arbitrary tools via MCP + `op_tool`; ship the
  high-value bespoke set; DEFER the §4 long tail behind the established traits); a **single
  egress-guarded HTTP chokepoint**; `op_tool` narrowed to operator-frozen externals; Anthropic as a
  native 2nd provider; a reedline TUI bin-module; self-update only with a pinned-verify contract or
  DEFER.
- **Slice 6a — `assemble_agent` refactor (`3937ef1`):** the agent+registry assembly was duplicated
  4× (`secretagent/src/{chat,run,gateway,voice}.rs`, two literally commented "mirrors run"). Now
  `secretagent/src/setup.rs::{build_provider, build_agent, build_registry}`. **Behavior-preserving**
  (proven byte-identical against the full 31-suite corpus + 2 seam tests, net −19 lines), with every
  site's divergence kept as an explicit param: `allow_unsandboxed` (voice/gateway→false, run→flag),
  the `RunContext` untouched (each site keeps operator/remote/voice), `build_registry` returns the
  backend label so run/gateway audit it but voice deliberately omits it. **This is the seam 6c–6f
  build through — add new tools/providers/the TUI HERE, not by re-duplicating.**
- **Slice 6b — release packaging (`7c32538` + `.gitattributes 7282193`):** `doctor` binary-integrity
  line (prints the running binary's sha256 vs the published `SHA256SUMS`; `sha2` dep, pure-Rust); a
  **distroless non-root** `Dockerfile` + locked-down `compose.yaml`; `install.sh`/`install.ps1`
  (fetch → verify minisign+sha256 → THEN place, fail-closed, prints PATH only;
  `scripts/test-install-verify.sh` smoke-tests the fail-closed logic); `.github/workflows/release.yml`
  (tag → matrix → `SHA256SUMS` → minisign → optional Authenticode → GitHub release + GHCR
  multi-arch container); `docs/RELEASE.md`. **Operator-gated finish:** generate a minisign keypair
  (`minisign -G -W`), pin the pubkey in `install.sh` + add the secret key as GH `MINISIGN_SECRET_KEY`,
  then cut a `v*` tag. (The Dockerfile builds in CI on tag; a *local* `docker build` is blocked by the
  homelab Pi-hole container-DNS flake — environment, not a defect.)
- **Slice 6c — egress seam + network tools (`8f9281e..e3eb623`, CI `28075958143`):** `sa-tools/src/egress.rs`
  is the ONE chokepoint (`egress_get`/`egress_request -> Tainted`: real `reqwest::Url` parse, reject
  `@`-userinfo + non-http(s), exact-host allow-list, deny resolved loopback/link-local/RFC-1918/ULA/
  CGNAT/multicast — v4-mapped v6 unwrapped — unless the IP literal is in `egress_allow` (matched by
  parsed `IpAddr`), reqwest pinned to the vetted IP via `.resolve` (DNS-rebind closed), redirects OFF +
  host/IP re-vet every hop (cap 5), body 8 MiB + 20s caps). `Fetch` re-pointed at it, `url_host`
  DELETED (the live SSRF is gone). `web_extract`/`http_request`/`web_search` are per-tool modules under
  `sa-tools/src/tools/`, all through the seam; `web_search` gets an operator-frozen `search_url` + a
  vault `*_ref` key (`ToolsConfig`, injected at construction via `setup.rs::resolve_secret`). Zero new
  crates. Adversarial `self-audit` → PASS. **The seam is the spine 6d's `op_tool` egress must respect.**

---

## TASK — the remaining slices, in order (each = a TDD slice ending in a concrete acceptance test)

For each slice: `writing-plans` (house style, `docs/superpowers/plans/`) → inline TDD → both-venue
gate → push → watch CI. **6c and 6h get a focused `self-audit`/adversarial review before push**
(they touch the egress boundary and an RCE primitive); 6d–6g get a self-audit if the slice warrants.
Use `/council` only if a slice surfaces a NEW architecture fork the milestone ADR didn't settle.

### 6c — egress-guarded HTTP seam + network tools (SECURITY-CRITICAL; also fixes a live bug)
The Skeptic found a **live, already-shipped SSRF in `crates/sa-tools/src/lib.rs` `Fetch::run`**:
`url_host` string-splits the host (so `http://allowed.com@169.254.169.254/` passes the allow-list via
the `@`-userinfo trick), `reqwest::Client::new()` follows redirects by default (an allow-listed host
can 302 to the metadata endpoint and the body returns), and there's no IP-literal/loopback/link-local
guard. **Build ONE shared `egress_get(policy, url) -> Tainted` chokepoint** that: parses the URL with
a real parser (reject `@`-userinfo + non-http(s)); denies any resolved IP that is
loopback/link-local (`169.254.0.0/16`, `::1`)/RFC-1918/ULA/`0.0.0.0`/multicast unless explicitly
allow-listed; **re-checks egress on EVERY redirect hop** (or disables redirects); caps body size +
timeout. **Re-point `Fetch` at it and delete `url_host`** (Phase 6 hardens, not just adds). Then add
`web_search` / `http_request` / `web_extract` through the SAME seam — no tool calls `reqwest`
directly. **Operator-frozen clients stay OUTSIDE the seam** (`sa-providers/src/openai.rs`,
`sa-connectors/src/{telegram,slack}.rs` — not model-reachable). *Acceptance:* an SSRF corpus
(metadata / loopback / `@`-userinfo / redirect-to-internal) is DENIED; an allow-listed search/fetch
round-trips; the new tools' output is `Tainted`. **Per-tool modules** (`sa-tools/src/tools/*.rs`,
mirroring the per-file connectors). Credential model = a per-tool vault `key_ref` + a shared default
ref (the existing `*_ref` convention — NO new "gateway" abstraction; inject the secret at tool
*construction* like `ExecuteCode::with_backend`, no `Tool`-trait change for the common case).

### 6d — system + external tools
A `shell` tool that routes through `sa_exec::Backend` (fail-closed landlock) **or is simply
`execute_code`** — NEVER a raw `std::process::Command` (that bypasses the sandbox + the
`approval_required` name-gate). Plus a generic **`op_tool`** (the 5d-voice operator-command-template
pattern generalized) for vision / image-gen / TTS / a browser CLI: **operator-FROZEN command** (model
fills only a data arg, never the program/URL/flags), endpoint host **allow-listed**, stdout wrapped
`Tainted::untrusted`. `op_tool` is a NARROW adapter, never a generic curl/bash escape hatch.
*Acceptance:* shell runs sandboxed (or is execute_code); an `op_tool` round-trips with `Tainted` output.

### 6e — providers
**Anthropic** as a native 2nd `impl Provider` in `sa-providers` (the Messages API differs — content
blocks, `tool_use`/`tool_result`; NOT an openai-compat shim; deletes the proxy dependency).
OpenAI/OpenRouter are already free (`base_url` + `api_key_ref` on `OpenAiCompat`). A runtime
`secretagent model <name>` switch = an **operator-only** config rewrite (NEVER reachable from a
Remote/cron principal — guard it). A **minimal** multi-model per-role map (plan/execute/summarize,
defaulting to one model — config not ceremony; lands through the 6a `assemble_agent` seam).
*Acceptance:* a task runs against Anthropic; `model` switches with no restart; a Remote run can't
repoint the endpoint.

### 6f — TUI
A **bin module** `secretagent/src/tui/` (NOT a new crate — JIT-crate rule; it owns no compile-enforced
invariant) using **reedline** (multiline + history + slash-autocomplete + streaming — the §4.5
acceptance is a line-editor spec, not a full-screen ratatui app). Reuse 6a's `assemble_agent` +
`Agent::run_task`/`turn` verbatim. *Acceptance:* the TUI drives a task end-to-end with streaming output.

### 6g — ops
`backup`/`restore` (the **SQLite Online Backup API** — never `cp` a live WAL DB; the age vault stays
**encrypted** in the archive; `chmod 600` the identity on restore; verify the audit hash-chain) +
`trajectory export` (read `messages`/audit → JSON/JSONL, **secret-free** — exclude/redact
`messages.content` or grep the artifact). Each a clap subcommand + a `doctor` line, consistent with
`service`/`schedule`. *Acceptance:* backup→restore round-trips a live DB; export is secret-free.

### 6h — self-update (LAST — or DEFERRED)
An RCE-as-a-service-with-vault-access primitive. Ship ONLY with the full contract: download-to-temp →
**verify a detached signature against a public key PINNED in the binary** (`include_bytes!`, never
fetched) → **no-downgrade** (version from the *signed* payload) → **atomic rename** → audit event;
**negative-control tests** that a tampered binary AND a downgrade are both rejected. **If that can't
be proven this milestone, DEFER it** (manual re-install is a safe v1) — it's the first thing to cut.
*Acceptance:* a tampered/downgrade update is refused; a genuine one swaps atomically + audits.

### 6i — parity-tail doc + acceptance
`docs/parity-tail.md` (what shipped vs deferred-behind-which-trait + why) + the honest §4 acceptance
amendment (Pillar C). *Acceptance:* the doc is accurate; `doctor` passes clean on a fresh box.

## Scope line (IN vs DEFERRED-with-triggers — from the ADR)
**DEFERRED behind the established traits (do NOT build these in Phase 6):** browser-automation via
chromiumoxide (musl/exfil/unverified-TLS — when needed, ship as an `op_tool` shell-out to an operator
Chromium); in-process vision/image/audio C-libs (use `op_tool` shell-out instead); daytona/singularity/
modal backends (behind the `Backend` enum); Skills Hub sync; the 16 remaining connectors (behind the
`Connector` trait); macOS notarization (no Apple account); per-tool rate limits / an egress-allow DSL.
The "60+ tools" target is replaced by an honest curated set + MCP/`op_tool` reach — documented in 6i.

## Operator-gated finishes (build testable-WITHOUT; defer the live step — the established precedent)
- **6b release:** generate `minisign -G -W`, pin the pubkey in `install.sh` + GH `MINISIGN_SECRET_KEY`
  (+ optional `WINDOWS_PFX_BASE64`/`WINDOWS_PFX_PASSWORD`), cut a `v*` tag → `release.yml` runs.
- **Carried from prior phases:** live Slack E2E (Slack app + xapp-/xoxb- tokens), the SSH exec
  backend check (an SSH host), the live whisper/piper voice round-trip (the binaries on PATH).

## Conventions / gates (non-negotiable — held through Phases 0–6b)
- **TDD**; commit per task; conventional-commit subject; footer = a blank line then
  `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>` then `Claude-Session: phase-6c` (bump per slice).
- The **`self-audit` PreToolUse hook blocks `git commit`** — append ` # self-audit-ok` to the bash
  command. **It is a shell COMMENT: anything chained after it (`&& git push`) is swallowed — run
  `git push` as a SEPARATE command.**
- Before EVERY commit: `cargo fmt --all --check` (0) / `cargo clippy --all-targets --all-features --
  -D warnings` (0) / relevant `cargo test`. (Clippy gotcha: a `#[cfg(test)] mod tests` must be the
  LAST item in a file — `items-after-test-module`.)
- **Both-venue gate before push** (Windows `cargo test --all` + WSL `wsl.exe bash -c 'export
  PATH="$HOME/.cargo/bin:$PATH" CARGO_TARGET_DIR="$HOME/sa-target"; cd
  /mnt/c/Users/dnoye/ClaudeSecondBrain/SecretAgent; cargo test --all; echo CARGO_EXIT=$?'`). WSL is
  the landlock/musl venue. Watch cargo's exit code (`CARGO_EXIT=0` is the definitive signal).
- **Keep rustls-only** — no openssl/native-tls/aws-lc-sys/zstd-sys (`wsl … cargo tree -e features -p
  secretagent | grep -iE "openssl|native-tls|aws-lc-sys|zstd-sys"` empty); a NEW dep must be
  pure-Rust + musl-clean. **Commit `Cargo.lock`** with any dep change.
- Then watch CI green on all 5 jobs: `gh run list --branch master --limit 6 --json databaseId,headSha`,
  then `gh run watch "$RUN" --exit-status --interval 25`. `gh` is at `/c/Program Files/GitHub CLI/gh.exe`.
- For a major NEW architecture fork → `/council`; before shipping a security-sensitive slice →
  `self-audit` or a focused adversarial-review Workflow.

## Build / CI gotchas (already solved — keep them)
- **The homelab Pi-hole DNS flakes for CONTAINERS** (`dl-cdn.alpinelinux.org` / `static.rust-lang.org`
  / crates.io intermittently unresolvable from inside `docker build`). A local `docker build` of the
  6b image is blocked by this — it is environmental, NOT a Dockerfile defect; the image builds in
  `release.yml`'s CI. If you must build locally, retry when container DNS is healthy (probe:
  `docker run --rm alpine nslookup crates.io`).
- **`sha256sum` output differs by platform** (git-bash/binary `hash *file` vs GNU/text `hash  file`)
  — compare hash FIELDS via awk (strip a leading `*`), never grep on the separator (see `install.sh`
  / `scripts/test-install-verify.sh`).
- Local Ollama has `hermes3:latest` (tools-capable) for live tests; set `model` in `config.toml`.
- Feature-gate heavy/optional deps; the bin enables them (the discord/email/slack/voice precedent).
  The `LF will be replaced by CRLF` git warnings are benign; `.gitattributes` forces LF on
  `.sh`/Dockerfile/yaml (CRLF breaks Linux shebangs).
