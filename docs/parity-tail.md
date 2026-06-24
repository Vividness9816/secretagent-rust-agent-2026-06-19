# SecretAgent — Parity tail (what shipped, what's deferred, and why)

> The honest **§4 acceptance amendment (Pillar C)** for the Phase-6 "parity v1" milestone, per
> **ADR-20260623-secretagent-phase6-milestone**. The spec's §4 *Parity Inventory* (≈60 tools, 20+
> connectors, voice, learning loop, …) is the destination; this document states plainly which of it
> SecretAgent reaches **by mechanism**, which is shipped **bespoke**, and which is **deferred behind
> an already-built trait** with the trigger to build it. A short, accurate map beats a padded
> "60+ tools, all green" that rots.

## The framing: parity **by mechanism**, not a tool count

SecretAgent does **not** ship 60 hand-written tools, and the literal "§4 all green" was rejected
(it trades 60 CI-green obligations for honesty and contradicts the project's own defer-the-tail
precedent). Instead, parity is reached three ways:

1. **MCP client** (Phase 2c) — the agent loads any [Model Context Protocol] server's tools,
   namespaced + allow-listed. *Most* of the §4 long tail is reachable here without new first-party code.
2. **`op_tool`** (Phase 6d) — an operator-frozen external-command template (program + flags + host
   frozen; the model fills only a data arg; argv-only, never `sh -c`; stdout `Tainted`). The escape
   hatch for vision / image-gen / TTS / a browser CLI **without** pulling a C library into the
   musl-static binary or opening a model-chosen-command hole.
3. **A curated bespoke set** — the high-value tools built natively because they earn their place:
   `fetch` / `web_search` / `web_extract` / `http_request` (through the one egress seam),
   `read_file` / `write_file`, `execute_code` (landlock / Docker / SSH backends), `shell`.

So "does SecretAgent have tool X?" is usually "yes — via MCP or an `op_tool`," not "only if we wrote it."

## What SHIPPED in Phase 6 (6a–6h)

| Slice | Shipped |
|---|---|
| 6a | The `assemble_agent` refactor (one `setup::{build_provider,build_agent,build_registry}` seam). |
| 6b | Release packaging: `release.yml` (checksums + **minisign** signature + optional Dylan-N Authenticode + distroless multi-arch container + a verify-before-place installer); `doctor` binary-integrity line. |
| 6c | The single egress-guarded HTTP seam (`egress_get`) + `web_search`/`http_request`/`web_extract`; **fixed a live `Fetch::run` SSRF**. |
| 6d | `shell` (via the sandbox) + the generic operator-frozen `op_tool`. |
| 6e | Native **Anthropic** Messages-API provider + the `build_provider` selection seam + an operator-only `model` switch + a minimal per-role model map. |
| 6f | A `reedline` TUI (line editor: multiline / history / slash-autocomplete). |
| 6g | `backup` / `restore` (SQLite Online Backup API; the vault stays encrypted) + a secret-free `export`. |
| 6h | `self-update` (pinned-key minisign verify → no-downgrade → sha256 → atomic swap → audit). |

Earlier phases shipped the rest of the spine: the vault + hash-chained audit (P0), memory + FTS5 +
the agentic loop + providers (P1), tools + the landlock sandbox + the `Tainted` injection guard +
the MCP client (P2), the skills/user-model/summarization learning loop (P3), the daemon + service
install + 4 connectors (Telegram/Discord/Email/Slack) + the NL→cron scheduler + the Principal/RunContext
trust spine (P4), and execution backends + subagents + CLI voice (P5).

## DEFERRED — behind which trait, and the trigger to build it

Each of these is **not** missing-by-accident; the *mechanism* to add it already exists, so it lands
the moment a real need appears. (Source: ADR §Revisit + the per-slice residuals.)

