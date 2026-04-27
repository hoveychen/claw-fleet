# Feishu (Lark) Integration — Decision Panel Mirror

Status: design — not yet implemented.

## Goal

Mirror the desktop Decision Panel surface to Feishu so the workspace owner can
respond to **Guard**, **Elicitation**, and **Plan-Approval** decisions from a
phone or desktop Feishu client, without running the fleet desktop app on the
device that received the alert.

## Non-goals

- A pluggable multi-IM abstraction. This design is Feishu-specific. Slack /
  Teams ports would reuse the trait surface but ship as their own modules.
- Strict feature parity with the local Decision Panel. A handful of
  capabilities (TTS, side-by-side option preview, per-event chimes) cannot be
  expressed in Feishu cards; documented fallbacks below.
- Mobile fleet client changes. The mobile client already gets Decision events
  through `RemoteBackend`; the Feishu bridge runs in parallel and does not
  alter mobile behavior.

## Background: Decision Panel capability inventory

Source: [DecisionPanel.tsx](../claw-fleet-desktop/app/components/DecisionPanel.tsx),
[useDecisionEvents.ts](../claw-fleet-desktop/app/hooks/useDecisionEvents.ts),
[store.ts](../claw-fleet-desktop/app/store.ts),
[audio.ts](../claw-fleet-desktop/app/audio.ts).

The feature surface has 12 distinct capabilities:

1. Markdown-rich question body
2. Single / multi-select options (mode switch at runtime)
3. Per-option `label` + `description` + side-panel markdown `preview`
4. "Other" free-text fallback (resizable textarea)
5. File / image attachments (clipboard paste + file picker, ≤50 MB, thumbnails)
6. Multi-step questionnaire (dot-indicator step navigation)
7. Plan inline-edit textarea (Approve / Approve Edited / Reject + reject feedback)
8. Guard risk tag + asynchronous LLM analysis panel
9. TTS narration (`---` divider splits summary / detail)
10. Per-event chimes (Guard → drop, Elicitation → ding_dong)
11. Cross-device dismiss (one client responds, others auto-clear)
12. Backend timeout polling → auto-clear

## Coverage matrix

| Capability | Native in Feishu | Strategy |
|---|---|---|
| Markdown body | yes (Card 2.0 `markdown` element) | direct |
| Single / multi select | yes (`select_static`, `multi_select_static`) | direct |
| Free text "Other" | yes (`input` element) | direct |
| Attachments | partial (no `file_input` in cards) | out-of-band: link to a fleet-cli upload page; webhook updates card with attachment count |
| Multi-step | yes (re-render whole card via `update_card`) | each step is a new card schema; progress in `header.subtitle` |
| Plan inline edit | yes (`input` with `multiline=true`) | direct |
| Guard risk tag | yes (`header.template=red`, `tag` element) | direct |
| LLM analysis panel | yes (`collapsible_panel`) | direct |
| TTS narration | no | desktop-only; phone receives the message without voice |
| Per-event chimes | no | Guard upgraded to **urgent_app** (Feishu-only, force pop-up + vibrate); Elicitation uses default notification |
| Cross-device dismiss | yes (`update_card` reaches all sessions of the same `message_id`) | direct |
| Auto-expire on timeout | yes (`update_card` to a "expired" rendering) | server-side polling already exists; same hook drives the Feishu update |
| Side-by-side preview | no (cards are linear) | render preview inside `collapsible_panel` after each option's description |

## Architecture

```
fleet-cli (Rust)              Feishu Open Platform        老板 phone / desktop Feishu
     |                                |                              |
     |-- 1. Decision created --------» POST /im/v1/messages --» [card appears]
     |   (Guard | Elicitation | Plan)   (msg_card payload)
     |                                                                |
     |«-- 2. webhook event «---------- card.action.trigger «-- [user clicks]
     |     (parse action.value)
     |
     |-- 3. respond_to_xxx (existing Backend trait)
     |
     |-- 4. update_card -----------» PATCH /im/v1/messages/{id} --» [card -> readonly summary]
         (cross-device dismiss)
```

The Feishu bridge is an additive module on `fleet-cli`. It does not change
`LocalBackend` semantics. It hooks into the existing Decision event bus, so
adding the bridge should not require touching Decision creation or response
code paths.

Per [project CLAUDE.md](../CLAUDE.md), every new capability supports both
`LocalBackend` and `RemoteBackend`. Feishu bridge runs on the fleet-cli side
in both cases:

