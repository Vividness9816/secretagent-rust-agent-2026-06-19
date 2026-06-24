# SecretAgent — Slack Live E2E Runbook (Windows / PowerShell)

> **Readiness: VERIFIED.** The Socket Mode parser in `crates/sa-connectors/src/slack.rs` was validated
> field-by-field against the real current Slack API (a 6-agent validation Workflow). Every field path
> matches: `team <- payload.team_id` (confirmed present on a single-workspace `event_callback`, so
> `map_message` does NOT silently drop messages), `user/channel/text <- payload.event.{user,channel,text}`,
> and the `bot_id || subtype` skip (a bot/own echo carries BOTH `subtype="bot_message"` AND `bot_id`).
> The one robustness gap found — `chat.postMessage` returning HTTP 200 with `{ok:false}` — is **fixed**
> (commit `a882d26`, `check_post_ok`). No outstanding code work; this is purely operator setup.

Completing this run proves **Phase-5 acceptance (a)**: *a task runs in a Docker backend on a remote host driven from Slack.*

---

## A. Create + configure the Slack app

### A1. Create from a manifest
1. Go to <https://api.slack.com/apps> → **Create New App** → **From an app manifest**.
2. Pick your workspace, paste the JSON manifest below, **Next** → **Create**.

```json
{
  "display_information": { "name": "Secret Agent" },
  "features": {
    "bot_user": { "display_name": "Secret Agent", "always_online": true },
    "app_home": { "messages_tab_enabled": true, "messages_tab_read_only_enabled": false }
  },
  "oauth_config": {
    "scopes": {
      "bot": [
        "chat:write", "im:history", "im:write", "channels:history",
        "groups:history", "mpim:history", "app_mentions:read", "users:read"
      ]
    }
  },
  "settings": {
    "event_subscriptions": {
      "bot_events": [
        "message.im", "message.channels", "message.groups", "message.mpim", "app_mention"
      ]
    },
    "org_deploy_enabled": false,
    "socket_mode_enabled": true,
    "is_hosted": false,
    "token_rotation_enabled": false
  }
}
```

Scope → event map: `message.im`+`im:history` = read DMs; `message.channels`+`channels:history` =
public-channel messages; `chat:write` = post replies. (The other lines add private channels / group
DMs / @-mentions and are optional.)

### A2. Generate the app-level (`xapp-`) token — REQUIRED for Socket Mode
**Basic Information** → **App-Level Tokens** → **Generate Token and Scopes** → name it `socket` → add
scope **`connections:write`** → **Generate** → copy the **`xapp-...`** token (shown once). This is the
token `apps.connections.open` uses to open the WebSocket.

### A3. Confirm events + Messages tab
1. **Event Subscriptions** → **Enable Events** ON (with Socket Mode on, there is no Request-URL field)
   → confirm `message.im`, `message.channels` under **Subscribe to bot events** → **Save Changes**.
2. **App Home** → enable the **Messages Tab** and tick **"Allow users to send Slash commands and
   messages from the messages tab"** (needed to reliably receive DMs).

### A4. Install → get the bot (`xoxb-`) token
**OAuth & Permissions** → **Install to Workspace** → authorize → copy the **Bot User OAuth Token**
(`xoxb-...`). (Reinstall whenever you change scopes/events later.)

### A5. Let the bot see messages
- **DM:** search the bot's name in Slack and open a DM, **or**
- **Channel:** `/invite @Secret Agent` in a channel.

You now hold two secrets: **`xapp-...`** (Socket Mode) and **`xoxb-...`** (Web API). Both are needed at runtime.

---

## B. (Optional) Find your team_id + user_id — or just let the first reject reveal it (recommended)

The M3 allow-list keys on the **`<team_id>:<user_id>` tuple** (a bare user id will never match).

- **Team ID (`T...`):** open Slack in a browser; the URL `https://app.slack.com/client/TXXXXXXX/...`
  — the `T...` segment is the team id.
- **Your user ID (`U...`):** avatar → **View full profile** → **(...) More** → **Copy member ID**.
  (Note: the bot's `auth.test` returns the *bot's* user id, not yours — use Copy member ID.)

**Recommended instead:** leave a placeholder in `allow_senders`, DM the bot once, and read the exact
tuple from the audit log (section D2) — zero guesswork.

---

## C. Isolated env + vault + config (PowerShell)

Uses an isolated folder (its own vault/config), so your real config + the Telegram E2E vault are untouched.

**C1. Make the folder:**
```powershell
New-Item -ItemType Directory -Force "C:\Users\dnoye\sa-e2e-slack" | Out-Null
```

**C2. Init the vault:**
```powershell
$env:SECRETAGENT_DATA_DIR="C:/Users/dnoye/sa-e2e-slack"; $env:SECRETAGENT_CONFIG_DIR="C:/Users/dnoye/sa-e2e-slack"; & "C:\Users\dnoye\ClaudeSecondBrain\SecretAgent\target\release\secretagent.exe" vault init
```

