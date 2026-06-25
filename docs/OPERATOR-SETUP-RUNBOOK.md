# SecretAgent — Operator setup & turnover runbook

> **Paste this whole file into a fresh session to continue.** It is self-contained.
>
> **For the fresh session:** SecretAgent is a clean-room Rust agent daemon at
> `C:\Users\dnoye\ClaudeSecondBrain\SecretAgent` (private remote
> `Vividness9816/secretagent-rust-agent-2026-06-19`, branch `master`). **All code is built and
> CI-green (Phases 0–6 complete).** Nothing is broken. What remains is **operator-gated setup** —
> credentials, hosts, and external apps that only the operator can provide. This runbook lists every
> outstanding task in simple steps. Your job is to help the operator work through them and debug if a
> step's "Verify" doesn't match. Read `docs/parity-tail.md`, `PROGRESS.md`, and the project memory for
> full context.

The tasks, in order:
1. **Slack** (live messaging connector) — the operator's priority.
2. **SSH execution backend** — run code on a remote host.
3. **Voice** — whisper/piper round-trip.
4. **Live Anthropic** — use a cloud model with a real key.
5. **(Verify) Release + self-update** — operator indicated this is done; confirm it.
6. **Just-use-it config** — the minimum to run SecretAgent day-to-day (do this if you skip the daemon).

**None of these are required for SecretAgent to function** — the proven baseline (local Ollama + the
vault + Docker `execute_code` + the **already-live Telegram connector**) works today. These add
capabilities. Do the ones you want.

---

## Before you begin (do this once)

1. **Get the binary.** Either it's on your PATH as `secretagent` (from the installer), or build it:
   ```powershell
   cd C:\Users\dnoye\ClaudeSecondBrain\SecretAgent
   cargo build --release
   ```
   The binary is then `C:\Users\dnoye\ClaudeSecondBrain\SecretAgent\target\release\secretagent.exe`.

2. **Pick ONE folder** to hold your config + vault + audit, and set two env vars **at the top of your
   PowerShell window**. Keep using that same window for a task — if you open a new window, re-run this
   block first. (This is the #1 source of "it can't find my key" errors: the vault and the config must
   point at the same folder for every command.)
   ```powershell
   $SA   = "C:\Users\dnoye\ClaudeSecondBrain\SecretAgent\target\release\secretagent.exe"  # or just "secretagent"
   $env:SECRETAGENT_DATA_DIR   = "C:\Users\dnoye\secretagent"   # vault + memory.db + audit.jsonl live here
   $env:SECRETAGENT_CONFIG_DIR = "C:\Users\dnoye\secretagent"   # config.toml is read from here
   New-Item -ItemType Directory -Force $env:SECRETAGENT_DATA_DIR | Out-Null
   & $SA vault init     # creates the encrypted vault (identity.age + store.age). Safe to re-run.
   ```
   Your config file is `C:\Users\dnoye\secretagent\config.toml`. Each task below adds a block to it.

3. **The `!` trick for secrets.** When a step says `vault set`, **type `!` then a space first**, e.g.
   `! & $SA vault set KEY "value"`. This runs the command in this session so the secret you paste
   stays out of the chat. `vault set` only ever prints `set <KEY>`, never the value.

4. **Sanity check** any time: `& $SA doctor` — it prints a status line per subsystem and always exits
   0 (the lines are informational, never a pass/fail gate).

---

## TASK 1 — Slack (live messaging connector)

**Goal:** DM the bot from Slack → it runs a task (e.g. code in a Docker sandbox) → it replies in Slack.

### 1a. Prerequisites
- Docker Desktop running, and pull the sandbox image: `docker pull alpine`
- A model that can call tools. Easiest: your homelab **Ollama** with `hermes3:latest` running on
  `http://localhost:11434`. (A model without tool-calling will reply in text but never run code.)
- A Slack workspace where you can create an app.

### 1b. Create the Slack app + get TWO tokens
1. Go to **https://api.slack.com/apps → Create New App → From scratch** → pick your workspace.
2. **OAuth & Permissions → Scopes → Bot Token Scopes**, add: `chat:write`, `im:history`,
   `channels:history`, `app_mentions:read`, `users:read`.