- **LocalBackend deployment**: fleet-cli runs on the same machine as the
  desktop. `app_secret` lives in local env / config file. Only the local user
  reads it.
- **RemoteBackend deployment**: fleet-cli runs on the user's own remote
  server. `app_secret` lives in server env. Same trust boundary as any other
  fleet HTTP secret.

## Module breakdown (`fleet-cli/src/feishu/`)

```
fleet-cli/src/feishu/
├── mod.rs            // public re-exports
├── oauth.rs          // localhost server, authorize URL, code → token exchange
├── client.rs         // tenant_access_token cache; send_card / update_card / urgent_app
├── bot.rs            // webhook handler: signature verify + card.action.trigger
└── card.rs           // GuardCard, ElicitationCard, PlanCard typed Serde models
```

### `oauth.rs`

- `start_oauth() -> OauthHandle { port, authorize_url, state }`
  binds to `127.0.0.1:0`, registers `state` in an in-memory map with a 5-minute
  TTL, returns the authorize URL the desktop should open in the system browser.
- handler for `GET /feishu/cb?code&state` resolves the pending state, exchanges
  `code + app_id + app_secret + redirect_uri + code_verifier` for
  `user_access_token`, immediately discards the access token, persists only
  `open_id`.
- `poll_oauth(state) -> Pending | Connected(open_id) | Failed(reason)`
  for desktop polling.
- PKCE is required: `code_challenge_method=S256`. (Feishu accepts PKCE; it
  does not waive `client_secret`, but PKCE still prevents code-interception
  attacks against the localhost callback.)

### `client.rs`

- `tenant_access_token` cache with TTL refresh. Exchanged from
  `app_id + app_secret`. Never propagated to the desktop client.
- `send_card(open_id, card_json) -> message_id`
- `update_card(message_id, card_json)`
- `urgent_app(message_id, [open_id])` — Guard only.
- All requests sign with `tenant_access_token`; no per-user OAuth tokens
  retained.

### `bot.rs`

- HTTP route `POST /webhook/feishu` registered onto fleet-cli's existing axum
  router.
- Verifies the `X-Lark-Signature` header against `encrypt_key` per Feishu's
  webhook spec. Drops requests with bad signature without responding 4xx
  (avoid leaking presence).
- Only `card.action.trigger` is handled; other event types are acknowledged
  and ignored.
- Action payload deserializes into `{ decision_id, choice, form_fields }` and
  is dispatched to the existing `respond_to_guard / respond_to_elicitation /
  respond_to_plan_approval` Backend methods.

### `card.rs`

Strongly-typed Rust structures that serialize to Feishu Card 2.0 JSON. Three
top-level builders:

- `GuardCard { workspace, command, risk_label, llm_analysis, decision_id }`
- `ElicitationCard { workspace, question, options, multi_select, allow_other,
  step, decision_id }`
- `PlanCard { workspace, plan_markdown, decision_id }`

Plus an `update_into_resolved(card, resolution)` helper for the post-response
"已处理" rendering used by `update_card`.

## Backend trait extension

`Backend` (in [`claw-fleet-core/src/backend.rs`](../claw-fleet-core/src/backend.rs))
gains four methods:

```rust
trait Backend {
    // ... existing ...

    fn start_feishu_oauth(&self) -> Result<OauthHandle>;
    fn poll_feishu_oauth(&self, state: &str) -> Result<OauthStatus>;
    fn feishu_status(&self) -> Result<FeishuConnection>;
    fn disconnect_feishu(&self) -> Result<()>;
}

enum OauthStatus { Pending, Connected(OpenId), Failed(String) }
enum FeishuConnection { NotConnected, Connected { open_id: String, name: Option<String> } }
```

`LocalBackend` delegates to a local `FeishuBridge` instance.
`RemoteBackend` calls four new HTTP endpoints on `fleet serve`. Both
`OauthStatus` and `FeishuConnection` derive `Serialize` + `Deserialize` per
the project rule.

## OAuth flow (Route D: desktop browser + localhost redirect)

```
[desktop fleet]            [fleet-cli]                         [system browser]              [Feishu]
   |
   | start_feishu_oauth ─»
   |    «─ (state, port, authorize_url)
   |
   | open authorize_url in browser ──────────────────»
   |
   |                                                                    | login + consent ─»
   |                                                                    «─ 302 redirect
   |
   |                            «── GET /feishu/cb?code&state ──────────|
   |                            |
   |                            | exchange (code, code_verifier, app_id, app_secret) ─»
   |                            |    «─ user_access_token + open_id
   |                            |
   |                            | persist open_id, drop access_token
   |                            | mark state = Connected(open_id)
   |                            | render minimal "you can close this tab" HTML response
   |
   | poll_feishu_oauth(state) (1s interval)
   |    «─ Connected(open_id)
   | UI flips to "Connected as <name>"
```