**C3. Store BOTH tokens — prefix each line with `!` so the token stays out of the chat transcript**
(`vault set` prints only `set <key>`, never the value). Replace the PASTE_ placeholders:
```powershell
! $env:SECRETAGENT_DATA_DIR="C:/Users/dnoye/sa-e2e-slack"; & "C:\Users\dnoye\ClaudeSecondBrain\SecretAgent\target\release\secretagent.exe" vault set SLACK_BOT_TOKEN "PASTE_XOXB"
```
```powershell
! $env:SECRETAGENT_DATA_DIR="C:/Users/dnoye/sa-e2e-slack"; & "C:\Users\dnoye\ClaudeSecondBrain\SecretAgent\target\release\secretagent.exe" vault set SLACK_APP_TOKEN "PASTE_XAPP"
```

**C4. Write `C:\Users\dnoye\sa-e2e-slack\config.toml`** (leave the `allow_senders` placeholder; D2 fills it):
```toml
[provider]
base_url = "http://localhost:11434/v1"
model    = "hermes3:latest"          # MUST support tool/function calling

[exec]
backend = "docker"
image   = "alpine"                   # required for the docker backend

[[connectors]]
name          = "slack-main"
kind          = "slack"
token_ref     = "SLACK_BOT_TOKEN"    # vault key-id for the xoxb- bot token
app_token_ref = "SLACK_APP_TOKEN"    # vault key-id for the xapp- app-level token (Socket Mode)
allow_senders = ["T00000000:U00000000"]   # the <team>:<user> TUPLE — fill via D2
allow_tools   = ["execute_code"]     # frozen grant: lets the model run code IN Docker (--network=none)
```

**Preconditions** (all verified present on this machine): Docker daemon up + `alpine` pulled
(`docker pull alpine`); Ollama at `localhost:11434` with `hermes3:latest`; the Slack app installed.

---

## D. Run, DM the bot, verify

**D1. Start the gateway** (foreground, info logs):
```powershell
$env:SECRETAGENT_DATA_DIR="C:/Users/dnoye/sa-e2e-slack"; $env:SECRETAGENT_CONFIG_DIR="C:/Users/dnoye/sa-e2e-slack"; $env:RUST_LOG="info"; & "C:\Users\dnoye\ClaudeSecondBrain\SecretAgent\target\release\secretagent.exe" gateway
```
Expect `gateway: 1 connector(s) running`. That count is the number of connectors that *actually
started* — if a token is missing/wrong you'll see `0 connector(s) running` plus a `failed to start …
skipped` warn line naming the reason.

**D2. Capture your tuple from the first reject.** DM the bot once. The message is denied (no reply)
and `C:\Users\dnoye\sa-e2e-slack\audit.jsonl` gains a line:
```
"action":"connector.rejected" ... "principal":"remote:slack-main:T01ABCD:U05WXYZ"
```
Strip the `remote:slack-main:` prefix → `T01ABCD:U05WXYZ` is your exact tuple. Paste it into
`allow_senders`, Ctrl-C, and restart (D1).

**D3. Verify the happy path.** DM the bot from the allow-listed account, e.g.
*"run `echo hello from docker` in the sandbox"*. You get a reply in Slack. `audit.jsonl` shows, in order:
- `"action":"connector.accepted" ... "principal":"remote:slack-main:T...:U..."` — M3 passed the tuple before any agent work.
- `"action":"tool.execute_code"` — the frozen grant let the **Docker-confined** tool run (`docker run --network=none alpine`).
- the reply posted back via `chat.postMessage`.

Full chain: Slack DM → Socket Mode envelope ACK'd (~3s) → `map_message` → M3 accept → `run_task` as
Remote with `["execute_code"]` → code runs in Docker (`--network=none`) → `chat.postMessage` reply.

**D4. (Optional) Prove M3 default-deny.** A message from a non-allow-listed tuple (e.g. the same user
id in another workspace) shows `connector.rejected` and gets no reply — code-proven by
`slack_tuple_identity_is_what_m3_allow_lists` in `gateway.rs`.

---

## Notes (non-blocking)
- A **read-only** Slack bot = `allow_tools = []` (it answers but runs no side-effect tools).
- A Remote run writes **no** durable memory (M2, by design) — Slack tasks don't compound skills/prefs.
- `chat.postMessage` logical failures (e.g. bot not in the channel) now surface as an error
  (`not ok: not_in_channel`) instead of a silent drop — make sure the bot is DM'd/invited (A5).
- The `sa-e2e-slack/` folder holds the vault + audit; it lives under your home dir, outside the repo,
  so it is never committed.
