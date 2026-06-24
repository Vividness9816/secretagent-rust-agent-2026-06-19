# SecretAgent — Progress ledger

Slice-by-slice build status. `master` is the integration branch; every slice is committed
TDD-style and gated (fmt 0 / clippy -D warnings 0 / tests pass) on **both** Windows and WSL/Linux
before push, then CI is watched green on all 5 jobs (`check` + 4 cross-compile legs:
Linux x86_64-musl & aarch64-musl, Windows MSVC, macOS aarch64).

**Current HEAD:** Phases 0–5 complete; **✅ Phase 6 (parity v1) BUILD COMPLETE** — 6a refactor + 6b packaging + 6c egress seam + 6d system/external tools + 6e providers (native Anthropic) + 6f TUI + 6g ops (backup/restore + secret-free export) + 6h self-update (pinned-key verify) + 6i parity-tail doc (`docs/parity-tail.md`, the Pillar-C §4 amendment) all shipped + CI-green. Operator-gated live legs remain (signed release + self-update key pin; Slack/SSH/voice/Anthropic creds).

---

## ✅ Phase 6 — Full tool surface, polish, packaging *(/council **ADR-20260623-secretagent-phase6-milestone**)* — BUILD COMPLETE
A milestone of 9 ordered slices (refactor-first / packaging-early / self-update-last), scoped to a
**parity-by-mechanism** line (the agent reaches arbitrary tools via MCP + `op_tool`; ship the
high-value bespoke set; defer the §4 long tail behind the established traits). **All 9 (6a–6i) shipped
+ CI-green.** The honest §4 acceptance amendment is `docs/parity-tail.md` (6i). See ROADMAP.md for the
full slice list + the deferred tail.

### ✅ 6i — Parity-tail doc + §4 acceptance amendment (Pillar C)
`docs/parity-tail.md` — the honest map: parity is **by mechanism** (the MCP client + `op_tool` reach
arbitrary tools) + a curated bespoke set, NOT a padded "60+ tools all green" (that was rejected by the
ADR). Documents what shipped (6a–6h + the P0–P5 spine), what's deferred **behind which already-built
trait** + the trigger to build each (browser-automation→`op_tool`, more backends→the `Backend` enum,
more connectors→the `Connector` trait, etc.), the per-slice residuals, the operator-gated live legs,
and the §4 acceptance statement (honesty *is* the Pillar-C acceptance). Closes the milestone.

### ✅ 6a — `assemble_agent` refactor (`3937ef1`)
Collapsed the agent+registry assembly duplicated 4× (`chat`/`run`/`gateway`/`voice`, two literally
commented "mirrors run") into `secretagent/src/setup.rs::{build_provider,build_agent,build_registry}`.
**Behavior-preserving** — proven byte-identical against the full 31-suite corpus + 2 new seam tests,
**net −19 lines**, with every site's divergence kept as an explicit param (the Skeptic's caveat:
`allow_unsandboxed` explicit [voice/gateway→false], the `RunContext` untouched, `build_registry`
returns the backend label so run/gateway audit it but voice deliberately omits it). The precondition
for a single egress chokepoint (6c) + a consistent tool registry.

### ✅ 6b — Release packaging (no tagged release cut yet — operator-gated)
| Artifact | What |
|---|---|
| `doctor` integrity line | prints the running binary's **SHA-256** + version so the operator can verify it against the published `SHA256SUMS` (the §9 offline check). `sha2` dep (pure-Rust, musl-clean — purity unchanged). TDD'd. |
| `Dockerfile` + `compose.yaml` | **distroless, non-root** multi-stage (musl-static build → `distroless/static:nonroot`); compose runs it **read-only rootfs + `cap_drop: ALL` + `no-new-privileges`**, only `sa-data` writable. |
| `install.sh` / `install.ps1` | **fetch → verify (minisign sig over `SHA256SUMS` + sha256) → THEN place**, fail-closed, prints PATH guidance only (never edits the shell rc). The verify-before-place fail-closed logic is smoke-tested by `scripts/test-install-verify.sh` (genuine passes; tampered + wrong-sum rejected). |
| `.github/workflows/release.yml` | tag (`v*`) → 4-target build matrix → `SHA256SUMS` → **minisign** (required) → optional **Authenticode** (Dylan-N, graceful-skip if no secret) → GitHub release + **GHCR multi-arch container**. |
| `docs/RELEASE.md` | the honest single-maintainer signing story (authenticity-from-one-key, NOT a CA chain), the minisign keygen + GH-secret setup, **macOS notarization DEFERRED** (no Apple account). |