Why Route D rather than QR + public redirect:

- No public hostname required; works for both LocalBackend and RemoteBackend.
- Familiar UX (GitHub Desktop, Linear, Notion all use this).
- Rebinds cleanly per machine, which matches the existing per-host fleet
  identity model.

`redirect_uri` is registered as `http://localhost:51823/feishu/cb` (constant
port). Feishu does allow localhost redirect_uris.

## Card schemas

### Guard card

```jsonc
{
  "schema": "2.0",
  "config": { "update_multi": true },
  "header": {
    "title":    { "tag": "plain_text", "content": "Approve: git push --force" },
    "subtitle": { "tag": "plain_text", "content": "workspace: claude-fleet" },
    "template": "red",
    "ud_icon":  { "token": "warning_outlined" }
  },
  "body": {
    "elements": [
      { "tag": "markdown", "content": "**Risk:** destructive\n\n**Command:**\n```\ngit push --force origin main\n```" },
      { "tag": "markdown", "content": "**LLM Analysis**\n..." },
      { "tag": "action", "actions": [
        { "tag": "button", "text": {"tag":"plain_text","content":"Allow"},
          "type": "primary", "value": {"decision_id":"<uuid>","choice":"allow"} },
        { "tag": "button", "text": {"tag":"plain_text","content":"Block"},
          "type": "danger",  "value": {"decision_id":"<uuid>","choice":"block"} }
      ]}
    ]
  }
}
```

After `send_card`, immediately call `urgent_app(message_id, [open_id])`. This
replaces the local Guard "drop" chime with Feishu's native vibrate +
forced-popup, which is a stronger interruption than a sound.

### Elicitation card

```jsonc
{
  "header": { "title": { "content": "Decision needed" }, "template": "blue" },
  "body": {
    "elements": [
      { "tag": "markdown", "content": "<question body>" },
      { "tag": "form", "name": "main_form", "elements": [
        { "tag": "select_static", "name": "choice",
          "placeholder": {"content":"Choose one"},
          "options": [
            {"text":{"content":"Option A — recommended"},"value":"a"},
            {"text":{"content":"Option B"},               "value":"b"},
            {"text":{"content":"Other"},                  "value":"other"}
          ]
        },
        { "tag": "input", "name": "other_text", "required": false,
          "placeholder": {"content":"Fill if you chose Other"},
          "label": {"content":"Custom"} },
        { "tag": "button", "text": {"content":"Submit"},
          "type": "primary", "action_type": "form_submit",
          "value": {"decision_id":"<uuid>"} }
      ]}
    ]
  }
}
```

Variations:
- Multi-select: replace `select_static` with `multi_select_static`. Same form.
- Per-option preview: append a `collapsible_panel` element after each option's
  description, containing the markdown preview body.
- Multi-step: each step is a fresh card schema; on `form_submit`, the bot
  responds by calling `update_card` with the next step's schema. `header.
  subtitle` carries `"Step 2 of 3"`.
- Attachments: a `text_link` button labelled "Attach files" jumps to a
  fleet-cli–hosted https upload page (reuses `fleet serve`). On upload, the
  bridge calls `update_card` to add a "1 attachment" line to the body.

### Plan-Approval card

```jsonc
{
  "header": { "title": { "content": "Plan review" }, "template": "blue" },
  "body": {
    "elements": [
      { "tag": "markdown", "content": "<plan body in markdown>" },
      { "tag": "form", "name": "plan_form", "elements": [
        { "tag": "input", "name": "edited_plan", "multiline": true,
          "default_value": "<plan body>" },
        { "tag": "input", "name": "reject_reason", "multiline": true,
          "label": {"content":"Reason (only if rejecting)"}, "required": false },
        { "tag": "action", "actions": [
          { "tag": "button", "text": {"content":"Approve"},
            "type": "primary", "action_type": "form_submit",
            "value": {"decision_id":"<uuid>","choice":"approve"} },
          { "tag": "button", "text": {"content":"Approve Edited"},
            "type": "primary", "action_type": "form_submit",
            "value": {"decision_id":"<uuid>","choice":"approve_edited"} },
          { "tag": "button", "text": {"content":"Reject"},
            "type": "danger", "action_type": "form_submit",
            "value": {"decision_id":"<uuid>","choice":"reject"} }
        ]}
      ]}
    ]
  }
}
```

Plan diff highlighting: Feishu markdown does not support diff coloring.
Generate a unified-diff block server-side and render inside a fenced ```` ```diff ````
block — it gets monospace styling but no color. Acceptable degradation.