| Deferred capability | Behind which built mechanism | Trigger to build |
|---|---|---|
| Browser automation (chromiumoxide) | An `op_tool` shell-out to an operator Chromium | A task `fetch`+`web_extract` provably can't serve **and** a rustls-only headless path is verified (musl + exfil + unverified-TLS are why it's not in-process). |
| In-process vision / image-gen / audio (C libs) | `op_tool` shell-out to the operator's CLI | A real workflow needs it; keeps the binary C-free / musl-static. |
| Serverless / HPC exec backends (Daytona / Singularity / Modal) | The closed `sa_exec::Backend` enum | A real user of one — add an arm (the Docker/SSH precedent). |
| The ~16 remaining messaging connectors | The `Connector` trait + the M3 boundary | A user on that platform — a new per-file connector (the Slack precedent). |
| Skills Hub / agentskills.io sync | The SQLite-canonical skills store | A real sync endpoint exists. |
| macOS notarization | The signing pipeline | An Apple Developer account exists / a macOS user hits Gatekeeper. |
| Multi-model **routing engine** | The minimal `[provider.models]` per-role map (shipped) | A real routing need beyond per-role config (deliberately not a routing engine). |
| Per-tool rate limits / an egress-allow DSL | The pure `Policy` value type | A real abuse/scale need. |

### Per-slice deferrals (documented residuals, all in-spirit)
- **6d** — `op_tool` is not `approval_required`-name-gated (operator-vouched; a Remote run is still bounded by its frozen `allow_tools`).
- **6e** — Anthropic SSE streaming (`chat` is single-chunk v1; the agentic `act` path is non-streaming); error-envelope message parsing (HTTP status is the v1 signal).
- **6f** — token-level streaming of the reply; an in-TUI approval prompt for side-effectful tools (today denied); persistent cross-session history.
- **6g** — a non-atomic per-file restore copy (recoverable by re-running from the intact backup); `export --out` overwrites its target (operator-trusted CLI); a torn-line re-append self-heal (pre-existing Phase-0); a **cryptographic audit head-anchor** so `verify_chain` can detect a clean tail truncation (today it detects a torn tail / reorder / mutation, not a clean truncation — stated honestly in the code + `doctor`).
- **6h** — delta/partial updates; staged rollout; auto-restart after the swap (the operator restarts); pre-release version ordering (a real release is `x.y.z`).

## Operator-gated live steps (built testable-WITHOUT; the live step is the operator's)

These are **done in code + tested hermetically**; only the live credential/host/key step remains —
the established Phase-4/5 precedent (build it provable-without, defer the live leg).

- **Release + self-update:** generate the minisign keypair (`minisign -G -W`), pin the pubkey in
  `install.sh` **and** in `self_update.rs` (`PINNED_MINISIGN_PUBKEY_B64`), add `MINISIGN_SECRET_KEY`
  as a GH secret, set `[update] base_url`, bump `Cargo.toml` to the tag, cut a `v*` tag. (See `docs/RELEASE.md`.)
- **Carried from prior phases:** live Slack E2E (a Slack app + `xapp-`/`xoxb-` tokens); the SSH exec
  backend (an SSH host); the whisper/piper voice round-trip (the binaries on PATH); a live Anthropic
  task (a real API key). Live Telegram E2E is **already proven** (2026-06-23).

## §4 acceptance — the honest statement (Pillar C)

Phase 6 does **not** close the literal §4 inventory, and that is the intended outcome. The milestone
delivers:

- a **signed, checksummed, reproducible-by-CI release** that `doctor` verifies on a fresh box;
- a **curated, hardened** tool / provider / connector / ops / self-update surface;
- **arbitrary-tool reach** via MCP + `op_tool` for everything else;
- every deferral **named, with the trait it lands behind and its trigger** (the tables above).

This is **more in-spirit with §4 than a padded "60+ all green"** would be: it is honest about what is
load-bearing today, it never claims a capability it can't stand behind, and it leaves a clear, cheap
path to each deferred item. That honesty *is* the Pillar-C acceptance.