**Operator-gated finish** (the established precedent — build testable-without, defer the live step): generate the minisign keypair (`minisign -G -W`), pin the pubkey in `install.sh` + add the secret key as GH `MINISIGN_SECRET_KEY` (+ optional Windows PFX secrets), then cut a `v*` tag. Gates: full 31-suite corpus + the doctor test green both venues; installer fail-closed test green; YAML valid; dep purity preserved.

### ✅ 6c — Egress-guarded HTTP seam + network tools (`8f9281e..e3eb623`) *(plan: `docs/superpowers/plans/2026-06-24-secretagent-phase6c-egress-seam.md`)*
| Commit | What |
|---|---|
| `a83d452` | **`sa-tools/src/egress.rs` — the single chokepoint.** `egress_get`/`egress_request` → `Tainted<String>`: real URL parse (`reqwest::Url`), reject `@`-userinfo + non-http(s), exact-host allow-list, deny resolved loopback/link-local/RFC-1918/ULA/unspecified/multicast/CGNAT IPs (v4-mapped v6 unwrapped first) unless the **IP literal** is in `egress_allow`, **pin reqwest to the vetted IP (`.resolve`)** to close DNS-rebind, **redirects OFF + re-vet host+IP every hop** (cap 5), body cap 8 MiB + 20s timeout. **Zero new crates** (reqwest re-exports `url`; tokio `net` feature only). |
| `9fcbf84` | **Re-point `Fetch` at the seam + DELETE `url_host`** — the live SSRF string-splitter (`http://allowed.com@169.254.169.254/` bypass + default-redirect client) is gone. Phase 6 hardens, not just adds. |
| `8965c3d` | `web_extract` (seam GET + dep-free `strip_html`) — `tools/web_extract.rs`. |
| `a33ea1d` | `http_request` ({method,url,body} — **no model-chosen headers**) — `tools/http_request.rs`. |
| `38197bc` | `web_search` (`WebSearch::with_key`, operator-frozen endpoint, model fills only the URL-encoded `q=`, Bearer key set by the seam) + `ToolsConfig {search_url, search_key_ref, default_key_ref}` (the `*_ref` convention, secret injected at construction). |
| `bba0cb9` | Register `web_extract`+`http_request` always + `web_search` if `search_url` set (key `search_key_ref→default_key_ref` from the vault); DRY `resolve_secret` now backs provider + search keys. |
| `e3eb623` | **Hardening (self-audit LOW):** match the IP allow-list exception by **parsed `IpAddr`** not string, so `0:0:0:0:0:0:0:1` matches `::1`. |

**Acceptance MET:** SSRF corpus (metadata IP / loopback / `@`-userinfo / redirect-to-internal / non-http scheme) **DENIED** before any body returns; an allow-listed-IP fetch + a POST **round-trip**; the seam output is **`Tainted::untrusted`**; `url_host` deleted; no model-reachable tool touches `reqwest` directly (operator-frozen provider/connectors stay outside the seam). **Adversarial `self-audit` → PASS** (no CRITICAL/HIGH; the 7-vector SSRF probe verified against the real `reqwest 0.12.28`/`url 2.5.8` — `.resolve` pins, v4-mapped unwrap, per-hop re-vet, query-encode all sound). Gates: fmt/clippy(all-features) 0; `cargo test --all` both venues (Win 173/0, WSL CARGO_EXIT=0); rustls-only clean; Cargo.lock unchanged (zero new crates). **CI green on all 5 jobs** (`28075958143`).