## Webhook callback chain

```
POST /webhook/feishu                      // bot.rs
   |
   | 1. verify X-Lark-Signature
   | 2. branch on event["header"]["event_type"]
   |    - "card.action.trigger"  → handle_card_action
   |    - "im.message.receive_v1" → optional /bind discovery (future)
   |    - other → 200 OK, no-op
   |
handle_card_action(event):
   | extract action.value.decision_id, action.value.choice, action.form_fields
   | dispatch to backend.respond_to_<kind>(decision_id, choice, form_fields)
   | render resolved card (read-only summary, buttons removed)
   | call update_card(message_id, resolved_card)
   |
   | existing cross-device dismiss event also fires (because respond_to_xxx
   | hits the same store as the desktop), so all subscribers (mobile, other
   | desktop sessions) clear their local panel; the Feishu card is
   | independently flipped to resolved by the call above.
```

Verification rule: an action whose `decision_id` does not match any pending
decision is silently dropped — never 4xx, to avoid leaking decision lifecycle
state.

## Credential management (Route A)

- `app_secret` lives only in fleet-cli env / config file. Never in desktop
  binary, never sent over the desktop ↔ fleet-cli wire.
- Four credentials, loaded by `AppCredentials::load()` in
  `claw-fleet-core/src/feishu.rs` with file-first, env-fallback precedence:

  | Field | Required | Used by |
  |---|---|---|
  | `app_id` | yes | OAuth, Card sending, webhook signing |
  | `app_secret` | yes | OAuth token exchange, `tenant_access_token` |
  | `encrypt_key` | optional | Webhook `X-Lark-Signature` validation when Feishu's "encrypt push" mode is on |
  | `verification_token` | optional | Webhook plain-mode `token` field check |

- Storage tiers (highest precedence first):
  1. `~/.fleet/feishu-creds.json` — written by the desktop **Settings → Interaction → Feishu** form, chmod `0600` on unix.
  2. Environment variables `FEISHU_APP_ID` / `FEISHU_APP_SECRET` /
     `FEISHU_ENCRYPT_KEY` / `FEISHU_VERIFICATION_TOKEN` — for `fleet serve`
     (headless) or CI scenarios where the UI is not available.