3. **Socket Mode → toggle ON.**
4. **Event Subscriptions → Enable Events ON.** Under **Subscribe to bot events**, add `message.im` and
   `message.channels`. Save. (With Socket Mode on, there's no Request-URL to fill in.)
5. **App Home → Show Tabs →** enable the **Messages Tab**, and tick *"Allow users to send Slash
   commands and messages from the messages tab."*
6. **Basic Information → App-Level Tokens → Generate Token and Scopes** → name it `socket`, add the
   scope **`connections:write`**, Generate → **copy the `xapp-…` token** (this is the *app-level* /
   Socket-Mode token; shown once).
7. **OAuth & Permissions → Install to Workspace → Allow** → **copy the `xoxb-…` Bot User OAuth Token.**
8. In Slack, **open a DM with your new bot** (or `/invite` it into a channel) so it's allowed to post.

### 1c. Store the two tokens (key names must match the config below)
```powershell
! & $SA vault set SLACK_BOT_TOKEN "xoxb-PASTE-BOT-TOKEN"
! & $SA vault set SLACK_APP_TOKEN "xapp-PASTE-APP-TOKEN"
```

### 1d. Add to `config.toml`
```toml
[provider]
base_url = "http://localhost:11434/v1"   # Ollama; kind defaults to "openai"
model    = "hermes3:latest"              # must support tool calling

[exec]
backend = "docker"                        # runs code as: docker run --network=none alpine
image   = "alpine"

[[connectors]]
name          = "slack-main"
kind          = "slack"
token_ref     = "SLACK_BOT_TOKEN"         # the xoxb- token (posts replies)
app_token_ref = "SLACK_APP_TOKEN"         # the xapp- token (Socket Mode)
allow_senders = ["FILL_ME_IN"]            # WHO may use the bot — filled in step 1f
allow_tools   = ["execute_code"]          # the only side-effect the Slack run may use
```

### 1e. Start the gateway
```powershell
$env:RUST_LOG = "info"; & $SA gateway
```
You want to see **`gateway: 1 connector(s) running`**. If you see `0 connector(s) running` plus a
`connector 'slack-main' failed to start: … — skipped` warning, it names the reason (wrong/missing
token, missing `app_token_ref`, etc.) — fix and restart.

### 1f. Authorize yourself ("reject-capture" — don't guess your ID)
`allow_senders` is an exact match on the `<team_id>:<user_id>` **tuple**, which you can't guess. Capture it:
1. From Slack, **DM the bot once.** Nothing happens (you're not allow-listed yet).
2. Open `C:\Users\dnoye\secretagent\audit.jsonl`, find the newest line with
   `"action":"connector.rejected"` and a `principal` like `remote:slack-main:T01ABCD:U05WXYZ`.
3. **Strip the `remote:slack-main:` prefix** → `T01ABCD:U05WXYZ` is your tuple.
4. Put it in `allow_senders` (replace `"FILL_ME_IN"`), **Ctrl-C the gateway, and start it again** (1e).

### 1g. Verify (the live test passes)
DM the bot something that uses the tool, e.g. **"run `echo hello from docker` in the sandbox."**
- You get a reply in Slack.
- `audit.jsonl` shows, in order: `connector.accepted` (principal `remote:slack-main:T…:U…`) →
  `tool.execute_code` → the reply is posted.

### 1h. If it breaks
- **No reply, error `not_in_channel`:** the bot isn't in the conversation — DM it or `/invite` it.
- **`connector.accepted` but no `tool.execute_code`:** the model didn't emit a tool call — use a
  tool-calling model (`hermes3`) or a cloud model (Task 4).
- **`0 connector(s) running`:** read the `… failed to start: …` warning; usually a token typo or the
  `xapp-` token is missing the `connections:write` scope.
- The bot never compounds memory from Slack (a remote run writes no skills/prefs) — that's by design.

---

## TASK 2 — SSH execution backend

**Goal:** the agent's `execute_code` runs the snippet on a **remote host** over SSH instead of locally.

### 2a. Prerequisites
- **Passwordless, key-based SSH** from this machine to the host. Test it — it must NOT prompt:
  ```powershell
  ssh user@remote-host true
  ```
- The host must already be in `known_hosts` (a first-connect host-key prompt will hang the run, because
  there's no terminal to answer it). `ssh` must be on PATH; the host needs a POSIX `/bin/sh`.
- **No vault entry is needed** — SSH auth uses your `~/.ssh` / ssh-agent, not a config secret.

### 2b. Add to `config.toml` (replace any existing `[exec]` block)
```toml
[exec]
backend = "ssh"
host    = "user@remote-host"   # passed verbatim to `ssh`; a ~/.ssh/config Host alias works too
```
Keep a `[provider]` with a tool-calling model (the Ollama block from Task 1 is fine).

### 2c. Run
`execute_code` is side-effectful, so you must pass `--yes` (without it the agent denies the call):
```powershell
& $SA doctor
& $SA run "run the shell command 'hostname; uname -a' using execute_code" --yes
```

### 2d. Verify
- `doctor` shows: `[ok]   exec backend: ssh:user@remote-host (remote …, operator-vouched; egress NOT
  confined by us)`. (The "egress NOT confined" wording is honest — see 2e.)
- The task's answer contains the **remote** host's `hostname`/`uname` (use a host whose name differs
  from your box so it's obvious it ran remotely).
- `audit.jsonl` has an `exec.backend` event (`key_id` `ssh:user@remote-host`) **and** a
  `tool.execute_code` event.

### 2e. Know this (by design, not a bug)
- On SSH, the **host's** policy governs the filesystem and network — our in-process file/egress limits
  are **not** enforced remotely (unlike Docker, which mounts the roots + `--network=none`). The
  `doctor` line says so plainly.
- **No environment is forwarded to the remote shell** (so your secrets never reach untrusted model
  code). A missing `ssh` CLI or unreachable host **fails closed** (no silent local fallback).

---

## TASK 3 — Voice (whisper + piper round-trip)

**Goal:** `secretagent voice in.wav` → transcribe → run the task → speak the answer to a wav file.

### 3a. Prerequisites — the two binaries (the contracts are strict)
- **Speech-to-text** must print **only the transcript to stdout.** Vanilla `openai-whisper` writes
  files and chatters progress — it will NOT work as-is. Use **whisper.cpp** (`whisper-cli`), or wrap
  your STT in a tiny script whose only stdout is the transcript text.
- **Text-to-speech:** **piper** (reads the text on **stdin**, takes the output path as its **last**
  argument — fits perfectly) + a voice model (`*.onnx`).

### 3b. Add to `config.toml`
```toml
[voice]
# STT: the audio path is appended LAST; the transcript MUST be printed to stdout.
stt_cmd = ["whisper-cli", "--output-txt", "-otxt", "-"]
# TTS: the answer text is fed on STDIN (never as an argument); the output wav path is appended LAST.
tts_cmd = ["piper", "--model", "C:/path/to/en_US-amy-medium.onnx", "--output_file"]
# Default-deny: a voice transcript can run NO side-effectful tool unless you list it here.
allow_tools = []
```
Keep a `[provider]` block (Ollama or Task 4's Anthropic).

### 3c. Run
```powershell
& $SA doctor          # want: [ok] voice stt: whisper-cli on PATH  and  [ok] voice tts: piper on PATH
& $SA voice .\in.wav
```

### 3d. Verify
- The reply **text prints to your console first** (so a broken TTS can't hide the answer), then
  `[voice] reply audio: C:\Users\dnoye\secretagent\voice-out.wav`.
- That wav exists at `…\voice-out.wav` (a fixed path; overwritten each run).
- `audit.jsonl` gains a `voice.transcribe` event whose `key_id` is just the STT **program name** and
  whose `principal` is `remote:voice:in.wav`.

### 3e. Know this (by design)
- The commands are run **argv-only, never via a shell** — no pipes/globs/`&&` inside the templates;
  use a wrapper script if you need shell logic.
- A voice transcript is treated as **untrusted** (like an inbound message): no `--yes`, can't create
  skills, and runs a side-effect tool **only** if its name is in `allow_tools`. The transcript is
  capped at 8192 characters.

---

## TASK 4 — Live Anthropic (cloud model)

**Goal:** run a task against the native Anthropic provider with a real API key.

### 4a. Add to `config.toml` (replace any existing `[provider]` block)
```toml
[provider]
kind = "anthropic"                 # selects the native Anthropic provider (default is "openai")
model = "claude-opus-4-8"          # a REAL claude id — there is NO anthropic default; you MUST set it
api_key_ref = "ANTHROPIC_API_KEY"  # the vault key name (not the secret)
# Do NOT set base_url — the anthropic provider ignores it and always uses https://api.anthropic.com.
```

### 4b. Store the key
```powershell
! & $SA vault set ANTHROPIC_API_KEY "sk-ant-PASTE-YOUR-KEY"
```

### 4c. Run
```powershell
& $SA run "What is 2 + 2? Reply with just the number."
& $SA model claude-haiku-4-5     # OPTIONAL: switch model later without editing the file; next run uses it
```

### 4d. Verify
- The run prints the model's answer and exits 0.
- **Secret-non-leak check** (the security half): the key must not be anywhere in the audit log —
  ```powershell
  Select-String -Path C:\Users\dnoye\secretagent\audit.jsonl -Pattern 'sk-ant'   # should find NOTHING
  ```

### 4e. Know this (avoids the two common confusions)
- **`doctor`'s "provider endpoint not reachable" is EXPECTED here and does NOT mean Anthropic is down.**
  `doctor` probes `base_url`, which the anthropic provider ignores (it shows the unused Ollama
  default). Verify with an actual `run`, not with `doctor`.
- If you set `kind="anthropic"` but **forget `model`**, it sends `llama3.2` and the API returns a 400.
  Always set a real claude id.

---

## TASK 5 — (Verify) Release + self-update *(you indicated this is done)*

You said this is complete. Confirm it actually published + armed self-update:
1. **GitHub Actions:** the `release` workflow ran green on your `v…` tag, and the release has the
   binaries + `SHA256SUMS` + `SHA256SUMS.minisig` + `latest.json` + `latest.json.minisig`.
2. **The key is pinned:** `secretagent/src/self_update.rs` → `PINNED_MINISIGN_PUBKEY_B64` is **your real
   minisign public key**, not empty. (And `install.sh`'s `MINISIGN_PUBKEY` is the same key.)
3. **`[update] base_url`** is set in `config.toml`, e.g.
   `base_url = "https://github.com/<owner>/<repo>/releases/latest/download"`.
4. **Doctor + dry-run:**
   ```powershell
   & $SA doctor          # want: [ok] self-update: configured (pinned key + base_url)  — NOT "inert"
   & $SA self-update --check
   ```
   If `doctor` says **`self-update: inert`**, the key isn't pinned yet — finish per `docs/RELEASE.md`
   (it's still safe; self-update just won't run until pinned).

---

## TASK 6 — Just-use-it config (do this if you skip Slack/the daemon)

To run SecretAgent day-to-day **without** any connector, you need only a provider — **but every tool is
default-deny until you open it up.** Here's a complete working `config.toml` for a Windows box with
Docker + local Ollama:

```toml
[provider]
base_url = "http://localhost:11434/v1"
model    = "hermes3:latest"            # default is llama3.2 — use a tool-calling model

[policy]
read_roots   = ["C:/Users/dnoye/agent-workspace"]   # read_file refuses everything outside these
write_roots  = ["C:/Users/dnoye/agent-workspace"]   # write_file likewise
egress_allow = ["api.github.com", "example.com"]    # exact hosts fetch/web_extract may reach

[exec]
backend = "docker"     # see the Windows note below
image   = "alpine"
```
Run a task (`--yes` lets it use side-effectful tools like write_file / execute_code):
```powershell
& $SA run "create hello.txt in the workspace with the text 'hi'" --yes
& $SA chat "remember that my cat is named Mochi"     # plain chat, streams, remembers across runs
```

> **Windows note (the one thing that looks broken but isn't):** on Windows/macOS, `execute_code` is
> **refused** unless `[exec] backend="docker"` (or `ssh`). The local sandbox (landlock) is **Linux-only**,
> so `doctor` shows `landlock: not applicable … execute_code disabled (expected)`. With Docker set,
> it works. On a **Linux** VPS — the project's real deployment target — `backend="local"` works
> natively and needs no Docker.

---

## Optional next steps (when you're ready)
- **Run it as an always-on agent:** with a connector configured (your **Telegram bot already works**),
  `secretagent gateway` runs the daemon, and `secretagent service install` (run elevated) makes it
  start on boot. The NL→cron scheduler (`secretagent schedule add "…"`) rides on the gateway.
- **Move to the real target:** the binary is a single musl-static Linux file — the intended home is a
  headless VPS, where `backend="local"` (landlock) sandboxes code with no Docker needed.