### ✅ 6d — System + external tools (`91448a2`) *(plan: `docs/superpowers/plans/2026-06-24-secretagent-phase6d-system-external-tools.md`)*
| Commit | What |
|---|---|
| `f15c583` | **`shell` tool** (`sa-tools/src/tools/shell.rs`) — a thin alias over the `execute_code` path (same operator-frozen `sa_exec::Backend`) under the name `"shell"` that `approval_required` already gates; **strictly fail-closed (no `allow_unsandboxed` override** — that hatch stays execute_code's local-CLI-only). |
| `6a13beb` | **`OpToolConfig { name, cmd, description }`** + `op_tools: Vec<_>` on `ToolsConfig` — the operator-frozen external-command template (argv: program + all flags + any host). |
| `2df3667` | **`op_tool` adapter** (`sa-tools/src/tools/op_tool.rs`) — generalizes the 5d-voice argv pattern: model fills ONLY a final `input` data arg appended last; spawned via argv (**never `sh -c`**, so a flag-looking input stays data — proven by a pure `build_argv` test); errors name **argv[0] only**. A NARROW adapter, not a curl/bash escape hatch. |
| `91448a2` | **Registry wiring** — `shell` armed with the operator backend; one `op_tool` per config entry, registered **LAST** and **skipped on a name collision** (builtins/network/MCP win) or empty cmd, so a misconfigured op_tool can never shadow a guarded tool. |

**Acceptance MET:** `shell` runs sandboxed (delegates to `execute_code`'s backend; fail-closed on `RefuseSandbox`); an `op_tool` round-trips (output re-tainted at the registry boundary); the model fills only the data arg (argv-separated, no `sh -c`). Gates: fmt/clippy(all-features) 0; `cargo test --all` both venues (Win 183/0, WSL CARGO_EXIT=0); rustls-only clean; **Cargo.lock unchanged (zero new crates)**. **CI green on all 5 jobs** (`28076555268` — after a re-run of a transient `aarch64-unknown-linux-musl` cross flake: `ring`'s build couldn't find `aarch64-linux-musl-gcc`; identical deps to the green 6c run → re-run fixed it). **Residual (→ 6i):** `op_tool` invocations are not name-gated by `approval_required` (operator-vouched command; Remote principals still bounded by the frozen `allow_tools`).

### ✅ 6e — Providers: native Anthropic + operator model switch (`774e376..53eaf59`, CI `28094148385`) *(plan: `docs/superpowers/plans/2026-06-24-secretagent-phase6e-providers.md`)*
| Commit | What |
|---|---|
| `774e376` | **Native Anthropic Messages API provider** (`sa-providers/src/anthropic.rs`) — a 2nd `impl Provider`, NOT an OpenAI-compat shim. Translates the loop's OpenAI-format messages ↔ Messages API: top-level `system` (FIRST `{role:system}` only — injection guard), `tool_use`/`tool_result` content blocks (`tool_use_id`; result in a `user` msg right after the `tool_use`; consecutive same-role merged so no adjacent same-role messages), **`input_schema`** (not `parameters`), required `max_tokens`=4096, `x-api-key` header (never logged), `anthropic-version`=2023-06-01. Response `content[]` iterated by order, first `tool_use` wins. `chat` non-streaming single-chunk for v1. **Wire contract verified** against platform.claude.com (a 4-agent contract-verify workflow). |
| `8e5e312` | **`build_provider` = the single selection seam** returning `Box<dyn Provider>` (chosen by `provider.kind` openai\|anthropic; model = `model_for("execute")`). `ProviderConfig` gains `kind` (default openai, backward-compatible) + a minimal `RoleModels` map. `summarize.rs` routed through the seam (dropped its duplicate vault read). |
| `aa28f34` | **Operator-only `model` switch** — `secretagent model <name>` rewrites `[provider] model` via **`toml_edit`** (format-preserving, comments kept). Operator-only **by construction** (a CLI subcommand, never a registry tool → no Remote/cron principal can repoint it). NEW dep `toml_edit` pure-Rust (winnow) + musl-clean. |
| `53eaf59` | **5-lens adversarial-review fixes.** **M1 (real bug):** `schedule.rs add()` hardcoded `OpenAiCompat`, bypassing the seam → an anthropic-configured operator's scheduler built the wrong provider; now routes through `build_provider`. **H1:** error-path test proves `x-api-key` never appears in the error chain (locks the `error_for_status` secret policy). **M2:** `Provider::as_any` + a test that `build_provider` passes the per-role override model through. **L2:** unknown-role fallback asserted. |

**Acceptance MET:** a task runs against **Anthropic** (wiremock `act`/`chat` round-trips + header assertions prove the translation + wire format); `model <name>` **switches** (format-preserving config rewrite, next-load effect); a **Remote run can't repoint** the model (CLI-only, structural). **Adversarial review = 5-lens Workflow** (caught the M1 scheduler bug + the H1 secret-test gap). Gates: fmt/clippy(all-features) 0; `cargo test --all` both venues (Win 197/0, WSL CARGO_EXIT=0); rustls-only clean (`toml_edit` pulled no C deps); Cargo.lock committed. **CI green on all 5 jobs** (`28094148385`, first try). **Deferred (→ 6i):** real SSE streaming for Anthropic `chat` (single-chunk v1; the agentic `act` path is non-streaming); full per-role provider routing (the rejected "routing engine"); error-envelope message parsing (HTTP status is the v1 signal).

### ✅ 6f — Interactive reedline TUI (`a28f4f7`, CI `28131407225`)
| What | Detail |
|---|---|
| `secretagent/src/tui.rs` (bin-module, NOT a crate) | A `reedline` REPL: multiline (a backslash-continuation `Validator`), in-session history (reedline default), slash-command autocomplete (a `Completer` over `/help`,`/exit`,`/quit`). NOT ratatui — the §4.5 acceptance is a line editor. |
| Engine reuse | Each input runs through the **6a `assemble_agent` seam** (`build_agent` + `build_registry`) + `Agent::run_task` verbatim, as the **interactive operator** (`RunContext::operator(false)` — no approval UI yet, so side-effects are DENIED not auto-approved). A failed turn reports + continues (doesn't kill the REPL). |
| Testable logic | Pure helpers `classify` / `slash_suggestions` / `is_input_complete` carry the logic + are unit-tested (3 tests); the reedline event loop is a thin TTY-only shell (not unit-testable without a TTY). |
| Feature-gated | `tui` feature (default-on, mirrors `voice`) → a headless/server build drops reedline entirely (`--no-default-features` verified to build). |

**Acceptance MET:** the TUI drives a task end-to-end (input → `run_task` → printed reply) with the line-editor features (multiline/history/slash-autocomplete). Gates: fmt/clippy(all-features) 0; `cargo test --all` both venues (Win 200/0, WSL CARGO_EXIT=0); **rustls-only clean** (reedline → crossterm, pure-Rust, no C/openssl in the musl graph); Cargo.lock committed. **CI green on all 5 jobs** (`28131407225`, first try). **Deferred (→ 6i):** token-level streaming of the reply (`run_task` is non-streaming, like the Anthropic `chat` deferral — the chat path streams but lacks tools); an in-TUI approval prompt for side-effectful tools (today they're denied); persistent cross-session history (in-session only).

### ✅ 6g — Ops: backup/restore + secret-free trajectory export (`39570e0..f2050f1`) *(plan: `docs/superpowers/plans/2026-06-24-secretagent-phase6g-ops.md`)*
| Commit | What |
|---|---|
| `39570e0` | **`Store::backup_to`** via the SQLite **Online Backup API** (a consistent snapshot of a live WAL DB — never `cp` it) + **`looks_like_secret` made `pub`** (the canonical recognizable-secret detector, reused for export redaction). Enables the rusqlite `backup` feature (part of bundled SQLite — **no new crate**). |
| `1a98354` | **`Audit::read_events`** — a tolerant, secret-free event reader for the export (missing file → empty; torn final line tolerated like `open`; a mid-log parse error is still a hard error). |
| `777a9e9` | **`ops.rs` + 3 CLI subcommands + a `doctor` audit-chain line.** `backup <dir>` (DB snapshot + copy the encrypted `store.age`/`identity.age`/`audit.jsonl`); `restore <dir>` (copy back + verify the audit chain); `export [--session --out]` (messages+audit → JSONL, recognizable secrets redacted, **fail-closed** pre-write re-scan). |
| `f2050f1` | **4-lens adversarial-review fixes (14 verified findings).** **HIGH (data loss):** restore deletes stale `memory.db-wal`/`-shm` so SQLite can't replay an old WAL onto the restored DB. **MEDIUM:** the AKIA detector iterated only the first `akia` (`.find`) → `match_indices` loop (decoy-prefix bypass closed); backup/restore `chmod 600` **every** artifact + `chmod 700` the backup dir; honest `verify_chain` labeling (a bare chain can't detect a clean tail truncation); loud incoherent-{DB,vault}-set warning. **LOW:** redact provenance too; detect `age-secret-key-`; refuse a self-targeted backup/restore (a self-copy zeroes the vault). |

**Acceptance MET:** `backup` → `restore` round-trips a live DB (the vault travels as ciphertext, the identity is restored 0600, stale WAL sidecars are purged); `export` is secret-free (redact-then-rescan, fail-closed) — proven by the cross-process round-trip + secret-free + sidecar-removal + self-target-guard + (unix) perms tests. **Adversarial review = a 4-lens Workflow** (16 raw → 14 confirmed → fixed; 2 refuted by the verifiers). Gates: fmt/clippy(all-features) 0; `cargo test --all` both venues (Win + WSL musl, 31 ok suites each); rustls-only clean; **Cargo.lock unchanged (zero new crates — the rusqlite `backup` feature is part of bundled SQLite)**. **Accepted residuals (documented in `ops.rs`):** non-atomic per-file copy (recoverable by re-running from the intact backup); `export --out` overwrites its target (operator-trusted CLI); a torn-line re-append self-heal (pre-existing Phase-0); the cryptographic audit head-anchor for tail-truncation detection (the named integrity upgrade).

### ✅ 6h — Self-update (pinned-key verify, no-downgrade, atomic swap) (`0be9730..22203d1`) *(plan: `docs/superpowers/plans/2026-06-24-secretagent-phase6h-self-update.md`)*
The operator chose to **BUILD** it (over the ADR's DEFER default) with the full contract.
| Commit | What |
|---|---|
| `0be9730` | **`secretagent self-update [--check]`** — the full fail-closed chain: fetch `latest.json`+`.minisig` → verify the detached **minisign** signature against the pubkey **PINNED in the binary** (`const PINNED_MINISIGN_PUBKEY_B64`, never fetched, never config) → parse the manifest ONLY after verify → **no-downgrade** (version from the SIGNED payload, `ensure_upgrade`) → download the binary → its **sha256 must match** the signed manifest (`ensure_sha256`) → **atomic rename** → `append_synced` audit. Crypto = **`minisign-verify`** (ZERO transitive deps → musl-clean; the same minisign scheme as 6b); test-signing via the **`minisign` DEV-dep** (never in the binary graph), so the **negative-control tests are self-contained**: tampered-manifest / wrong-key / downgrade / sha256-mismatch ALL refused. Ships **inert/fail-closed** until the operator pins a key (the empty const errors). `build.rs` emits `SA_TARGET`; `[update] base_url` (config; the KEY is NOT config); `reqwest` (rustls, already in-tree) + `serde` added to the bin; a `doctor` line. |
| `617fdcf` | **`release.yml` emits + minisign-signs `latest.json`** (version from the tag + per-target `{url, sha256}` from the matrix artifacts) — the feed self-update verifies. `RELEASE.md` documents the operator-gated finish (pin the pubkey const + set `[update] base_url`) + the version discipline (bump `Cargo.toml` to match the tag). |
| `22203d1` | **5-lens RCE adversarial-review fixes (18 findings; the integrity chain HELD — every verifier confirmed parse-after-verify / no-downgrade / sha256-bind / operator-only / inert-until-pinned).** All were availability/hardening, not bypasses: **MEDIUM** bounded+timed download (a compromised mirror could OOM/disk-fill before verify → per-call streaming cap + timeout); **MEDIUM** Windows atomic-swap rollback (a failed 2nd rename no longer leaves the install path empty); **LOW** O_EXCL unpredictable staging temp (closes the symlink-clobber + the sha256→rename TOCTOU); audit moved BEFORE the irreversible swap. |

**Acceptance MET:** a **tampered** (bad signature OR bad binary hash), a **wrong-key**, and a **downgrade** update are ALL refused (negative-control unit tests); a genuine update verifies → no-downgrades → sha256-matches → swaps **atomically** → audits. The live network swap is **operator-gated** (pin the minisign key in the const + set `[update] base_url` + cut a signed release — the 6b precedent). Gates: fmt/clippy(all-features) 0; `cargo test --all` both venues (Win + WSL musl, 31 ok suites each); **binary purity clean** (`minisign-verify` zero-dep; reqwest already rustls); Cargo.lock committed. **Deferred (honest, → 6i):** delta/partial updates; staged rollout; auto-restart after swap (operator restarts); pre-release version ordering (a real release is x.y.z).

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

### ✅ 5b — Slack connector (Socket Mode) *(plan: `docs/superpowers/plans/2026-06-23-secretagent-phase5b-slack-connector.md`)*
| Commit | What |
|---|---|
| `9e3e8d6` | `ConnectorConfig.app_token_ref` — Slack needs TWO vault tokens: `token_ref` = xoxb- bot (chat.postMessage) + `app_token_ref` = xapp- app-level (Socket Mode). |
| `b53a7f7` | Pure `parse_envelope` + `map_message` (network-free, the Telegram/Discord unit-test precedent). **Identity = the `(team_id,user_id)` tuple** encoded `"<team>:<user>"` so M3 can't be spoofed cross-workspace; skips bot/own (`bot_id`) + edits (`subtype`) + empty. Adds `tokio-websockets` (rustls/webpki/`ring`, already in-tree via twilight → **no new license surface**) + the `slack` feature. |
| `5151e42` | `SlackConnector` recv/send: `apps.connections.open` (xapp-) → wss → envelope loop with **per-envelope ACK** → `map_message`; `chat.postMessage` (xoxb-) + `clamp_reply`. Reconnect on disconnect/close. Both tokens vault-held + never logged; the ticket-bearing wss URL + any URL-bearing error stripped. Tokens-never-leak test. |
| `9fa372f` | Gateway `"slack"` arm in `construct_connector` (loads both vault tokens) + bin enables `slack`. M3 dispatch test proves `allow_senders` keys on the full tuple, **rejecting a cross-workspace same-user-id imposter**. |
| `54b27ff` | **adversarial-review fixes** — HIGH: `ClientBuilder::uri`'s `InvalidUri` embeds the ticket-bearing URL → stripped via `build_socket_client` (+ test); HIGH: Socket Mode is at-least-once → dedup by `envelope_id` in a bounded ring (`note_envelope`, cap 256; + test) so a side-effect-armed redelivery never runs twice. |

**Transport = Socket Mode** (ADR-decided; an outbound WSS, no public inbound endpoint, fits the NAT'd daemon — so signing-secret HMAC is N/A). Reuses the 4c `Connector` trait + the M3 `dispatch_inbound` boundary + `RunContext::remote` **verbatim**. A **5-lens adversarial-review Workflow** (10 agents) ran before push: 4 confirmed (2 HIGH fixed: ticket-URL leak + redelivery dedup; 1 MEDIUM = the missing-test, added), 1 refuted by the review, and 1 HIGH ("unbounded `buf` DoS") re-classified as a false positive on inspection (`buf` is structurally ≤1; floods are TCP-backpressured). Both venues green; rustls/C-lib purity unchanged (no aws-lc-sys); `cargo deny` clean. **Acceptance (a)** (a Docker-backed `execute_code` driven from Slack) needs the operator-gated live E2E (Slack app + xapp-/xoxb- tokens), built testable-without and deferred — the Telegram/Discord/Email precedent.

### ✅ 5c — Subagent (`Principal::Subagent` + `subagent_of`) *(plan: `docs/superpowers/plans/2026-06-23-secretagent-phase5c-subagent.md`)*
| Commit | What |
|---|---|
| `b6d8599` | A 3rd `Principal::Subagent { parent: Box<RunContext> }` + `RunContext::subagent_of(&self)`. Side-effect authority **delegates** to the boxed parent (`Subagent::may_run_side_effect → parent.may_run_side_effect`) → **≤ parent, capped, never exceeds it**; but `may_persist` / `may_auto_activate_skill` / `is_operator` are hard-**false** and `provenance` is hard-**Untrusted** (or-worse) regardless of a Trusted parent. A bounded `depth: usize` on `RunContext` (`MAX_SUBAGENT_DEPTH = 2`) decrements per `subagent_of` (saturating). |
| `b452af2` | Spawn wired INTO `run_task` as a **synthetic, non-registry `subagent` tool-spec** (offered only while `depth > 0`), intercepted before the approval gate: re-enters `run_task` with `ctx.subagent_of()` (depth−1, authority ≤ parent), carrying the same `registry` so the sub-run can call `execute_code` (acceptance b). Async recursion is `Box::pin`'d. Fail-closed at the depth floor / on an empty task (`subagent.denied`). The answer returns as `Tainted::untrusted` re-fed as `role:"tool"` DATA — never an instruction. **No bin change** (run/gateway already pass a `RunContext`). |
| `4c3241d` | **adversarial-review fixes** — MEDIUM (amplification) + LOW (session-id collision): **remote/cron runs get `depth: 0`** so an untrusted message can't fan out subagents (zero authority gain, would only multiply cost) and never mints a `::sub.N` sub-session (closing the attacker-reachable collision). Only the attended operator fans out (depth 2, bounded). |

**The trust spine held:** a subagent is the parent's **equal for *doing*** (side-effects ≤ parent, by delegation) but strictly **less for *persisting*** (no durable write, no skill activation, Untrusted input) — proven by a runtime subset test over every tool + the live run_task tests. A **3-lens adversarial-review Workflow** (trust-escalation / fork-bomb-DoS / injection-leak) ran before push: **SHIP on all 3**, no CRITICAL/HIGH; the 2 confirmed MEDIUM+LOW (both fork-bomb-lens, both *bounded* not boundary-breaks) fixed in `4c3241d`. Both venues green; **no new dep** → rustls-only + musl-static unchanged; `cargo deny` unaffected. **Acceptance (b)** (a subagent runs a parallel pipeline via execute_code) is proven hermetically (`subagent_runs_execute_code_under_an_operator_parent`); a live operator run is the optional eyeball.

### ✅ 5d — Voice (feature-gated shell-out STT/TTS) *(/council **ADR-20260623-secretagent-phase5d-voice**; plan: `docs/superpowers/plans/2026-06-23-secretagent-phase5d-voice.md`)*
| Commit | What |
|---|---|
| `5032c63` | `VoiceConfig { stt_cmd, tts_cmd, allow_tools }` (operator-frozen `[voice]` argv templates + the frozen default-deny side-effect grant). |
| `46f2120` | `secretagent voice <input.wav>`: a feature-gated `voice` bin module that **shells out** (`Command::new`, **never `sh -c`**) — STT (audio path as final argv → transcript on stdout) → `run_task` as **`RunContext::remote("voice", source, allow_tools)`** (Untrusted, no-persist, no-auto-activate, default-deny side-effects, depth-0, **no `--yes`**) → TTS (answer on **stdin**, output wav at a **fixed `data_dir()` path**). Hardening: transcript length cap (`MAX_TRANSCRIPT`), audit (`voice.transcribe`) + doctor probe report **argv[0] only** (no secret leak), `--no-default-features` drops the whole surface. **Zero new deps.** |

**The /council decision (5-seat, 2-round, real cross-over):** FORK A → **A1** (operator argv templates, shell-out) — A2's cloud-enum arm would be an in-binary HTTP client violating the ADR's shell-out ruling (its author conceded). FORK B → **B2** (`Remote` trust model) — the decisive code trace showed `may_persist()` keys off the **Principal**, so B1 (`operator(false)`) would mint operator-attributed durable skills from untrusted machine-transcribed audio *every run*; B2 is consistent with the just-shipped 5c subagent treatment (machine-generated content = Untrusted + no-persist). FORK C → feature-gate + fixed-path `output.wav` + doctor-probe + no `--play`, plus unanimous hardening. **Focused `self-audit` before push: SHIP, zero real findings** (all 5 trust-laundering paths traced + refuted; argv/stdin-only spawn confirmed; argv[0]-only reporting confirmed). Both venues green; no new dep → rustls/musl/`cargo deny` unchanged. **Acceptance (c)** proven hermetically (`voice_ctx_is_untrusted_nonpersisting_and_default_deny` + the pure spawn/argv tests); a live whisper/piper round-trip is operator-gated.

---

## ✅ Phase 5 BUILD COMPLETE — all four slices shipped + CI-green
5a execution backends · 5b Slack connector · 5c subagent · 5d voice. **Operator-gated live tests remain** (the established precedent): live Slack E2E (acceptance a — Slack app + xapp-/xoxb- tokens), SSH backend live check (5a — an SSH host), live whisper/piper voice round-trip (5d — the binaries on PATH).

**Accepted residual (5c):** the **attended operator's** subagent fan-out is bounded but uncapped — worst case `1 + 8 + 64 = 73` `run_task` invocations (depth 2 × `MAX_TOOL_STEPS` fan-out). This is the operator's own attended run (a remote can't trigger it — depth 0); a **global per-run spawn budget** is the named upgrade if token cost ever bites.

**Accepted residuals (5d):** voice cannot learn skills (no-persist — the correct trade for attacker-influenceable audio; the safe seam for voice learning is an explicit transcript-confirm→re-issue step, deferred); cloud STT/TTS is operator-wrapped (a `curl`/vendor CLI in `stt_cmd`), no turnkey cloud arm; `--play`/auto-playback deferred (operator runs their own player); the `remote:voice:<source>` audit label uses the `remote:` *trust class* (untrusted-input), not network origin (documented in the ADR).

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