- Feishu does not waive `client_secret` even with PKCE — confirmed against
  [the v2 token endpoint docs](https://open.feishu.cn/document/uAjLw4CM/ukTMukTMukTM/authentication-management/access-token/get-user-access-token).
  This rules out the GitHub Desktop / Linear "no-secret on the client"
  pattern. Each fleet-cli deployment uses its own Feishu app, mirroring the
  per-deployment identity model fleet already has for SSH probes.

## Deployment guide

End-to-end walkthrough for a workspace owner setting up the integration for
the first time. Steps are listed in execution order — do not skip ahead, the
Feishu console rejects events from unregistered redirect URLs.

### 1. Create the Feishu app

In the Feishu Open Platform console at [open.feishu.cn](https://open.feishu.cn)
(or `open.larksuite.com` for non-CN tenants):

1. Create an **Enterprise self-built application** (企业自建应用). A personal
   Feishu account with a personal tenant works for development.
2. Enable the **Bot** capability (机器人能力).
3. Restrict **Visibility / Available scope** (可用范围) to the workspace
   owner only — the bot only ever messages one user.

### 2. Permissions

Under **权限管理**, grant:

- `im:message` — send Card messages.
- `im:message:send_as_bot` — send as the bot identity.
- `im:message.urgent` — fire `urgent_app` for Guard events.
- `contact:user.id:readonly` — read `open_id` and display name on OAuth.

(Older docs may also list `im:resource` for file uploads — only needed if you
ship the attachment fallback in the future.)

### 3. Redirect URL (OAuth)

Under **安全设置 → Redirect URL**, add exactly:

```
http://localhost:51823/feishu/cb
```

The port is hard-coded in `claw_fleet_core::feishu::FEISHU_OAUTH_PORT`. The
OAuth listener binds 127.0.0.1:51823 on demand and only routes the
`/feishu/cb` path.

### 4. Event subscription (webhook)

Under **事件与回调 → 事件配置**, subscribe to `card.action.trigger`. The
**Request URL** depends on deployment shape:

| Deployment | Webhook URL |
|---|---|
| Pure local (LocalBackend, desktop only) | `http://localhost:51823/webhook/feishu` — Feishu cannot reach localhost; expose via `cloudflared tunnel --url http://localhost:51823` or `ngrok http 51823` and paste the public URL. |
| `fleet serve` on a server (RemoteBackend) | `https://<your-fleet-host>/webhook/feishu` |

For local testing without a tunnel, you can still send Cards (one-way) — only
button callbacks need the inbound webhook.

### 5. Fill credentials in Fleet

You have two routes; they share the same `~/.fleet/feishu-creds.json` file
when you use the UI:

- **Desktop UI (recommended for local use):** Settings → Interaction →
  Feishu (Lark) Decision Panel Mirror. Paste **App ID**, **App Secret**, and
  optionally **Encrypt Key** / **Verification Token** (both blank is fine if
  you leave Feishu's webhook in plain mode). Click **Save Credentials**.
- **`fleet serve` (headless):** export `FEISHU_APP_ID` / `FEISHU_APP_SECRET`
  (and optionally `FEISHU_ENCRYPT_KEY` / `FEISHU_VERIFICATION_TOKEN`) before
  launching the binary. macOS GUI launches do not see `~/.zshrc` exports —
  use the desktop UI on the desktop, env vars on the server.

### 6. Connect (OAuth + bot pairing)

In Settings → Interaction → Feishu, click **Connect Feishu**. The browser
opens the Feishu authorize page; consent grants the bot a one-time code, the
desktop exchanges it for `open_id` + display name, and the row flips to
**Connected as `<your name>`**. Search the bot's name in Feishu and send any
message to open the conversation — Cards arrive there.

### 7. Verify

Trigger a Guard decision (e.g., run a destructive command in a watched
session). The desktop card appears in Feishu within 1–2 s; tapping the
button on Feishu mirrors back to the desktop within ~500 ms.

## Security considerations

- Webhook signature: `X-Lark-Signature` must validate against `encrypt_key`
  before any state mutation. Reject silently on mismatch.
- OAuth `state` parameter: 32-byte random, single-use, server-stored, 5-minute
  TTL. Defends against CSRF and against another local process racing the
  callback.
- PKCE `code_verifier`: 43–128 character random string per RFC 7636. Defends
  against authorization-code interception on the localhost loopback (other
  local processes binding the port after the listener starts).
- `user_access_token` from the OAuth exchange is dropped immediately after
  reading `open_id`. Storing it would create a credential we never use.
- Per-action authorization: every card action's `decision_id` must map to a
  decision currently owned by the authenticated `open_id`. Decisions for a
  workspace owned by user A are silently rejected if presented by user B.

## Fallbacks for capabilities Feishu cannot express

| Capability | Fallback |
|---|---|
| TTS narration | Desktop client retains existing TTS; phone receives no voice. Acceptable: voice was redundant on phone (the screen + vibrate already grabs attention). |
| Per-event chimes (drop / ding_dong) | Guard escalates to `urgent_app` (force pop-up + vibrate). Elicitation uses default Feishu notification sound. |
| Side-by-side option preview | Per-option `collapsible_panel` containing the preview markdown. With ≤ 3 options, panels open by default. |
| Large attachments (> 4 MB inline limit) | "Attach files" link to a fleet-cli–hosted upload page; webhook acks attachment count back into the card. |
| Plan diff coloring | Render unified diff in ```` ```diff ```` fence (monospace, no color). |

## Open questions and future work

- **`urgent_app` rate limits** — Feishu rate-limits urgent-message API per
  tenant. Need to measure whether burst Guard events hit the cap.
- **Multi-user fleet deployment** — current design has each user create their
  own Feishu app. If fleet ships a public release, an OAuth proxy service
  (Notion-style) becomes necessary; this design is not blocked by deferring
  it.
- **Refresh token strategy** — `tenant_access_token` is short-lived (~2 h)
  and refreshed from `app_id + app_secret`. No user-side refresh token to
  manage. Document this as the explicit reason we drop `user_access_token`.
- **File upload UX** — the out-of-band fleet-cli upload page exists but no
  design for unauthenticated access. Likely solution: per-decision short-lived
  upload token issued in the card's "Attach" link.
- **Card schema versioning** — Feishu Card 2.0 is the current major. Pin the
  schema version in `card.rs` and surface a clear error if Feishu deprecates
  it later.
